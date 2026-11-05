use crate::Observe;
use std::{
    future::Future,
    pin::{Pin, pin},
    sync::{
        Arc,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
    task::{Context, Poll, Wake, Waker},
    time::{Duration, Instant},
};

pub struct Watch {
    epoch: Instant,
    slots: Box<[Wait]>,
    next: AtomicUsize,
    observe: Arc<dyn Observe>,
}

#[derive(Default)]
struct Wait {
    pool: AtomicUsize,
    generation: AtomicU64,
    woken: AtomicU64,
    polled: AtomicU64,
}

impl Watch {
    #[must_use]
    pub fn new(slots: usize, observe: Arc<dyn Observe>) -> Self {
        Self {
            epoch: Instant::now(),
            slots: (0..slots).map(|_| Wait::default()).collect(),
            next: AtomicUsize::new(0),
            observe,
        }
    }

    fn now(&self) -> u64 {
        micros(self.epoch.elapsed()).max(1)
    }

    pub async fn watched<F: Future>(&'static self, pool: usize, work: F) -> F::Output {
        let pool = pool + 1;
        let start = self.next.fetch_add(1, Ordering::Relaxed);
        let count = self.slots.len();
        let slot = (0..count)
            .map(|n| &self.slots[(start + n) % count])
            .find(|slot| {
                slot.pool
                    .compare_exchange(0, pool, Ordering::AcqRel, Ordering::Relaxed)
                    .is_ok()
            });
        let Some(slot) = slot else {
            self.observe.wait_unwatched();
            return work.await;
        };
        let _freed = Freed(slot);
        Watched {
            work: pin!(work),
            watch: self,
            slot,
            generation: slot.generation.load(Ordering::Acquire),
            relay: None,
        }
        .await
    }

    #[must_use]
    pub fn stalled(&self, after: Duration, pools: usize) -> Vec<u32> {
        let now = self.now();
        let after = micros(after);
        let mut stalled = vec![0u32; pools];
        for slot in &self.slots {
            let pool = slot.pool.load(Ordering::Acquire);
            let woken = slot.woken.load(Ordering::Acquire);
            if pool == 0 || woken == 0 || woken <= slot.polled.load(Ordering::Acquire) {
                continue;
            }
            if now.saturating_sub(woken) >= after
                && let Some(count) = stalled.get_mut(pool - 1)
            {
                *count += 1;
            }
        }
        stalled
    }
}

fn micros(duration: Duration) -> u64 {
    u64::try_from(duration.as_micros()).unwrap_or(u64::MAX)
}

struct Relay {
    waker: Waker,
    watch: &'static Watch,
    slot: &'static Wait,
    generation: u64,
}

impl Wake for Relay {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        if self.slot.generation.load(Ordering::Acquire) == self.generation {
            self.slot.woken.store(self.watch.now(), Ordering::Release);
        }
        self.waker.wake_by_ref();
    }
}

struct Watched<'a, F> {
    work: Pin<&'a mut F>,
    watch: &'static Watch,
    slot: &'static Wait,
    generation: u64,
    relay: Option<(Waker, Waker)>,
}

impl<F: Future> Future for Watched<'_, F> {
    type Output = F::Output;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<F::Output> {
        self.slot.polled.store(self.watch.now(), Ordering::Release);
        let relay = match self.relay.take() {
            Some((waker, relay)) if waker.will_wake(cx.waker()) => (waker, relay),
            _ => {
                let waker = cx.waker().clone();
                let relay = Waker::from(Arc::new(Relay {
                    waker: waker.clone(),
                    watch: self.watch,
                    slot: self.slot,
                    generation: self.generation,
                }));
                (waker, relay)
            }
        };
        let polled = self.work.as_mut().poll(&mut Context::from_waker(&relay.1));
        self.relay = Some(relay);
        polled
    }
}

struct Freed(&'static Wait);

impl Drop for Freed {
    fn drop(&mut self) {
        self.0.generation.fetch_add(1, Ordering::AcqRel);
        self.0.woken.store(0, Ordering::Release);
        self.0.pool.store(0, Ordering::Release);
    }
}
