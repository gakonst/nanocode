use std::{future::Future, pin::Pin, sync::Arc};

use arc_swap::{ArcSwap, ArcSwapOption};
use futures_util::future::{FutureExt, Shared};
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

pub struct Atom<T>(ArcSwap<T>);

impl<T: Default> Default for Atom<T> {
    fn default() -> Self {
        Self::new(T::default())
    }
}

impl<T: std::fmt::Debug> std::fmt::Debug for Atom<T> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_tuple("Atom").field(&self.0.load()).finish()
    }
}

impl<T> Atom<T> {
    pub fn new(value: T) -> Self {
        Self(ArcSwap::from_pointee(value))
    }

    #[must_use]
    pub fn get(&self) -> Arc<T> {
        self.0.load_full()
    }

    pub fn read<R>(&self, read: impl FnOnce(&T) -> R) -> R {
        read(&self.0.load())
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
            current = arc_swap::Guard::into_inner(previous);
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
            current = arc_swap::Guard::into_inner(previous);
        }
    }
}

pub struct Slot<T> {
    value: crossbeam_utils::atomic::AtomicCell<Option<Box<T>>>,
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
            value: crossbeam_utils::atomic::AtomicCell::new(value.map(Box::new)),
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
    head: ArcSwapOption<Node<T>>,
}

impl<T> Default for Inbox<T> {
    fn default() -> Self {
        Self {
            head: ArcSwapOption::empty(),
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
    result: ArcSwapOption<T>,
}

pub struct SingleFlight<T> {
    running: ArcSwapOption<Flight<T>>,
}

impl<T> Default for SingleFlight<T> {
    fn default() -> Self {
        Self {
            running: ArcSwapOption::empty(),
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
                result: ArcSwapOption::empty(),
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
    waiting: std::collections::VecDeque<(u64, Arc<Signal>)>,
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
            queue.waiting.retain(|(ticket, _)| *ticket != self.ticket);
            if queue.holder != Some(self.ticket) {
                return None;
            }
            let next = queue.waiting.pop_front();
            queue.holder = next.as_ref().map(|(ticket, _)| *ticket);
            next.map(|(_, admitted)| admitted)
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
                            queue
                                .waiting
                                .push_back((turn.ticket, Arc::clone(&admitted)));
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
                queue
                    .waiting
                    .push_back((turn.ticket, Arc::clone(&admitted)));
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
    }
}

impl<T> Owned<T> {
    pub fn new(value: T) -> Self {
        Self {
            sequencer: Sequencer::new(),
            value: Slot::new(Some(value)),
        }
    }

    fn held(&self, turn: Turn) -> Held<'_, T> {
        Held {
            owner: self,
            value: self.value.take(),
            _turn: turn,
        }
    }

    pub async fn acquire(&self) -> Held<'_, T> {
        let turn = self.sequencer.enter().await;
        self.held(turn)
    }

    pub fn try_acquire(&self) -> Option<Held<'_, T>> {
        self.sequencer.try_enter().map(|turn| self.held(turn))
    }

    fn leased(self: Arc<Self>, turn: Turn) -> Leased<T> {
        let value = self.value.take();
        Leased {
            owner: self,
            value,
            _turn: turn,
        }
    }

    pub async fn acquire_owned(self: Arc<Self>) -> Leased<T> {
        let turn = self.sequencer.enter().await;
        self.leased(turn)
    }

    pub fn try_acquire_owned(self: Arc<Self>) -> Option<Leased<T>> {
        let turn = self.sequencer.try_enter()?;
        Some(self.leased(turn))
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
        let runtime = tokio::runtime::Handle::try_current().map_err(std::io::Error::other)?;
        let (jobs, received) = mpsc::unbounded_channel();
        drop(runtime.spawn(Self::serve(state, received)));
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
                    match futures_util::future::select(work, abandoned).await {
                        futures_util::future::Either::Left((output, _)) => Some(output),
                        futures_util::future::Either::Right(_) => None,
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
                    match futures_util::future::select(work, abandoned).await {
                        futures_util::future::Either::Left((output, _)) => Some(output),
                        futures_util::future::Either::Right(_) => None,
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
