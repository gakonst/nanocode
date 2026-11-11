use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use nanocode_sync::{Atom, Map};
use opentelemetry::{
    Context,
    trace::{SpanId, SpanKind, Status, TraceId},
};
use opentelemetry_sdk::{
    Resource,
    error::OTelSdkResult,
    trace::{Span, SpanData, SpanProcessor},
};

#[derive(Clone, Copy, Debug)]
pub struct Holding {
    pub targets: &'static [&'static str],
    pub slow: Duration,
    pub one_in: u128,
    pub most: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ended {
    Span,
    Skipped(u64),
}

#[derive(Debug)]
pub struct Held<P> {
    inner: P,
    holding: Option<Holding>,
    ended: fn(Ended),
    held: Atom<Traces>,
}

#[derive(Clone, Debug, Default)]
struct Traces {
    spans: Map<TraceId, imbl::Vector<SpanData>>,
    due: imbl::Vector<(Instant, TraceId)>,
    count: usize,
}

impl Traces {
    fn take(&mut self, trace: TraceId) -> Vec<SpanData> {
        let spans = self.spans.remove(&trace).unwrap_or_default();
        self.count -= spans.len();
        spans.into_iter().collect()
    }

    fn expire(&mut self, now: Option<Instant>) -> Vec<SpanData> {
        let mut ready = Vec::new();
        while let Some(&(at, trace)) = self.due.front() {
            if now.is_some_and(|now| at > now) {
                break;
            }
            self.due.pop_front();
            ready.extend(self.take(trace));
        }
        ready
    }

    fn hold(&mut self, span: SpanData, due: Instant) {
        let trace = span.span_context.trace_id();
        let mut spans = self.spans.remove(&trace).unwrap_or_else(|| {
            self.due.push_back((due, trace));
            imbl::Vector::new()
        });
        spans.push_back(span);
        self.spans.insert(trace, spans);
        self.count += 1;
    }
}

impl<P: SpanProcessor> Held<P> {
    pub fn new(inner: P, holding: Option<Holding>, ended: fn(Ended)) -> Self {
        Self {
            inner,
            holding,
            ended,
            held: Atom::default(),
        }
    }

    fn release(&self) {
        let ready = self.held.update(|held| held.expire(None));
        for span in ready {
            self.inner.on_end(span);
        }
    }
}

impl Holding {
    fn holds(&self, span: &SpanData) -> bool {
        span.attributes.iter().any(|kv| {
            kv.key.as_str() == "target"
                && self.targets.iter().any(|target| {
                    let emitted = kv.value.as_str();
                    emitted == *target || emitted.starts_with(&format!("{target}::"))
                })
        })
    }

    fn exports(&self, root: &SpanData) -> bool {
        let took = root
            .end_time
            .duration_since(root.start_time)
            .unwrap_or_default();
        took >= self.slow
            || matches!(root.status, Status::Error { .. })
            || u128::from_be_bytes(root.span_context.trace_id().to_bytes())
                .is_multiple_of(self.one_in)
    }
}

fn request_root(span: &SpanData) -> bool {
    span.span_kind == SpanKind::Server
        && (span.parent_span_id == SpanId::INVALID || span.parent_span_is_remote)
}

fn outside(root: &SpanData, spans: Vec<SpanData>) -> (Vec<SpanData>, u64) {
    let parents: HashMap<SpanId, SpanId> = spans
        .iter()
        .map(|span| (span.span_context.span_id(), span.parent_span_id))
        .collect();
    let root = root.span_context.span_id();
    let under = |span: &SpanData| {
        let mut parent = span.parent_span_id;
        for _ in 0..=parents.len() {
            if parent == root {
                return true;
            }
            match parents.get(&parent) {
                Some(&next) => parent = next,
                None => return false,
            }
        }
        false
    };
    let (own, others): (Vec<_>, Vec<_>) = spans.into_iter().partition(under);
    (others, u64::try_from(own.len()).unwrap_or(u64::MAX))
}

impl<P: SpanProcessor> SpanProcessor for Held<P> {
    fn on_start(&self, span: &mut Span, cx: &Context) {
        self.inner.on_start(span, cx);
    }

    fn on_end(&self, span: SpanData) {
        (self.ended)(Ended::Span);
        let Some(holding) = self.holding.filter(|holding| holding.holds(&span)) else {
            self.inner.on_end(span);
            return;
        };
        let now = Instant::now();
        let (ready, skipped) = self.held.update(|held| {
            let mut ready = held.expire(Some(now));
            let mut skipped = 0;
            if request_root(&span) {
                let spans = held.take(span.span_context.trace_id());
                if holding.exports(&span) {
                    ready.extend(spans);
                    ready.push(span.clone());
                } else {
                    let (others, own) = outside(&span, spans);
                    ready.extend(others);
                    skipped = own + 1;
                }
            } else if held.count >= holding.most {
                ready.push(span.clone());
            } else {
                held.hold(span.clone(), now + holding.slow);
            }
            (ready, skipped)
        });
        for span in ready {
            self.inner.on_end(span);
        }
        if skipped > 0 {
            (self.ended)(Ended::Skipped(skipped));
        }
    }

    fn force_flush(&self) -> OTelSdkResult {
        self.release();
        self.inner.force_flush()
    }

    fn shutdown_with_timeout(&self, timeout: Duration) -> OTelSdkResult {
        self.release();
        self.inner.shutdown_with_timeout(timeout)
    }

    fn set_resource(&mut self, resource: &Resource) {
        self.inner.set_resource(resource);
    }
}
