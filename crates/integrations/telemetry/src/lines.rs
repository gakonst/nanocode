use std::fmt::{Debug, Write as _};

use tracing::{
    Event, Metadata, Subscriber,
    field::{Field, Visit},
};
use tracing_subscriber::{
    Layer,
    filter::{FilterFn, filter_fn},
    layer::Context,
    registry::LookupSpan,
};

use crate::METRICS_TARGET;

pub struct Lines(pub fn(&str));

impl<S: Subscriber> Layer<S> for Lines {
    fn on_event(&self, event: &Event<'_>, _: Context<'_, S>) {
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
    pub fn layer<S>(self) -> impl Layer<S>
    where
        S: Subscriber + for<'span> LookupSpan<'span>,
    {
        self.with_filter(filter_fn(|metadata| {
            metadata.is_event() && metadata.target() == METRICS_TARGET
        }))
    }
}

impl<S: Subscriber> Layer<S> for Counters {
    fn on_event(&self, event: &Event<'_>, _: Context<'_, S>) {
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

    pub fn filter(self) -> FilterFn<impl Fn(&Metadata<'_>) -> bool> {
        filter_fn(move |metadata| {
            let from = |targets: &[&str]| {
                targets.iter().any(|target| {
                    metadata.target() == *target
                        || metadata.target().starts_with(&format!("{target}::"))
                })
            };
            *metadata.level() <= tracing::Level::INFO
                && ((metadata.is_span()
                    || self.content
                        && (!metadata.is_event()
                            || metadata.fields().field("content_kind").is_some()))
                    && from(&self.targets)
                    || metadata.is_span() && self.requests.is_some_and(from))
        })
    }
}
