use std::{future::Future, pin::Pin, sync::Arc};

mod armed;
pub mod future;
mod hasher;
mod in_order;
#[cfg(not(target_family = "wasm"))]
mod lanes;
#[cfg(not(target_family = "wasm"))]
mod lease;
mod map;
mod ordered;
#[cfg(not(target_family = "wasm"))]
mod reentry;
#[doc(hidden)]
pub mod select;
#[cfg(not(target_family = "wasm"))]
mod session;
mod shared;
pub mod sink;
mod spine;
#[cfg(not(target_family = "wasm"))]
mod splice;
pub mod stream;
mod tree;
pub mod unbounded;
mod unordered;
mod vector;
mod waits;
mod wakes;
mod weighed;

pub use armed::Armed;
pub use future::FutureExt;
pub use in_order::{InOrder, join_in_order, try_join_in_order};
#[cfg(not(target_family = "wasm"))]
pub use lanes::{Bounds, Lanes, together};
#[cfg(not(target_family = "wasm"))]
pub use lease::{Backoff, Keeping, Lease, Renew, keep_lease, retry};
pub use map::AtomMap;
#[cfg(not(target_family = "wasm"))]
pub use map::{Kept, Throttle};
pub use nanocode_sys::AtomicWaker;
pub use ordered::{OrdMap, OrdSet};
#[cfg(not(target_family = "wasm"))]
pub use reentry::{Claim, Reentry};
#[cfg(not(target_family = "wasm"))]
pub use session::{Locker, SessionLock, SessionLockError, SessionLocks};
pub use shared::{Map, Set};
pub use sink::{Sink, SinkExt};
#[cfg(not(target_family = "wasm"))]
pub use splice::{Duplex, pipe, splice};
pub use stream::{Stream, StreamExt, TryStream, TryStreamExt};
pub use vector::Vector;
pub use waits::{SlowWait, slow_waits};
pub use wakes::{Wake, WakeHub, WakeOutbox, Wakes, Wakeup, Wakeups, woken};
pub use weighed::Weighed;

use future::Shared;
use nanocode_sys::Enclosed;
pub use nanocode_sys::{Guard, Swap, SwapOption};
use tokio::sync::{mpsc, oneshot};

pub struct Signal {
    finish: Slot<oneshot::Sender<()>>,
    finished: Shared<oneshot::Receiver<()>>,
}

impl Default for Signal {
    fn default() -> Self {
        Self::new()
    }
}

impl Signal {
    #[must_use]
    pub fn new() -> Self {
        let (finish, finished) = oneshot::channel();
        Self {
            finish: Slot::new(Some(finish)),
            finished: finished.shared(),
        }
    }

    pub fn fire(&self) {
        drop(self.finish.take());
    }

    #[must_use]
    pub fn is_fired(&self) -> bool {
        self.finish.is_empty()
    }

    pub async fn wait(&self) {
        let _ = self.finished.clone().await;
    }
}

pub struct Atom<T>(Enclosed<T>);

impl<T: Default> Default for Atom<T> {
    fn default() -> Self {
        Self::new(T::default())
    }
}

impl<T: std::fmt::Debug> std::fmt::Debug for Atom<T> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_tuple("Atom")
            .field(&self.0.load_full())
            .finish()
    }
}

impl<T> Atom<T> {
    pub fn new(value: T) -> Self {
        Self(Enclosed::from_pointee(value))
    }

    #[must_use]
    pub fn get(&self) -> Arc<T> {
        self.0.load_full()
    }

    pub fn read<R>(&self, read: impl FnOnce(&T) -> R) -> R {
        self.0.with(read)
    }

    pub fn set(&self, value: T) -> Arc<T> {
        self.0.swap(Arc::new(value))
    }
}

impl<T: Clone> Atom<T> {
    pub fn update<R>(&self, mut change: impl FnMut(&mut T) -> R) -> R {
        let mut current = self.0.load_full();
        loop {
            let mut next = T::clone(&current);
            let output = change(&mut next);
            let previous = self.0.compare_and_swap(&current, Arc::new(next));
            if Arc::ptr_eq(&previous, &current) {
                return output;
            }
            current = previous;
        }
    }

    pub fn try_update<R>(&self, mut change: impl FnMut(&mut T) -> Option<R>) -> Option<R> {
        self.modify(|value| change(value).ok_or(())).ok()
    }

    pub fn modify<R, E>(&self, mut change: impl FnMut(&mut T) -> Result<R, E>) -> Result<R, E> {
        let mut current = self.0.load_full();
        loop {
            let mut next = T::clone(&current);
            let output = change(&mut next)?;
            let previous = self.0.compare_and_swap(&current, Arc::new(next));
            if Arc::ptr_eq(&previous, &current) {
                return Ok(output);
            }
            current = previous;
        }
    }
}

pub struct Slot<T> {
    value: nanocode_sys::AtomicBox<T>,
    occupied: std::sync::atomic::AtomicIsize,
}

impl<T> Default for Slot<T> {
    fn default() -> Self {
        Self::new(None)
    }
}

impl<T> Slot<T> {
    #[must_use]
    pub fn new(value: Option<T>) -> Self {
        Self {
            occupied: std::sync::atomic::AtomicIsize::new(isize::from(value.is_some())),
            value: nanocode_sys::AtomicBox::new(value.map(Box::new)),
        }
    }

    pub fn put(&self, value: T) -> Option<T> {
        let previous = self.value.swap(Some(Box::new(value)));
        if previous.is_none() {
            self.occupied
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
        previous.map(|previous| *previous)
    }

    pub fn take(&self) -> Option<T> {
        let value = self.value.swap(None)?;
        self.occupied
            .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
        Some(*value)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.occupied.load(std::sync::atomic::Ordering::SeqCst) <= 0
    }
}

struct Draining<'a>(&'a std::sync::atomic::AtomicBool);

impl Drop for Draining<'_> {
    fn drop(&mut self) {
        self.0.store(false, std::sync::atomic::Ordering::SeqCst);
    }
}

pub struct Ordered<T> {
    pending: Atom<std::collections::BTreeMap<u64, Arc<Slot<T>>>>,
    next: std::sync::atomic::AtomicU64,
    draining: std::sync::atomic::AtomicBool,
}

impl<T> Ordered<T> {
    #[must_use]
    pub fn new(first: u64) -> Self {
        Self {
            pending: Atom::default(),
            next: std::sync::atomic::AtomicU64::new(first),
            draining: std::sync::atomic::AtomicBool::new(false),
        }
    }

    pub fn publish(&self, ticket: u64, value: T, deliver: impl Fn(T)) {
        use std::sync::atomic::Ordering::SeqCst;
        let value = Arc::new(Slot::new(Some(value)));
        self.pending.update(|pending| {
            pending.insert(ticket, Arc::clone(&value));
        });
        while !self.draining.swap(true, SeqCst) {
            {
                let _draining = Draining(&self.draining);
                loop {
                    let next = self.next.load(SeqCst);
                    let Some(ready) = self.pending.try_update(|pending| pending.remove(&next))
                    else {
                        break;
                    };
                    self.next.store(next.wrapping_add(1), SeqCst);
                    if let Some(value) = ready.take() {
                        deliver(value);
                    }
                }
            }
            let next = self.next.load(SeqCst);
            if !self.pending.read(|pending| pending.contains_key(&next)) {
                return;
            }
        }
    }
}

struct Node<T> {
    value: T,
    next: Option<Arc<Self>>,
}

pub struct Inbox<T> {
    head: SwapOption<Node<T>>,
}

impl<T> Default for Inbox<T> {
    fn default() -> Self {
        Self {
            head: SwapOption::empty(),
        }
    }
}

impl<T> Drop for Inbox<T> {
    fn drop(&mut self) {
        let mut next = self.head.swap(None);
        while let Some(node) = next.take() {
            if let Ok(node) = Arc::try_unwrap(node) {
                next = node.next;
            }
        }
    }
}

impl<T: Clone> Inbox<T> {
    pub fn push(&self, value: T) {
        let mut node = Arc::new(Node { value, next: None });
        loop {
            let head = self.head.load_full();
            if let Some(unpublished) = Arc::get_mut(&mut node) {
                unpublished.next.clone_from(&head);
            }
            let previous = self.head.compare_and_swap(&head, Some(Arc::clone(&node)));
            if previous.as_ref().map(Arc::as_ptr) == head.as_ref().map(Arc::as_ptr) {
                return;
            }
        }
    }

    pub fn drain(&self) -> Vec<T> {
        let mut values = Vec::new();
        if self.head.load().is_none() {
            return values;
        }
        let mut next = self.head.swap(None);
        while let Some(node) = next.take() {
            match Arc::try_unwrap(node) {
                Ok(node) => {
                    values.push(node.value);
                    next = node.next;
                }
                Err(shared) => {
                    values.push(shared.value.clone());
                    next.clone_from(&shared.next);
                }
            }
        }
        values.reverse();
        values
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.head.load().is_none()
    }

    pub fn any(&self, mut predicate: impl FnMut(&T) -> bool) -> bool {
        let mut next = self.head.load_full();
        while let Some(node) = next.take() {
            if predicate(&node.value) {
                return true;
            }
            next.clone_from(&node.next);
        }
        false
    }
}

struct Flight<T> {
    done: Signal,
    result: SwapOption<T>,
}

pub struct SingleFlight<T> {
    running: SwapOption<Flight<T>>,
}

impl<T> Default for SingleFlight<T> {
    fn default() -> Self {
        Self {
            running: SwapOption::empty(),
        }
    }
}

struct Leading<'a, T> {
    owner: &'a SingleFlight<T>,
    flight: Arc<Flight<T>>,
}

impl<T> Drop for Leading<'_, T> {
    fn drop(&mut self) {
        self.owner.running.rcu(|running| match running {
            Some(current) if Arc::ptr_eq(current, &self.flight) => None,
            other => other.clone(),
        });
        self.flight.done.fire();
    }
}

impl<T: Clone> SingleFlight<T> {
    pub async fn run<F, Fut>(&self, start: F) -> T
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = T>,
    {
        let mut start = Some(start);
        loop {
            let flight = Arc::new(Flight {
                done: Signal::new(),
                result: SwapOption::empty(),
            });
            let previous = self
                .running
                .rcu(|running| running.clone().or_else(|| Some(Arc::clone(&flight))));
            if let Some(running) = previous {
                running.done.wait().await;
                if let Some(result) = running.result.load_full() {
                    return T::clone(&result);
                }
                continue;
            }
            let Some(start) = start.take() else {
                continue;
            };
            let leading = Leading {
                owner: self,
                flight: Arc::clone(&flight),
            };
            let outcome = start().await;
            flight.result.store(Some(Arc::new(outcome.clone())));
            drop(leading);
            return outcome;
        }
    }
}

#[derive(Clone, Default)]
struct Queue {
    holder: Option<u64>,
    waiting: crate::OrdMap<u64, Arc<Signal>>,
}

const LINE_FREE: u8 = 0;
const LINE_HELD: u8 = 1;
const LINE_SWITCHING: u8 = 2;
const LINE_QUEUED: u8 = 3;
const FAST_TICKET: u64 = u64::MAX;

#[derive(Default)]
struct Line {
    mode: std::sync::atomic::AtomicU8,
    queue: Atom<Queue>,
    tickets: std::sync::atomic::AtomicU64,
}

fn back_off(spins: &mut u32) {
    if *spins < 64 {
        *spins += 1;
        std::hint::spin_loop();
    } else {
        std::thread::yield_now();
    }
}

#[derive(Clone, Default)]
pub struct Sequencer(Arc<Line>);

pub struct Turn {
    line: Arc<Line>,
    ticket: u64,
}

impl Drop for Turn {
    fn drop(&mut self) {
        use std::sync::atomic::Ordering::{AcqRel, Acquire};
        if self.ticket == FAST_TICKET {
            let mut spins = 0;
            loop {
                match self
                    .line
                    .mode
                    .compare_exchange(LINE_HELD, LINE_FREE, AcqRel, Acquire)
                {
                    Ok(_) => return,
                    Err(LINE_SWITCHING) => back_off(&mut spins),
                    Err(_) => break,
                }
            }
        }
        let next = self.line.queue.update(|queue| {
            queue.waiting.remove(&self.ticket);
            if queue.holder != Some(self.ticket) {
                return None;
            }
            let next = queue.waiting.get_min().map(|(ticket, _)| *ticket);
            queue.holder = next;
            next.and_then(|ticket| queue.waiting.remove(&ticket))
        });
        if let Some(next) = next {
            next.fire();
        }
    }
}

impl Sequencer {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn is_unshared(&self) -> bool {
        Arc::strong_count(&self.0) == 1
    }

    fn ticket(&self) -> u64 {
        self.0
            .tickets
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    }

    fn fast(&self) -> Turn {
        Turn {
            line: Arc::clone(&self.0),
            ticket: FAST_TICKET,
        }
    }

    pub async fn enter(&self) -> Turn {
        use std::sync::atomic::Ordering::{AcqRel, Acquire, Release};
        let mut spins = 0;
        loop {
            match self.0.mode.load(Acquire) {
                LINE_FREE => {
                    if self
                        .0
                        .mode
                        .compare_exchange(LINE_FREE, LINE_HELD, AcqRel, Acquire)
                        .is_ok()
                    {
                        return self.fast();
                    }
                }
                LINE_HELD => {
                    if self
                        .0
                        .mode
                        .compare_exchange(LINE_HELD, LINE_SWITCHING, AcqRel, Acquire)
                        .is_ok()
                    {
                        let turn = Turn {
                            line: Arc::clone(&self.0),
                            ticket: self.ticket(),
                        };
                        let admitted = Arc::new(Signal::new());
                        self.0.queue.update(|queue| {
                            queue.holder = Some(FAST_TICKET);
                            queue.waiting.insert(turn.ticket, Arc::clone(&admitted));
                        });
                        self.0.mode.store(LINE_QUEUED, Release);
                        admitted.wait().await;
                        return turn;
                    }
                }
                LINE_SWITCHING => back_off(&mut spins),
                _ => break,
            }
        }
        if let Some(turn) = self.try_enter_queued() {
            return turn;
        }
        let turn = Turn {
            line: Arc::clone(&self.0),
            ticket: self.ticket(),
        };
        let admitted = Arc::new(Signal::new());
        let immediate = self.0.queue.update(|queue| {
            if queue.holder.is_none() && queue.waiting.is_empty() {
                queue.holder = Some(turn.ticket);
                true
            } else {
                queue.waiting.insert(turn.ticket, Arc::clone(&admitted));
                false
            }
        });
        if !immediate {
            admitted.wait().await;
        }
        turn
    }

    #[must_use]
    pub fn try_enter(&self) -> Option<Turn> {
        use std::sync::atomic::Ordering::{AcqRel, Acquire};
        match self.0.mode.load(Acquire) {
            LINE_QUEUED => self.try_enter_queued(),
            _ => self
                .0
                .mode
                .compare_exchange(LINE_FREE, LINE_HELD, AcqRel, Acquire)
                .ok()
                .map(|_| self.fast()),
        }
    }

    fn try_enter_queued(&self) -> Option<Turn> {
        if self
            .0
            .queue
            .read(|queue| queue.holder.is_some() || !queue.waiting.is_empty())
        {
            return None;
        }
        let ticket = self.ticket();
        self.0
            .queue
            .try_update(|queue| {
                (queue.holder.is_none() && queue.waiting.is_empty())
                    .then(|| queue.holder = Some(ticket))
            })
            .map(|()| Turn {
                line: Arc::clone(&self.0),
                ticket,
            })
    }
}

pub struct Owned<T> {
    sequencer: Sequencer,
    value: Slot<T>,
    holder: Arc<waits::Holder>,
}

impl<T: Default> Default for Owned<T> {
    fn default() -> Self {
        Self::new(T::default())
    }
}

pub struct Held<'owner, T> {
    owner: &'owner Owned<T>,
    value: Option<T>,
    _turn: Turn,
}

impl<T> std::ops::Deref for Held<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        self.value
            .as_ref()
            .expect("a held value stays present until release")
    }
}

impl<T> std::ops::DerefMut for Held<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        self.value
            .as_mut()
            .expect("a held value stays present until release")
    }
}

impl<T> Drop for Held<'_, T> {
    fn drop(&mut self) {
        if let Some(value) = self.value.take() {
            self.owner.value.put(value);
        }
        self.owner.holder.release();
    }
}

pub struct Leased<T> {
    owner: Arc<Owned<T>>,
    value: Option<T>,
    _turn: Turn,
}

impl<T> std::ops::Deref for Leased<T> {
    type Target = T;

    fn deref(&self) -> &T {
        self.value
            .as_ref()
            .expect("a leased value stays present until release")
    }
}

impl<T> std::ops::DerefMut for Leased<T> {
    fn deref_mut(&mut self) -> &mut T {
        self.value
            .as_mut()
            .expect("a leased value stays present until release")
    }
}

impl<T> Drop for Leased<T> {
    fn drop(&mut self) {
        if let Some(value) = self.value.take() {
            self.owner.value.put(value);
        }
        self.owner.holder.release();
    }
}

impl<T> Owned<T> {
    pub fn new(value: T) -> Self {
        Self {
            sequencer: Sequencer::new(),
            value: Slot::new(Some(value)),
            holder: waits::Holder::registered(),
        }
    }

    fn held(&self, turn: Turn, at: &'static std::panic::Location<'static>) -> Held<'_, T> {
        self.holder.take(at);
        Held {
            owner: self,
            value: self.value.take(),
            _turn: turn,
        }
    }

    #[track_caller]
    pub fn acquire(&self) -> impl Future<Output = Held<'_, T>> + '_ {
        let at = std::panic::Location::caller();
        async move {
            let turn = match self.sequencer.try_enter() {
                Some(turn) => turn,
                None => {
                    let _waiting = waits::Wait::start(at, &self.holder);
                    self.sequencer.enter().await
                }
            };
            self.held(turn, at)
        }
    }

    #[track_caller]
    pub fn try_acquire(&self) -> Option<Held<'_, T>> {
        let at = std::panic::Location::caller();
        self.sequencer.try_enter().map(|turn| self.held(turn, at))
    }

    fn leased(
        self: Arc<Self>,
        turn: Turn,
        at: &'static std::panic::Location<'static>,
    ) -> Leased<T> {
        self.holder.take(at);
        let value = self.value.take();
        Leased {
            owner: self,
            value,
            _turn: turn,
        }
    }

    #[track_caller]
    pub fn acquire_owned(self: Arc<Self>) -> impl Future<Output = Leased<T>> {
        let at = std::panic::Location::caller();
        async move {
            let turn = match self.sequencer.try_enter() {
                Some(turn) => turn,
                None => {
                    let _waiting = waits::Wait::start(at, &self.holder);
                    self.sequencer.enter().await
                }
            };
            self.leased(turn, at)
        }
    }

    #[track_caller]
    pub fn try_acquire_owned(self: Arc<Self>) -> Option<Leased<T>> {
        let at = std::panic::Location::caller();
        let turn = self.sequencer.try_enter()?;
        Some(self.leased(turn, at))
    }
}

#[cfg(not(target_family = "wasm"))]
type Job<S> =
    Box<dyn for<'a> FnOnce(&'a mut S) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> + Send>;
#[cfg(target_family = "wasm")]
type Job<S> = Box<dyn for<'a> FnOnce(&'a mut S) -> Pin<Box<dyn Future<Output = ()> + 'a>>>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Stopped;

impl std::fmt::Display for Stopped {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("the owning task stopped")
    }
}

impl std::error::Error for Stopped {}

pub struct Actor<S> {
    jobs: mpsc::UnboundedSender<Job<S>>,
}

impl<S> Clone for Actor<S> {
    fn clone(&self) -> Self {
        Self {
            jobs: self.jobs.clone(),
        }
    }
}

impl<S: 'static> Actor<S> {
    #[cfg(not(target_family = "wasm"))]
    pub fn spawn(state: S) -> std::io::Result<Self>
    where
        S: Send,
    {
        let runtime = nanocode_platform::Handle::try_current().map_err(std::io::Error::other)?;
        let (jobs, received) = mpsc::unbounded_channel();
        runtime.spawn(Self::serve(state, received));
        Ok(Self { jobs })
    }

    #[cfg(target_family = "wasm")]
    pub fn spawn(state: S) -> std::io::Result<Self> {
        let (jobs, received) = mpsc::unbounded_channel();
        nanocode_platform::spawn_local(Self::serve(state, received));
        Ok(Self { jobs })
    }

    async fn serve(mut state: S, mut jobs: mpsc::UnboundedReceiver<Job<S>>) {
        while let Some(job) = jobs.recv().await {
            job(&mut state).await;
        }
    }

    #[cfg(not(target_family = "wasm"))]
    pub fn run<R, F>(
        &self,
        job: F,
    ) -> impl Future<Output = Result<R, Stopped>> + Send + use<R, F, S>
    where
        S: Send,
        R: Send + 'static,
        F: for<'a> FnOnce(&'a mut S) -> Pin<Box<dyn Future<Output = R> + Send + 'a>>
            + Send
            + 'static,
    {
        let (mut sender, receiver) = oneshot::channel();
        let job: Job<S> = Box::new(move |state| {
            Box::pin(async move {
                if sender.is_closed() {
                    return;
                }
                let work = job(state);
                let output = {
                    let abandoned = Box::pin(sender.closed());
                    match future::select(work, abandoned).await {
                        future::Either::Left((output, _)) => Some(output),
                        future::Either::Right(_) => None,
                    }
                };
                if let Some(output) = output {
                    drop(sender.send(output));
                }
            })
        });
        let sent = self.jobs.send(job).is_ok();
        async move {
            if !sent {
                return Err(Stopped);
            }
            receiver.await.map_err(|_| Stopped)
        }
    }

    #[cfg(target_family = "wasm")]
    pub fn run<R, F>(&self, job: F) -> impl Future<Output = Result<R, Stopped>> + use<R, F, S>
    where
        R: 'static,
        F: for<'a> FnOnce(&'a mut S) -> Pin<Box<dyn Future<Output = R> + 'a>> + 'static,
    {
        let (mut sender, receiver) = oneshot::channel();
        let job: Job<S> = Box::new(move |state| {
            Box::pin(async move {
                if sender.is_closed() {
                    return;
                }
                let work = job(state);
                let output = {
                    let abandoned = Box::pin(sender.closed());
                    match future::select(work, abandoned).await {
                        future::Either::Left((output, _)) => Some(output),
                        future::Either::Right(_) => None,
                    }
                };
                if let Some(output) = output {
                    drop(sender.send(output));
                }
            })
        });
        let sent = self.jobs.send(job).is_ok();
        async move {
            if !sent {
                return Err(Stopped);
            }
            receiver.await.map_err(|_| Stopped)
        }
    }

    #[must_use]
    pub fn is_stopped(&self) -> bool {
        self.jobs.is_closed()
    }
}

type Letter<S> = Box<dyn FnOnce(&mut S) + Send>;

const MAILBOX_BUDGET: usize = 128;

struct Desk<S> {
    state: S,
    letters: mpsc::UnboundedReceiver<Letter<S>>,
}

struct Mailroom<S> {
    desk: nanocode_sys::AtomicBox<Desk<S>>,
    letters: mpsc::UnboundedSender<Letter<S>>,
    queued: std::sync::atomic::AtomicUsize,
}

pub struct Mailbox<S>(Arc<Mailroom<S>>);

impl<S> Clone for Mailbox<S> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

impl<S: Default> Default for Mailbox<S> {
    fn default() -> Self {
        Self::new(S::default())
    }
}

impl<S> Mailbox<S> {
    #[must_use]
    pub fn new(state: S) -> Self {
        let (letters, received) = mpsc::unbounded_channel();
        Self(Arc::new(Mailroom {
            desk: nanocode_sys::AtomicBox::new(Some(Box::new(Desk {
                state,
                letters: received,
            }))),
            letters,
            queued: std::sync::atomic::AtomicUsize::new(0),
        }))
    }
}

impl<S: Send + 'static> Mailbox<S> {
    pub fn send(&self, job: impl FnOnce(&mut S) + Send + 'static) {
        post(&self.0, Box::new(job), hand_off::<S>);
    }

    pub fn call<R, F>(&self, job: F) -> impl Future<Output = R> + Send + use<R, F, S>
    where
        R: Send + 'static,
        F: FnOnce(&mut S) -> R + Send + 'static,
    {
        let (reply, receiver) = oneshot::channel();
        self.send(move |state| {
            if !reply.is_closed() {
                drop(reply.send(job(state)));
            }
        });
        replied(receiver)
    }
}

async fn replied<R>(mut receiver: oneshot::Receiver<R>) -> R {
    let reply = match receiver.try_recv() {
        Err(oneshot::error::TryRecvError::Empty) => receiver.await.ok(),
        reply => reply.ok(),
    };
    match reply {
        Some(output) => output,
        None => panic!("a mailbox job panicked before it replied"),
    }
}

type HandOff<S> = fn(&Arc<Mailroom<S>>) -> bool;

fn post<S>(room: &Arc<Mailroom<S>>, letter: Letter<S>, hand_off: HandOff<S>) {
    if room.letters.send(letter).is_err() {
        return;
    }
    if room
        .queued
        .fetch_add(1, std::sync::atomic::Ordering::AcqRel)
        == 0
    {
        deliver(room, hand_off);
    }
}

fn deliver<S>(room: &Arc<Mailroom<S>>, hand_off: HandOff<S>) {
    let mut budget = MAILBOX_BUDGET;
    loop {
        let Some(mut desk) = room.desk.take() else {
            return;
        };
        let delivered = desk.letters.try_recv().is_ok_and(|letter| {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| letter(&mut desk.state)))
                .is_ok()
        });
        room.desk.store(Some(desk));
        if room
            .queued
            .fetch_sub(1, std::sync::atomic::Ordering::AcqRel)
            == 1
        {
            return;
        }
        if delivered {
            budget -= 1;
        }
        if budget == 0 {
            if hand_off(room) {
                return;
            }
            budget = MAILBOX_BUDGET;
        }
    }
}

#[cfg(not(target_family = "wasm"))]
fn hand_off<S: Send + 'static>(room: &Arc<Mailroom<S>>) -> bool {
    let Ok(runtime) = nanocode_platform::Handle::try_current() else {
        return false;
    };
    let room = Arc::clone(room);
    runtime.spawn(async move { deliver(&room, hand_off::<S>) });
    true
}

#[cfg(target_family = "wasm")]
fn hand_off<S: Send + 'static>(room: &Arc<Mailroom<S>>) -> bool {
    let room = Arc::clone(room);
    nanocode_platform::spawn_local(async move { deliver(&room, hand_off::<S>) });
    true
}

#[cfg(not(target_family = "wasm"))]
async fn detached<T: Send + 'static>(future: impl Future<Output = T> + Send + 'static) -> T {
    let Some(host) = nanocode_platform::host() else {
        return match nanocode_platform::spawn_task(future).await {
            Ok(output) => output,
            Err(stopped) => match stopped.try_into_panic() {
                Ok(panic) => std::panic::resume_unwind(panic),
                Err(cancelled) => std::panic::resume_unwind(Box::new(cancelled.to_string())),
            },
        };
    };
    let (sent, received) = oneshot::channel();
    host.spawn(Box::pin(async move {
        drop(sent.send(future.await));
    }));
    match received.await {
        Ok(output) => output,
        Err(stopped) => std::panic::resume_unwind(Box::new(stopped.to_string())),
    }
}

#[cfg(not(target_family = "wasm"))]
type RecentRead<V, E> = Shared<future::BoxFuture<'static, Result<V, Arc<E>>>>;

#[cfg(not(target_family = "wasm"))]
type RecentReads<V, E> = map::Aged<Arc<str>, RecentRead<V, E>>;

#[cfg(not(target_family = "wasm"))]
pub struct Recent<V, E> {
    reads: Atom<RecentReads<V, E>>,
    ttl: std::time::Duration,
    kept: usize,
}

#[cfg(not(target_family = "wasm"))]
impl<V, E> Recent<V, E>
where
    V: Clone + Send + Sync + 'static,
    E: Send + Sync + 'static,
{
    pub fn new(ttl: std::time::Duration, kept: usize) -> Self {
        Self {
            reads: Atom::new(map::Aged::default()),
            ttl,
            kept,
        }
    }

    pub fn peek(&self, key: &str) -> Option<V> {
        let now = nanocode_platform::now();
        self.reads.read(|reads| {
            reads
                .fresh(key, now, self.ttl)?
                .peek()?
                .as_ref()
                .ok()
                .cloned()
        })
    }

    pub fn keep(&self, key: &str, value: V) {
        let read: RecentRead<V, E> = future::ready(Ok(value)).boxed().shared();
        let _ = read.clone().now_or_never();
        let now = nanocode_platform::now();
        let key: Arc<str> = key.into();
        self.reads.update(|reads| {
            reads.put(Arc::clone(&key), now, read.clone(), self.ttl, self.kept);
        });
    }

    pub async fn get<F>(&self, key: &str, read: impl FnOnce() -> F) -> Result<V, Arc<E>>
    where
        F: Future<Output = Result<V, E>> + Send + 'static,
    {
        self.get_if(key, |_| true, read).await
    }

    pub async fn get_if<F>(
        &self,
        key: &str,
        keep: impl Fn(&V) -> bool,
        read: impl FnOnce() -> F,
    ) -> Result<V, Arc<E>>
    where
        F: Future<Output = Result<V, E>> + Send + 'static,
    {
        let now = nanocode_platform::now();
        let shared = match self
            .reads
            .read(|reads| reads.fresh(key, now, self.ttl).cloned())
        {
            Some(shared) => shared,
            None => {
                let reading = read();
                let started: RecentRead<V, E> =
                    async move { detached(reading).await.map_err(Arc::new) }
                .boxed()
                .shared();
                let owned: Arc<str> = key.into();
                self.reads.update(|reads| {
                    if let Some(existing) = reads.fresh(key, now, self.ttl) {
                        return existing.clone();
                    }
                    reads.put(
                        Arc::clone(&owned),
                        now,
                        started.clone(),
                        self.ttl,
                        self.kept,
                    );
                    started.clone()
                })
            }
        };
        let value = shared.clone().await;
        let ours =
            |reads: &RecentReads<V, E>| reads.held(key).is_some_and(|held| held.ptr_eq(&shared));
        if value.as_ref().map_or(true, |value| !keep(value)) && self.reads.read(ours) {
            self.reads.update(|reads| {
                if ours(reads) {
                    reads.remove(key);
                }
            });
        }
        value
    }
}

#[derive(Clone, Default)]
struct Keyed {
    lines: crate::Map<Arc<str>, std::sync::Weak<Line>>,
    swept: usize,
}

#[derive(Default)]
pub struct Lines(Atom<Keyed>);

impl Lines {
    pub async fn enter(&self, key: &str) -> Turn {
        self.line(key).enter().await
    }

    pub fn try_enter(&self, key: &str) -> Option<Turn> {
        self.line(key).try_enter()
    }

    fn line(&self, key: &str) -> Sequencer {
        if let Some(line) = self
            .0
            .read(|keyed| keyed.lines.get(key).and_then(std::sync::Weak::upgrade))
        {
            return Sequencer(line);
        }
        Sequencer(self.0.update(|keyed| {
            if let Some(line) = keyed.lines.get(key).and_then(std::sync::Weak::upgrade) {
                return line;
            }
            if keyed.lines.len() >= 2 * keyed.swept {
                keyed.lines.retain(|_, line| line.strong_count() > 0);
                keyed.swept = keyed.lines.len();
            }
            let line = Arc::new(Line::default());
            keyed.lines.insert(key.into(), Arc::downgrade(&line));
            line
        }))
    }
}

#[cfg(not(target_family = "wasm"))]
struct Waiter<V, E, C> {
    answer: oneshot::Sender<Result<Arc<V>, E>>,
    context: C,
}

#[cfg(not(target_family = "wasm"))]
type Waiting<V, E, C> = crate::Map<Arc<str>, crate::Vector<Arc<Slot<Waiter<V, E, C>>>>>;

#[cfg(not(target_family = "wasm"))]
pub struct Coalesce<V, E, C = ()>(Atom<Waiting<V, E, C>>);

#[cfg(not(target_family = "wasm"))]
impl<V, E, C> Default for Coalesce<V, E, C> {
    fn default() -> Self {
        Self(Atom::new(crate::Map::new()))
    }
}

#[cfg(not(target_family = "wasm"))]
impl<V, E, C> Coalesce<V, E, C>
where
    V: Send + Sync + 'static,
    E: Clone + Send + From<Stopped> + 'static,
    C: Clone + Send + 'static,
{
    pub async fn read<F, Fut>(self: &Arc<Self>, key: &str, context: C, read: F) -> Result<Arc<V>, E>
    where
        F: Fn(C) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<V, E>> + Send + 'static,
    {
        let (answer, answered) = oneshot::channel();
        let waiter = Arc::new(Slot::new(Some(Waiter { answer, context })));
        let key: Arc<str> = key.into();
        let lead = self.0.update(|waiting| match waiting.get_mut(&key) {
            Some(next) => {
                next.push_back(Arc::clone(&waiter));
                false
            }
            None => {
                waiting.insert(Arc::clone(&key), crate::Vector::new());
                true
            }
        });
        if lead && let Some(waiter) = waiter.take() {
            nanocode_platform::spawn(Arc::clone(self).serve(key, read, vec![waiter]));
        }
        answered.await.unwrap_or_else(|_| Err(E::from(Stopped)))
    }

    async fn serve<F, Fut>(
        self: Arc<Self>,
        key: Arc<str>,
        read: F,
        mut answers: Vec<Waiter<V, E, C>>,
    ) where
        F: Fn(C) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<V, E>> + Send + 'static,
    {
        let mut running = Serving {
            coalesce: Arc::clone(&self),
            key,
            served: false,
        };
        while let Some(context) = answers.first().map(|waiter| waiter.context.clone()) {
            let answer = read(context).await.map(Arc::new);
            for waiter in answers {
                drop(waiter.answer.send(answer.clone()));
            }
            let next = self.0.update(|waiting| {
                let next = waiting
                    .get_mut(&running.key)
                    .map(std::mem::take)
                    .unwrap_or_default();
                if next.is_empty() {
                    waiting.remove(&running.key);
                }
                next
            });
            answers = next.iter().filter_map(|waiter| waiter.take()).collect();
        }
        running.served = true;
    }
}

#[cfg(not(target_family = "wasm"))]
struct Serving<V, E, C> {
    coalesce: Arc<Coalesce<V, E, C>>,
    key: Arc<str>,
    served: bool,
}

#[cfg(not(target_family = "wasm"))]
impl<V, E, C> Drop for Serving<V, E, C> {
    fn drop(&mut self) {
        if self.served {
            return;
        }
        let queued = self.coalesce.0.update(|waiting| waiting.remove(&self.key));
        for waiter in queued.iter().flatten() {
            drop(waiter.take());
        }
    }
}

#[cfg(not(target_family = "wasm"))]
#[derive(Clone)]
struct Fetched<T> {
    value: Option<T>,
    at: Option<std::time::Instant>,
    refreshing: bool,
}

#[cfg(not(target_family = "wasm"))]
type Fetches<T> = Arc<Atom<map::Aged<Arc<str>, Fetched<T>>>>;

#[cfg(not(target_family = "wasm"))]
pub struct Refreshed<T> {
    entries: Fetches<T>,
    max_read: std::time::Duration,
    kept: usize,
}

#[cfg(not(target_family = "wasm"))]
struct Refreshing<T: Clone> {
    entries: Fetches<T>,
    key: Arc<str>,
    value: Option<T>,
    kept: usize,
}

#[cfg(not(target_family = "wasm"))]
impl<T: Clone> Drop for Refreshing<T> {
    fn drop(&mut self) {
        let value = self.value.take();
        let now = nanocode_platform::now();
        self.entries.update(|entries| {
            if let Some(entry) = entries.held(&self.key) {
                let mut entry = entry.clone();
                entry.refreshing = false;
                entry.at = Some(now);
                if value.is_some() {
                    entry.value.clone_from(&value);
                }
                entries.put(
                    Arc::clone(&self.key),
                    now,
                    entry,
                    std::time::Duration::MAX,
                    self.kept,
                );
            }
        });
    }
}

#[cfg(not(target_family = "wasm"))]
impl<T: Clone + Send + Sync + 'static> Refreshed<T> {
    pub fn new(max_read: std::time::Duration, kept: usize) -> Self {
        Self {
            entries: Arc::new(Atom::new(map::Aged::default())),
            max_read,
            kept,
        }
    }

    pub fn now(
        &self,
        key: &str,
        fresh: std::time::Duration,
        read: impl Future<Output = Option<T>> + Send + 'static,
    ) -> Option<T> {
        self.now_unless(key, fresh, |_| false, read)
    }

    pub fn now_unless(
        &self,
        key: &str,
        fresh: std::time::Duration,
        outdated: impl FnOnce(&T) -> bool,
        read: impl Future<Output = Option<T>> + Send + 'static,
    ) -> Option<T> {
        let (stale, held) = self.entries.read(|entries| match entries.held(key) {
            None => (true, None),
            Some(entry) => (
                entry.at.is_none_or(|at| at.elapsed() >= fresh)
                    || entry.value.as_ref().is_some_and(outdated),
                Some((entry.refreshing, entry.value.clone())),
            ),
        });
        if let Some((refreshing, value)) = held
            && (!stale || refreshing)
        {
            return value;
        }
        let key: Arc<str> = key.into();
        let now = nanocode_platform::now();
        let (refresh, value) = self.entries.update(|entries| {
            let mut entry = entries.held(&key).cloned().unwrap_or(Fetched {
                value: None,
                at: None,
                refreshing: false,
            });
            let refresh = stale && !entry.refreshing;
            let value = entry.value.clone();
            entry.refreshing |= refresh;
            entries.put(
                Arc::clone(&key),
                now,
                entry,
                std::time::Duration::MAX,
                self.kept,
            );
            (refresh, value)
        });
        if refresh {
            let mut refreshing = Refreshing {
                entries: Arc::clone(&self.entries),
                key,
                value: None,
                kept: self.kept,
            };
            let max_read = self.max_read;
            nanocode_platform::spawn(async move {
                let value = nanocode_platform::timeout(max_read, read)
                    .await
                    .ok()
                    .flatten();
                refreshing.value = value;
                drop(refreshing);
            });
        }
        value
    }
}
