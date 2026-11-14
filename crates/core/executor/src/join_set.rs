use std::{
    fmt,
    future::{Future, poll_fn},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    task::{Context, Poll, Wake, Waker},
};

use nanocode_sys::AtomicWaker;

use crate::{
    queue::Shared,
    runtime::Handle,
    task::{AbortHandle, Id, JoinError, JoinHandle},
};

struct Ready {
    indices: Shared<usize>,
    waker: AtomicWaker,
}

struct Entry {
    index: usize,
    queued: AtomicBool,
    ready: Arc<Ready>,
}

impl Wake for Entry {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        if !self.queued.swap(true, Ordering::AcqRel) {
            self.ready.indices.push(self.index);
        }
        self.ready.waker.wake();
    }
}

struct Slot<T> {
    handle: JoinHandle<T>,
    entry: Arc<Entry>,
    waker: Waker,
}

pub struct JoinSet<T> {
    slots: Vec<Option<Slot<T>>>,
    free: Vec<usize>,
    ready: Arc<Ready>,
    len: usize,
}

impl<T> Default for JoinSet<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> fmt::Debug for JoinSet<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("JoinSet")
            .field("len", &self.len)
            .finish()
    }
}

impl<T> JoinSet<T> {
    #[must_use]
    pub fn new() -> Self {
        Self {
            slots: Vec::new(),
            free: Vec::new(),
            ready: Arc::new(Ready {
                indices: Shared::new(),
                waker: AtomicWaker::new(),
            }),
            len: 0,
        }
    }

    #[must_use]
    pub const fn len(&self) -> usize {
        self.len
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn push(&mut self, handle: JoinHandle<T>) -> AbortHandle {
        let abort = handle.abort_handle();
        let index = self.free.pop().unwrap_or(self.slots.len());
        let entry = Arc::new(Entry {
            index,
            queued: AtomicBool::new(true),
            ready: self.ready.clone(),
        });
        self.ready.indices.push(index);
        let slot = Slot {
            handle,
            waker: Waker::from(entry.clone()),
            entry,
        };
        if index == self.slots.len() {
            self.slots.push(Some(slot));
        } else {
            self.slots[index] = Some(slot);
        }
        self.len += 1;
        abort
    }

    #[track_caller]
    pub fn spawn<F>(&mut self, future: F) -> AbortHandle
    where
        F: Future<Output = T> + Send + 'static,
        T: Send + 'static,
    {
        self.push(Handle::current().spawn(future))
    }

    #[track_caller]
    pub fn spawn_on<F>(&mut self, future: F, handle: &Handle) -> AbortHandle
    where
        F: Future<Output = T> + Send + 'static,
        T: Send + 'static,
    {
        self.push(handle.spawn(future))
    }

    #[track_caller]
    pub fn spawn_blocking<F>(&mut self, work: F) -> AbortHandle
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        self.push(Handle::current().spawn_blocking(work))
    }

    pub fn poll_join_next(
        &mut self,
        context: &mut Context<'_>,
    ) -> Poll<Option<Result<T, JoinError>>> {
        self.poll_join_next_with_id(context)
            .map(|joined| joined.map(|joined| joined.map(|(_, output)| output)))
    }

    #[allow(clippy::type_complexity)]
    pub fn poll_join_next_with_id(
        &mut self,
        context: &mut Context<'_>,
    ) -> Poll<Option<Result<(Id, T), JoinError>>> {
        if self.len == 0 {
            return Poll::Ready(None);
        }
        self.ready.waker.register(context.waker());
        while let Some(index) = self.ready.indices.pop() {
            let Some(Some(slot)) = self.slots.get_mut(index) else {
                continue;
            };
            slot.entry.queued.store(false, Ordering::Release);
            let id = slot.handle.id();
            let mut entry_context = Context::from_waker(&slot.waker);
            let Poll::Ready(output) = std::pin::Pin::new(&mut slot.handle).poll(&mut entry_context)
            else {
                continue;
            };
            self.slots[index] = None;
            self.free.push(index);
            self.len -= 1;
            return Poll::Ready(Some(output.map(|output| (id, output))));
        }
        Poll::Pending
    }

    pub async fn join_next(&mut self) -> Option<Result<T, JoinError>> {
        poll_fn(|context| self.poll_join_next(context)).await
    }

    pub async fn join_next_with_id(&mut self) -> Option<Result<(Id, T), JoinError>> {
        poll_fn(|context| self.poll_join_next_with_id(context)).await
    }

    pub fn try_join_next(&mut self) -> Option<Result<T, JoinError>> {
        let waker = Waker::noop();
        match self.poll_join_next(&mut Context::from_waker(waker)) {
            Poll::Ready(joined) => joined,
            Poll::Pending => None,
        }
    }

    pub async fn join_all(mut self) -> Vec<T> {
        let mut outputs = Vec::with_capacity(self.len);
        while let Some(joined) = self.join_next().await {
            match joined {
                Ok(output) => outputs.push(output),
                Err(error) if error.is_panic() => std::panic::resume_unwind(error.into_panic()),
                Err(error) => panic!("{error}"),
            }
        }
        outputs
    }

    pub fn abort_all(&mut self) {
        for slot in self.slots.iter().flatten() {
            slot.handle.abort();
        }
    }

    pub fn detach_all(&mut self) {
        self.slots.clear();
        self.free.clear();
        self.len = 0;
        drop(self.ready.indices.drain());
    }

    pub async fn shutdown(&mut self) {
        self.abort_all();
        while self.join_next().await.is_some() {}
    }
}

impl<T> Drop for JoinSet<T> {
    fn drop(&mut self) {
        self.abort_all();
    }
}

impl<T, F> FromIterator<F> for JoinSet<T>
where
    F: Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    fn from_iter<I: IntoIterator<Item = F>>(iter: I) -> Self {
        let mut set = Self::new();
        for future in iter {
            set.spawn(future);
        }
        set
    }
}
