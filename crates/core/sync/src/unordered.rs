use std::{
    cmp::Ordering as Order,
    collections::BinaryHeap,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    task::{Context, Poll, Wake, Waker},
};

use nanocode_sys::AtomicWaker;
use tokio::sync::mpsc;

use crate::{
    future::Future,
    stream::{FusedStream, Stream},
};

struct Parent {
    waker: AtomicWaker,
}

struct Task {
    index: usize,
    queued: AtomicBool,
    woken: AtomicBool,
    ready: mpsc::UnboundedSender<Arc<Self>>,
    parent: Arc<Parent>,
}

impl Wake for Task {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.woken.store(true, Ordering::Relaxed);
        if !self.queued.swap(true, Ordering::SeqCst) && self.ready.send(Arc::clone(self)).is_ok() {
            self.parent.waker.wake();
        }
    }
}

struct Entry<F, T> {
    future: Pin<Box<F>>,
    tag: T,
    task: Arc<Task>,
    waker: Waker,
}

struct Unordered<F, T> {
    entries: Vec<Option<Entry<F, T>>>,
    free: Vec<usize>,
    len: usize,
    terminated: bool,
    parent: Arc<Parent>,
    ready: mpsc::UnboundedSender<Arc<Task>>,
    queued: mpsc::UnboundedReceiver<Arc<Task>>,
}

const NEW_TASKS_MAX: usize = 2;

impl<F, T> Default for Unordered<F, T> {
    fn default() -> Self {
        let (ready, queued) = mpsc::unbounded_channel();
        Self {
            entries: Vec::new(),
            free: Vec::new(),
            len: 0,
            terminated: false,
            parent: Arc::new(Parent {
                waker: AtomicWaker::new(),
            }),
            ready,
            queued,
        }
    }
}

impl<F: Future, T> Unordered<F, T> {
    fn push(&mut self, future: F, tag: T) {
        let index = self.free.pop().unwrap_or(self.entries.len());
        let task = Arc::new(Task {
            index,
            queued: AtomicBool::new(true),
            woken: AtomicBool::new(false),
            ready: self.ready.clone(),
            parent: Arc::clone(&self.parent),
        });
        let entry = Entry {
            future: Box::pin(future),
            tag,
            waker: Waker::from(Arc::clone(&task)),
            task: Arc::clone(&task),
        };
        if index == self.entries.len() {
            self.entries.push(Some(entry));
        } else {
            assert!(
                self.entries[index].is_none(),
                "a free index holds no future"
            );
            self.entries[index] = Some(entry);
        }
        self.len += 1;
        self.terminated = false;
        assert!(
            self.ready.send(task).is_ok(),
            "the set holds its own ready queue open"
        );
    }

    fn poll_ready(&mut self, context: &mut Context<'_>) -> Poll<Option<(T, F::Output)>> {
        let len = self.len;
        let mut polled = 0;
        let mut yielded = 0;
        self.parent.waker.register(context.waker());
        loop {
            let Ok(task) = self.queued.try_recv() else {
                if self.len == 0 {
                    self.terminated = true;
                    return Poll::Ready(None);
                }
                return Poll::Pending;
            };
            let index = task.index;
            let Some(entry) = self.entries[index]
                .as_mut()
                .filter(|entry| Arc::ptr_eq(&entry.task, &task))
            else {
                continue;
            };
            assert!(
                task.queued.swap(false, Ordering::SeqCst),
                "a dequeued task was queued"
            );
            task.woken.store(false, Ordering::Relaxed);
            let mut own = Context::from_waker(&entry.waker);
            match entry.future.as_mut().poll(&mut own) {
                Poll::Pending => {
                    yielded += usize::from(task.woken.load(Ordering::Relaxed));
                    polled += 1;
                    if yielded >= NEW_TASKS_MAX || polled == len {
                        context.waker().wake_by_ref();
                        return Poll::Pending;
                    }
                }
                Poll::Ready(output) => {
                    let entry = self.entries[index]
                        .take()
                        .expect("the polled entry is there");
                    self.free.push(index);
                    self.len -= 1;
                    return Poll::Ready(Some((entry.tag, output)));
                }
            }
        }
    }
}

pub struct FuturesUnordered<F> {
    inner: Unordered<F, ()>,
}

impl<F> Default for FuturesUnordered<F> {
    fn default() -> Self {
        Self {
            inner: Unordered::default(),
        }
    }
}

impl<F: Future> FuturesUnordered<F> {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub const fn len(&self) -> usize {
        self.inner.len
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.inner.len == 0
    }

    pub fn push(&mut self, future: F) {
        self.inner.push(future, ());
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }
}

impl<F: Future> Stream for FuturesUnordered<F> {
    type Item = F::Output;

    fn poll_next(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<F::Output>> {
        self.get_mut()
            .inner
            .poll_ready(context)
            .map(|ready| ready.map(|((), output)| output))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.inner.len, Some(self.inner.len))
    }
}

impl<F: Future> FusedStream for FuturesUnordered<F> {
    fn is_terminated(&self) -> bool {
        self.inner.terminated
    }
}

impl<F: Future> FromIterator<F> for FuturesUnordered<F> {
    fn from_iter<I: IntoIterator<Item = F>>(futures: I) -> Self {
        let mut set = Self::new();
        set.extend(futures);
        set
    }
}

impl<F: Future> Extend<F> for FuturesUnordered<F> {
    fn extend<I: IntoIterator<Item = F>>(&mut self, futures: I) {
        for future in futures {
            self.push(future);
        }
    }
}

struct Finished<T> {
    index: i64,
    output: T,
}

impl<T> PartialEq for Finished<T> {
    fn eq(&self, other: &Self) -> bool {
        self.index == other.index
    }
}

impl<T> Eq for Finished<T> {}

impl<T> PartialOrd for Finished<T> {
    fn partial_cmp(&self, other: &Self) -> Option<Order> {
        Some(self.cmp(other))
    }
}

impl<T> Ord for Finished<T> {
    fn cmp(&self, other: &Self) -> Order {
        other.index.cmp(&self.index)
    }
}

pub struct FuturesOrdered<F: Future> {
    running: Unordered<F, i64>,
    finished: BinaryHeap<Finished<F::Output>>,
    next_incoming: i64,
    next_outgoing: i64,
}

impl<F: Future> Default for FuturesOrdered<F> {
    fn default() -> Self {
        Self {
            running: Unordered::default(),
            finished: BinaryHeap::new(),
            next_incoming: 0,
            next_outgoing: 0,
        }
    }
}

impl<F: Future> FuturesOrdered<F> {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.running.len + self.finished.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn push_back(&mut self, future: F) {
        self.running.push(future, self.next_incoming);
        self.next_incoming += 1;
    }

    pub fn push_front(&mut self, future: F) {
        self.next_outgoing -= 1;
        self.running.push(future, self.next_outgoing);
    }
}

impl<F: Future> Stream for FuturesOrdered<F> {
    type Item = F::Output;

    fn poll_next(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<F::Output>> {
        let this = self.get_mut();
        if this
            .finished
            .peek()
            .is_some_and(|finished| finished.index == this.next_outgoing)
        {
            this.next_outgoing += 1;
            let finished = this.finished.pop().expect("the peeked output is there");
            return Poll::Ready(Some(finished.output));
        }
        loop {
            match this.running.poll_ready(context) {
                Poll::Ready(Some((index, output))) => {
                    if index == this.next_outgoing {
                        this.next_outgoing += 1;
                        return Poll::Ready(Some(output));
                    }
                    this.finished.push(Finished { index, output });
                }
                Poll::Ready(None) => return Poll::Ready(None),
                Poll::Pending => return Poll::Pending,
            }
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let len = self.len();
        (len, Some(len))
    }
}

impl<F: Future> FusedStream for FuturesOrdered<F> {
    fn is_terminated(&self) -> bool {
        self.running.terminated && self.finished.is_empty()
    }
}

impl<F: Future> FromIterator<F> for FuturesOrdered<F> {
    fn from_iter<I: IntoIterator<Item = F>>(futures: I) -> Self {
        let mut ordered = Self::new();
        for future in futures {
            ordered.push_back(future);
        }
        ordered
    }
}

impl<F: Future> Extend<F> for FuturesOrdered<F> {
    fn extend<I: IntoIterator<Item = F>>(&mut self, futures: I) {
        for future in futures {
            self.push_back(future);
        }
    }
}

impl<F: Future> Unpin for FuturesOrdered<F> {}
