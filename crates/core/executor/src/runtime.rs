use std::{
    cell::{Cell, RefCell},
    fmt,
    future::Future,
    io,
    sync::{
        Arc, Weak,
        atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering, fence},
    },
    task::{Context, Poll, Wake, Waker},
    thread::Thread,
    time::{Duration, Instant},
};

use nanocode_sys::{SwapOption, queue::Consumer};

use crate::{
    blocking::{self, KEEP_ALIVE, MAX_BLOCKING_THREADS, Pool},
    coop::Budget,
    queue::Shared as Queue,
    reactor::{self, Reactor},
    task::{JoinHandle, Runnable, Task},
};

const WORKER_THREAD: &str = "nanocode-worker";
const EVENT_INTERVAL: u32 = 61;

const RUNNING: u8 = 0;
const PARKED: u8 = 1;
const DRIVING: u8 = 2;
const ABSENT: u8 = 3;

const NO_RUNTIME: &str =
    "there is no reactor running, must be called from the context of a nanocode runtime";
const NESTED: &str = "Cannot start a runtime from within a runtime. This happens because a function (like `block_on`) attempted to block the current thread while the thread is being used to drive asynchronous tasks.";

struct Slot {
    state: AtomicU8,
    thread: SwapOption<Thread>,
}

impl Slot {
    fn new(state: u8) -> Self {
        Self {
            state: AtomicU8::new(state),
            thread: SwapOption::new(None),
        }
    }

    fn unpark(&self) {
        if let Some(thread) = self.thread.load_full() {
            thread.unpark();
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    CurrentThread,
    MultiThread,
}

pub(crate) struct Shared {
    queue: Queue<Arc<dyn Runnable>>,
    slots: Box<[Slot]>,
    kind: Kind,
    shutdown: AtomicBool,
    owned: Queue<Weak<dyn Runnable>>,
    registered: AtomicUsize,
    live: AtomicUsize,
    running: AtomicUsize,
    waiter: SwapOption<Thread>,
    blocking: Arc<Pool>,
    reactor: &'static Reactor,
}

impl Shared {
    pub(crate) fn is_shutdown(&self) -> bool {
        self.shutdown.load(Ordering::Acquire)
    }

    pub(crate) fn finished(&self) {
        self.live.fetch_sub(1, Ordering::AcqRel);
    }

    pub(crate) fn schedule(&self, task: Arc<dyn Runnable>) {
        if self.is_shutdown() {
            task.cancel();
            return;
        }
        self.queue.push(task);
        self.notify();
    }

    fn notify(&self) {
        let mut driving = false;
        for slot in &self.slots {
            match slot.state.load(Ordering::SeqCst) {
                RUNNING => return,
                PARKED => {
                    if slot
                        .state
                        .compare_exchange(PARKED, RUNNING, Ordering::SeqCst, Ordering::SeqCst)
                        .is_ok()
                    {
                        slot.unpark();
                        return;
                    }
                }
                DRIVING => driving = true,
                _ => {}
            }
        }
        if driving {
            self.reactor.wake();
        }
    }

    fn notify_all(&self) {
        for slot in &self.slots {
            slot.unpark();
        }
        self.reactor.wake();
    }

    fn register(&self, task: Weak<dyn Runnable>) {
        self.live.fetch_add(1, Ordering::AcqRel);
        self.owned.push(task);
        let registered = self.registered.fetch_add(1, Ordering::AcqRel) + 1;
        if registered > 2 * self.live.load(Ordering::Acquire) + self.slots.len() {
            self.compact();
        }
    }

    fn compact(&self) {
        let Some(mut consumer) = self.owned.hold() else {
            return;
        };
        let mut kept = Vec::new();
        while let Some(task) = self.owned.pop_held(&mut consumer) {
            if task.upgrade().is_some_and(|task| !task.done()) {
                kept.push(task);
            }
        }
        self.registered.store(kept.len(), Ordering::Release);
        for task in kept {
            self.owned.push(task);
        }
        self.owned.give_back(consumer);
    }

    fn cancel_all(&self) {
        for task in self.owned.drain() {
            if let Some(task) = task.upgrade() {
                task.cancel();
            }
        }
        for task in self.queue.try_drain() {
            task.cancel();
        }
    }

    fn worker_exited(&self) {
        if self.running.fetch_sub(1, Ordering::SeqCst) == 1
            && let Some(waiter) = self.waiter.load_full()
        {
            waiter.unpark();
        }
    }

    fn run_queue(&self, mut held: Option<&mut Consumer>) -> bool {
        let mut ran = 0;
        while ran < EVENT_INTERVAL {
            if self.is_shutdown() {
                return true;
            }
            let next = match held.as_deref_mut() {
                Some(consumer) => self.queue.pop_held(consumer),
                None => self.queue.pop(),
            };
            let Some(task) = next else {
                return wake_deferred() || ran > 0;
            };
            task.run();
            ran += 1;
        }
        self.reactor.try_turn(Some(Duration::ZERO), || {});
        wake_deferred();
        true
    }

    fn idle(&self, slot: &Slot, main: Option<&AtomicBool>) {
        let pending = || {
            self.queue.len() > 0
                || self.is_shutdown()
                || main.is_some_and(|main| main.load(Ordering::SeqCst))
        };
        slot.state.store(DRIVING, Ordering::SeqCst);
        if pending() {
            slot.state.store(RUNNING, Ordering::SeqCst);
            return;
        }
        if self
            .reactor
            .try_turn(None, || slot.state.store(RUNNING, Ordering::SeqCst))
        {
            slot.state.store(RUNNING, Ordering::SeqCst);
            return;
        }
        slot.state.store(PARKED, Ordering::SeqCst);
        if !pending() {
            self.reactor.park_for_driver();
        }
        slot.state.store(RUNNING, Ordering::SeqCst);
    }

    fn work(self: &Arc<Self>, index: usize, mut held: Option<Box<Consumer>>) {
        let slot = &self.slots[index];
        let handle = Handle(self.clone());
        let _context = handle.enter();
        let _driving = Driving::enter(self.reactor);
        loop {
            if self.is_shutdown() {
                break;
            }
            if !self.run_queue(held.as_deref_mut()) {
                self.idle(slot, None);
            }
        }
        slot.state.store(ABSENT, Ordering::SeqCst);
        if let Some(consumer) = held {
            self.queue.give_back(consumer);
        }
        self.worker_exited();
    }
}

thread_local! {
    static CONTEXT: RefCell<Option<Handle>> = const { RefCell::new(None) };
    static BLOCKING_THREAD: Cell<bool> = const { Cell::new(false) };
    static DEFERRED: RefCell<Vec<Waker>> = const { RefCell::new(Vec::new()) };
}

pub(crate) fn defer(waker: &Waker) -> bool {
    if !BLOCKING_THREAD.with(Cell::get) {
        return false;
    }
    DEFERRED.with(|deferred| deferred.borrow_mut().push(waker.clone()));
    true
}

fn wake_deferred() -> bool {
    let woken = DEFERRED.with(|deferred| std::mem::take(&mut *deferred.borrow_mut()));
    let any = !woken.is_empty();
    for waker in woken.into_iter().rev() {
        waker.wake();
    }
    any
}

struct Driving {
    reactor: &'static Reactor,
    was: bool,
}

impl Driving {
    fn enter(reactor: &'static Reactor) -> Self {
        reactor.enter_driver();
        Self {
            reactor,
            was: BLOCKING_THREAD.with(|blocking| blocking.replace(true)),
        }
    }
}

impl Drop for Driving {
    fn drop(&mut self) {
        BLOCKING_THREAD.with(|blocking| blocking.set(self.was));
        self.reactor.exit_driver();
    }
}

#[derive(Debug)]
pub struct TryCurrentError;

impl fmt::Display for TryCurrentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(NO_RUNTIME)
    }
}

impl std::error::Error for TryCurrentError {}

#[derive(Clone)]
pub struct Handle(Arc<Shared>);

impl fmt::Debug for Handle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Handle")
    }
}

pub struct EnterGuard<'a> {
    previous: Option<Handle>,
    _handle: std::marker::PhantomData<&'a Handle>,
}

impl Drop for EnterGuard<'_> {
    fn drop(&mut self) {
        let previous = self.previous.take();
        CONTEXT.with(|context| *context.borrow_mut() = previous);
    }
}

impl Handle {
    pub fn current() -> Self {
        Self::try_current().unwrap_or_else(|_| panic!("{NO_RUNTIME}"))
    }

    pub fn try_current() -> Result<Self, TryCurrentError> {
        CONTEXT
            .with(|context| context.borrow().clone())
            .ok_or(TryCurrentError)
    }

    pub fn enter(&self) -> EnterGuard<'_> {
        let previous = CONTEXT.with(|context| context.borrow_mut().replace(self.clone()));
        EnterGuard {
            previous,
            _handle: std::marker::PhantomData,
        }
    }

    #[track_caller]
    pub fn spawn<F>(&self, future: F) -> JoinHandle<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        let task = Task::new(self.0.clone(), future);
        let runnable: Arc<dyn Runnable> = task.clone();
        self.0.register(Arc::downgrade(&runnable));
        self.0.schedule(runnable);
        JoinHandle::new(task.clone(), task)
    }

    #[track_caller]
    pub fn spawn_blocking<F, R>(&self, work: F) -> JoinHandle<R>
    where
        F: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        blocking::spawn(Some(self), &self.0.blocking, work)
    }

    #[track_caller]
    pub fn block_on<F: Future>(&self, future: F) -> F::Output {
        assert!(!BLOCKING_THREAD.with(Cell::get), "{NESTED}");
        let _context = self.enter();
        if self.0.kind == Kind::CurrentThread
            && let Some(consumer) = self.0.queue.hold()
        {
            return self.drive(future, consumer);
        }
        park_on(future)
    }

    fn drive<F: Future>(&self, future: F, consumer: Box<Consumer>) -> F::Output {
        let shared = &self.0;
        let slot = &shared.slots[0];
        slot.thread.store(Some(Arc::new(std::thread::current())));
        slot.state.store(RUNNING, Ordering::SeqCst);
        let main = Arc::new(Main {
            woken: AtomicBool::new(true),
            shared: Arc::downgrade(shared),
        });
        let waker = Waker::from(main.clone());
        let mut future = std::pin::pin!(future);
        let mut core = Core {
            shared,
            consumer: Some(consumer),
        };
        let _driving = Driving::enter(shared.reactor);
        {
            loop {
                if main.woken.swap(false, Ordering::SeqCst) {
                    let _budget = Budget::enter();
                    if let Poll::Ready(output) =
                        future.as_mut().poll(&mut Context::from_waker(&waker))
                    {
                        return output;
                    }
                }
                if !shared.run_queue(core.consumer.as_deref_mut()) {
                    shared.idle(slot, Some(&main.woken));
                }
            }
        }
    }
}

struct Core<'a> {
    shared: &'a Shared,
    consumer: Option<Box<Consumer>>,
}

impl Drop for Core<'_> {
    fn drop(&mut self) {
        let slot = &self.shared.slots[0];
        slot.state.store(ABSENT, Ordering::SeqCst);
        slot.thread.store(None);
        if let Some(consumer) = self.consumer.take() {
            self.shared.queue.give_back(consumer);
        }
        if self.shared.queue.len() > 0 {
            self.shared.notify();
        }
    }
}

struct Main {
    woken: AtomicBool,
    shared: Weak<Shared>,
}

impl Wake for Main {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.woken.store(true, Ordering::SeqCst);
        if let Some(shared) = self.shared.upgrade() {
            shared.notify();
        }
    }
}

struct Parked {
    thread: Thread,
    notified: AtomicBool,
}

impl Wake for Parked {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.notified.store(true, Ordering::Release);
        self.thread.unpark();
    }
}

fn park_on<F: Future>(future: F) -> F::Output {
    let parked = Arc::new(Parked {
        thread: std::thread::current(),
        notified: AtomicBool::new(false),
    });
    let waker = Waker::from(parked.clone());
    let mut context = Context::from_waker(&waker);
    let mut future = std::pin::pin!(future);
    loop {
        {
            let _budget = Budget::enter();
            if let Poll::Ready(output) = future.as_mut().poll(&mut context) {
                return output;
            }
        }
        while !parked.notified.swap(false, Ordering::Acquire) {
            std::thread::park();
        }
    }
}

pub struct Builder {
    kind: Kind,
    workers: Option<usize>,
    name: Option<String>,
    max_blocking: usize,
    keep_alive: Duration,
}

impl Builder {
    #[must_use]
    pub const fn new_current_thread() -> Self {
        Self {
            kind: Kind::CurrentThread,
            workers: None,
            name: None,
            max_blocking: MAX_BLOCKING_THREADS,
            keep_alive: KEEP_ALIVE,
        }
    }

    #[must_use]
    pub const fn new_multi_thread() -> Self {
        Self {
            kind: Kind::MultiThread,
            workers: None,
            name: None,
            max_blocking: MAX_BLOCKING_THREADS,
            keep_alive: KEEP_ALIVE,
        }
    }

    pub fn worker_threads(&mut self, workers: usize) -> &mut Self {
        assert!(workers > 0, "Worker threads cannot be set to 0");
        self.workers = Some(workers);
        self
    }

    pub fn max_blocking_threads(&mut self, threads: usize) -> &mut Self {
        assert!(threads > 0, "Max blocking threads cannot be set to 0");
        self.max_blocking = threads;
        self
    }

    pub const fn thread_keep_alive(&mut self, keep_alive: Duration) -> &mut Self {
        self.keep_alive = keep_alive;
        self
    }

    pub fn thread_name(&mut self, name: impl Into<String>) -> &mut Self {
        self.name = Some(name.into());
        self
    }

    pub const fn enable_all(&mut self) -> &mut Self {
        self
    }

    pub const fn enable_io(&mut self) -> &mut Self {
        self
    }

    pub const fn enable_time(&mut self) -> &mut Self {
        self
    }

    pub fn build(&mut self) -> io::Result<Runtime> {
        let reactor = reactor::shared()?;
        let workers = match self.kind {
            Kind::CurrentThread => 1,
            Kind::MultiThread => self.workers.unwrap_or_else(|| {
                std::thread::available_parallelism().map_or(1, std::num::NonZero::get)
            }),
        };
        let initial = match self.kind {
            Kind::CurrentThread => ABSENT,
            Kind::MultiThread => RUNNING,
        };
        let shared = Arc::new(Shared {
            queue: Queue::new(),
            slots: (0..workers).map(|_| Slot::new(initial)).collect(),
            kind: self.kind,
            shutdown: AtomicBool::new(false),
            owned: Queue::new(),
            registered: AtomicUsize::new(0),
            live: AtomicUsize::new(0),
            running: AtomicUsize::new(0),
            waiter: SwapOption::new(None),
            blocking: Arc::new(Pool::new(
                self.max_blocking,
                self.keep_alive,
                self.name.clone(),
            )),
            reactor,
        });
        if self.kind == Kind::MultiThread {
            let name = self
                .name
                .clone()
                .unwrap_or_else(|| WORKER_THREAD.to_owned());
            for index in 0..workers {
                let held = if workers == 1 {
                    shared.queue.hold()
                } else {
                    None
                };
                let worker = shared.clone();
                shared.running.fetch_add(1, Ordering::SeqCst);
                let spawned = std::thread::Builder::new()
                    .name(name.clone())
                    .spawn(move || worker.work(index, held));
                match spawned {
                    Ok(handle) => shared.slots[index]
                        .thread
                        .store(Some(Arc::new(handle.thread().clone()))),
                    Err(error) => {
                        shared.running.fetch_sub(1, Ordering::SeqCst);
                        let runtime = Runtime {
                            handle: Handle(shared),
                            stopped: false,
                        };
                        runtime.shutdown(Some(Instant::now()));
                        return Err(error);
                    }
                }
            }
        }
        Ok(Runtime {
            handle: Handle(shared),
            stopped: false,
        })
    }
}

pub struct Runtime {
    handle: Handle,
    stopped: bool,
}

impl fmt::Debug for Runtime {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Runtime")
    }
}

impl Runtime {
    pub fn new() -> io::Result<Self> {
        Builder::new_multi_thread().build()
    }

    #[must_use]
    pub const fn handle(&self) -> &Handle {
        &self.handle
    }

    pub fn enter(&self) -> EnterGuard<'_> {
        self.handle.enter()
    }

    #[track_caller]
    pub fn spawn<F>(&self, future: F) -> JoinHandle<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        self.handle.spawn(future)
    }

    #[track_caller]
    pub fn spawn_blocking<F, R>(&self, work: F) -> JoinHandle<R>
    where
        F: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        self.handle.spawn_blocking(work)
    }

    #[track_caller]
    pub fn block_on<F: Future>(&self, future: F) -> F::Output {
        self.handle.block_on(future)
    }

    pub fn shutdown_timeout(mut self, duration: Duration) {
        self.stopped = true;
        self.stop(Instant::now().checked_add(duration));
    }

    pub fn shutdown_background(mut self) {
        self.stopped = true;
        self.stop(Some(Instant::now()));
    }

    fn shutdown(mut self, deadline: Option<Instant>) {
        self.stopped = true;
        self.stop(deadline);
    }

    fn stop(&self, deadline: Option<Instant>) {
        let shared = &self.handle.0;
        shared.shutdown.store(true, Ordering::SeqCst);
        shared.waiter.store(Some(Arc::new(std::thread::current())));
        fence(Ordering::SeqCst);
        shared.notify_all();
        loop {
            if shared.running.load(Ordering::SeqCst) == 0 {
                break;
            }
            match deadline {
                None => std::thread::park(),
                Some(deadline) => {
                    let now = Instant::now();
                    if now >= deadline {
                        break;
                    }
                    std::thread::park_timeout(deadline - now);
                }
            }
        }
        {
            let _context = self.handle.enter();
            shared.cancel_all();
        }
        shared.blocking.shutdown(deadline);
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        if !self.stopped {
            self.stopped = true;
            self.stop(None);
        }
    }
}

#[track_caller]
pub fn spawn<F>(future: F) -> JoinHandle<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    Handle::current().spawn(future)
}

#[track_caller]
pub fn spawn_blocking<F, R>(work: F) -> JoinHandle<R>
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    blocking::spawn_anywhere(work)
}
