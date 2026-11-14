use std::{
    borrow::Cow,
    fmt, mem,
    sync::Arc,
    task::{Context as TaskContext, Poll, Wake, Waker},
    time::{Instant, SystemTime},
};

use nanocode_sync::{Sequencer, Slot};
use nanocode_trace::{
    Event,
    field::{Field, Visit},
    span::{Attributes, Record},
};

use crate::{
    subscriber::{Context, Layer, Registry, SpanRef},
    model::{Event as SpanEvent, KeyValue, SpanContext, SpanKind, Status, Text, Value},
    trace::{Builder, Parent, Started, Tracer},
};

const SPAN_NAME_FIELD: &str = "otel.name";
const SPAN_KIND_FIELD: &str = "otel.kind";
const SPAN_STATUS_CODE_FIELD: &str = "otel.status_code";
const SPAN_STATUS_DESCRIPTION_FIELD: &str = "otel.status_description";
const MESSAGE_FIELD: &str = "message";
const ERROR_FIELD: &str = "error";
const EVENT_EXCEPTION_NAME: &str = "exception";
const FIELD_EXCEPTION_MESSAGE: &str = "exception.message";
const FIELD_EXCEPTION_STACKTRACE: &str = "exception.stacktrace";
const THREAD_ID_PREFIX: &str = "ThreadId(";
const THREAD_ID_SUFFIX: &str = ")";

thread_local! {
    static THREAD_ID: i64 = {
        let debug = format!("{:?}", std::thread::current().id());
        let digits = debug
            .strip_prefix(THREAD_ID_PREFIX)
            .and_then(|rest| rest.strip_suffix(THREAD_ID_SUFFIX))
            .expect("a thread ID prints as ThreadId(N)");
        let id: u64 = digits.parse().expect("a thread ID is an integer");
        id as i64
    };
}

pub struct OtelLayer {
    tracer: Tracer,
}

enum State {
    Building {
        builder: Builder,
        parent: Parent,
        status: Status,
    },
    Started(Started),
}

struct OtelData {
    state: State,
    end_time: Option<SystemTime>,
    tracer: Tracer,
}

struct Timings {
    idle: i64,
    busy: i64,
    last: Instant,
    entered: u64,
}

fn nanos(from: Instant, to: Instant) -> i64 {
    i64::try_from(to.duration_since(from).as_nanos()).expect("a span lasts under 292 years")
}

fn span_kind(text: &str) -> Option<SpanKind> {
    [
        ("server", SpanKind::Server),
        ("client", SpanKind::Client),
        ("producer", SpanKind::Producer),
        ("consumer", SpanKind::Consumer),
        ("internal", SpanKind::Internal),
    ]
    .into_iter()
    .find_map(|(name, kind)| text.eq_ignore_ascii_case(name).then_some(kind))
}

const fn status(text: &str) -> Status {
    if text.eq_ignore_ascii_case("ok") {
        Status::Ok
    } else if text.eq_ignore_ascii_case("error") {
        Status::Error(Cow::Borrowed(""))
    } else {
        Status::Unset
    }
}

#[derive(Default)]
struct Updates {
    name: Option<Text>,
    kind: Option<SpanKind>,
    status: Option<Status>,
    attributes: Option<Vec<KeyValue>>,
}

impl Updates {
    fn apply(self, builder: &mut Builder, held: &mut Status) {
        if let Some(name) = self.name {
            builder.name = name;
        }
        if let Some(kind) = self.kind {
            builder.kind = Some(kind);
        }
        if let Some(status) = self.status {
            *held = status;
        }
        if let Some(attributes) = self.attributes {
            builder.attributes.extend(attributes);
        }
    }

    fn apply_started(self, span: &mut Started) {
        if let Some(status) = self.status {
            span.set_status(status);
        }
        if let Some(attributes) = self.attributes {
            span.set_attributes(attributes);
        }
    }

    fn attribute(&mut self, attribute: KeyValue) {
        self.attributes.get_or_insert_with(Vec::new).push(attribute);
    }
}

fn chain(error: &(dyn std::error::Error + 'static)) -> Vec<Text> {
    let mut chain = Vec::new();
    let mut next = error.source();
    while let Some(source) = next {
        chain.push(Cow::Owned(source.to_string()));
        next = source.source();
    }
    chain
}

struct SpanAttributes<'a>(&'a mut Updates);

impl Visit for SpanAttributes<'_> {
    fn record_bool(&mut self, field: &Field, value: bool) {
        self.0.attribute(KeyValue::new(field.name(), value));
    }

    fn record_f64(&mut self, field: &Field, value: f64) {
        self.0.attribute(KeyValue::new(field.name(), value));
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.0.attribute(KeyValue::new(field.name(), value));
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        match field.name() {
            SPAN_NAME_FIELD => self.0.name = Some(Cow::Owned(value.to_owned())),
            SPAN_KIND_FIELD => self.0.kind = span_kind(value),
            SPAN_STATUS_CODE_FIELD => self.0.status = Some(status(value)),
            SPAN_STATUS_DESCRIPTION_FIELD => {
                self.0.status = Some(Status::Error(Cow::Owned(value.to_owned())));
            }
            name => self.0.attribute(KeyValue::new(name, value.to_owned())),
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        let text = format!("{value:?}");
        match field.name() {
            SPAN_NAME_FIELD => self.0.name = Some(Cow::Owned(text)),
            SPAN_KIND_FIELD => self.0.kind = span_kind(&text),
            SPAN_STATUS_CODE_FIELD => self.0.status = Some(status(&text)),
            SPAN_STATUS_DESCRIPTION_FIELD => self.0.status = Some(Status::Error(Cow::Owned(text))),
            name => self.0.attribute(KeyValue::new(name, text)),
        }
    }

    fn record_error(&mut self, field: &Field, value: &(dyn std::error::Error + 'static)) {
        let chain = chain(value);
        let message = value.to_string();
        self.0
            .attribute(KeyValue::new(FIELD_EXCEPTION_MESSAGE, message.clone()));
        self.0.attribute(KeyValue::new(
            FIELD_EXCEPTION_STACKTRACE,
            Value::Strings(chain.clone()),
        ));
        self.0.attribute(KeyValue::new(field.name(), message));
        self.0.attribute(KeyValue::new(
            format!("{}.chain", field.name()),
            Value::Strings(chain),
        ));
    }
}

struct EventFields<'a> {
    name: &'a mut Text,
    attributes: &'a mut Vec<KeyValue>,
    updates: &'a mut Option<Updates>,
}

impl EventFields<'_> {
    fn error(&mut self, text: String) {
        self.updates.get_or_insert_with(Updates::default).status =
            Some(Status::Error(Cow::Owned(text.clone())));
        *self.name = Cow::Borrowed(EVENT_EXCEPTION_NAME);
        self.attributes
            .push(KeyValue::new(FIELD_EXCEPTION_MESSAGE, text));
    }
}

impl Visit for EventFields<'_> {
    fn record_bool(&mut self, field: &Field, value: bool) {
        match field.name() {
            MESSAGE_FIELD => *self.name = Cow::Owned(value.to_string()),
            name => self.attributes.push(KeyValue::new(name, value)),
        }
    }

    fn record_f64(&mut self, field: &Field, value: f64) {
        match field.name() {
            MESSAGE_FIELD => *self.name = Cow::Owned(value.to_string()),
            name => self.attributes.push(KeyValue::new(name, value)),
        }
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        match field.name() {
            MESSAGE_FIELD => *self.name = Cow::Owned(value.to_string()),
            name => self.attributes.push(KeyValue::new(name, value)),
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        match field.name() {
            MESSAGE_FIELD => *self.name = Cow::Owned(value.to_owned()),
            ERROR_FIELD if self.name.is_empty() => self.error(format!("{value:?}")),
            name => self.attributes.push(KeyValue::new(name, value.to_owned())),
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        match field.name() {
            MESSAGE_FIELD => *self.name = Cow::Owned(format!("{value:?}")),
            ERROR_FIELD if self.name.is_empty() => self.error(format!("{value:?}")),
            name => self
                .attributes
                .push(KeyValue::new(name, format!("{value:?}"))),
        }
    }

    fn record_error(&mut self, field: &Field, value: &(dyn std::error::Error + 'static)) {
        let chain = chain(value);
        let message = value.to_string();
        self.attributes
            .push(KeyValue::new(FIELD_EXCEPTION_MESSAGE, message.clone()));
        self.attributes.push(KeyValue::new(
            FIELD_EXCEPTION_STACKTRACE,
            Value::Strings(chain.clone()),
        ));
        let updates = self
            .updates
            .get_or_insert_with(Updates::default)
            .attributes
            .get_or_insert_with(Vec::new);
        updates.push(KeyValue::new(FIELD_EXCEPTION_MESSAGE, message.clone()));
        updates.push(KeyValue::new(
            FIELD_EXCEPTION_STACKTRACE,
            Value::Strings(chain.clone()),
        ));
        self.attributes.push(KeyValue::new(field.name(), message));
        self.attributes.push(KeyValue::new(
            format!("{}.chain", field.name()),
            Value::Strings(chain),
        ));
    }
}

impl OtelData {
    fn start(&mut self) -> &mut Started {
        if let State::Building {
            builder,
            parent,
            status,
        } = &mut self.state
        {
            let taken = Builder {
                name: mem::take(&mut builder.name),
                kind: builder.kind,
                start_time: builder.start_time,
                attributes: mem::take(&mut builder.attributes),
                events: mem::take(&mut builder.events),
                links: mem::take(&mut builder.links),
            };
            let mut started = self.tracer.start(taken, parent);
            started.set_status(mem::take(status));
            self.state = State::Started(started);
        }
        match &mut self.state {
            State::Started(started) => started,
            State::Building { .. } => unreachable!("a span was started just above"),
        }
    }
}

impl OtelLayer {
    #[must_use]
    pub const fn new(tracer: Tracer) -> Self {
        Self { tracer }
    }

    fn started(span: &SpanRef<'_, OtelSpan>) -> Option<(SpanContext, Parent)> {
        let data = span.data()?;
        data.0.with(|slot| {
            let started = slot.data.as_mut()?.start();
            Some((started.context(), started.children()))
        })
    }

    fn parent(&self, attributes: &Attributes<'_>, context: &Context<'_, OtelSpan>) -> Parent {
        if let Some(started) = attributes
            .parent()
            .and_then(|parent| context.span(parent))
            .and_then(|span| Self::started(&span))
        {
            return started.1;
        }
        if !attributes.is_contextual() {
            return Parent::Root;
        }
        context
            .lookup_current()
            .and_then(|span| Self::started(&span))
            .map_or_else(Parent::current, |started| started.1)
    }
}

fn location(metadata: &nanocode_trace::Metadata<'_>, attributes: &mut Vec<KeyValue>) {
    if let Some(file) = metadata.file() {
        attributes.push(KeyValue::new("code.file.path", file.to_owned()));
    }
    if let Some(module) = metadata.module_path() {
        attributes.push(KeyValue::new("code.module.name", module.to_owned()));
    }
    if let Some(line) = metadata.line() {
        attributes.push(KeyValue::new("code.line.number", i64::from(line)));
    }
}

struct Exclusive<T> {
    turns: Sequencer,
    value: Slot<T>,
}

struct Unpark(std::thread::Thread);

impl Wake for Unpark {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}

fn wait_for<F: Future>(future: F) -> F::Output {
    let waker = Waker::from(Arc::new(Unpark(std::thread::current())));
    let mut context = TaskContext::from_waker(&waker);
    let mut future = std::pin::pin!(future);
    loop {
        if let Poll::Ready(output) = future.as_mut().poll(&mut context) {
            return output;
        }
        std::thread::park();
    }
}

impl<T> Exclusive<T> {
    fn new(value: T) -> Self {
        Self {
            turns: Sequencer::new(),
            value: Slot::new(Some(value)),
        }
    }

    fn with<R>(&self, change: impl FnOnce(&mut T) -> R) -> R {
        let turn = self
            .turns
            .try_enter()
            .unwrap_or_else(|| wait_for(self.turns.enter()));
        let mut value = self
            .value
            .take()
            .expect("the holder of a span's turn owns its OpenTelemetry data");
        let output = change(&mut value);
        let previous = self.value.put(value);
        assert!(
            previous.is_none(),
            "only the holder of a span's turn puts its data back"
        );
        drop(turn);
        output
    }
}

pub struct OtelSpan(Exclusive<OtelSlot>);

struct OtelSlot {
    data: Option<OtelData>,
    timings: Option<Timings>,
}

impl Layer for OtelLayer {
    type Span = OtelSpan;

    fn new_span(
        &self,
        attributes: &Attributes<'_>,
        _span: &SpanRef<'_, OtelSpan>,
        context: &Context<'_, OtelSpan>,
    ) -> OtelSpan {
        let timings = Timings {
            idle: 0,
            busy: 0,
            last: Instant::now(),
            entered: 0,
        };
        let parent = self.parent(attributes, context);
        let metadata = attributes.metadata();
        let mut builder = Builder {
            name: Cow::Borrowed(metadata.name()),
            kind: None,
            start_time: SystemTime::now(),
            attributes: Vec::new(),
            events: Vec::new(),
            links: Vec::new(),
        };
        location(metadata, &mut builder.attributes);
        THREAD_ID.with(|id| builder.attributes.push(KeyValue::new("thread.id", *id)));
        if let Some(name) = std::thread::current().name() {
            builder
                .attributes
                .push(KeyValue::new("thread.name", name.to_owned()));
        }
        builder
            .attributes
            .push(KeyValue::new("target", metadata.target().to_owned()));
        let mut updates = Updates::default();
        attributes.record(&mut SpanAttributes(&mut updates));
        let mut status = Status::Unset;
        updates.apply(&mut builder, &mut status);
        OtelSpan(Exclusive::new(OtelSlot {
            data: Some(OtelData {
                state: State::Building {
                    builder,
                    parent,
                    status,
                },
                end_time: None,
                tracer: self.tracer.clone(),
            }),
            timings: Some(timings),
        }))
    }

    fn on_enter(&self, span: &SpanRef<'_, OtelSpan>, _context: &Context<'_, OtelSpan>) {
        let data = span.data().expect("a visible span has its layer's data");
        data.0.with(|slot| {
            if let Some(timings) = &mut slot.timings {
                if timings.entered == 0 {
                    let now = Instant::now();
                    timings.idle += nanos(timings.last, now);
                    timings.last = now;
                }
                timings.entered += 1;
            }
        });
    }

    fn on_exit(&self, span: &SpanRef<'_, OtelSpan>, _context: &Context<'_, OtelSpan>) {
        let data = span.data().expect("a visible span has its layer's data");
        data.0.with(|slot| {
            if let Some(data) = &mut slot.data {
                data.end_time = Some(SystemTime::now());
            }
            if let Some(timings) = &mut slot.timings {
                timings.entered -= 1;
                if timings.entered == 0 {
                    let now = Instant::now();
                    timings.busy += nanos(timings.last, now);
                    timings.last = now;
                }
            }
        });
    }

    fn on_record(
        &self,
        span: &SpanRef<'_, OtelSpan>,
        values: &Record<'_>,
        _context: &Context<'_, OtelSpan>,
    ) {
        let mut updates = Updates::default();
        values.record(&mut SpanAttributes(&mut updates));
        let data = span.data().expect("a visible span has its layer's data");
        data.0.with(|slot| {
            if let Some(data) = &mut slot.data {
                match &mut data.state {
                    State::Building {
                        builder, status, ..
                    } => updates.apply(builder, status),
                    State::Started(started) => updates.apply_started(started),
                }
            }
        });
    }

    fn on_follows_from(
        &self,
        span: &SpanRef<'_, OtelSpan>,
        follows: &SpanRef<'_, OtelSpan>,
        _context: &Context<'_, OtelSpan>,
    ) {
        let data = span.data().expect("a visible span has its layer's data");
        if data.0.with(|slot| slot.data.is_none()) {
            return;
        }
        let Some(followed) = Self::started(follows) else {
            return;
        };
        let link = followed.0;
        data.0.with(|slot| {
            if let Some(data) = &mut slot.data {
                match &mut data.state {
                    State::Building { builder, .. } => builder.links.push(link),
                    State::Started(started) => started.add_link(link),
                }
            }
        });
    }

    fn on_event(&self, event: &Event<'_>, context: &Context<'_, OtelSpan>) {
        let Some(span) = context.event_span(event) else {
            return;
        };
        let metadata = event.metadata();
        let timestamp = SystemTime::now();
        let mut name = Cow::Borrowed("");
        let mut attributes = vec![
            KeyValue::new("level", metadata.level().as_str()),
            KeyValue::new("target", metadata.target()),
        ];
        let mut updates = None;
        event.record(&mut EventFields {
            name: &mut name,
            attributes: &mut attributes,
            updates: &mut updates,
        });
        if name.is_empty() {
            name = Cow::Borrowed(metadata.name());
        }
        let data = span.data().expect("a visible span has its layer's data");
        data.0.with(|slot| {
            let Some(data) = &mut slot.data else {
                return;
            };
            location(metadata, &mut attributes);
            let error = *metadata.level() == nanocode_trace::Level::ERROR;
            match &mut data.state {
                State::Building {
                    builder, status, ..
                } => {
                    if *status == Status::Unset && error {
                        *status = Status::Error(Cow::Borrowed(""));
                    }
                    if let Some(updates) = updates {
                        updates.apply(builder, status);
                    }
                    builder.events.push(SpanEvent {
                        name,
                        timestamp,
                        attributes,
                        dropped_attributes_count: 0,
                    });
                }
                State::Started(started) => {
                    if error {
                        started.set_status(Status::Error(Cow::Borrowed("")));
                    }
                    if let Some(updates) = updates {
                        updates.apply_started(started);
                    }
                    started.add_event(name, attributes);
                }
            }
        });
    }

    fn on_close(&self, span: &SpanRef<'_, OtelSpan>, _context: &Context<'_, OtelSpan>) {
        let held = span.data().expect("a visible span has its layer's data");
        let (data, timings) = held.0.with(|slot| (slot.data.take(), slot.timings.take()));
        let Some(data) = data else {
            return;
        };
        let timings = timings.map(|timings| {
            [
                KeyValue::new("busy_ns", timings.busy),
                KeyValue::new("idle_ns", timings.idle),
            ]
        });
        match data.state {
            State::Building {
                builder,
                parent,
                status,
            } => {
                let mut started = self.tracer.start(builder, &parent);
                if let Some(timings) = timings {
                    started.set_attributes(timings);
                }
                started.set_status(status);
                started.end(data.end_time);
            }
            State::Started(mut started) => {
                if let Some(timings) = timings {
                    started.set_attributes(timings);
                }
                started.end(data.end_time);
            }
        }
    }
}

fn with_data(span: &nanocode_trace::Span, change: impl FnOnce(&mut OtelData)) {
    span.with_subscriber(|(id, dispatch)| {
        let Some(registry) = dispatch.downcast_ref::<Registry>() else {
            return;
        };
        registry.data::<OtelSpan, _>(id, |otel| {
            otel.0.with(|slot| {
                if let Some(data) = &mut slot.data {
                    change(data);
                }
            });
        });
    });
}

#[must_use]
pub fn span_context(span: &nanocode_trace::Span) -> SpanContext {
    let mut context = SpanContext::INVALID;
    with_data(span, |data| {
        context = data.start().context();
    });
    context
}

pub fn add_link(span: &nanocode_trace::Span, link: SpanContext) {
    if !link.is_valid() {
        return;
    }
    with_data(span, |data| match &mut data.state {
        State::Building { builder, .. } => builder.links.push(link),
        State::Started(started) => started.add_link(link),
    });
}

