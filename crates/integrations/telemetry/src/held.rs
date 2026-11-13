use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use nanocode_observability::{ExportError, SpanData, SpanKind, SpanProcessor, Status};
use nanocode_sync::{Atom, Map};

use crate::genai::shown;

const NO_PARENT: u64 = 0;

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
    spans: Map<(u128, usize), SpanData>,
    lengths: Map<u128, usize>,
    due: Map<u64, (Instant, u128)>,
    first_due: u64,
    next_due: u64,
    count: usize,
}

impl Traces {
    fn take(&mut self, trace: u128) -> Vec<SpanData> {
        let length = self.lengths.remove(&trace).unwrap_or(0);
        let spans: Vec<SpanData> = (0..length)
            .map(|position| {
                self.spans
                    .remove(&(trace, position))
                    .expect("a held trace holds a span at each position below its length")
            })
            .collect();
        self.count -= spans.len();
        spans
    }

    fn expire(&mut self, now: Option<Instant>) -> Vec<SpanData> {
        let mut ready = Vec::new();
        for _ in self.first_due..self.next_due {
            let &(at, trace) = self
                .due
                .get(&self.first_due)
                .expect("every due position from first to next is held");
            if now.is_some_and(|now| at > now) {
                break;
            }
            self.due.remove(&self.first_due);
            self.first_due += 1;
            ready.extend(self.take(trace));
        }
        ready
    }

    fn hold(&mut self, span: SpanData, due: Instant) {
        let trace = span.context.trace_id;
        let length = match self.lengths.get(&trace) {
            Some(&length) => length,
            None => {
                self.due.insert(self.next_due, (due, trace));
                self.next_due += 1;
                0
            }
        };
        self.spans.insert((trace, length), span);
        self.lengths.insert(trace, length + 1);
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
        span.attributes.iter().any(|pair| {
            pair.key == "target"
                && self.targets.iter().any(|target| {
                    let emitted = shown(&pair.value);
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
            || matches!(root.status, Status::Error(_))
            || root.context.trace_id.is_multiple_of(self.one_in)
    }
}

fn request_root(span: &SpanData) -> bool {
    span.kind == SpanKind::Server && span.parent_span_id == NO_PARENT
}

fn outside(root: &SpanData, spans: Vec<SpanData>) -> (Vec<SpanData>, u64) {
    let parents: HashMap<u64, u64> = spans
        .iter()
        .map(|span| (span.context.span_id, span.parent_span_id))
        .collect();
    let root = root.context.span_id;
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
    (
        others,
        u64::try_from(own.len()).expect("a span count fits in u64"),
    )
}

impl<P: SpanProcessor> SpanProcessor for Held<P> {
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
                let spans = held.take(span.context.trace_id);
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

    fn shutdown_with_timeout(&self, timeout: Duration) -> Result<(), ExportError> {
        self.release();
        self.inner.shutdown_with_timeout(timeout)
    }
}
