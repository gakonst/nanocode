use std::{
    collections::BTreeMap,
    fmt,
    panic::{AssertUnwindSafe, UnwindSafe},
    pin::{Pin, pin},
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    task::{Context, Poll, Wake, Waker},
};

pub use std::future::{Future, PollFn, Ready, poll_fn, ready};

use nanocode_sys::{AtomicBox, AtomicWaker};

use crate::{
    Atom,
    stream::{Collect, FuturesOrdered, StreamExt},
};

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;
pub type LocalBoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + 'a>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Either<A, B> {
    Left(A),
    Right(B),
}

impl<A, B> Future for Either<A, B>
where
    A: Future + Unpin,
    B: Future<Output = A::Output> + Unpin,
{
    type Output = A::Output;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<A::Output> {
        match self.get_mut() {
            Self::Left(left) => Pin::new(left).poll(context),
            Self::Right(right) => Pin::new(right).poll(context),
        }
    }
}

pub trait FutureExt: Future {
    fn map<U, F>(self, change: F) -> Map<Self, F>
    where
        F: FnOnce(Self::Output) -> U,
        Self: Sized,
    {
        Map {
            future: Box::pin(self),
            change: Some(change),
        }
    }

    fn boxed<'a>(self) -> BoxFuture<'a, Self::Output>
    where
        Self: Sized + Send + 'a,
    {
        Box::pin(self)
    }

    fn boxed_local<'a>(self) -> LocalBoxFuture<'a, Self::Output>
    where
        Self: Sized + 'a,
    {
        Box::pin(self)
    }

    fn shared(self) -> Shared<Self>
    where
        Self: Sized,
        Self::Output: Clone,
    {
        Shared::new(self)
    }

    fn now_or_never(self) -> Option<Self::Output>
    where
        Self: Sized,
    {
        let mut context = Context::from_waker(Waker::noop());
        match pin!(self).poll(&mut context) {
            Poll::Ready(output) => Some(output),
            Poll::Pending => None,
        }
    }

    fn catch_unwind(self) -> CatchUnwind<Self>
    where
        Self: Sized + UnwindSafe,
    {
        CatchUnwind {
            future: Box::pin(self),
        }
    }

    fn poll_unpin(&mut self, context: &mut Context<'_>) -> Poll<Self::Output>
    where
        Self: Unpin,
    {
        Pin::new(self).poll(context)
    }

    fn left_future<B>(self) -> Either<Self, B>
    where
        Self: Sized,
    {
        Either::Left(self)
    }

    fn right_future<A>(self) -> Either<A, Self>
    where
        Self: Sized,
    {
        Either::Right(self)
    }
}

impl<T: Future + ?Sized> FutureExt for T {}

pub struct Map<Fut, F> {
    future: Pin<Box<Fut>>,
    change: Option<F>,
}

impl<Fut, F, U> Future for Map<Fut, F>
where
    Fut: Future,
    F: FnOnce(Fut::Output) -> U,
{
    type Output = U;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<U> {
        let this = self.get_mut();
        assert!(
            this.change.is_some(),
            "Map must not be polled after it returned `Poll::Ready`"
        );
        let Poll::Ready(output) = this.future.as_mut().poll(context) else {
            return Poll::Pending;
        };
        let change = this.change.take().expect("the change is there until Ready");
        Poll::Ready(change(output))
    }
}

pub struct CatchUnwind<Fut> {
    future: Pin<Box<Fut>>,
}

impl<Fut: Future + UnwindSafe> Future for CatchUnwind<Fut> {
    type Output = std::thread::Result<Fut::Output>;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let future = &mut self.get_mut().future;
        match std::panic::catch_unwind(AssertUnwindSafe(|| future.as_mut().poll(context))) {
            Ok(Poll::Pending) => Poll::Pending,
            Ok(Poll::Ready(output)) => Poll::Ready(Ok(output)),
            Err(panic) => Poll::Ready(Err(panic)),
        }
    }
}

pub struct Select<A, B> {
    inner: Option<(A, B)>,
}

pub const fn select<A, B>(first: A, second: B) -> Select<A, B>
where
    A: Future + Unpin,
    B: Future + Unpin,
{
    Select {
        inner: Some((first, second)),
    }
}

impl<A, B> Future for Select<A, B>
where
    A: Future + Unpin,
    B: Future + Unpin,
{
    type Output = Either<(A::Output, B), (B::Output, A)>;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let (first, second) = this.inner.as_mut().expect("cannot poll Select twice");
        if let Poll::Ready(output) = Pin::new(first).poll(context) {
            let (_, second) = this.inner.take().expect("Select holds both until Ready");
            return Poll::Ready(Either::Left((output, second)));
        }
        if let Poll::Ready(output) = Pin::new(second).poll(context) {
            let (first, _) = this.inner.take().expect("Select holds both until Ready");
            return Poll::Ready(Either::Right((output, first)));
        }
        Poll::Pending
    }
}

enum MaybeDone<F: Future> {
    Future(Pin<Box<F>>),
    Done(F::Output),
    Gone,
}

impl<F: Future> MaybeDone<F> {
    fn poll_done(&mut self, context: &mut Context<'_>) -> bool {
        match self {
            Self::Future(future) => match future.as_mut().poll(context) {
                Poll::Ready(output) => {
                    *self = Self::Done(output);
                    true
                }
                Poll::Pending => false,
            },
            Self::Done(_) => true,
            Self::Gone => panic!("MaybeDone polled after value taken"),
        }
    }

    fn take_output(&mut self) -> F::Output {
        match std::mem::replace(self, Self::Gone) {
            Self::Done(output) => output,
            Self::Future(_) | Self::Gone => {
                panic!("a joined future's output is taken once, after it finished")
            }
        }
    }
}

pub struct Join<A: Future, B: Future> {
    first: MaybeDone<A>,
    second: MaybeDone<B>,
}

pub fn join<A: Future, B: Future>(first: A, second: B) -> Join<A, B> {
    Join {
        first: MaybeDone::Future(Box::pin(first)),
        second: MaybeDone::Future(Box::pin(second)),
    }
}

impl<A: Future, B: Future> Future for Join<A, B> {
    type Output = (A::Output, B::Output);

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let first = this.first.poll_done(context);
        let second = this.second.poll_done(context);
        if first && second {
            Poll::Ready((this.first.take_output(), this.second.take_output()))
        } else {
            Poll::Pending
        }
    }
}

const JOIN_ALL_POLLED_TOGETHER: usize = 30;

pub struct JoinAll<F: Future> {
    kind: JoinAllKind<F>,
}

enum JoinAllKind<F: Future> {
    Small(Vec<MaybeDone<F>>),
    Big(Collect<FuturesOrdered<F>, Vec<F::Output>>),
}

pub fn join_all<I>(futures: I) -> JoinAll<I::Item>
where
    I: IntoIterator,
    I::Item: Future,
{
    let futures = futures.into_iter();
    let kind = match futures.size_hint().1 {
        Some(most) if most <= JOIN_ALL_POLLED_TOGETHER => JoinAllKind::Small(
            futures
                .map(|future| MaybeDone::Future(Box::pin(future)))
                .collect(),
        ),
        _ => JoinAllKind::Big(futures.collect::<FuturesOrdered<_>>().collect()),
    };
    JoinAll { kind }
}

impl<F: Future> Future for JoinAll<F> {
    type Output = Vec<F::Output>;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        match &mut self.get_mut().kind {
            JoinAllKind::Small(futures) => {
                let mut done = true;
                for future in futures.iter_mut() {
                    if !future.poll_done(context) {
                        done = false;
                    }
                }
                if !done {
                    return Poll::Pending;
                }
                Poll::Ready(
                    std::mem::take(futures)
                        .iter_mut()
                        .map(MaybeDone::take_output)
                        .collect(),
                )
            }
            JoinAllKind::Big(collect) => Pin::new(collect).poll(context),
        }
    }
}

impl<F: Future> FromIterator<F> for JoinAll<F> {
    fn from_iter<T: IntoIterator<Item = F>>(futures: T) -> Self {
        join_all(futures)
    }
}

pub struct SelectAll<Fut> {
    inner: Vec<Fut>,
}

pub fn select_all<I>(futures: I) -> SelectAll<I::Item>
where
    I: IntoIterator,
    I::Item: Future + Unpin,
{
    let inner: Vec<I::Item> = futures.into_iter().collect();
    assert!(!inner.is_empty(), "select_all needs at least one future");
    SelectAll { inner }
}

impl<Fut> SelectAll<Fut> {
    #[must_use]
    pub fn into_inner(self) -> Vec<Fut> {
        self.inner
    }
}

impl<Fut: Future + Unpin> Future for SelectAll<Fut> {
    type Output = (Fut::Output, usize, Vec<Fut>);

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let found = this.inner.iter_mut().enumerate().find_map(|(at, future)| {
            match Pin::new(future).poll(context) {
                Poll::Pending => None,
                Poll::Ready(output) => Some((at, output)),
            }
        });
        let Some((at, output)) = found else {
            return Poll::Pending;
        };
        drop(this.inner.swap_remove(at));
        Poll::Ready((output, at, std::mem::take(&mut this.inner)))
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct Aborted;

impl fmt::Display for Aborted {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("`Abortable` future has been aborted")
    }
}

impl std::error::Error for Aborted {}

#[derive(Debug, Default)]
struct AbortInner {
    waker: AtomicWaker,
    aborted: AtomicBool,
}

#[derive(Debug, Clone)]
pub struct AbortHandle {
    inner: Arc<AbortInner>,
}

#[derive(Debug)]
pub struct AbortRegistration {
    inner: Arc<AbortInner>,
}

impl AbortRegistration {
    #[must_use]
    pub fn handle(&self) -> AbortHandle {
        AbortHandle {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl AbortHandle {
    #[must_use]
    pub fn new_pair() -> (Self, AbortRegistration) {
        let inner = Arc::new(AbortInner::default());
        (
            Self {
                inner: Arc::clone(&inner),
            },
            AbortRegistration { inner },
        )
    }

    pub fn abort(&self) {
        self.inner.aborted.store(true, Ordering::Relaxed);
        self.inner.waker.wake();
    }

    #[must_use]
    pub fn is_aborted(&self) -> bool {
        self.inner.aborted.load(Ordering::Relaxed)
    }
}

pub struct Abortable<T> {
    task: Pin<Box<T>>,
    inner: Arc<AbortInner>,
}

pub fn abortable<Fut: Future>(future: Fut) -> (Abortable<Fut>, AbortHandle) {
    let (handle, registration) = AbortHandle::new_pair();
    (Abortable::new(future, registration), handle)
}

impl<T> Abortable<T> {
    pub fn new(task: T, registration: AbortRegistration) -> Self {
        Self {
            task: Box::pin(task),
            inner: registration.inner,
        }
    }

    #[must_use]
    pub fn is_aborted(&self) -> bool {
        self.inner.aborted.load(Ordering::Relaxed)
    }

    fn try_poll<I>(
        &mut self,
        context: &mut Context<'_>,
        poll: impl Fn(Pin<&mut T>, &mut Context<'_>) -> Poll<I>,
    ) -> Poll<Result<I, Aborted>> {
        if self.is_aborted() {
            return Poll::Ready(Err(Aborted));
        }
        if let Poll::Ready(output) = poll(self.task.as_mut(), context) {
            return Poll::Ready(Ok(output));
        }
        self.inner.waker.register(context.waker());
        if self.is_aborted() {
            return Poll::Ready(Err(Aborted));
        }
        Poll::Pending
    }
}

impl<Fut: Future> Future for Abortable<Fut> {
    type Output = Result<Fut::Output, Aborted>;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        self.get_mut().try_poll(context, Future::poll)
    }
}

impl<St: crate::stream::Stream> crate::stream::Stream for Abortable<St> {
    type Item = St::Item;

    fn poll_next(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<St::Item>> {
        self.get_mut()
            .try_poll(context, crate::stream::Stream::poll_next)
            .map(|polled| polled.ok().flatten())
    }
}

type SharedWakers = Atom<BTreeMap<u64, Arc<AtomicWaker>>>;

struct Notifier {
    wakers: SharedWakers,
}

impl Notifier {
    fn wake_all(&self) {
        for waker in self.wakers.get().values() {
            waker.wake();
        }
    }
}

impl Wake for Notifier {
    fn wake(self: Arc<Self>) {
        self.wake_all();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.wake_all();
    }
}

struct SharedInner<F: Future> {
    future: AtomicBox<Pin<Box<F>>>,
    output: OnceLock<F::Output>,
    poisoned: AtomicBool,
    notifier: Arc<Notifier>,
    waker: Waker,
    next_key: AtomicU64,
}

pub struct Shared<F: Future> {
    inner: Option<Arc<SharedInner<F>>>,
    key: u64,
    waker: Arc<AtomicWaker>,
}

struct Poisoner<'a> {
    poisoned: &'a AtomicBool,
    notifier: &'a Notifier,
}

impl Drop for Poisoner<'_> {
    fn drop(&mut self) {
        if std::thread::panicking() {
            self.poisoned.store(true, Ordering::SeqCst);
            self.notifier.wake_all();
        }
    }
}

impl<F: Future> Shared<F> {
    fn new(future: F) -> Self {
        let waker = Arc::new(AtomicWaker::new());
        let notifier = Arc::new(Notifier {
            wakers: Atom::new(BTreeMap::from([(0, Arc::clone(&waker))])),
        });
        let inner = Arc::new(SharedInner {
            future: AtomicBox::new(Some(Box::new(Box::pin(future)))),
            output: OnceLock::new(),
            poisoned: AtomicBool::new(false),
            waker: Waker::from(Arc::clone(&notifier)),
            notifier,
            next_key: AtomicU64::new(1),
        });
        Self {
            inner: Some(inner),
            key: 0,
            waker,
        }
    }

    #[must_use]
    pub fn ptr_eq(&self, other: &Self) -> bool {
        match (self.inner.as_ref(), other.inner.as_ref()) {
            (Some(this), Some(other)) => Arc::ptr_eq(this, other),
            _ => false,
        }
    }

    #[must_use]
    pub fn peek(&self) -> Option<&F::Output> {
        let inner = self.inner.as_ref()?;
        assert!(
            !inner.poisoned.load(Ordering::SeqCst),
            "inner future panicked during poll"
        );
        inner.output.get()
    }
}

impl<F: Future> Shared<F>
where
    F::Output: Clone,
{
    fn take_or_clone(&mut self) -> F::Output {
        let inner = self
            .inner
            .take()
            .expect("the handle holds its inner until Ready");
        self.forget(&inner);
        match Arc::try_unwrap(inner) {
            Ok(inner) => inner
                .output
                .into_inner()
                .expect("the output is set before it is taken"),
            Err(inner) => inner
                .output
                .get()
                .expect("the output is set before it is taken")
                .clone(),
        }
    }

    fn forget(&self, inner: &SharedInner<F>) {
        let key = self.key;
        inner.notifier.wakers.update(|wakers| {
            wakers.remove(&key);
        });
    }
}

impl<F: Future> Clone for Shared<F> {
    fn clone(&self) -> Self {
        let waker = Arc::new(AtomicWaker::new());
        let Some(inner) = self.inner.as_ref() else {
            return Self {
                inner: None,
                key: u64::MAX,
                waker,
            };
        };
        let key = inner.next_key.fetch_add(1, Ordering::Relaxed);
        inner.notifier.wakers.update(|wakers| {
            wakers.insert(key, Arc::clone(&waker));
        });
        Self {
            inner: Some(Arc::clone(inner)),
            key,
            waker,
        }
    }
}

impl<F: Future> Drop for Shared<F> {
    fn drop(&mut self) {
        if let Some(inner) = self.inner.as_ref() {
            let key = self.key;
            inner.notifier.wakers.update(|wakers| {
                wakers.remove(&key);
            });
        }
    }
}

impl<F: Future> Future for Shared<F>
where
    F::Output: Clone,
{
    type Output = F::Output;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<F::Output> {
        let this = self.get_mut();
        let inner = Arc::clone(
            this.inner
                .as_ref()
                .expect("Shared future polled again after completion"),
        );
        if inner.output.get().is_some() {
            return Poll::Ready(this.take_or_clone());
        }
        this.waker.register(context.waker());
        let Some(mut future) = inner.future.take() else {
            assert!(
                !inner.poisoned.load(Ordering::SeqCst),
                "inner future panicked during poll"
            );
            if inner.output.get().is_some() {
                return Poll::Ready(this.take_or_clone());
            }
            return Poll::Pending;
        };
        let poisoner = Poisoner {
            poisoned: &inner.poisoned,
            notifier: &inner.notifier,
        };
        let mut own = Context::from_waker(&inner.waker);
        let polled = (*future).as_mut().poll(&mut own);
        drop(poisoner);
        match polled {
            Poll::Pending => {
                inner.future.store(Some(future));
                Poll::Pending
            }
            Poll::Ready(output) => {
                drop(future);
                assert!(
                    inner.output.set(output).is_ok(),
                    "one poller sets the shared output"
                );
                inner.notifier.wake_all();
                drop(inner);
                Poll::Ready(this.take_or_clone())
            }
        }
    }
}

impl<Fut, F> Unpin for Map<Fut, F> {}
impl<A: Future, B: Future> Unpin for Join<A, B> {}
impl<F: Future> Unpin for JoinAll<F> {}
impl<Fut> Unpin for SelectAll<Fut> {}
