use nanocode_sys::{
    AtomicBox, AtomicWaker,
    queue::{Consumer, Pop, Queue},
};
use std::{
    fmt,
    future::Future,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    task::{Context, Poll},
};

const CLOSED: usize = 0b001;
const DRAINING: usize = 0b010;
const AGAIN: usize = 0b100;
const FLAG_BITS: u32 = 3;

const GRANTED: usize = 0b001;
const CANCELLED: usize = 0b010;
const SHUT: usize = 0b100;
const NODE_FLAG_BITS: u32 = 3;

struct Node {
    needed: usize,
    state: AtomicUsize,
    waker: AtomicWaker,
}

impl Node {
    fn assigned(state: usize) -> usize {
        state >> NODE_FLAG_BITS
    }
}

struct Drainer {
    consumer: Consumer,
    head: Option<Arc<Node>>,
}

pub struct Semaphore {
    word: AtomicUsize,
    queued: AtomicUsize,
    queue: Box<Queue<Arc<Node>>>,
    drainer: AtomicBox<Drainer>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct AcquireError(());

impl fmt::Display for AcquireError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("semaphore closed")
    }
}

impl std::error::Error for AcquireError {}

#[derive(Debug, PartialEq, Eq)]
pub enum TryAcquireError {
    Closed,
    NoPermits,
}

impl fmt::Display for TryAcquireError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Closed => formatter.write_str("semaphore closed"),
            Self::NoPermits => formatter.write_str("no permits available"),
        }
    }
}

impl std::error::Error for TryAcquireError {}

impl fmt::Debug for Semaphore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Semaphore")
            .field("permits", &self.available_permits())
            .finish()
    }
}

enum Assigned {
    Granted,
    Partial,
    Cancelled,
}

impl Semaphore {
    pub const MAX_PERMITS: usize = usize::MAX >> FLAG_BITS;

    #[must_use]
    pub fn new(permits: usize) -> Self {
        assert!(
            permits <= Self::MAX_PERMITS,
            "a semaphore holds at most MAX_PERMITS permits"
        );
        let queue = Box::new(Queue::new());
        let consumer = queue.consumer().expect("a new queue has no consumer yet");
        Self {
            word: AtomicUsize::new(permits << FLAG_BITS),
            queued: AtomicUsize::new(0),
            queue,
            drainer: AtomicBox::new(Some(Box::new(Drainer {
                consumer,
                head: None,
            }))),
        }
    }

    pub fn available_permits(&self) -> usize {
        self.word.load(Ordering::Acquire) >> FLAG_BITS
    }

    pub fn is_closed(&self) -> bool {
        self.word.load(Ordering::Acquire) & CLOSED != 0
    }

    pub fn add_permits(&self, added: usize) {
        if added == 0 {
            return;
        }
        assert!(
            added <= Self::MAX_PERMITS,
            "a semaphore holds at most MAX_PERMITS permits"
        );
        let previous = self.word.fetch_add(added << FLAG_BITS, Ordering::SeqCst);
        assert!(
            (previous >> FLAG_BITS) <= Self::MAX_PERMITS - added,
            "a semaphore holds at most MAX_PERMITS permits"
        );
        if self.queued.load(Ordering::SeqCst) != 0 {
            self.drain();
        }
    }

    pub fn forget_permits(&self, forgotten: usize) -> usize {
        let mut current = self.word.load(Ordering::Acquire);
        loop {
            let taken = forgotten.min(current >> FLAG_BITS);
            match self.word.compare_exchange_weak(
                current,
                current - (taken << FLAG_BITS),
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return taken,
                Err(actual) => current = actual,
            }
        }
    }

    pub fn close(&self) {
        self.word.fetch_or(CLOSED, Ordering::SeqCst);
        self.drain();
    }

    fn try_take(&self, permits: usize) -> Result<(), TryAcquireError> {
        let mut current = self.word.load(Ordering::Acquire);
        loop {
            if current & CLOSED != 0 {
                return Err(TryAcquireError::Closed);
            }
            if current >> FLAG_BITS < permits {
                return Err(TryAcquireError::NoPermits);
            }
            match self.word.compare_exchange_weak(
                current,
                current - (permits << FLAG_BITS),
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return Ok(()),
                Err(actual) => current = actual,
            }
        }
    }

    fn take_up_to(&self, wanted: usize) -> usize {
        let mut current = self.word.load(Ordering::Acquire);
        loop {
            let taken = wanted.min(current >> FLAG_BITS);
            if taken == 0 || current & CLOSED != 0 {
                return 0;
            }
            match self.word.compare_exchange_weak(
                current,
                current - (taken << FLAG_BITS),
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return taken,
                Err(actual) => current = actual,
            }
        }
    }

    fn drain(&self) {
        let mut current = self.word.load(Ordering::SeqCst);
        loop {
            let next = if current & DRAINING != 0 {
                current | AGAIN
            } else {
                (current | DRAINING) & !AGAIN
            };
            match self
                .word
                .compare_exchange_weak(current, next, Ordering::SeqCst, Ordering::SeqCst)
            {
                Ok(_) if current & DRAINING != 0 => return,
                Ok(_) => break,
                Err(actual) => current = actual,
            }
        }
        loop {
            let mut drainer = self
                .drainer
                .take()
                .expect("the draining flag's holder owns the drainer");
            self.drain_queue(&mut drainer);
            self.drainer.store(Some(drainer));
            let mut current = self.word.load(Ordering::SeqCst);
            loop {
                let next = if current & AGAIN != 0 {
                    current & !AGAIN
                } else {
                    current & !DRAINING
                };
                match self.word.compare_exchange_weak(
                    current,
                    next,
                    Ordering::SeqCst,
                    Ordering::SeqCst,
                ) {
                    Ok(_) if current & AGAIN != 0 => break,
                    Ok(_) => return,
                    Err(actual) => current = actual,
                }
            }
        }
    }

    fn next_node(&self, drainer: &mut Drainer) -> Option<Arc<Node>> {
        if let Some(node) = drainer.head.take() {
            return Some(node);
        }
        match self.queue.pop(&mut drainer.consumer) {
            Pop::Value(node) => Some(node),
            Pop::Empty | Pop::Pushing => None,
        }
    }

    fn drain_queue(&self, drainer: &mut Drainer) {
        while let Some(node) = self.next_node(drainer) {
            if self.is_closed() {
                node.state.fetch_or(SHUT, Ordering::AcqRel);
                node.waker.wake();
                continue;
            }
            let state = node.state.load(Ordering::Acquire);
            if state & CANCELLED != 0 {
                continue;
            }
            let taken = self.take_up_to(node.needed - Node::assigned(state));
            if taken == 0 {
                drainer.head = Some(node);
                return;
            }
            match Self::assign(&node, taken) {
                Assigned::Granted => {
                    self.queued.fetch_sub(1, Ordering::SeqCst);
                    node.waker.wake();
                }
                Assigned::Partial => {
                    drainer.head = Some(node);
                    return;
                }
                Assigned::Cancelled => {
                    self.word.fetch_add(taken << FLAG_BITS, Ordering::SeqCst);
                }
            }
        }
    }

    fn assign(node: &Node, taken: usize) -> Assigned {
        let mut state = node.state.load(Ordering::Acquire);
        loop {
            if state & CANCELLED != 0 {
                return Assigned::Cancelled;
            }
            let assigned = Node::assigned(state) + taken;
            assert!(
                assigned <= node.needed,
                "a waiter is assigned at most what it needs"
            );
            let granted = assigned == node.needed;
            let next =
                (assigned << NODE_FLAG_BITS) | (state & SHUT) | if granted { GRANTED } else { 0 };
            match node
                .state
                .compare_exchange_weak(state, next, Ordering::AcqRel, Ordering::Acquire)
            {
                Ok(_) if granted => return Assigned::Granted,
                Ok(_) => return Assigned::Partial,
                Err(actual) => state = actual,
            }
        }
    }

    pub fn try_acquire(&self) -> Result<SemaphorePermit<'_>, TryAcquireError> {
        self.try_acquire_many(1)
    }

    pub fn try_acquire_many(&self, permits: u32) -> Result<SemaphorePermit<'_>, TryAcquireError> {
        self.try_take(permits as usize)?;
        Ok(SemaphorePermit {
            semaphore: self,
            permits,
        })
    }

    pub fn try_acquire_owned(self: Arc<Self>) -> Result<OwnedSemaphorePermit, TryAcquireError> {
        self.try_acquire_many_owned(1)
    }

    pub fn try_acquire_many_owned(
        self: Arc<Self>,
        permits: u32,
    ) -> Result<OwnedSemaphorePermit, TryAcquireError> {
        self.try_take(permits as usize)?;
        Ok(OwnedSemaphorePermit {
            semaphore: self,
            permits,
        })
    }

    pub async fn acquire(&self) -> Result<SemaphorePermit<'_>, AcquireError> {
        self.acquire_many(1).await
    }

    pub async fn acquire_many(&self, permits: u32) -> Result<SemaphorePermit<'_>, AcquireError> {
        Acquire::new(self, permits as usize).await?;
        Ok(SemaphorePermit {
            semaphore: self,
            permits,
        })
    }

    pub async fn acquire_owned(self: Arc<Self>) -> Result<OwnedSemaphorePermit, AcquireError> {
        self.acquire_many_owned(1).await
    }

    pub async fn acquire_many_owned(
        self: Arc<Self>,
        permits: u32,
    ) -> Result<OwnedSemaphorePermit, AcquireError> {
        Acquire::new(&self, permits as usize).await?;
        Ok(OwnedSemaphorePermit {
            semaphore: self,
            permits,
        })
    }

    pub(crate) fn release(&self, permits: usize) {
        self.add_permits(permits);
    }
}

enum Queued {
    Start,
    Waiting(Arc<Node>),
    Done,
}

pub(crate) struct Acquire<'a> {
    semaphore: &'a Semaphore,
    permits: usize,
    queued: Queued,
}

impl<'a> Acquire<'a> {
    pub(crate) const fn new(semaphore: &'a Semaphore, permits: usize) -> Self {
        Self {
            semaphore,
            permits,
            queued: Queued::Start,
        }
    }

    fn enqueue(&mut self, context: &Context<'_>) -> Poll<Result<(), AcquireError>> {
        let semaphore = self.semaphore;
        match semaphore.try_take(self.permits) {
            Ok(()) => {
                self.queued = Queued::Done;
                return Poll::Ready(Ok(()));
            }
            Err(TryAcquireError::Closed) => {
                self.queued = Queued::Done;
                return Poll::Ready(Err(AcquireError(())));
            }
            Err(TryAcquireError::NoPermits) => {}
        }
        let node = Arc::new(Node {
            needed: self.permits,
            state: AtomicUsize::new(0),
            waker: AtomicWaker::new(),
        });
        node.waker.register(context.waker());
        semaphore.queued.fetch_add(1, Ordering::SeqCst);
        semaphore.queue.push(Arc::clone(&node));
        self.queued = Queued::Waiting(Arc::clone(&node));
        semaphore.drain();
        self.settle(&node)
    }

    fn settle(&mut self, node: &Node) -> Poll<Result<(), AcquireError>> {
        if self.semaphore.is_closed() {
            return Poll::Ready(Err(AcquireError(())));
        }
        if node.state.load(Ordering::Acquire) & GRANTED != 0 {
            self.queued = Queued::Done;
            return Poll::Ready(Ok(()));
        }
        Poll::Pending
    }
}

impl Future for Acquire<'_> {
    type Output = Result<(), AcquireError>;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        match &this.queued {
            Queued::Done => panic!("an acquire is polled after it finished"),
            Queued::Start => this.enqueue(context),
            Queued::Waiting(node) => {
                let node = Arc::clone(node);
                node.waker.register(context.waker());
                this.settle(&node)
            }
        }
    }
}

impl Drop for Acquire<'_> {
    fn drop(&mut self) {
        let Queued::Waiting(node) = &self.queued else {
            return;
        };
        let previous = node.state.fetch_or(CANCELLED, Ordering::AcqRel);
        let returned = if previous & GRANTED != 0 {
            self.permits
        } else {
            self.semaphore.queued.fetch_sub(1, Ordering::SeqCst);
            Node::assigned(previous)
        };
        self.semaphore.release(returned);
    }
}

#[must_use]
pub struct SemaphorePermit<'a> {
    semaphore: &'a Semaphore,
    permits: u32,
}

impl SemaphorePermit<'_> {
    pub fn forget(mut self) {
        self.permits = 0;
    }

    pub const fn num_permits(&self) -> usize {
        self.permits as usize
    }
}

impl Drop for SemaphorePermit<'_> {
    fn drop(&mut self) {
        self.semaphore.release(self.permits as usize);
    }
}

impl fmt::Debug for SemaphorePermit<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SemaphorePermit")
            .field("permits", &self.permits)
            .finish_non_exhaustive()
    }
}

#[must_use]
pub struct OwnedSemaphorePermit {
    semaphore: Arc<Semaphore>,
    permits: u32,
}

impl OwnedSemaphorePermit {
    pub fn forget(mut self) {
        self.permits = 0;
    }

    pub const fn num_permits(&self) -> usize {
        self.permits as usize
    }

    pub const fn semaphore(&self) -> &Arc<Semaphore> {
        &self.semaphore
    }
}

impl Drop for OwnedSemaphorePermit {
    fn drop(&mut self) {
        self.semaphore.release(self.permits as usize);
    }
}

impl fmt::Debug for OwnedSemaphorePermit {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OwnedSemaphorePermit")
            .field("permits", &self.permits)
            .finish_non_exhaustive()
    }
}
