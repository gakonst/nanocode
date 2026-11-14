use std::{
    cell::{Cell, RefCell},
    future::{Future, poll_fn},
    ops::Range,
    pin::{Pin, pin},
    process::ExitCode,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    task::{Context, Poll, Wake, Waker},
    time::Duration,
};

use nanocode_sync::{
    Notify, Semaphore, SemaphorePermit,
    broadcast::{self, error::RecvError},
    mpsc::{self, error::TrySendError},
    watch,
};

use crate::{
    executor::{Clock, Ended, Executor, Task},
    rng::{FNV_OFFSET, Rng, fnv},
};

const PAUSE_MICROS_MAX: u64 = 40;
const CANCEL_MICROS_MAX: u64 = 30;
const CANCEL_PPM: u64 = 250_000;
const WAITERS_MAX: u64 = 3;
const WAKES_MAX: u64 = 4;
const PERMITS_MAX: u64 = 3;
const HOLDERS_MAX: u64 = 4;
const ACQUIRES_MAX: u64 = 6;
const ADDS_MAX: u64 = 3;
const CAPACITY_MAX: u64 = 4;
const SENDERS_MAX: u64 = 3;
const MESSAGES_MAX: u64 = 8;
const VALUES_MAX: u64 = 10;
const RECEIVERS_MAX: u64 = 3;
const TRY_SENDS_MAX: u64 = 8;
const CLOSE_PPM: u64 = 20_000;
const PATIENCE: Duration = Duration::from_secs(1);
const PATIENCE_STEP: Duration = Duration::from_micros(100);

type Job<'a> = Pin<Box<dyn Future<Output = ()> + 'a>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyncMutation {
    None,
    SkipNotify,
    LeakPermit,
    ReverseQueue,
    Resend,
    ShrinkCapacity,
    WatchRewind,
    BroadcastExtra,
    RingOffByOne,
}

impl SyncMutation {
    const ALL: [Self; 9] = [
        Self::None,
        Self::SkipNotify,
        Self::LeakPermit,
        Self::ReverseQueue,
        Self::Resend,
        Self::ShrinkCapacity,
        Self::WatchRewind,
        Self::BroadcastExtra,
        Self::RingOffByOne,
    ];

    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|mutation| mutation.name() == name)
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::SkipNotify => "skip_notify",
            Self::LeakPermit => "leak_permit",
            Self::ReverseQueue => "reverse_queue",
            Self::Resend => "resend",
            Self::ShrinkCapacity => "shrink_capacity",
            Self::WatchRewind => "watch_rewind",
            Self::BroadcastExtra => "broadcast_extra",
            Self::RingOffByOne => "ring_off_by_one",
        }
    }

    pub fn names() -> String {
        Self::ALL[1..]
            .iter()
            .map(|mutation| mutation.name())
            .collect::<Vec<_>>()
            .join("|")
    }
}

struct Yield(bool);

impl Future for Yield {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<()> {
        if self.0 {
            return Poll::Ready(());
        }
        self.0 = true;
        context.waker().wake_by_ref();
        Poll::Pending
    }
}

struct World {
    clock: Arc<Clock>,
    mutation: SyncMutation,
    trace: RefCell<Vec<String>>,
    wrong: RefCell<Vec<String>>,
}

impl World {
    fn note(&self, line: String) {
        self.trace.borrow_mut().push(line);
    }

    fn fail(&self, line: String) {
        self.wrong.borrow_mut().push(line);
    }

    async fn pause(&self, rng: &mut Rng) {
        match rng.below(3) {
            0 => {}
            1 => Yield(false).await,
            _ => {
                self.clock
                    .sleep(Duration::from_micros(rng.range(1, PAUSE_MICROS_MAX)))
                    .await;
            }
        }
    }

    fn limit(rng: &mut Rng) -> Option<Duration> {
        rng.chance(CANCEL_PPM)
            .then(|| Duration::from_micros(rng.range(1, CANCEL_MICROS_MAX)))
    }

    async fn within<F: Future>(&self, limit: Option<Duration>, future: F) -> Option<F::Output> {
        let mut future = pin!(future);
        let mut sleep = limit.map(|limit| self.clock.sleep(limit));
        poll_fn(|context| {
            if let Poll::Ready(output) = future.as_mut().poll(context) {
                return Poll::Ready(Some(output));
            }
            let expired = sleep
                .as_mut()
                .is_some_and(|sleep| Pin::new(sleep).poll(context).is_ready());
            if expired {
                Poll::Ready(None)
            } else {
                Poll::Pending
            }
        })
        .await
    }

    async fn until(&self, mut done: impl FnMut() -> bool) -> bool {
        let deadline = self.clock.now_ns() + nanos(PATIENCE);
        while !done() {
            if self.clock.now_ns() >= deadline {
                return false;
            }
            self.clock.sleep(PATIENCE_STEP).await;
        }
        true
    }
}

fn nanos(duration: Duration) -> u64 {
    u64::try_from(duration.as_nanos()).expect("a simulated duration fits in u64 nanoseconds")
}

fn count(value: u64) -> usize {
    usize::try_from(value).expect("a simulated count fits in usize")
}

fn wide(value: usize) -> u64 {
    u64::try_from(value).expect("a simulated count fits in u64")
}

async fn join_all(mut jobs: Vec<Job<'_>>, world: &World, what: &str) {
    poll_fn(|context| {
        jobs.retain_mut(|job| job.as_mut().poll(context).is_pending());
        if jobs.is_empty() {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    })
    .await;
    world.note(format!("{what} joined"));
}

struct Script {
    notify_wakes: Vec<u64>,
    everyone: u64,
    permits: u64,
    holders: Vec<u64>,
    adds: Vec<u64>,
    capacity: u64,
    senders: Vec<u64>,
    watch_values: u64,
    watch_receivers: u64,
    ring: u64,
    ring_values: u64,
    ring_receivers: u64,
}

impl Script {
    fn of(seed: u64) -> Self {
        let mut rng = Rng::derive(seed, "sync-script");
        let notify_wakes = (0..rng.range(1, WAITERS_MAX))
            .map(|_| rng.range(1, WAKES_MAX))
            .collect();
        let everyone = rng.range(1, WAITERS_MAX);
        let permits = rng.range(1, PERMITS_MAX);
        let holders = (0..rng.range(2, HOLDERS_MAX))
            .map(|_| rng.range(1, ACQUIRES_MAX))
            .collect();
        let adds = (0..rng.below(ADDS_MAX + 1))
            .map(|_| rng.range(1, PERMITS_MAX))
            .collect();
        let capacity = rng.range(1, CAPACITY_MAX);
        let senders = (0..rng.range(1, SENDERS_MAX))
            .map(|_| rng.range(1, MESSAGES_MAX))
            .collect();
        Self {
            notify_wakes,
            everyone,
            permits,
            holders,
            adds,
            capacity,
            senders,
            watch_values: rng.range(1, VALUES_MAX),
            watch_receivers: rng.range(1, RECEIVERS_MAX),
            ring: rng.range(1, CAPACITY_MAX),
            ring_values: rng.range(1, VALUES_MAX),
            ring_receivers: rng.range(1, RECEIVERS_MAX),
        }
    }

    fn describe(&self) -> String {
        format!(
            "notify={:?}/{} semaphore={}+{:?} holders={:?} mpsc={} senders={:?} watch={}x{} broadcast={}:{}x{}",
            self.notify_wakes,
            self.everyone,
            self.permits,
            self.adds,
            self.holders,
            self.capacity,
            self.senders,
            self.watch_values,
            self.watch_receivers,
            self.ring,
            self.ring_values,
            self.ring_receivers
        )
    }
}

async fn notify_waiter(world: &World, notify: &Notify, done: &Cell<u64>, wakes: u64, index: usize, seed: u64) {
    let mut rng = Rng::derive(seed, &format!("notify-waiter-{index}"));
    let mut got = 0;
    while got < wakes {
        world.pause(&mut rng).await;
        let limit = World::limit(&mut rng);
        let mut notified = pin!(notify.notified());
        if rng.chance(CANCEL_PPM) && notified.as_mut().enable() {
            got += 1;
            done.set(done.get() + 1);
            world.note(format!("notify waiter {index} enabled into a wake"));
            continue;
        }
        if world.within(limit, notified).await.is_some() {
            got += 1;
            done.set(done.get() + 1);
            world.note(format!("notify waiter {index} woke"));
        } else {
            world.note(format!("notify waiter {index} gave up"));
        }
    }
}

async fn notify_scenario(world: &World, script: &Script, seed: u64) {
    let notify = Notify::new();
    let done = Cell::new(0_u64);
    let total: u64 = script.notify_wakes.iter().sum();
    let mut jobs: Vec<Job<'_>> = script
        .notify_wakes
        .iter()
        .enumerate()
        .map(|(index, wakes)| {
            Box::pin(notify_waiter(world, &notify, &done, *wakes, index, seed)) as Job<'_>
        })
        .collect();
    jobs.push(Box::pin(async {
        let mut rng = Rng::derive(seed, "notifier");
        let rounds = if world.mutation == SyncMutation::SkipNotify {
            total - 1
        } else {
            total
        };
        for issued in 0..rounds {
            if !world.until(|| done.get() == issued).await {
                world.fail(format!(
                    "notify: {issued} notify_one calls but {} wakes after waiting {PATIENCE:?}",
                    done.get()
                ));
                return;
            }
            world.pause(&mut rng).await;
            if rng.chance(CANCEL_PPM) {
                notify.notify_last();
            } else {
                notify.notify_one();
            }
        }
        if !world.until(|| done.get() == total).await {
            world.fail(format!(
                "notify: lost wakeup: {rounds} notify_one calls, {} of {total} wakes after waiting {PATIENCE:?}",
                done.get()
            ));
        }
    }));
    join_all(jobs, world, "notify").await;
    let everyone = Notify::new();
    let ready = Cell::new(0_u64);
    let woke = Cell::new(0_u64);
    let waiters = script.everyone;
    let mut jobs: Vec<Job<'_>> = (0..waiters)
        .map(|index| {
            let (everyone, ready, woke) = (&everyone, &ready, &woke);
            Box::pin(async move {
                let mut rng = Rng::derive(seed, &format!("everyone-{index}"));
                world.pause(&mut rng).await;
                let mut notified = pin!(everyone.notified());
                notified.as_mut().enable();
                ready.set(ready.get() + 1);
                notified.await;
                woke.set(woke.get() + 1);
            }) as Job<'_>
        })
        .collect();
    jobs.push(Box::pin(async {
        if !world.until(|| ready.get() == waiters).await {
            world.fail("notify_waiters: the waiters never got ready".to_owned());
            return;
        }
        everyone.notify_waiters();
        if !world.until(|| woke.get() == waiters).await {
            world.fail(format!(
                "notify_waiters: {} of {waiters} enabled waiters woke",
                woke.get()
            ));
        }
    }));
    join_all(jobs, world, "notify_waiters").await;
}

struct Woke {
    granted: Arc<AtomicBool>,
    waker: Waker,
}

impl Wake for Woke {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.granted.store(true, Ordering::SeqCst);
        self.waker.wake_by_ref();
    }
}

struct Waiting {
    ticket: u64,
    holder: usize,
    granted: Arc<AtomicBool>,
}

struct Ledger {
    total: Cell<u64>,
    held: Cell<u64>,
    next_ticket: Cell<u64>,
    queue: RefCell<Vec<Waiting>>,
}

impl Ledger {
    fn enqueue(&self, holder: usize, reverse: bool, granted: &Arc<AtomicBool>) -> u64 {
        let ticket = self.next_ticket.get();
        self.next_ticket.set(ticket + 1);
        let waiting = Waiting {
            ticket,
            holder,
            granted: Arc::clone(granted),
        };
        let mut queue = self.queue.borrow_mut();
        if reverse {
            queue.insert(0, waiting);
        } else {
            queue.push(waiting);
        }
        ticket
    }

    fn leave(&self, ticket: u64) {
        self.queue
            .borrow_mut()
            .retain(|waiting| waiting.ticket != ticket);
    }

    fn ahead(&self, ticket: Option<u64>) -> Option<(u64, usize)> {
        self.queue
            .borrow()
            .iter()
            .take_while(|waiting| Some(waiting.ticket) != ticket)
            .find(|waiting| !waiting.granted.load(Ordering::SeqCst))
            .map(|waiting| (waiting.ticket, waiting.holder))
    }

    fn waiting(&self) -> Vec<u64> {
        self.queue
            .borrow()
            .iter()
            .map(|waiting| waiting.ticket)
            .collect()
    }
}

struct Tracked<'a, F> {
    future: Pin<&'a mut F>,
    ledger: &'a Ledger,
    holder: usize,
    reverse: bool,
    ticket: Option<u64>,
    granted: Arc<AtomicBool>,
}

impl<F: Future> Future for Tracked<'_, F> {
    type Output = (F::Output, Option<u64>);

    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let this = &mut *self;
        let waker = Waker::from(Arc::new(Woke {
            granted: Arc::clone(&this.granted),
            waker: context.waker().clone(),
        }));
        match this.future.as_mut().poll(&mut Context::from_waker(&waker)) {
            Poll::Ready(output) => Poll::Ready((output, this.ticket.take())),
            Poll::Pending => {
                if this.ticket.is_none() {
                    this.ticket = Some(this.ledger.enqueue(
                        this.holder,
                        this.reverse,
                        &this.granted,
                    ));
                }
                Poll::Pending
            }
        }
    }
}

impl<F> Drop for Tracked<'_, F> {
    fn drop(&mut self) {
        if let Some(ticket) = self.ticket.take() {
            self.ledger.leave(ticket);
        }
    }
}

struct Holding<'a> {
    world: &'a World,
    ledger: &'a Ledger,
    index: usize,
    first: bool,
}

impl Holding<'_> {
    async fn hold(&self, wanted: u64, rng: &mut Rng, permit: SemaphorePermit<'_>) {
        let ledger = self.ledger;
        ledger.held.set(ledger.held.get() + wanted);
        if ledger.held.get() > ledger.total.get() {
            self.world.fail(format!(
                "semaphore: {} permits held of {} in all",
                ledger.held.get(),
                ledger.total.get()
            ));
        }
        self.world
            .note(format!("holder {} holds {wanted}", self.index));
        self.world.pause(rng).await;
        ledger.held.set(ledger.held.get() - wanted);
        if self.world.mutation == SyncMutation::LeakPermit && self.index == 0 && self.first {
            std::mem::forget(permit);
        } else {
            drop(permit);
        }
    }
}

async fn holder(world: &World, semaphore: &Semaphore, ledger: &Ledger, script: &Script, index: usize, acquires: u64, seed: u64) {
    let mut rng = Rng::derive(seed, &format!("holder-{index}"));
    let reverse = world.mutation == SyncMutation::ReverseQueue;
    for round in 0..acquires {
        let holding = Holding {
            world,
            ledger,
            index,
            first: round == 0,
        };
        world.pause(&mut rng).await;
        let wanted = rng.range(1, script.permits);
        let permits = u32::try_from(wanted).expect("a simulated permit count fits in u32");
        if rng.chance(CANCEL_PPM) {
            match semaphore.try_acquire_many(permits) {
                Ok(permit) => {
                    if let Some((ticket, first)) = ledger.ahead(None) {
                        world.fail(format!(
                            "semaphore: holder {index} took {wanted} with try_acquire while holder {first} waits with ticket {ticket}"
                        ));
                    }
                    holding.hold(wanted, &mut rng, permit).await;
                }
                Err(_) => world.note(format!("holder {index} try_acquire {wanted} refused")),
            }
            continue;
        }
        let limit = World::limit(&mut rng);
        let mut acquire = pin!(semaphore.acquire_many(permits));
        let tracked = Tracked {
            future: acquire.as_mut(),
            ledger,
            holder: index,
            reverse,
            ticket: None,
            granted: Arc::new(AtomicBool::new(false)),
        };
        let Some((acquired, ticket)) = world.within(limit, tracked).await else {
            world.note(format!("holder {index} gave up on {wanted}"));
            continue;
        };
        let Ok(permit) = acquired else {
            world.fail(format!("semaphore: holder {index} saw the open semaphore closed"));
            return;
        };
        if let Some((first, first_holder)) = ledger.ahead(ticket) {
            world.fail(format!(
                "semaphore: holder {index} (ticket {ticket:?}) got {wanted} before holder {first_holder} (ticket {first}) queued ahead of it"
            ));
        }
        if let Some(ticket) = ticket {
            ledger.leave(ticket);
        }
        holding.hold(wanted, &mut rng, permit).await;
    }
}

async fn semaphore_scenario(world: &World, script: &Script, seed: u64) {
    let semaphore = Semaphore::new(count(script.permits));
    let ledger = Ledger {
        total: Cell::new(script.permits),
        held: Cell::new(0),
        next_ticket: Cell::new(0),
        queue: RefCell::new(Vec::new()),
    };
    let mut jobs: Vec<Job<'_>> = script
        .holders
        .iter()
        .enumerate()
        .map(|(index, acquires)| {
            Box::pin(holder(world, &semaphore, &ledger, script, index, *acquires, seed)) as Job<'_>
        })
        .collect();
    jobs.push(Box::pin(async {
        let mut rng = Rng::derive(seed, "adder");
        for added in &script.adds {
            world.pause(&mut rng).await;
            ledger.total.set(ledger.total.get() + added);
            semaphore.add_permits(count(*added));
        }
    }));
    join_all(jobs, world, "semaphore").await;
    let available = wide(semaphore.available_permits());
    if available != ledger.total.get() || ledger.held.get() != 0 {
        world.fail(format!(
            "semaphore: {available} permits available after every holder left, {} expected",
            ledger.total.get()
        ));
    }
    let waiting = ledger.waiting();
    if !waiting.is_empty() {
        world.fail(format!("semaphore: waiters left queued: {waiting:?}"));
    }
}

struct Mail<'a> {
    world: &'a World,
    sent: &'a RefCell<Vec<Vec<u64>>>,
    flight: &'a Cell<u64>,
    capacity: u64,
}

impl Mail<'_> {
    async fn deliver(&self, channel: &mpsc::Sender<(usize, u64)>, message: (usize, u64), rng: &mut Rng) -> bool {
        let world = self.world;
        match rng.below(3) {
            0 => {
                let limit = World::limit(rng);
                world
                    .within(limit, channel.send(message))
                    .await
                    .is_some_and(|sent| sent.is_ok())
            }
            1 => {
                let limit = World::limit(rng);
                match world.within(limit, channel.reserve()).await {
                    Some(Ok(permit)) => {
                        world.pause(rng).await;
                        permit.send(message);
                        true
                    }
                    _ => false,
                }
            }
            _ => {
                for _ in 0..TRY_SENDS_MAX {
                    match channel.try_send(message) {
                        Ok(()) => return true,
                        Err(TrySendError::Full(_)) => world.pause(rng).await,
                        Err(TrySendError::Closed(_)) => return false,
                    }
                }
                false
            }
        }
    }

    async fn sender(&self, channel: mpsc::Sender<(usize, u64)>, index: usize, messages: u64, seed: u64) {
        let world = self.world;
        let mut rng = Rng::derive(seed, &format!("sender-{index}"));
        let model = if world.mutation == SyncMutation::ShrinkCapacity {
            self.capacity - 1
        } else {
            self.capacity
        };
        for sequence in 0..messages {
            world.pause(&mut rng).await;
            let message = (index, sequence);
            if !self.deliver(&channel, message, &mut rng).await {
                world.note(format!("sender {index} dropped message {sequence}"));
                continue;
            }
            self.sent.borrow_mut()[index].push(sequence);
            self.flight.set(self.flight.get() + 1);
            if self.flight.get() > model {
                world.fail(format!(
                    "mpsc: {} messages in flight in a channel of capacity {}",
                    self.flight.get(),
                    self.capacity
                ));
            }
            if world.mutation == SyncMutation::Resend
                && index == 0
                && sequence == 0
                && channel.send(message).await.is_ok()
            {
                self.flight.set(self.flight.get() + 1);
            }
        }
    }
}

async fn mpsc_scenario(world: &World, script: &Script, seed: u64) {
    let (channel, mut receiver) = mpsc::channel::<(usize, u64)>(count(script.capacity));
    let sent = RefCell::new(vec![Vec::new(); script.senders.len()]);
    let received = RefCell::new(vec![Vec::new(); script.senders.len()]);
    let flight = Cell::new(0_u64);
    let mail = Mail {
        world,
        sent: &sent,
        flight: &flight,
        capacity: script.capacity,
    };
    let mail = &mail;
    let mut jobs: Vec<Job<'_>> = script
        .senders
        .iter()
        .enumerate()
        .map(|(index, messages)| {
            Box::pin(mail.sender(channel.clone(), index, *messages, seed)) as Job<'_>
        })
        .collect();
    drop(channel);
    let (received_ref, flight_ref) = (&received, &flight);
    jobs.push(Box::pin(async move {
        let mut rng = Rng::derive(seed, "receiver");
        let mut open = true;
        loop {
            world.pause(&mut rng).await;
            if open && rng.chance(CLOSE_PPM) {
                receiver.close();
                open = false;
                world.note("receiver closed".to_owned());
            }
            let limit = World::limit(&mut rng);
            match world.within(limit, receiver.recv()).await {
                None => {}
                Some(None) => break,
                Some(Some((index, sequence))) => {
                    flight_ref.set(flight_ref.get().saturating_sub(1));
                    received_ref.borrow_mut()[index].push(sequence);
                }
            }
        }
    }));
    join_all(jobs, world, "mpsc").await;
    let sent = sent.into_inner();
    let received = received.into_inner();
    for (index, (sent, received)) in sent.iter().zip(&received).enumerate() {
        if sent != received {
            world.fail(format!(
                "mpsc: sender {index} sent {sent:?} but the receiver got {received:?}"
            ));
        }
    }
}

async fn watcher(world: &World, mut receiver: watch::Receiver<u64>, values: u64, index: u64, seed: u64) {
    let mut rng = Rng::derive(seed, &format!("watcher-{index}"));
    let mut last = 0;
    loop {
        world.pause(&mut rng).await;
        let limit = World::limit(&mut rng);
        match world.within(limit, receiver.changed()).await {
            None => {}
            Some(Err(_)) => break,
            Some(Ok(())) => {
                let value = *receiver.borrow_and_update();
                if value <= last {
                    world.fail(format!("watch: receiver {index} saw {value} after {last}"));
                }
                last = value;
            }
        }
    }
    let model = if world.mutation == SyncMutation::WatchRewind {
        values + 1
    } else {
        values
    };
    let borrowed = *receiver.borrow();
    if last != model || borrowed != values {
        world.fail(format!(
            "watch: receiver {index} ended on {last} (borrow {borrowed}) after the sender sent up to {values}"
        ));
    }
}

async fn watch_scenario(world: &World, script: &Script, seed: u64) {
    let (sender, receiver) = watch::channel(0_u64);
    let values = script.watch_values;
    let mut jobs: Vec<Job<'_>> = (0..script.watch_receivers)
        .map(|index| Box::pin(watcher(world, receiver.clone(), values, index, seed)) as Job<'_>)
        .collect();
    drop(receiver);
    jobs.push(Box::pin(async move {
        let mut rng = Rng::derive(seed, "watch-sender");
        for value in 1..=values {
            world.pause(&mut rng).await;
            match rng.below(3) {
                0 => {
                    if sender.send(value).is_err() {
                        world.fail("watch: send refused with receivers alive".to_owned());
                    }
                }
                1 => {
                    sender.send_replace(value);
                }
                _ => sender.send_modify(|current| *current = value),
            }
        }
        drop(sender);
    }));
    join_all(jobs, world, "watch").await;
}

struct Ring<'a> {
    world: &'a World,
    sent: &'a Cell<u64>,
    retained: u64,
    values: u64,
}

impl Ring<'_> {
    async fn listen(&self, mut receiver: broadcast::Receiver<u64>, index: u64, seed: u64) {
        let world = self.world;
        let mut rng = Rng::derive(seed, &format!("listener-{index}"));
        let mut expected = 1_u64;
        loop {
            world.pause(&mut rng).await;
            let limit = World::limit(&mut rng);
            match world.within(limit, receiver.recv()).await {
                None => {}
                Some(Err(RecvError::Closed)) => break,
                Some(Err(RecvError::Lagged(missed))) => {
                    expected += missed;
                    let kept = (self.sent.get() + 1).saturating_sub(expected);
                    if kept != self.retained {
                        world.fail(format!(
                            "broadcast: receiver {index} lagged by {missed} to {expected} with {} sent, keeping {kept} of a ring that holds {}",
                            self.sent.get(),
                            self.retained
                        ));
                    }
                }
                Some(Ok(value)) => {
                    if value != expected {
                        world.fail(format!(
                            "broadcast: receiver {index} got {value} when {expected} was next"
                        ));
                    }
                    expected = value + 1;
                }
            }
        }
        if expected != self.values + 1 {
            world.fail(format!(
                "broadcast: receiver {index} closed at {expected} after {} values",
                self.values
            ));
        }
    }
}

async fn broadcast_scenario(world: &World, script: &Script, seed: u64) {
    let (sender, receiver) = broadcast::channel::<u64>(count(script.ring));
    let values = script.ring_values;
    let sent = Cell::new(0_u64);
    let retained = if world.mutation == SyncMutation::RingOffByOne {
        script.ring.next_power_of_two() + 1
    } else {
        script.ring.next_power_of_two()
    };
    let ring = Ring {
        world,
        sent: &sent,
        retained,
        values,
    };
    let ring = &ring;
    let mut jobs: Vec<Job<'_>> = (0..script.ring_receivers)
        .map(|index| Box::pin(ring.listen(receiver.resubscribe(), index, seed)) as Job<'_>)
        .collect();
    drop(receiver);
    let sent_ref = &sent;
    jobs.push(Box::pin(async move {
        let mut rng = Rng::derive(seed, "broadcast-sender");
        let extra = u64::from(world.mutation == SyncMutation::BroadcastExtra);
        for value in 1..=values + extra {
            world.pause(&mut rng).await;
            if sender.send(value).is_err() {
                world.fail(format!("broadcast: send of {value} refused"));
            }
            sent_ref.set(value);
        }
        drop(sender);
    }));
    join_all(jobs, world, "broadcast").await;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SyncStats {
    pub seeds: u64,
    pub events: u64,
}

impl SyncStats {
    pub const fn add(&mut self, other: &Self) {
        self.seeds += other.seeds;
        self.events += other.events;
    }
}

pub struct SyncOutcome {
    pub digest: u64,
    pub wrong: Option<String>,
    pub stats: SyncStats,
    pub shape: String,
}

fn simulate(seed: u64, mutation: SyncMutation) -> SyncOutcome {
    let script = Script::of(seed);
    let shape = script.describe();
    let clock = Clock::new();
    crate::host::enter(seed, &clock);
    let world = World {
        clock: Arc::clone(&clock),
        mutation,
        trace: RefCell::new(Vec::new()),
        wrong: RefCell::new(Vec::new()),
    };
    let tasks: Vec<Task<'_>> = vec![
        Box::pin(notify_scenario(&world, &script, seed)),
        Box::pin(semaphore_scenario(&world, &script, seed)),
        Box::pin(mpsc_scenario(&world, &script, seed)),
        Box::pin(watch_scenario(&world, &script, seed)),
        Box::pin(broadcast_scenario(&world, &script, seed)),
    ];
    let ended = Executor::new(Arc::clone(&clock), Rng::derive(seed, "scheduler")).run(tasks, None);
    if ended == Ended::Stuck {
        world.fail("stuck: every task waits and no timer is set".to_owned());
    }
    let trace = world.trace.into_inner();
    let wrong = world.wrong.into_inner();
    let mut digest = fnv(FNV_OFFSET, shape.as_bytes());
    for line in &trace {
        digest = fnv(digest, line.as_bytes());
    }
    digest = fnv(digest, &clock.now_ns().to_le_bytes());
    SyncOutcome {
        digest,
        wrong: wrong.first().cloned(),
        stats: SyncStats {
            seeds: 1,
            events: wide(trace.len()),
        },
        shape,
    }
}

pub fn run_seed(seed: u64, mutation: SyncMutation) -> SyncOutcome {
    std::panic::catch_unwind(|| simulate(seed, mutation)).unwrap_or_else(|panic| {
        let message = panic
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| panic.downcast_ref::<&str>().map(|text| (*text).to_owned()))
            .unwrap_or_default();
        SyncOutcome {
            digest: 0,
            wrong: Some(format!("panic: {message}")),
            stats: SyncStats::default(),
            shape: String::new(),
        }
    })
}

pub fn judge_seed(
    seed: u64,
    mutation: SyncMutation,
    twice: bool,
    verbose: bool,
    totals: &mut SyncStats,
) -> Option<String> {
    let first = run_seed(seed, mutation);
    totals.add(&first.stats);
    if verbose {
        println!("seed {seed} digest {:016x} {}", first.digest, first.shape);
    }
    if first.wrong.is_some() || !twice {
        return first
            .wrong
            .map(|message| format!("{} :: {message}", first.shape));
    }
    let second = run_seed(seed, mutation);
    (second.digest != first.digest || second.wrong.is_some()).then(|| {
        format!(
            "{} :: nondeterminism: trace digest {:016x} then {:016x}",
            first.shape, first.digest, second.digest
        )
    })
}

pub fn replay(seed: u64, mutation: SyncMutation) -> String {
    let mut command = format!("nanocode-sim --scenario sync --seed {seed}");
    if mutation != SyncMutation::None {
        command.push_str(" --mutation ");
        command.push_str(mutation.name());
    }
    command
}

pub fn report(
    seeds: &Range<u64>,
    passed: u64,
    failed: u64,
    twice: bool,
    mutation: SyncMutation,
    totals: &SyncStats,
) -> ExitCode {
    println!(
        "RESULT seeds={}..{} scenario=sync mutation={} passed={passed} failed={failed} twice={twice} stats={totals:?}",
        seeds.start,
        seeds.end,
        mutation.name()
    );
    match mutation {
        SyncMutation::None if failed == 0 => ExitCode::SUCCESS,
        SyncMutation::None => ExitCode::FAILURE,
        _ if failed > 0 => {
            println!("MUTATION {} caught by {failed} seeds", mutation.name());
            ExitCode::SUCCESS
        }
        _ => {
            println!("MUTATION {} NOT caught", mutation.name());
            ExitCode::FAILURE
        }
    }
}
