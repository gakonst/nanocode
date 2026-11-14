use std::{
    any::Any,
    cell::Cell,
    fmt,
    future::Future,
    io,
    panic::{AssertUnwindSafe, catch_unwind},
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering},
    },
    task::{Context, Poll, Wake, Waker},
};

use nanocode_sys::{AtomicBox, AtomicPinBox, AtomicWaker, Exclusive};

use crate::runtime::Shared;

const IDLE: u8 = 0;
const SCHEDULED: u8 = 1;
const RUNNING: u8 = 2;
const NOTIFIED: u8 = 3;
const DONE: u8 = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Id(u64);

impl fmt::Display for Id {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

impl Id {
    pub(crate) fn next() -> Self {
        Self(NEXT_ID.fetch_add(1, Ordering::Relaxed))
    }
}

thread_local! {
    static CURRENT: Cell<Option<Id>> = const { Cell::new(None) };
}

#[must_use]
pub fn try_id() -> Option<Id> {
    CURRENT.with(Cell::get)
}

#[must_use]
pub fn id() -> Id {
    try_id().expect("can't get a task id when not inside a task")
}

pub(crate) struct Current(Option<Id>);

impl Current {
    pub(crate) fn enter(id: Option<Id>) -> Self {
        Self(CURRENT.with(|current| current.replace(id)))
    }
}

impl Drop for Current {
    fn drop(&mut self) {
        CURRENT.with(|current| current.set(self.0));
    }
}

enum Repr {
    Cancelled,
    Panic(Exclusive<Box<dyn Any + Send + 'static>>, Option<String>),
}

pub struct JoinError {
    id: Id,
    repr: Repr,
}

impl JoinError {
    pub(crate) const fn cancelled(id: Id) -> Self {
        Self {
            id,
            repr: Repr::Cancelled,
        }
    }

    pub(crate) fn panic(id: Id, payload: Box<dyn Any + Send + 'static>) -> Self {
        let message = panic_message(payload.as_ref()).map(str::to_owned);
        Self {
            id,
            repr: Repr::Panic(Exclusive::new(payload), message),
        }
    }

    #[must_use]
    pub const fn is_cancelled(&self) -> bool {
        matches!(self.repr, Repr::Cancelled)
    }

    #[must_use]
    pub const fn is_panic(&self) -> bool {
        matches!(self.repr, Repr::Panic(..))
    }

    #[must_use]
    pub fn into_panic(self) -> Box<dyn Any + Send + 'static> {
        self.try_into_panic()
            .expect("`JoinError` reason is not a panic.")
    }

    pub fn try_into_panic(self) -> Result<Box<dyn Any + Send + 'static>, Self> {
        match self.repr {
            Repr::Panic(payload, _) => Ok(payload.into_inner()),
            Repr::Cancelled => Err(self),
        }
    }

    #[must_use]
    pub const fn id(&self) -> Id {
        self.id
    }
}

fn panic_message(payload: &(dyn Any + Send)) -> Option<&str> {
    payload
        .downcast_ref::<&'static str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
}

impl fmt::Display for JoinError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.repr {
            Repr::Cancelled => write!(formatter, "task {} was cancelled", self.id),
            Repr::Panic(_, message) => match message {
                Some(message) => {
                    write!(
                        formatter,
                        "task {} panicked with message {message:?}",
                        self.id
                    )
                }
                None => write!(formatter, "task {} panicked", self.id),
            },
        }
    }
}

impl fmt::Debug for JoinError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.repr {
            Repr::Cancelled => write!(formatter, "JoinError::Cancelled({:?})", self.id),
            Repr::Panic(_, message) => match message {
                Some(message) => {
                    write!(
                        formatter,
                        "JoinError::Panic({:?}, {message:?}, ...)",
                        self.id
                    )
                }
                None => write!(formatter, "JoinError::Panic({:?}, ...)", self.id),
            },
        }
    }
}

impl std::error::Error for JoinError {}

impl From<JoinError> for io::Error {
    fn from(error: JoinError) -> Self {
        Self::other(match error.repr {
            Repr::Cancelled => "task was cancelled",
            Repr::Panic(..) => "task panicked",
        })
    }
}

pub(crate) trait Runnable: Send + Sync {
    fn run(self: Arc<Self>);
    fn cancel(&self);
    fn done(&self) -> bool;
}

pub(crate) trait Joinable<T>: Send + Sync {
    fn poll_join(&self, context: &mut Context<'_>) -> Poll<Result<T, JoinError>>;
    fn is_finished(&self) -> bool;
    fn id(&self) -> Id;
}

pub(crate) trait Abortable: Send + Sync {
    fn abort_task(self: Arc<Self>);
    fn finished(&self) -> bool;
    fn task_id(&self) -> Id;
}

pub(crate) struct Task<F: Future> {
    state: AtomicU8,
    aborted: AtomicBool,
    finished: AtomicBool,
    id: Id,
    shared: Arc<Shared>,
    future: AtomicPinBox<F>,
    output: AtomicBox<Result<F::Output, JoinError>>,
    joiner: AtomicWaker,
}

impl<F> Task<F>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    pub(crate) fn new(shared: Arc<Shared>, future: F) -> Arc<Self> {
        Arc::new(Self {
            state: AtomicU8::new(SCHEDULED),
            aborted: AtomicBool::new(false),
            finished: AtomicBool::new(false),
            id: Id::next(),
            shared,
            future: AtomicPinBox::new(future),
            output: AtomicBox::empty(),
            joiner: AtomicWaker::new(),
        })
    }

    fn complete(&self, output: Result<F::Output, JoinError>) {
        self.output.store(Some(Box::new(output)));
        self.finished.store(true, Ordering::Release);
        self.state.store(DONE, Ordering::Release);
        self.shared.finished();
        self.joiner.wake();
    }

    fn schedule(self: &Arc<Self>) {
        let mut state = self.state.load(Ordering::Acquire);
        loop {
            let next = match state {
                IDLE => SCHEDULED,
                RUNNING => NOTIFIED,
                _ => return,
            };
            match self
                .state
                .compare_exchange_weak(state, next, Ordering::AcqRel, Ordering::Acquire)
            {
                Ok(_) => break,
                Err(actual) => state = actual,
            }
        }
        if state == IDLE {
            self.shared.schedule(self.clone());
        }
    }
}

impl<F> Wake for Task<F>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    fn wake(self: Arc<Self>) {
        self.schedule();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.schedule();
    }
}

impl<F> Runnable for Task<F>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    fn run(self: Arc<Self>) {
        self.state.store(RUNNING, Ordering::Release);
        let Some(mut future) = self.future.take() else {
            return;
        };
        if self.aborted.load(Ordering::Acquire) || self.shared.is_shutdown() {
            drop(future);
            self.complete(Err(JoinError::cancelled(self.id)));
            return;
        }
        let waker = Waker::from(self.clone());
        let polled = {
            let _current = Current::enter(Some(self.id));
            let _budget = crate::coop::Budget::enter();
            let mut context = Context::from_waker(&waker);
            catch_unwind(AssertUnwindSafe(|| future.as_mut().poll(&mut context)))
        };
        match polled {
            Ok(Poll::Ready(output)) => {
                drop(future);
                self.complete(Ok(output));
            }
            Ok(Poll::Pending) => {
                self.future.store(Some(future));
                if self
                    .state
                    .compare_exchange(RUNNING, IDLE, Ordering::AcqRel, Ordering::Acquire)
                    .is_err()
                {
                    self.state.store(SCHEDULED, Ordering::Release);
                    self.shared.schedule(self.clone());
                }
            }
            Err(payload) => {
                let _ = catch_unwind(AssertUnwindSafe(|| drop(future)));
                self.complete(Err(JoinError::panic(self.id, payload)));
            }
        }
    }

    fn done(&self) -> bool {
        self.finished.load(Ordering::Acquire)
    }

    fn cancel(&self) {
        self.aborted.store(true, Ordering::Release);
        if let Some(future) = self.future.take() {
            let _current = Current::enter(Some(self.id));
            let _ = catch_unwind(AssertUnwindSafe(|| drop(future)));
            self.complete(Err(JoinError::cancelled(self.id)));
        }
    }
}

impl<F> Joinable<F::Output> for Task<F>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    fn poll_join(&self, context: &mut Context<'_>) -> Poll<Result<F::Output, JoinError>> {
        if !self.finished.load(Ordering::Acquire) {
            self.joiner.register(context.waker());
            if !self.finished.load(Ordering::Acquire) {
                return Poll::Pending;
            }
        }
        match self.output.take() {
            Some(output) => Poll::Ready(*output),
            None => panic!("a JoinHandle's output is taken once, after the task finished"),
        }
    }

    fn is_finished(&self) -> bool {
        self.finished.load(Ordering::Acquire)
    }

    fn id(&self) -> Id {
        self.id
    }
}

impl<F> Abortable for Task<F>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    fn abort_task(self: Arc<Self>) {
        if self.finished.load(Ordering::Acquire) {
            return;
        }
        self.aborted.store(true, Ordering::Release);
        self.schedule();
    }

    fn finished(&self) -> bool {
        self.finished.load(Ordering::Acquire)
    }

    fn task_id(&self) -> Id {
        self.id
    }
}

pub struct JoinHandle<T> {
    task: Arc<dyn Joinable<T>>,
    abort: Arc<dyn Abortable>,
}

impl<T> JoinHandle<T> {
    pub(crate) fn new(task: Arc<dyn Joinable<T>>, abort: Arc<dyn Abortable>) -> Self {
        Self { task, abort }
    }

    pub fn abort(&self) {
        self.abort.clone().abort_task();
    }

    #[must_use]
    pub fn is_finished(&self) -> bool {
        self.task.is_finished()
    }

    #[must_use]
    pub fn abort_handle(&self) -> AbortHandle {
        AbortHandle {
            task: self.abort.clone(),
        }
    }

    #[must_use]
    pub fn id(&self) -> Id {
        self.task.id()
    }
}

impl<T> Unpin for JoinHandle<T> {}

impl<T> Future for JoinHandle<T> {
    type Output = Result<T, JoinError>;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        if crate::coop::poll_budget(context).is_pending() {
            return Poll::Pending;
        }
        let joined = self.task.poll_join(context);
        if joined.is_ready() {
            crate::coop::spend();
        }
        joined
    }
}

impl<T> fmt::Debug for JoinHandle<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("JoinHandle")
            .field("id", &self.task.id())
            .finish()
    }
}

#[derive(Clone)]
pub struct AbortHandle {
    task: Arc<dyn Abortable>,
}

impl AbortHandle {
    pub fn abort(&self) {
        self.task.clone().abort_task();
    }

    #[must_use]
    pub fn is_finished(&self) -> bool {
        self.task.finished()
    }

    #[must_use]
    pub fn id(&self) -> Id {
        self.task.task_id()
    }
}

impl fmt::Debug for AbortHandle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AbortHandle")
            .field("id", &self.task.task_id())
            .finish()
    }
}

pub async fn yield_now() {
    let mut yielded = false;
    std::future::poll_fn(|context| {
        if yielded {
            return Poll::Ready(());
        }
        yielded = true;
        if !crate::runtime::defer(context.waker()) {
            context.waker().wake_by_ref();
        }
        Poll::Pending
    })
    .await;
}
