use std::{
    collections::BTreeMap,
    future::Future,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    task::{Context, Poll, Wake, Waker},
    time::Duration,
};

use nanocode_sync::Slot;

use crate::rng::Rng;

type Timers = BTreeMap<(u64, u64), Waker>;

pub struct Clock {
    now_ns: AtomicU64,
    next_timer: AtomicU64,
    timers: Slot<Timers>,
}

impl Clock {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            now_ns: AtomicU64::new(0),
            next_timer: AtomicU64::new(0),
            timers: Slot::new(Some(Timers::new())),
        })
    }

    pub fn now_ns(&self) -> u64 {
        self.now_ns.load(Ordering::SeqCst)
    }

    pub fn sleep(self: &Arc<Self>, duration: Duration) -> Sleep {
        let delay_ns =
            u64::try_from(duration.as_nanos()).expect("a simulated delay fits in u64 nanoseconds");
        Sleep {
            clock: Arc::clone(self),
            deadline_ns: self.now_ns().saturating_add(delay_ns),
            timer: None,
        }
    }

    fn timers<R>(&self, change: impl FnOnce(&mut Timers) -> R) -> R {
        let mut timers = self
            .timers
            .take()
            .expect("the simulator polls one task at a time, so the timers are never taken twice");
        let result = change(&mut timers);
        assert!(
            self.timers.put(timers).is_none(),
            "the timers have one home"
        );
        result
    }

    fn next_deadline_ns(&self) -> Option<u64> {
        self.timers(|timers| timers.keys().next().map(|(deadline_ns, _)| *deadline_ns))
    }

    fn advance(&self, to_ns: u64) {
        assert!(to_ns >= self.now_ns(), "virtual time never runs backwards");
        self.now_ns.store(to_ns, Ordering::SeqCst);
        let due = self.timers(|timers| {
            let later = timers.split_off(&(to_ns + 1, 0));
            std::mem::replace(timers, later)
        });
        for waker in due.into_values() {
            waker.wake();
        }
    }
}

pub struct Sleep {
    clock: Arc<Clock>,
    deadline_ns: u64,
    timer: Option<u64>,
}

impl Sleep {
    fn cancel(&mut self) {
        if let Some(timer) = self.timer.take() {
            let key = (self.deadline_ns, timer);
            self.clock.timers(|timers| timers.remove(&key));
        }
    }
}

impl Future for Sleep {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<()> {
        let sleep = &mut *self;
        if sleep.clock.now_ns() >= sleep.deadline_ns {
            sleep.cancel();
            return Poll::Ready(());
        }
        let clock = &sleep.clock;
        let timer = *sleep
            .timer
            .get_or_insert_with(|| clock.next_timer.fetch_add(1, Ordering::SeqCst));
        let key = (sleep.deadline_ns, timer);
        let waker = context.waker().clone();
        clock.timers(|timers| timers.insert(key, waker));
        Poll::Pending
    }
}

impl Drop for Sleep {
    fn drop(&mut self) {
        self.cancel();
    }
}

pub type Task<'a> = Pin<Box<dyn Future<Output = ()> + 'a>>;

struct Woken(AtomicBool);

impl Wake for Woken {
    fn wake(self: Arc<Self>) {
        self.0.store(true, Ordering::SeqCst);
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.0.store(true, Ordering::SeqCst);
    }
}

struct Running<'a> {
    future: Task<'a>,
    woken: Arc<Woken>,
    waker: Waker,
}

impl<'a> Running<'a> {
    fn new(future: Task<'a>) -> Self {
        let woken = Arc::new(Woken(AtomicBool::new(true)));
        let waker = Waker::from(Arc::clone(&woken));
        Self {
            future,
            woken,
            waker,
        }
    }

    fn is_ready(&self) -> bool {
        self.woken.0.load(Ordering::SeqCst)
    }

    fn poll(&mut self) -> Poll<()> {
        self.woken.0.store(false, Ordering::SeqCst);
        let mut context = Context::from_waker(&self.waker);
        self.future.as_mut().poll(&mut context)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ended {
    Finished,
    Crashed,
    Stuck,
}

pub struct Executor {
    clock: Arc<Clock>,
    scheduler: Rng,
}

impl Executor {
    pub const fn new(clock: Arc<Clock>, scheduler: Rng) -> Self {
        Self { clock, scheduler }
    }

    pub fn run(&mut self, tasks: Vec<Task<'_>>, crash_at_ns: Option<u64>) -> Ended {
        let mut running: Vec<Running<'_>> = tasks.into_iter().map(Running::new).collect();
        let mut ready = Vec::with_capacity(running.len());
        while !running.is_empty() {
            ready.clear();
            ready.extend((0..running.len()).filter(|at| running[*at].is_ready()));
            if ready.is_empty() {
                if let Some(ended) = self.wait(crash_at_ns) {
                    return ended;
                }
                continue;
            }
            let at = ready[self.scheduler.pick(ready.len())];
            if running[at].poll().is_ready() {
                running.swap_remove(at);
            }
        }
        Ended::Finished
    }

    fn wait(&self, crash_at_ns: Option<u64>) -> Option<Ended> {
        let Some(deadline_ns) = self.clock.next_deadline_ns() else {
            return Some(Ended::Stuck);
        };
        match crash_at_ns {
            Some(crash_at_ns) if deadline_ns > crash_at_ns => {
                self.clock.advance(crash_at_ns.max(self.clock.now_ns()));
                Some(Ended::Crashed)
            }
            _ => {
                self.clock.advance(deadline_ns);
                None
            }
        }
    }
}
