use std::{
    collections::BTreeMap,
    io,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering, fence},
    },
    task::{Context, Poll},
    thread::Thread,
    time::{Duration, Instant},
};

use nanocode_sys::{
    AtomicBox, AtomicWaker,
    queue::{Consumer, Pop, Queue},
};

use crate::sys::{Event, Events, Poller, Raw};

const EVENTS_PER_WAIT: usize = 1024;
const REACTOR_THREAD: &str = "nanocode-reactor";

const READABLE: u64 = 1;
const WRITABLE: u64 = 1 << 1;
const READ_CLOSED: u64 = 1 << 2;
const WRITE_CLOSED: u64 = 1 << 3;
const ERROR: u64 = 1 << 4;
const READINESS_BITS: u64 = 32;
const READINESS_MASK: u64 = (1 << READINESS_BITS) - 1;
const NO_DEADLINE: u64 = u64::MAX;
const AWAKE: u64 = 0;
#[cfg(unix)]
const INITIAL: u64 = 0;
#[cfg(windows)]
const INITIAL: u64 = READABLE | WRITABLE;
#[cfg(windows)]
const POLL_RETRY: Duration = Duration::from_millis(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Read,
    Write,
}

impl Direction {
    const fn ready_mask(self) -> u64 {
        match self {
            Self::Read => READABLE | READ_CLOSED,
            Self::Write => WRITABLE | WRITE_CLOSED,
        }
    }

    const fn clear_mask(self) -> u64 {
        match self {
            Self::Read => READABLE,
            Self::Write => WRITABLE,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Ready {
    word: u64,
    direction: Direction,
}

impl Ready {
    #[must_use]
    pub const fn is_readable(&self) -> bool {
        self.word & READABLE != 0
    }

    #[must_use]
    pub const fn is_writable(&self) -> bool {
        self.word & WRITABLE != 0
    }

    #[must_use]
    pub const fn is_read_closed(&self) -> bool {
        self.word & READ_CLOSED != 0
    }

    #[must_use]
    pub const fn is_write_closed(&self) -> bool {
        self.word & WRITE_CLOSED != 0
    }

    #[must_use]
    pub const fn is_error(&self) -> bool {
        self.word & ERROR != 0
    }
}

pub struct Io {
    word: AtomicU64,
    read: AtomicWaker,
    write: AtomicWaker,
}

impl Io {
    const fn new() -> Self {
        Self {
            word: AtomicU64::new(INITIAL),
            read: AtomicWaker::new(),
            write: AtomicWaker::new(),
        }
    }

    const fn waker(&self, direction: Direction) -> &AtomicWaker {
        match direction {
            Direction::Read => &self.read,
            Direction::Write => &self.write,
        }
    }

    fn ready(&self, direction: Direction) -> Option<Ready> {
        let word = self.word.load(Ordering::Acquire);
        (word & direction.ready_mask() != 0).then_some(Ready { word, direction })
    }

    pub fn poll_ready(&self, context: &mut Context<'_>, direction: Direction) -> Poll<Ready> {
        if let Some(ready) = self.ready(direction) {
            return Poll::Ready(ready);
        }
        self.waker(direction).register(context.waker());
        self.ready(direction).map_or(Poll::Pending, Poll::Ready)
    }

    pub fn clear(&self, ready: Ready) {
        let cleared = ready.word & !ready.direction.clear_mask();
        let _ =
            self.word
                .compare_exchange(ready.word, cleared, Ordering::AcqRel, Ordering::Acquire);
    }

    fn deliver(&self, event: &Event) {
        let mut bits = 0;
        if event.readable {
            bits |= READABLE;
        }
        if event.writable {
            bits |= WRITABLE;
        }
        if event.read_closed {
            bits |= READ_CLOSED;
        }
        if event.write_closed {
            bits |= WRITE_CLOSED;
        }
        if event.error {
            bits |= ERROR;
        }
        let mut current = self.word.load(Ordering::Acquire);
        loop {
            let tick = (current >> READINESS_BITS).wrapping_add(1) & READINESS_MASK;
            let next = (tick << READINESS_BITS) | (current & READINESS_MASK) | bits;
            match self.word.compare_exchange_weak(
                current,
                next,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => break,
                Err(actual) => current = actual,
            }
        }
        if bits & (Direction::Read.ready_mask() | ERROR) != 0 {
            self.read.wake();
        }
        if bits & (Direction::Write.ready_mask() | ERROR) != 0 {
            self.write.wake();
        }
    }
}

pub struct Timer {
    fired: AtomicBool,
    waker: AtomicWaker,
}

impl Timer {
    fn poll_fired(&self, context: &mut Context<'_>) -> Poll<()> {
        if self.fired.load(Ordering::Acquire) {
            return Poll::Ready(());
        }
        self.waker.register(context.waker());
        if self.fired.load(Ordering::Acquire) {
            return Poll::Ready(());
        }
        Poll::Pending
    }

    fn fire(&self) {
        self.fired.store(true, Ordering::Release);
        self.waker.wake();
    }
}

enum Command {
    Add(u64, Arc<Io>),
    Remove(u64),
    Arm(Instant, u64, Arc<Timer>),
    Disarm(Instant, u64),
}

pub struct Driver {
    consumer: Option<Consumer>,
    sources: BTreeMap<u64, Arc<Io>>,
    timers: BTreeMap<(Instant, u64), Arc<Timer>>,
    events: Events,
}

pub struct Reactor {
    poller: Poller,
    commands: Queue<Command>,
    driver: AtomicBox<Driver>,
    waiting: Queue<Thread>,
    waiting_consumer: AtomicBox<Consumer>,
    waiting_count: AtomicUsize,
    drivers: AtomicUsize,
    fallback_driving: AtomicBool,
    fallback: OnceLock<Thread>,
    next_token: AtomicU64,
    next_wake_ns: AtomicU64,
    origin: Instant,
}

static REACTOR: OnceLock<Result<Reactor, String>> = OnceLock::new();

pub fn shared() -> io::Result<&'static Reactor> {
    REACTOR
        .get_or_init(start)
        .as_ref()
        .map_err(|message| io::Error::other(message.clone()))
}

pub fn reactor() -> io::Result<&'static Reactor> {
    let reactor = shared()?;
    if reactor.drivers.load(Ordering::SeqCst) == 0 {
        reactor.fallback()?;
    }
    Ok(reactor)
}

fn start() -> Result<Reactor, String> {
    let poller = Poller::new().map_err(|error| format!("the reactor's poller failed: {error}"))?;
    let driver = Driver {
        consumer: None,
        sources: BTreeMap::new(),
        timers: BTreeMap::new(),
        events: Events::with_capacity(EVENTS_PER_WAIT),
    };
    Ok(Reactor {
        poller,
        commands: Queue::new(),
        driver: AtomicBox::new(Some(Box::new(driver))),
        waiting: Queue::new(),
        waiting_consumer: AtomicBox::empty(),
        waiting_count: AtomicUsize::new(0),
        drivers: AtomicUsize::new(0),
        fallback_driving: AtomicBool::new(false),
        fallback: OnceLock::new(),
        next_token: AtomicU64::new(0),
        next_wake_ns: AtomicU64::new(AWAKE),
        origin: Instant::now(),
    })
}

fn fallback_loop(reactor: &'static Reactor) {
    loop {
        if reactor.drivers.load(Ordering::SeqCst) > 0 {
            std::thread::park();
            continue;
        }
        let Some(mut driver) = reactor.driver.take() else {
            reactor.park_for_driver();
            continue;
        };
        reactor.fallback_driving.store(true, Ordering::SeqCst);
        if reactor.drivers.load(Ordering::SeqCst) == 0 {
            reactor.turn_with(&mut driver, None, || {});
        }
        reactor.fallback_driving.store(false, Ordering::SeqCst);
        reactor.release(driver);
    }
}

impl Driver {
    fn drain(&mut self, reactor: &Reactor) -> bool {
        let mut drained = false;
        let mut consumer = self.consumer.take().unwrap_or_else(|| {
            reactor
                .commands
                .consumer()
                .expect("the driver holds the command queue's one consumer")
        });
        loop {
            match reactor.commands.pop(&mut consumer) {
                Pop::Value(command) => {
                    drained = true;
                    self.apply(command);
                }
                Pop::Pushing => std::hint::spin_loop(),
                Pop::Empty => break,
            }
        }
        self.consumer = Some(consumer);
        drained
    }

    fn apply(&mut self, command: Command) {
        match command {
            Command::Add(token, io) => {
                let previous = self.sources.insert(token, io);
                assert!(previous.is_none(), "a reactor token is used once");
            }
            Command::Remove(token) => {
                self.sources.remove(&token);
            }
            Command::Arm(deadline, id, timer) => {
                let previous = self.timers.insert((deadline, id), timer);
                assert!(previous.is_none(), "a timer id is used once");
            }
            Command::Disarm(deadline, id) => {
                self.timers.remove(&(deadline, id));
            }
        }
    }

    fn fire(&mut self, now: Instant) -> bool {
        let mut fired = false;
        while let Some(entry) = self.timers.first_entry() {
            if entry.key().0 > now {
                break;
            }
            entry.remove().fire();
            fired = true;
        }
        fired
    }
}

impl Reactor {
    fn fallback(&'static self) -> io::Result<()> {
        if self.fallback.get().is_some() {
            return Ok(());
        }
        let mut failure = None;
        self.fallback.get_or_init(|| {
            match std::thread::Builder::new()
                .name(REACTOR_THREAD.to_owned())
                .spawn(move || fallback_loop(self))
            {
                Ok(handle) => handle.thread().clone(),
                Err(error) => {
                    failure = Some(error);
                    std::thread::current()
                }
            }
        });
        failure.map_or(Ok(()), Err)
    }

    fn nanos(&self, deadline: Instant) -> u64 {
        u64::try_from(deadline.saturating_duration_since(self.origin).as_nanos())
            .unwrap_or(NO_DEADLINE - 1)
    }

    fn token(&self) -> u64 {
        self.next_token.fetch_add(1, Ordering::Relaxed)
    }

    pub fn wake(&self) {
        let _ = self.poller.wake();
    }

    fn release(&self, driver: Box<Driver>) {
        self.driver.store(Some(driver));
        fence(Ordering::SeqCst);
        self.notify_waiting();
    }

    pub fn park_for_driver(&self) {
        self.waiting_count.fetch_add(1, Ordering::SeqCst);
        self.waiting.push(std::thread::current());
        fence(Ordering::SeqCst);
        if self.fallback_driving.load(Ordering::SeqCst) && self.drivers.load(Ordering::SeqCst) > 0 {
            self.wake();
        }
        if let Some(driver) = self.driver.take() {
            self.release(driver);
            return;
        }
        std::thread::park();
    }

    fn notify_waiting(&self) {
        while self.waiting_count.load(Ordering::SeqCst) > 0 {
            let Some(mut consumer) = self
                .waiting_consumer
                .take()
                .or_else(|| self.waiting.consumer().map(Box::new))
            else {
                return;
            };
            loop {
                match self.waiting.pop(&mut consumer) {
                    Pop::Value(thread) => {
                        self.waiting_count.fetch_sub(1, Ordering::SeqCst);
                        thread.unpark();
                    }
                    Pop::Pushing => std::hint::spin_loop(),
                    Pop::Empty => break,
                }
            }
            self.waiting_consumer.store(Some(consumer));
            fence(Ordering::SeqCst);
        }
    }

    pub fn enter_driver(&'static self) {
        if self.drivers.fetch_add(1, Ordering::SeqCst) == 0
            && self.fallback_driving.load(Ordering::SeqCst)
        {
            self.wake();
        }
    }

    pub fn exit_driver(&'static self) {
        if self.drivers.fetch_sub(1, Ordering::SeqCst) == 1 {
            if let Some(thread) = self.fallback.get() {
                thread.unpark();
            } else {
                let _ = self.fallback();
            }
        }
        self.notify_waiting();
    }

    pub fn try_turn(&self, timeout: Option<Duration>, woken: impl FnOnce()) -> bool {
        let Some(mut driver) = self.driver.take() else {
            return false;
        };
        self.turn_with(&mut driver, timeout, woken);
        self.release(driver);
        true
    }

    fn turn_with(&self, driver: &mut Driver, timeout: Option<Duration>, woken: impl FnOnce()) {
        driver.drain(self);
        let now = Instant::now();
        if driver.fire(now) {
            woken();
            return;
        }
        let deadline = driver.timers.keys().next().map(|(deadline, _)| *deadline);
        let sleep = match (deadline, timeout) {
            (Some(deadline), Some(timeout)) => {
                Some(deadline.saturating_duration_since(now).min(timeout))
            }
            (Some(deadline), None) => Some(deadline.saturating_duration_since(now)),
            (None, timeout) => timeout,
        };
        let blocking = sleep != Some(Duration::ZERO);
        if blocking {
            let planned = sleep.map_or(NO_DEADLINE, |sleep| self.nanos(now + sleep).max(AWAKE + 1));
            self.next_wake_ns.store(planned, Ordering::SeqCst);
            if driver.drain(self) {
                self.next_wake_ns.store(AWAKE, Ordering::SeqCst);
                woken();
                return;
            }
        }
        if let Err(error) = self.poller.wait(&mut driver.events, sleep) {
            panic!("the reactor's wait failed: {error}");
        }
        if blocking {
            self.next_wake_ns.store(AWAKE, Ordering::SeqCst);
        }
        woken();
        driver.drain(self);
        for event in driver.events.iter() {
            if let Some(io) = driver.sources.get(&event.token) {
                io.deliver(&event);
            }
        }
        driver.fire(Instant::now());
    }

    pub fn register(&'static self, fd: Raw<'_>) -> io::Result<Registration> {
        let token = self.token();
        let io = Arc::new(Io::new());
        self.commands.push(Command::Add(token, io.clone()));
        let registration = Registration {
            reactor: self,
            token,
            io,
            #[cfg(windows)]
            retry: [AtomicBox::empty(), AtomicBox::empty()],
        };
        self.poller.register(fd, token)?;
        Ok(registration)
    }

    pub fn arm(&'static self, deadline: Instant) -> Armed {
        let id = self.token();
        let timer = Arc::new(Timer {
            fired: AtomicBool::new(false),
            waker: AtomicWaker::new(),
        });
        self.commands
            .push(Command::Arm(deadline, id, timer.clone()));
        fence(Ordering::SeqCst);
        if self.nanos(deadline) < self.next_wake_ns.load(Ordering::SeqCst) {
            self.wake();
        }
        Armed {
            reactor: self,
            deadline,
            id,
            timer,
        }
    }
}

pub struct Registration {
    reactor: &'static Reactor,
    token: u64,
    io: Arc<Io>,
    #[cfg(windows)]
    retry: [AtomicBox<Armed>; 2],
}

impl Registration {
    #[must_use]
    pub fn io(&self) -> &Io {
        &self.io
    }

    pub fn deregister(&self, fd: Raw<'_>) -> io::Result<()> {
        self.reactor.poller.deregister(fd)
    }

    #[cfg(windows)]
    pub fn poll_io<R>(
        &self,
        context: &mut Context<'_>,
        direction: Direction,
        mut operation: impl FnMut() -> io::Result<R>,
    ) -> Poll<io::Result<R>> {
        match operation() {
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                let armed = self.reactor.arm(Instant::now() + POLL_RETRY);
                if armed.poll_fired(context).is_ready() {
                    context.waker().wake_by_ref();
                }
                let slot = match direction {
                    Direction::Read => &self.retry[0],
                    Direction::Write => &self.retry[1],
                };
                slot.store(Some(Box::new(armed)));
                Poll::Pending
            }
            result => Poll::Ready(result),
        }
    }

    #[cfg(windows)]
    pub fn try_io<R>(
        &self,
        _direction: Direction,
        operation: impl FnOnce() -> io::Result<R>,
    ) -> io::Result<R> {
        operation()
    }

    #[cfg(unix)]
    pub fn poll_io<R>(
        &self,
        context: &mut Context<'_>,
        direction: Direction,
        mut operation: impl FnMut() -> io::Result<R>,
    ) -> Poll<io::Result<R>> {
        loop {
            let Poll::Ready(ready) = self.io.poll_ready(context, direction) else {
                return Poll::Pending;
            };
            match operation() {
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => self.io.clear(ready),
                result => return Poll::Ready(result),
            }
        }
    }

    #[cfg(unix)]
    pub fn try_io<R>(
        &self,
        direction: Direction,
        operation: impl FnOnce() -> io::Result<R>,
    ) -> io::Result<R> {
        let Some(ready) = self.io.ready(direction) else {
            return Err(io::ErrorKind::WouldBlock.into());
        };
        let result = operation();
        if let Err(error) = &result
            && error.kind() == io::ErrorKind::WouldBlock
        {
            self.io.clear(ready);
        }
        result
    }

    pub async fn ready(&self, direction: Direction) -> Ready {
        std::future::poll_fn(|context| self.io.poll_ready(context, direction)).await
    }

    pub fn clear(&self, ready: Ready) {
        self.io.clear(ready);
    }
}

impl Drop for Registration {
    fn drop(&mut self) {
        self.reactor.commands.push(Command::Remove(self.token));
    }
}

pub struct Armed {
    reactor: &'static Reactor,
    deadline: Instant,
    id: u64,
    timer: Arc<Timer>,
}

impl Armed {
    pub fn poll_fired(&self, context: &mut Context<'_>) -> Poll<()> {
        self.timer.poll_fired(context)
    }
}

impl Drop for Armed {
    fn drop(&mut self) {
        if !self.timer.fired.load(Ordering::Acquire) {
            self.reactor
                .commands
                .push(Command::Disarm(self.deadline, self.id));
        }
    }
}
