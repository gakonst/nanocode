use std::{
    future::Future,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    task::{Context, Poll, Waker},
};

use crate::{Stream, StreamExt};
use nanocode_sys::AtomicWaker;

pub struct InOrder<F> {
    parent: Arc<AtomicWaker>,
    entries: Vec<Entry<F>>,
}

struct Entry<F> {
    future: Pin<Box<F>>,
    woken: Arc<Woken>,
    waker: Waker,
}

struct Woken {
    flagged: AtomicBool,
    parent: Arc<AtomicWaker>,
}

impl std::task::Wake for Woken {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.flagged.store(true, Ordering::Release);
        self.parent.wake();
    }
}

impl<F> Default for InOrder<F> {
    fn default() -> Self {
        Self {
            parent: Arc::default(),
            entries: Vec::new(),
        }
    }
}

impl<F: Future> InOrder<F> {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, future: F) {
        let woken = Arc::new(Woken {
            flagged: AtomicBool::new(true),
            parent: Arc::clone(&self.parent),
        });
        let waker = Waker::from(Arc::clone(&woken));
        self.entries.push(Entry {
            future: Box::pin(future),
            woken,
            waker,
        });
    }

    #[must_use]
    pub const fn len(&self) -> usize {
        self.entries.len()
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn poll_ready(&mut self, context: &mut Context<'_>) -> Poll<Option<F::Output>> {
        self.parent.register(context.waker());
        for at in 0..self.entries.len() {
            let entry = &mut self.entries[at];
            if !entry.woken.flagged.load(Ordering::Acquire)
                || !entry.woken.flagged.swap(false, Ordering::AcqRel)
            {
                continue;
            }
            let mut own = Context::from_waker(&entry.waker);
            if let Poll::Ready(output) = entry.future.as_mut().poll(&mut own) {
                drop(self.entries.remove(at));
                return Poll::Ready(Some(output));
            }
        }
        if self.entries.is_empty() {
            Poll::Ready(None)
        } else {
            Poll::Pending
        }
    }
}

impl<F: Future> Stream for InOrder<F> {
    type Item = F::Output;

    fn poll_next(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<F::Output>> {
        self.get_mut().poll_ready(context)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.entries.len(), Some(self.entries.len()))
    }
}

pub async fn join_in_order<F: Future>(futures: impl IntoIterator<Item = F>) -> Vec<F::Output> {
    let mut pending = InOrder::new();
    for (at, future) in futures.into_iter().enumerate() {
        pending.push(async move { (at, future.await) });
    }
    let mut outputs = std::iter::repeat_with(|| None)
        .take(pending.len())
        .collect::<Vec<_>>();
    while let Some((at, output)) = pending.next().await {
        outputs[at] = Some(output);
    }
    outputs
        .into_iter()
        .map(|output| output.expect("every joined future finished"))
        .collect()
}

pub async fn try_join_in_order<F, T, E>(futures: impl IntoIterator<Item = F>) -> Result<Vec<T>, E>
where
    F: Future<Output = Result<T, E>>,
{
    let mut pending = InOrder::new();
    for (at, future) in futures.into_iter().enumerate() {
        pending.push(async move { (at, future.await) });
    }
    let mut outputs = std::iter::repeat_with(|| None)
        .take(pending.len())
        .collect::<Vec<_>>();
    while let Some((at, output)) = pending.next().await {
        outputs[at] = Some(output?);
    }
    Ok(outputs
        .into_iter()
        .map(|output| output.expect("every joined future finished"))
        .collect())
}
