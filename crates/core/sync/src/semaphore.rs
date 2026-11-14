use crate::{Atom, OrdMap};
use nanocode_sys::AtomicWaker;
use std::{
    fmt,
    future::Future,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicU8, Ordering},
    },
    task::{Context, Poll},
};

const GRANT_WAITING: u8 = 0;
const GRANT_GIVEN: u8 = 1;
const GRANT_CLOSED: u8 = 2;

struct Grant {
    waker: AtomicWaker,
    state: AtomicU8,
}

#[derive(Clone)]
struct Waiter {
    needed: usize,
    assigned: usize,
    grant: Arc<Grant>,
}

#[derive(Clone, Default)]
struct State {
    permits: usize,
    closed: bool,
    next: u64,
    waiters: OrdMap<u64, Waiter>,
}

impl State {
    fn assign(&mut self) -> Vec<Arc<Grant>> {
        let mut given = Vec::new();
        if self.closed {
            return given;
        }
        while let Some((ticket, waiter)) = self.waiters.get_min() {
            let ticket = *ticket;
            let take = (waiter.needed - waiter.assigned).min(self.permits);
            self.permits -= take;
            let waiter = self
                .waiters
                .get_mut(&ticket)
                .expect("the first waiter is still queued");
            waiter.assigned += take;
            if waiter.assigned < waiter.needed {
                break;
            }
            let waiter = self
                .waiters
                .remove(&ticket)
                .expect("a satisfied waiter is still queued");
            given.push(waiter.grant);
        }
        assert!(
            self.permits == 0 || self.waiters.get_min().is_none(),
            "free permits never sit beside a waiter"
        );
        given
    }
}

pub struct Semaphore {
    state: Atom<State>,
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

fn give(grants: Vec<Arc<Grant>>, state: u8) {
    for grant in grants {
        grant.state.store(state, Ordering::Release);
        grant.waker.wake();
    }
}

impl Semaphore {
    pub const MAX_PERMITS: usize = usize::MAX >> 3;

    #[must_use]
    pub fn new(permits: usize) -> Self {
        assert!(
            permits <= Self::MAX_PERMITS,
            "a semaphore holds at most MAX_PERMITS permits"
        );
        Self {
            state: Atom::new(State {
                permits,
                ..State::default()
            }),
        }
    }

    pub fn available_permits(&self) -> usize {
        self.state.read(|state| state.permits)
    }

    pub fn add_permits(&self, added: usize) {
        if added == 0 {
            return;
        }
        let given = self.state.update(|state| {
            state.permits += added;
            assert!(
                state.permits <= Self::MAX_PERMITS,
                "a semaphore holds at most MAX_PERMITS permits"
            );
            state.assign()
        });
        give(given, GRANT_GIVEN);
    }

    pub fn forget_permits(&self, forgotten: usize) -> usize {
        self.state.update(|state| {
            let taken = forgotten.min(state.permits);
            state.permits -= taken;
            taken
        })
    }

    pub fn close(&self) {
        let woken = self.state.update(|state| {
            state.closed = true;
            state
                .waiters
                .values()
                .map(|waiter| Arc::clone(&waiter.grant))
                .collect()
        });
        give(woken, GRANT_CLOSED);
    }

    pub fn is_closed(&self) -> bool {
        self.state.read(|state| state.closed)
    }

    fn try_take(&self, permits: usize) -> Result<(), TryAcquireError> {
        self.state.modify(|state| {
            if state.closed {
                return Err(TryAcquireError::Closed);
            }
            if state.permits < permits {
                return Err(TryAcquireError::NoPermits);
            }
            state.permits -= permits;
            Ok(())
        })
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
    Waiting { ticket: u64, grant: Arc<Grant> },
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
}

enum Entry {
    Taken,
    Closed,
    Queued(u64),
}

impl Future for Acquire<'_> {
    type Output = Result<(), AcquireError>;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let grant = match &this.queued {
            Queued::Done => panic!("an acquire is polled after it finished"),
            Queued::Waiting { grant, .. } => Arc::clone(grant),
            Queued::Start => {
                let grant = Arc::new(Grant {
                    waker: AtomicWaker::new(),
                    state: AtomicU8::new(GRANT_WAITING),
                });
                grant.waker.register(context.waker());
                let needed = this.permits;
                let entry = this.semaphore.state.update(|state| {
                    if state.closed {
                        return Entry::Closed;
                    }
                    if state.waiters.get_min().is_none() && state.permits >= needed {
                        state.permits -= needed;
                        return Entry::Taken;
                    }
                    let assigned = if state.waiters.get_min().is_none() {
                        std::mem::take(&mut state.permits)
                    } else {
                        0
                    };
                    let ticket = state.next;
                    state.next += 1;
                    state.waiters.insert(
                        ticket,
                        Waiter {
                            needed,
                            assigned,
                            grant: Arc::clone(&grant),
                        },
                    );
                    Entry::Queued(ticket)
                });
                match entry {
                    Entry::Taken => {
                        this.queued = Queued::Done;
                        return Poll::Ready(Ok(()));
                    }
                    Entry::Closed => {
                        this.queued = Queued::Done;
                        return Poll::Ready(Err(AcquireError(())));
                    }
                    Entry::Queued(ticket) => {
                        this.queued = Queued::Waiting {
                            ticket,
                            grant: Arc::clone(&grant),
                        };
                        return Poll::Pending;
                    }
                }
            }
        };
        grant.waker.register(context.waker());
        if this.semaphore.is_closed() {
            return Poll::Ready(Err(AcquireError(())));
        }
        match grant.state.load(Ordering::Acquire) {
            GRANT_GIVEN => {
                this.queued = Queued::Done;
                Poll::Ready(Ok(()))
            }
            GRANT_CLOSED => Poll::Ready(Err(AcquireError(()))),
            _ => Poll::Pending,
        }
    }
}

impl Drop for Acquire<'_> {
    fn drop(&mut self) {
        let Queued::Waiting { ticket, .. } = &self.queued else {
            return;
        };
        let ticket = *ticket;
        let needed = self.permits;
        let given = self.semaphore.state.update(|state| {
            let returned = state
                .waiters
                .remove(&ticket)
                .map_or(needed, |waiter| waiter.assigned);
            state.permits += returned;
            state.assign()
        });
        give(given, GRANT_GIVEN);
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
