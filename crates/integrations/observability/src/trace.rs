use std::{
    cell::Cell,
    env,
    str::FromStr,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, SystemTime},
};

use crate::model::{
    Event, ExportError, Exported, KeyValue, SpanContext, SpanData, SpanKind, Status, Text,
};

const OTEL_SPAN_ATTRIBUTE_COUNT_LIMIT: &str = "OTEL_SPAN_ATTRIBUTE_COUNT_LIMIT";
const OTEL_SPAN_EVENT_COUNT_LIMIT: &str = "OTEL_SPAN_EVENT_COUNT_LIMIT";
const OTEL_SPAN_LINK_COUNT_LIMIT: &str = "OTEL_SPAN_LINK_COUNT_LIMIT";
const OTEL_TRACES_SAMPLER: &str = "OTEL_TRACES_SAMPLER";
const OTEL_TRACES_SAMPLER_ARG: &str = "OTEL_TRACES_SAMPLER_ARG";
const SPAN_ATTRIBUTES_DEFAULT: u32 = 128;
const SPAN_EVENTS_DEFAULT: u32 = 128;
const SPAN_LINKS_DEFAULT: u32 = 128;
const EVENT_ATTRIBUTES_MAX: usize = 128;
const RATIO_ALL: f64 = 1.0;
const RATIO_SCALE: f64 = (1_u64 << 63) as f64;
const XOSHIRO_ROTATE_OUTPUT: u32 = 23;
const XOSHIRO_SHIFT: u32 = 17;
const XOSHIRO_ROTATE_STATE: u32 = 45;

pub trait SpanProcessor: Send + Sync {
    fn on_end(&self, span: SpanData);

    fn shutdown_with_timeout(&self, timeout: Duration) -> Exported;
}

#[derive(Clone, Copy, Debug)]
enum Root {
    AlwaysOn,
    AlwaysOff,
    Ratio(f64),
}

#[derive(Clone, Copy, Debug)]
struct Sampler {
    parent_based: bool,
    root: Root,
}

impl Sampler {
    fn from_env() -> Self {
        let based = |root| Self {
            parent_based: true,
            root,
        };
        let alone = |root| Self {
            parent_based: false,
            root,
        };
        let ratio = || {
            env::var(OTEL_TRACES_SAMPLER_ARG)
                .ok()
                .and_then(|ratio| ratio.parse::<f64>().ok())
                .unwrap_or(RATIO_ALL)
        };
        match env::var(OTEL_TRACES_SAMPLER).as_deref() {
            Ok("always_on") => alone(Root::AlwaysOn),
            Ok("always_off") => alone(Root::AlwaysOff),
            Ok("traceidratio") => alone(Root::Ratio(ratio())),
            Ok("parentbased_always_off") => based(Root::AlwaysOff),
            Ok("parentbased_traceidratio") => based(Root::Ratio(ratio())),
            _ => based(Root::AlwaysOn),
        }
    }

    fn samples(self, parent: &Parent, trace_id: u128) -> bool {
        match parent {
            Parent::Span(context) if self.parent_based => context.sampled,
            _ => match self.root {
                Root::AlwaysOn => true,
                Root::AlwaysOff => false,
                Root::Ratio(ratio) if ratio >= RATIO_ALL => true,
                Root::Ratio(ratio) => {
                    let bound = (ratio.max(0.0) * RATIO_SCALE) as u64;
                    let low = trace_id as u64;
                    (low >> 1) < bound
                }
            },
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Limits {
    attributes: usize,
    events: usize,
    links: usize,
}

impl Limits {
    fn from_env() -> Self {
        let read = |name: &str, default: u32| {
            env::var(name)
                .ok()
                .and_then(|limit| u32::from_str(&limit).ok())
                .unwrap_or(default) as usize
        };
        Self {
            attributes: read(OTEL_SPAN_ATTRIBUTE_COUNT_LIMIT, SPAN_ATTRIBUTES_DEFAULT),
            events: read(OTEL_SPAN_EVENT_COUNT_LIMIT, SPAN_EVENTS_DEFAULT),
            links: read(OTEL_SPAN_LINK_COUNT_LIMIT, SPAN_LINKS_DEFAULT),
        }
    }
}

struct Provider {
    processors: Vec<Box<dyn SpanProcessor>>,
    sampler: Sampler,
    limits: Limits,
    shut_down: AtomicBool,
}

#[derive(Clone)]
pub struct Tracer(Arc<Provider>);

thread_local! {
    static SUPPRESSED: Cell<bool> = const { Cell::new(false) };
    static RANDOM: Cell<Option<[u64; 4]>> = const { Cell::new(None) };
}

pub(crate) fn suppress_on_this_thread() {
    SUPPRESSED.with(|suppressed| suppressed.set(true));
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum Parent {
    Root,
    Suppressed,
    Span(SpanContext),
}

impl Parent {
    pub(crate) fn current() -> Self {
        if SUPPRESSED.with(Cell::get) {
            Self::Suppressed
        } else {
            Self::Root
        }
    }

    const fn context(&self) -> SpanContext {
        match self {
            Self::Span(context) => *context,
            Self::Root | Self::Suppressed => SpanContext::INVALID,
        }
    }
}

fn next_random() -> u64 {
    RANDOM.with(|cell| {
        let mut state = cell.get().unwrap_or_else(|| {
            let mut seed = [0_u8; 32];
            nanocode_platform::fill_random(&mut seed)
                .expect("the operating system provides random bytes");
            let words: [u64; 4] = std::array::from_fn(|index| {
                u64::from_le_bytes(seed[index * 8..index * 8 + 8].try_into().expect("8 bytes"))
            });
            assert!(words.iter().any(|word| *word != 0));
            words
        });
        let result = state[0]
            .wrapping_add(state[3])
            .rotate_left(XOSHIRO_ROTATE_OUTPUT)
            .wrapping_add(state[0]);
        let shifted = state[1] << XOSHIRO_SHIFT;
        state[2] ^= state[0];
        state[3] ^= state[1];
        state[1] ^= state[2];
        state[0] ^= state[3];
        state[2] ^= shifted;
        state[3] = state[3].rotate_left(XOSHIRO_ROTATE_STATE);
        cell.set(Some(state));
        result
    })
}

#[derive(Clone, Debug)]
pub(crate) struct Builder {
    pub(crate) name: Text,
    pub(crate) kind: Option<SpanKind>,
    pub(crate) start_time: SystemTime,
    pub(crate) attributes: Vec<KeyValue>,
    pub(crate) events: Vec<Event>,
    pub(crate) links: Vec<SpanContext>,
}

#[derive(Clone, Debug)]
struct Recording {
    parent_span_id: u64,
    kind: SpanKind,
    name: Text,
    start_time: SystemTime,
    end_time: SystemTime,
    attributes: Vec<KeyValue>,
    dropped_attributes_count: u32,
    events: Vec<Event>,
    dropped_events_count: u32,
    links: Vec<SpanContext>,
    dropped_links_count: u32,
    status: Status,
}

#[derive(Clone)]
pub(crate) struct Started {
    context: SpanContext,
    recording: Option<Recording>,
    children: Parent,
    tracer: Tracer,
}

fn count(dropped: usize) -> u32 {
    u32::try_from(dropped).expect("dropped items fit the OTLP count")
}

fn truncate<T>(items: &mut Vec<T>, limit: usize) -> u32 {
    let dropped = items.len().saturating_sub(limit);
    items.truncate(limit);
    count(dropped)
}

impl Tracer {
    #[must_use]
    pub fn new(processors: Vec<Box<dyn SpanProcessor>>) -> Self {
        Self(Arc::new(Provider {
            processors,
            sampler: Sampler::from_env(),
            limits: Limits::from_env(),
            shut_down: AtomicBool::new(false),
        }))
    }

    pub(crate) fn start(&self, builder: Builder, parent: &Parent) -> Started {
        let shut = || Started {
            context: SpanContext::INVALID,
            recording: None,
            children: match parent {
                Parent::Suppressed => Parent::Suppressed,
                Parent::Root | Parent::Span(_) => Parent::Span(SpanContext::INVALID),
            },
            tracer: self.clone(),
        };
        if matches!(parent, Parent::Suppressed) || self.0.shut_down.load(Ordering::Relaxed) {
            return shut();
        }
        let span_id = next_random();
        let trace_id = match parent {
            Parent::Span(context) => context.trace_id,
            Parent::Root | Parent::Suppressed => {
                u128::from(next_random()) | (u128::from(next_random()) << u64::BITS)
            }
        };
        let sampled = self.0.sampler.samples(parent, trace_id);
        let context = SpanContext {
            trace_id,
            span_id,
            sampled,
        };
        let recording = sampled.then(|| self.recording(builder, parent));
        Started {
            context,
            recording,
            children: Parent::Span(context),
            tracer: self.clone(),
        }
    }

    fn recording(&self, builder: Builder, parent: &Parent) -> Recording {
        let limits = self.0.limits;
        let Builder {
            name,
            kind,
            start_time,
            mut attributes,
            mut events,
            mut links,
        } = builder;
        let dropped_attributes_count = truncate(&mut attributes, limits.attributes);
        let dropped_links_count = truncate(&mut links, limits.links);
        let dropped_events_count = truncate(&mut events, limits.events);
        for event in &mut events {
            event.dropped_attributes_count = truncate(&mut event.attributes, EVENT_ATTRIBUTES_MAX);
        }
        Recording {
            parent_span_id: parent.context().span_id,
            kind: kind.unwrap_or(SpanKind::Internal),
            name,
            start_time,
            end_time: start_time,
            attributes,
            dropped_attributes_count,
            events,
            dropped_events_count,
            links,
            dropped_links_count,
            status: Status::Unset,
        }
    }

    pub fn shutdown_with_timeout(&self, timeout: Duration) -> Exported {
        if self
            .0
            .shut_down
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return Err(ExportError::AlreadyShutdown);
        }
        let errors: Vec<ExportError> = self
            .0
            .processors
            .iter()
            .filter_map(|processor| processor.shutdown_with_timeout(timeout).err())
            .collect();
        if errors.is_empty() {
            Ok(())
        } else {
            Err(ExportError::InternalFailure(format!(
                "Shutdown errors: {errors:?}"
            )))
        }
    }
}

impl Started {
    pub(crate) const fn context(&self) -> SpanContext {
        self.context
    }

    pub(crate) const fn children(&self) -> Parent {
        self.children
    }

    pub(crate) fn set_status(&mut self, status: Status) {
        if let Some(recording) = &mut self.recording
            && status > recording.status
        {
            recording.status = status;
        }
    }

    pub(crate) fn set_attributes(&mut self, attributes: impl IntoIterator<Item = KeyValue>) {
        let limit = self.tracer.0.limits.attributes;
        if let Some(recording) = &mut self.recording {
            for attribute in attributes {
                if recording.attributes.len() < limit {
                    recording.attributes.push(attribute);
                } else {
                    recording.dropped_attributes_count += 1;
                }
            }
        }
    }

    pub(crate) fn add_event(&mut self, name: Text, mut attributes: Vec<KeyValue>) {
        let limit = self.tracer.0.limits.events;
        if let Some(recording) = &mut self.recording {
            if recording.events.len() < limit {
                let dropped_attributes_count = truncate(&mut attributes, EVENT_ATTRIBUTES_MAX);
                recording.events.push(Event {
                    name,
                    timestamp: SystemTime::now(),
                    attributes,
                    dropped_attributes_count,
                });
            } else {
                recording.dropped_events_count += 1;
            }
        }
    }

    pub(crate) fn add_link(&mut self, link: SpanContext) {
        let limit = self.tracer.0.limits.links;
        if let Some(recording) = &mut self.recording {
            if recording.links.len() < limit {
                recording.links.push(link);
            } else {
                recording.dropped_links_count += 1;
            }
        }
    }

    pub(crate) fn end(self, timestamp: Option<SystemTime>) {
        let Some(mut recording) = self.recording else {
            return;
        };
        let provider = &self.tracer.0;
        if provider.shut_down.load(Ordering::Relaxed) {
            return;
        }
        match timestamp {
            Some(timestamp) => recording.end_time = timestamp,
            None if recording.end_time == recording.start_time => {
                recording.end_time = SystemTime::now();
            }
            None => {}
        }
        let data = SpanData {
            context: self.context,
            parent_span_id: recording.parent_span_id,
            kind: recording.kind,
            name: recording.name,
            start_time: recording.start_time,
            end_time: recording.end_time,
            attributes: recording.attributes,
            dropped_attributes_count: recording.dropped_attributes_count,
            events: recording.events,
            dropped_events_count: recording.dropped_events_count,
            links: recording.links,
            dropped_links_count: recording.dropped_links_count,
            status: recording.status,
        };
        if let Some((last, others)) = provider.processors.split_last() {
            for processor in others {
                processor.on_end(data.clone());
            }
            last.on_end(data);
        }
    }
}
