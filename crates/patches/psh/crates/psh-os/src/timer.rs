//! Timers for `Host::sleep`: one process-wide thread waits for the earliest deadline and
//! wakes the engine's waker, instead of a thread per sleep.

use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Condvar, LazyLock, Mutex};
use std::task::{Context, Poll, Waker};
use std::time::{Duration, Instant};

#[derive(Default)]
struct Slot {
    fired: bool,
    waker: Option<Waker>,
}

struct Due {
    at: Instant,
    seq: u64,
    slot: Arc<Mutex<Slot>>,
}

impl PartialEq for Due {
    fn eq(&self, o: &Self) -> bool {
        (self.at, self.seq) == (o.at, o.seq)
    }
}
impl Eq for Due {}
impl PartialOrd for Due {
    fn partial_cmp(&self, o: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for Due {
    fn cmp(&self, o: &Self) -> std::cmp::Ordering {
        (self.at, self.seq).cmp(&(o.at, o.seq))
    }
}

struct Timers {
    heap: Mutex<(BinaryHeap<Reverse<Due>>, u64, bool)>,
    cond: Condvar,
}

static TIMERS: LazyLock<Timers> = LazyLock::new(|| Timers {
    heap: Mutex::new((BinaryHeap::new(), 0, false)),
    cond: Condvar::new(),
});

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn run() {
    let t = &*TIMERS;
    let mut g = lock(&t.heap);
    loop {
        let now = Instant::now();
        while g.0.peek().is_some_and(|Reverse(d)| d.at <= now) {
            let Some(Reverse(d)) = g.0.pop() else { break };
            let waker = {
                let mut s = lock(&d.slot);
                s.fired = true;
                s.waker.take()
            };
            if let Some(w) = waker {
                w.wake();
            }
        }
        g = match g.0.peek() {
            Some(Reverse(d)) => {
                let wait = d.at.saturating_duration_since(now);
                t.cond
                    .wait_timeout(g, wait)
                    .unwrap_or_else(|e| e.into_inner())
                    .0
            }
            None => t.cond.wait(g).unwrap_or_else(|e| e.into_inner()),
        };
    }
}

/// A future ready `duration` from now.
pub fn sleep(duration: Duration) -> Pin<Box<dyn Future<Output = ()>>> {
    let slot = Arc::new(Mutex::new(Slot::default()));
    let at = Instant::now() + duration;
    {
        let t = &*TIMERS;
        let mut g = lock(&t.heap);
        if !g.2 {
            let started = crate::spawn_helper("psh-timer", 64 << 10, run);
            if started.is_err() {
                // No thread: the timer is due at once rather than never.
                lock(&slot).fired = true;
            }
            g.2 = started.is_ok();
        }
        g.1 += 1;
        let seq = g.1;
        g.0.push(Reverse(Due {
            at,
            seq,
            slot: slot.clone(),
        }));
        t.cond.notify_one();
    }
    Box::pin(std::future::poll_fn(move |cx: &mut Context<'_>| {
        let mut s = lock(&slot);
        if s.fired {
            Poll::Ready(())
        } else {
            s.waker = Some(cx.waker().clone());
            Poll::Pending
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block_on(mut f: Pin<Box<dyn Future<Output = ()>>>) {
        struct Unpark(std::thread::Thread);
        impl std::task::Wake for Unpark {
            fn wake(self: Arc<Self>) {
                self.0.unpark();
            }
        }
        let waker = Waker::from(Arc::new(Unpark(std::thread::current())));
        let mut cx = Context::from_waker(&waker);
        while f.as_mut().poll(&mut cx).is_pending() {
            std::thread::park();
        }
    }

    #[test]
    fn sleeps_in_order() {
        let start = Instant::now();
        let long = sleep(Duration::from_millis(60));
        let short = sleep(Duration::from_millis(20));
        block_on(short);
        let t1 = start.elapsed();
        block_on(long);
        let t2 = start.elapsed();
        assert!(t1 >= Duration::from_millis(20));
        assert!(t2 >= Duration::from_millis(60));
    }
}
