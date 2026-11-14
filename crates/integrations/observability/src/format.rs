use std::{
    cell::RefCell,
    fmt::{self, Write as _},
    io,
    sync::atomic::{AtomicU64, Ordering},
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use nanocode_sync::Atom;
use nanocode_trace::{
    Event, Level,
    field::{self, Field, FieldSet, Value, Visit},
    span::{Attributes, Record},
};

use crate::{
    json, pretty,
    style::{Color, Style},
    subscriber::{Context, Layer, Scope, SpanRef},
};

const NANOS_PER_SECOND: i64 = 1_000_000_000;
const SECONDS_PER_DAY: i64 = 86_400;
const NANOS_PER_MICRO: u32 = 1_000;
const LARGEST_FOUR_DIGIT_YEAR: i64 = 9_999;
const TIMING_UNITS: [&str; 4] = ["ns", "µs", "ms", "s"];
const TIMING_STEP: f64 = 1_000.0;
const SPAN_NEW: &str = "new";
const SPAN_CLOSE: &str = "close";
const MESSAGE: &[&str] = &["message"];
const MESSAGE_AND_TIMES: &[&str] = &["message", "time.busy", "time.idle"];
const NO_COLOR: &str = "NO_COLOR";

pub trait Output: Send + Sync + 'static {
    fn write_line(&self, line: &[u8]) -> io::Result<()>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Pretty,
    Compact,
    Json,
}

pub struct FmtLayer<O> {
    format: Format,
    ansi: bool,
    output: O,
}

impl<O: Output> FmtLayer<O> {
    pub fn new(format: Format, output: O) -> Self {
        let ansi = format == Format::Pretty
            && std::env::var(NO_COLOR).map_or(true, |value| value.is_empty());
        Self {
            format,
            ansi,
            output,
        }
    }

    fn format_fields(&self, text: &mut String, fields: &dyn Fields) -> fmt::Result {
        match self.format {
            Format::Compact => {
                let mut visitor = DefaultVisitor::new(text, true);
                fields.record(&mut visitor);
                visitor.result
            }
            Format::Pretty => {
                let mut visitor = pretty::PrettyVisitor::new(text, true, self.ansi);
                fields.record(&mut visitor);
                visitor.finish()
            }
            Format::Json => json::format_fields(text, fields),
        }
    }

    fn add_fields(&self, text: &mut String, values: &Record<'_>) -> fmt::Result {
        match self.format {
            Format::Compact => {
                if !text.is_empty() {
                    text.push(' ');
                }
                self.format_fields(text, values)
            }
            Format::Pretty => {
                let empty = text.is_empty();
                let mut visitor = pretty::PrettyVisitor::new(text, empty, self.ansi);
                values.record(&mut visitor);
                visitor.finish()
            }
            Format::Json => json::add_fields(text, values),
        }
    }

    fn span_event(
        &self,
        span: &SpanRef<'_, SpanFields>,
        context: &Context<'_, SpanFields>,
        timings: Option<(TimingDisplay, TimingDisplay)>,
    ) {
        let metadata = span.metadata();
        let callsite = metadata.callsite();
        match timings {
            None => {
                let fields = FieldSet::new(MESSAGE, callsite);
                let mut names = fields.iter();
                let message = names.next().expect("the span event has a message field");
                let values = [(&message, Some(&SPAN_NEW as &dyn Value))];
                let value_set = fields.value_set(&values);
                self.on_event(&Event::new_child_of(span.id(), metadata, &value_set), context);
            }
            Some((busy, idle)) => {
                let fields = FieldSet::new(MESSAGE_AND_TIMES, callsite);
                let mut names = fields.iter();
                let message = names.next().expect("the close event has a message field");
                let busy_field = names.next().expect("the close event has a busy field");
                let idle_field = names.next().expect("the close event has an idle field");
                let busy = field::display(busy);
                let idle = field::display(idle);
                let values = [
                    (&message, Some(&SPAN_CLOSE as &dyn Value)),
                    (&busy_field, Some(&busy as &dyn Value)),
                    (&idle_field, Some(&idle as &dyn Value)),
                ];
                let value_set = fields.value_set(&values);
                self.on_event(&Event::new_child_of(span.id(), metadata, &value_set), context);
            }
        }
    }
}

pub(crate) trait Fields {
    fn record(&self, visitor: &mut dyn Visit);
}

impl Fields for Attributes<'_> {
    fn record(&self, visitor: &mut dyn Visit) {
        Attributes::record(self, visitor);
    }
}

impl Fields for Record<'_> {
    fn record(&self, visitor: &mut dyn Visit) {
        Record::record(self, visitor);
    }
}

impl Fields for Event<'_> {
    fn record(&self, visitor: &mut dyn Visit) {
        Event::record(self, visitor);
    }
}

pub struct SpanFields {
    fields: Atom<Option<String>>,
    timings: Timings,
}

impl SpanFields {
    pub(crate) fn text<R>(&self, read: impl FnOnce(Option<&str>) -> R) -> R {
        self.fields.read(|fields| read(fields.as_deref()))
    }
}

struct Timings {
    created: Instant,
    idle: AtomicU64,
    busy: AtomicU64,
    last: AtomicU64,
    entered_count: AtomicU64,
}

impl Timings {
    fn new() -> Self {
        Self {
            created: nanocode_platform::now(),
            idle: AtomicU64::new(0),
            busy: AtomicU64::new(0),
            last: AtomicU64::new(0),
            entered_count: AtomicU64::new(0),
        }
    }

    fn now(&self) -> u64 {
        u64::try_from(
            nanocode_platform::now()
                .saturating_duration_since(self.created)
                .as_nanos(),
        )
        .expect("a span's age fits in u64 nanoseconds")
    }

    fn enter(&self) {
        if self.entered_count.fetch_add(1, Ordering::AcqRel) == 0 {
            let now = self.now();
            let last = self.last.swap(now, Ordering::AcqRel);
            self.idle.fetch_add(now.saturating_sub(last), Ordering::AcqRel);
        }
    }

    fn exit(&self) {
        let previous = self
            .entered_count
            .try_update(Ordering::AcqRel, Ordering::Acquire, |count| count.checked_sub(1))
            .expect("a span exits after it entered");
        if previous == 1 {
            let now = self.now();
            let last = self.last.swap(now, Ordering::AcqRel);
            self.busy.fetch_add(now.saturating_sub(last), Ordering::AcqRel);
        }
    }

    fn close(&self) -> (TimingDisplay, TimingDisplay) {
        let idle = self.idle.load(Ordering::Acquire)
            + self.now().saturating_sub(self.last.load(Ordering::Acquire));
        (
            TimingDisplay(self.busy.load(Ordering::Acquire)),
            TimingDisplay(idle),
        )
    }
}

impl<O: Output> Layer for FmtLayer<O> {
    type Span = SpanFields;

    fn new_span(
        &self,
        attributes: &Attributes<'_>,
        _span: &SpanRef<'_, SpanFields>,
        _context: &Context<'_, SpanFields>,
    ) -> SpanFields {
        let mut text = String::new();
        let fields = if self.format_fields(&mut text, attributes).is_ok() {
            Some(text)
        } else {
            eprintln!(
                "[tracing-subscriber] Unable to format the following event, ignoring: {attributes:?}"
            );
            None
        };
        SpanFields {
            fields: Atom::new(fields),
            timings: Timings::new(),
        }
    }

    fn on_new_span(
        &self,
        _attributes: &Attributes<'_>,
        span: &SpanRef<'_, SpanFields>,
        context: &Context<'_, SpanFields>,
    ) {
        self.span_event(span, context, None);
    }

    fn on_record(
        &self,
        span: &SpanRef<'_, SpanFields>,
        values: &Record<'_>,
        _context: &Context<'_, SpanFields>,
    ) {
        let data = span.data().expect("a visible span has its layer's data");
        data.fields.update(|fields| match fields {
            Some(text) => drop(self.add_fields(text, values)),
            None => {
                let mut text = String::new();
                if self.format_fields(&mut text, values).is_ok() {
                    *fields = Some(text);
                }
            }
        });
    }

    fn on_enter(&self, span: &SpanRef<'_, SpanFields>, _context: &Context<'_, SpanFields>) {
        let data = span.data().expect("a visible span has its layer's data");
        data.timings.enter();
    }

    fn on_exit(&self, span: &SpanRef<'_, SpanFields>, _context: &Context<'_, SpanFields>) {
        let data = span.data().expect("a visible span has its layer's data");
        data.timings.exit();
    }

    fn on_close(&self, span: &SpanRef<'_, SpanFields>, context: &Context<'_, SpanFields>) {
        let data = span.data().expect("a visible span has its layer's data");
        self.span_event(span, context, Some(data.timings.close()));
    }

    fn on_event(&self, event: &Event<'_>, context: &Context<'_, SpanFields>) {
        thread_local! {
            static BUFFER: RefCell<String> = const { RefCell::new(String::new()) };
        }
        BUFFER.with(|buffer| {
            let mut fresh = String::new();
            let mut borrowed = buffer.try_borrow_mut();
            let line = match &mut borrowed {
                Ok(buffer) => &mut **buffer,
                Err(_) => &mut fresh,
            };
            let formatted = match self.format {
                Format::Compact => format_compact(context, line, event),
                Format::Pretty => pretty::format_event(context, line, event, self.ansi),
                Format::Json => json::format_event(context, line, event),
            };
            if formatted.is_ok() {
                drop(self.output.write_line(line.as_bytes()));
            }
            line.clear();
        });
    }
}

pub(crate) struct EventMeta<'a> {
    pub(crate) target: &'a str,
    pub(crate) level: Level,
    pub(crate) file: Option<&'a str>,
    pub(crate) line: Option<u32>,
}

pub(crate) const fn event_meta<'a>(event: &'a Event<'a>) -> EventMeta<'a> {
    let metadata = event.metadata();
    EventMeta {
        target: metadata.target(),
        level: *metadata.level(),
        file: metadata.file(),
        line: metadata.line(),
    }
}

pub(crate) fn write_timestamp(writer: &mut String) -> fmt::Result {
    write!(writer, "{}", DateTime::from(nanocode_platform::system_time()))
}

pub(crate) fn current_scope<'a>(
    context: &Context<'a, SpanFields>,
    event: &Event<'_>,
) -> Option<Scope<'a, SpanFields>> {
    event
        .parent()
        .and_then(|id| context.span(id))
        .or_else(|| context.lookup_current())
        .map(|span| span.scope())
}

fn format_compact(
    context: &Context<'_, SpanFields>,
    writer: &mut String,
    event: &Event<'_>,
) -> fmt::Result {
    let meta = event_meta(event);
    write_timestamp(writer)?;
    writer.push(' ');
    write!(writer, "{} ", FmtLevel::new(meta.level, false))?;
    let mut seen = false;
    for span in current_scope(context, event)
        .into_iter()
        .flat_map(Scope::from_root)
    {
        seen = true;
        write!(writer, "{}:", span.name())?;
    }
    if seen {
        writer.push(' ');
    }
    write!(writer, "{}:", meta.target)?;
    writer.push(' ');
    let mut visitor = DefaultVisitor::new(writer, true);
    event.record(&mut visitor);
    visitor.result?;
    for span in context
        .event_scope(event)
        .into_iter()
        .flat_map(Scope::from_root)
    {
        if let Some(data) = span.data() {
            data.text(|text| match text {
                Some(text) if !text.is_empty() => write!(writer, " {text}"),
                _ => Ok(()),
            })?;
        }
    }
    writer.push('\n');
    Ok(())
}

pub(crate) struct DefaultVisitor<'a> {
    writer: &'a mut String,
    is_empty: bool,
    result: fmt::Result,
}

impl<'a> DefaultVisitor<'a> {
    pub(crate) const fn new(writer: &'a mut String, is_empty: bool) -> Self {
        Self {
            writer,
            is_empty,
            result: Ok(()),
        }
    }

    fn maybe_pad(&mut self) {
        if self.is_empty {
            self.is_empty = false;
        } else {
            self.result = write!(self.writer, " ");
        }
    }
}

impl Visit for DefaultVisitor<'_> {
    fn record_str(&mut self, field: &Field, value: &str) {
        if self.result.is_err() {
            return;
        }
        if field.name() == "message" {
            self.record_debug(field, &format_args!("{value}"));
        } else {
            self.record_debug(field, &value);
        }
    }

    fn record_error(&mut self, field: &Field, value: &(dyn std::error::Error + 'static)) {
        if let Some(source) = value.source() {
            self.record_debug(
                field,
                &format_args!(
                    "{} {}{}{}{}",
                    EscapeGuard(format_args!("{value}")),
                    field.name(),
                    ".sources",
                    "=",
                    ErrorSourceList(source)
                ),
            );
        } else {
            self.record_debug(field, &format_args!("{}", EscapeGuard(format_args!("{value}"))));
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        if self.result.is_err() {
            return;
        }
        let name = field.name();
        self.maybe_pad();
        self.result = match name {
            "message" => write!(self.writer, "{:?}", EscapeGuard(value)),
            name if name.starts_with("r#") => write!(self.writer, "{}={value:?}", &name[2..]),
            name => write!(self.writer, "{name}={value:?}"),
        };
    }
}

pub(crate) struct EscapeGuard<T>(pub(crate) T);

struct EscapingWriter<'a, 'b>(&'a mut fmt::Formatter<'b>);

impl fmt::Write for EscapingWriter<'_, '_> {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        for character in text.chars() {
            match character {
                '\x1b' => self.0.write_str("\\x1b")?,
                '\x07' => self.0.write_str("\\x07")?,
                '\x08' => self.0.write_str("\\x08")?,
                '\x0c' => self.0.write_str("\\x0c")?,
                '\x7f' => self.0.write_str("\\x7f")?,
                '\u{80}'..='\u{9f}' => write!(self.0, "\\u{{{:x}}}", character as u32)?,
                _ => self.0.write_char(character)?,
            }
        }
        Ok(())
    }
}

impl<T: fmt::Debug> fmt::Debug for EscapeGuard<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(EscapingWriter(formatter), "{:?}", self.0)
    }
}

impl<T: fmt::Display> fmt::Display for EscapeGuard<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(EscapingWriter(formatter), "{}", self.0)
    }
}

pub(crate) struct ErrorSourceList<'a>(pub(crate) &'a (dyn std::error::Error + 'static));

impl fmt::Display for ErrorSourceList<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut list = formatter.debug_list();
        let mut current = Some(self.0);
        while let Some(error) = current {
            list.entry(&EscapeGuard(format_args!("{error}")));
            current = error.source();
        }
        list.finish()
    }
}

pub(crate) struct FmtLevel {
    level: Level,
    ansi: bool,
}

impl FmtLevel {
    pub(crate) const fn new(level: Level, ansi: bool) -> Self {
        Self { level, ansi }
    }
}

impl fmt::Display for FmtLevel {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (text, color) = match self.level {
            Level::TRACE => ("TRACE", Color::Purple),
            Level::DEBUG => ("DEBUG", Color::Blue),
            Level::INFO => (" INFO", Color::Green),
            Level::WARN => (" WARN", Color::Yellow),
            Level::ERROR => ("ERROR", Color::Red),
        };
        if self.ansi {
            write!(formatter, "{}", color.paint(text))
        } else {
            formatter.pad(text)
        }
    }
}

pub(crate) const fn level_style(level: Level) -> Style {
    let color = match level {
        Level::TRACE => Color::Purple,
        Level::DEBUG => Color::Blue,
        Level::INFO => Color::Green,
        Level::WARN => Color::Yellow,
        Level::ERROR => Color::Red,
    };
    Style::new().foreground(color)
}

pub(crate) struct TimingDisplay(pub(crate) u64);

impl fmt::Display for TimingDisplay {
    #[allow(clippy::cast_precision_loss)]
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut time = self.0 as f64;
        for unit in TIMING_UNITS {
            if time < 10.0 {
                return write!(formatter, "{time:.2}{unit}");
            } else if time < 100.0 {
                return write!(formatter, "{time:.1}{unit}");
            } else if time < 1000.0 {
                return write!(formatter, "{time:.0}{unit}");
            }
            time /= TIMING_STEP;
        }
        write!(formatter, "{:.0}s", time * TIMING_STEP)
    }
}

pub(crate) struct DateTime {
    year: i64,
    month: u8,
    day: u8,
    hour: u8,
    minute: u8,
    second: u8,
    nanos: u32,
}

impl fmt::Display for DateTime {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.year > LARGEST_FOUR_DIGIT_YEAR {
            write!(formatter, "+{}", self.year)?;
        } else if self.year < 0 {
            write!(formatter, "{:05}", self.year)?;
        } else {
            write!(formatter, "{:04}", self.year)?;
        }
        write!(
            formatter,
            "-{:02}-{:02}T{:02}:{:02}:{:02}.{:06}Z",
            self.month,
            self.day,
            self.hour,
            self.minute,
            self.second,
            self.nanos / NANOS_PER_MICRO
        )
    }
}

impl From<SystemTime> for DateTime {
    fn from(timestamp: SystemTime) -> Self {
        const LEAP_EPOCH: i64 = 946_684_800 + SECONDS_PER_DAY * (31 + 29);
        const DAYS_PER_400_YEARS: i64 = 365 * 400 + 97;
        const DAYS_PER_100_YEARS: i64 = 365 * 100 + 24;
        const DAYS_PER_4_YEARS: i64 = 365 * 4 + 1;
        const DAYS_IN_MONTH: [i64; 12] = [31, 30, 31, 30, 31, 31, 30, 31, 30, 31, 31, 29];
        let (seconds, nanos) = match timestamp.duration_since(UNIX_EPOCH) {
            Ok(duration) => (
                i64::try_from(duration.as_secs()).expect("seconds since 1970 fit in i64"),
                duration.subsec_nanos(),
            ),
            Err(error) => {
                let duration = error.duration();
                let seconds =
                    i64::try_from(duration.as_secs()).expect("seconds before 1970 fit in i64");
                match duration.subsec_nanos() {
                    0 => (-seconds, 0),
                    nanos => (
                        -seconds - 1,
                        u32::try_from(NANOS_PER_SECOND).expect("a second's nanoseconds fit in u32")
                            - nanos,
                    ),
                }
            }
        };
        let mut days = seconds / SECONDS_PER_DAY - LEAP_EPOCH / SECONDS_PER_DAY;
        let mut remaining_seconds = seconds % SECONDS_PER_DAY;
        if remaining_seconds < 0 {
            remaining_seconds += SECONDS_PER_DAY;
            days -= 1;
        }
        let mut cycles_400 = days / DAYS_PER_400_YEARS;
        let mut remaining_days = days % DAYS_PER_400_YEARS;
        if remaining_days < 0 {
            remaining_days += DAYS_PER_400_YEARS;
            cycles_400 -= 1;
        }
        let cycles_100 = (remaining_days / DAYS_PER_100_YEARS).min(3);
        remaining_days -= cycles_100 * DAYS_PER_100_YEARS;
        let cycles_4 = (remaining_days / DAYS_PER_4_YEARS).min(24);
        remaining_days -= cycles_4 * DAYS_PER_4_YEARS;
        let remaining_years = (remaining_days / 365).min(3);
        remaining_days -= remaining_years * 365;
        let mut years = remaining_years + 4 * cycles_4 + 100 * cycles_100 + 400 * cycles_400;
        let mut months = 0;
        while DAYS_IN_MONTH[months] <= remaining_days {
            remaining_days -= DAYS_IN_MONTH[months];
            months += 1;
        }
        let mut month = i64::try_from(months).expect("a month index fits in i64");
        if month >= 10 {
            month -= 12;
            years += 1;
        }
        let narrow = |value: i64| u8::try_from(value).expect("a date field fits in u8");
        Self {
            year: years + 2000,
            month: narrow(month + 3),
            day: narrow(remaining_days + 1),
            hour: narrow(remaining_seconds / 3600),
            minute: narrow(remaining_seconds / 60 % 60),
            second: narrow(remaining_seconds % 60),
            nanos,
        }
    }
}

