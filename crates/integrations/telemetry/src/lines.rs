use std::fmt::{Debug, Write as _};

use nanocode_observability::{
    filter::FilterFn,
    subscriber::{Context, Filtered, Layer, SpanRef},
};
use nanocode_trace::{
    Event, Metadata,
    field::{Field, Visit},
    span::Attributes,
};

use crate::METRICS_TARGET;

pub struct Lines(pub fn(&str));

impl Layer for Lines {
    type Span = ();

    fn new_span(&self, _: &Attributes<'_>, _: &SpanRef<'_, ()>, _: &Context<'_, ()>) {}

    fn on_event(&self, event: &Event<'_>, _: &Context<'_, ()>) {
        (self.0)(&line(event));
    }
}

#[must_use]
pub fn line(event: &Event<'_>) -> String {
    let metadata = event.metadata();
    let mut line = format!("{} {}:", metadata.level(), metadata.target());
    event.record(&mut Fields(&mut line));
    line
}

struct Fields<'a>(&'a mut String);

impl Visit for Fields<'_> {
    fn record_debug(&mut self, field: &Field, value: &dyn Debug) {
        if field.name() == "message" {
            let _ = write!(self.0, " {value:?}");
        } else {
            let _ = write!(self.0, " {}={value:?}", field.name());
        }
    }
}

pub struct Counters(pub fn(String, u64));

impl Counters {
    #[must_use]
    pub fn layer(self) -> Filtered {
        Filtered::new(
            self,
            FilterFn(|metadata: &Metadata<'_>| {
                metadata.is_event() && metadata.target() == METRICS_TARGET
            }),
        )
    }
}

impl Layer for Counters {
    type Span = ();

    fn new_span(&self, _: &Attributes<'_>, _: &SpanRef<'_, ()>, _: &Context<'_, ()>) {}

    fn on_event(&self, event: &Event<'_>, _: &Context<'_, ()>) {
        event.record(&mut Counted(self.0));
    }
}

struct Counted(fn(String, u64));

impl Visit for Counted {
    fn record_u64(&mut self, field: &Field, value: u64) {
        if let Some(name) = counter_name(field.name()) {
            (self.0)(name, value);
        }
    }

    fn record_debug(&mut self, _: &Field, _: &dyn Debug) {}
}

fn counter_name(field: &str) -> Option<String> {
    let name = field.strip_prefix("monotonic_counter.")?;
    let name = name.strip_prefix("nc.").unwrap_or(name).replace('.', "_");
    Some(format!("nanocode_{name}_total"))
}

#[derive(Clone, Debug)]
pub struct Exports {
    pub targets: Vec<&'static str>,
    pub requests: Option<&'static [&'static str]>,
    pub content: bool,
}

impl Exports {
    #[must_use]
    pub fn content_from_env() -> bool {
        std::env::var("OTEL_INSTRUMENTATION_GENAI_CAPTURE_MESSAGE_CONTENT")
            .is_ok_and(|value| value.eq_ignore_ascii_case("true"))
    }

    #[must_use]
    pub fn filter(self) -> FilterFn<impl Fn(&Metadata<'_>) -> bool + Send + Sync + 'static> {
        FilterFn(move |metadata: &Metadata<'_>| {
            let from = |targets: &[&str]| {
                targets.iter().any(|target| {
                    metadata.target() == *target
                        || metadata.target().starts_with(&format!("{target}::"))
                })
            };
            *metadata.level() <= nanocode_trace::Level::INFO
                && ((metadata.is_span()
                    || self.content
                        && (!metadata.is_event()
                            || metadata.fields().field("content_kind").is_some()))
                    && from(&self.targets)
                    || metadata.is_span() && self.requests.is_some_and(from))
        })
    }
}
