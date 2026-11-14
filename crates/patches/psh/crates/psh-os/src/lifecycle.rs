//! The lifecycle of one external process, shared by its `ExtProc` (owned by the engine)
//! and the threads that wait for it: exit observation, who reaps, and abandonment.
//!
//! Platform-neutral state; the platform spawners do the side effects (signals, reaping)
//! a transition asks for, while holding the lock where a signal must not race the reap.
//! The rules (research/execution-core.md §5, pitfalls 6 and 7):
//!
//! - The process is never reaped while its `ExtProc` exists, so a signal to its group
//!   cannot reach a recycled pid.
//! - Dropping the `ExtProc` after the engine collected the exit reaps it at once.
//! - Dropping it while the process runs (or after an exit nobody collected: a cancel
//!   racing the exit) *abandons* it: TERM to the group now, KILL to the group after the
//!   grace period, then a reap. The escalation owns the reap.
//! - Releasing it (`release`) signals nothing: the waiter reaps when the process exits.

use std::sync::{Condvar, Mutex, MutexGuard};
use std::task::{Context, Poll, Waker};
use std::time::Instant;

use psh_engine::host::{ExitStatus, Signal};

/// Who holds the process now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Owner {
    /// The engine, through the `ExtProc`.
    Engine,
    /// Released: the waiter reaps at the exit.
    Released,
    /// Abandoned: the escalation sends KILL after the grace period and reaps.
    Abandoned,
}

/// The state behind a [`Tracker`]'s lock.
#[derive(Debug)]
pub struct Life {
    status: Option<ExitStatus>,
    collected: bool,
    owner: Owner,
    term_sent: bool,
    reaped: bool,
    stopped: Option<Signal>,
    waker: Option<Waker>,
}

/// What dropping the `ExtProc` has to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dropped {
    /// The exit was collected: reap now.
    Reap,
    /// Abandoned (running, or exited uncollected): send TERM to the group when `term`,
    /// then escalate (KILL after the grace period, then reap).
    Escalate { term: bool },
}

impl Life {
    /// The exit, once a waiter observed it (the process may be unreaped still).
    pub fn status(&self) -> Option<ExitStatus> {
        self.status
    }

    pub fn owner(&self) -> Owner {
        self.owner
    }

    /// Whether the process was reaped: its id may be recycled, signal nothing.
    pub fn reaped(&self) -> bool {
        self.reaped
    }

    /// Records that the process was reaped (by whoever owns the reap).
    pub fn set_reaped(&mut self) {
        self.reaped = true;
    }

    /// A TERM went to the process or its group (an abandon then sends none).
    pub fn term_sent(&mut self) {
        self.term_sent = true;
    }
}

/// Shared by an `ExtProc` and its waiter and escalation threads.
#[derive(Debug)]
pub struct Tracker {
    life: Mutex<Life>,
    stop: std::sync::atomic::AtomicBool,
    /// Signalled at the exit, for the escalation thread.
    exited: Condvar,
}

impl Default for Tracker {
    fn default() -> Self {
        Tracker::new()
    }
}

impl Tracker {
    pub fn new() -> Tracker {
        Tracker {
            life: Mutex::new(Life {
                status: None,
                collected: false,
                owner: Owner::Engine,
                term_sent: false,
                reaped: false,
                stopped: None,
                waker: None,
            }),
            exited: Condvar::new(),
            stop: std::sync::atomic::AtomicBool::new(false),
        }
    }

    pub fn lock(&self) -> MutexGuard<'_, Life> {
        self.life.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Ready with the exit status once a waiter observed it.
    pub fn poll(&self, cx: &mut Context<'_>) -> Poll<ExitStatus> {
        let mut life = self.lock();
        match life.status {
            Some(s) => {
                life.collected = true;
                Poll::Ready(s)
            }
            None => {
                if !life.waker.as_ref().is_some_and(|w| w.will_wake(cx.waker())) {
                    life.waker = Some(cx.waker().clone());
                }
                Poll::Pending
            }
        }
    }

    pub fn poll_stop(&self, cx: &mut Context<'_>) -> Poll<Signal> {
        if !self.stop.swap(false, std::sync::atomic::Ordering::AcqRel) {
            return Poll::Pending;
        }
        let mut life = self.lock();
        match life.stopped.take() {
            Some(sig) => Poll::Ready(sig),
            None => {
                if !life.waker.as_ref().is_some_and(|w| w.will_wake(cx.waker())) {
                    life.waker = Some(cx.waker().clone());
                }
                Poll::Pending
            }
        }
    }

    pub fn stopped(&self, sig: Signal) -> Option<Waker> {
        let mut life = self.lock();
        life.stopped = Some(sig);
        self.stop.store(true, std::sync::atomic::Ordering::Release);
        life.waker.take()
    }

    /// A waiter observed the exit (the process is still unreaped). A released process
    /// is reaped now, by `reap`, under the lock. Returns the waker to wake (outside the
    /// lock).
    pub fn exited(&self, status: ExitStatus, reap: impl FnOnce()) -> Option<Waker> {
        let mut life = self.lock();
        life.status = Some(status);
        self.exited.notify_all();
        if life.owner == Owner::Released && !life.reaped {
            reap();
            life.reaped = true;
        }
        life.waker.take()
    }

    /// The `ExtProc` is dropped without `release`.
    pub fn abandon(life: &mut Life) -> Dropped {
        if life.collected {
            life.owner = Owner::Abandoned;
            return Dropped::Reap;
        }
        life.owner = Owner::Abandoned;
        let term = !life.term_sent;
        life.term_sent = true;
        Dropped::Escalate { term }
    }

    /// `release`: whether the process has exited already (reap it now); otherwise the
    /// waiter reaps it.
    pub fn release(life: &mut Life) -> bool {
        life.owner = Owner::Released;
        life.status.is_some()
    }

    /// Waits until the exit was observed or `deadline` passed; whether it exited.
    pub fn wait_exit_until(&self, deadline: Option<Instant>) -> bool {
        let mut life = self.lock();
        loop {
            if life.status.is_some() {
                return true;
            }
            match deadline {
                None => {
                    life = self.exited.wait(life).unwrap_or_else(|e| e.into_inner());
                }
                Some(d) => {
                    let now = Instant::now();
                    if now >= d {
                        return false;
                    }
                    life = self
                        .exited
                        .wait_timeout(life, d - now)
                        .unwrap_or_else(|e| e.into_inner())
                        .0;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn collected_exit_reaps_on_drop() {
        let t = Tracker::new();
        let mut reaped = false;
        t.exited(ExitStatus::Exited(3), || reaped = true);
        assert!(!reaped);
        let mut cx = Context::from_waker(Waker::noop());
        assert_eq!(t.poll(&mut cx), Poll::Ready(ExitStatus::Exited(3)));
        assert_eq!(Tracker::abandon(&mut t.lock()), Dropped::Reap);
    }

    #[test]
    fn running_abandon_terms_once() {
        let t = Tracker::new();
        t.lock().term_sent();
        assert_eq!(
            Tracker::abandon(&mut t.lock()),
            Dropped::Escalate { term: false }
        );
        let t = Tracker::new();
        assert_eq!(
            Tracker::abandon(&mut t.lock()),
            Dropped::Escalate { term: true }
        );
    }

    #[test]
    fn uncollected_exit_is_abandoned() {
        let t = Tracker::new();
        t.exited(ExitStatus::Signaled(15), || {});
        assert_eq!(
            Tracker::abandon(&mut t.lock()),
            Dropped::Escalate { term: true }
        );
    }

    #[test]
    fn released_process_is_reaped_by_the_waiter() {
        let t = Tracker::new();
        assert!(!Tracker::release(&mut t.lock()));
        let mut reaped = false;
        t.exited(ExitStatus::Exited(0), || reaped = true);
        assert!(reaped && t.lock().reaped());
        let t = Tracker::new();
        t.exited(ExitStatus::Exited(0), || {});
        assert!(Tracker::release(&mut t.lock()));
    }

    #[test]
    fn wait_exit_until_times_out_or_sees_the_exit() {
        let t = std::sync::Arc::new(Tracker::new());
        assert!(!t.wait_exit_until(Some(Instant::now() + Duration::from_millis(10))));
        let t2 = t.clone();
        let h = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            t2.exited(ExitStatus::Exited(0), || {});
        });
        assert!(t.wait_exit_until(Some(Instant::now() + Duration::from_secs(10))));
        h.join().unwrap();
    }

    #[test]
    fn poll_wakes_at_the_exit() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};
        struct Flag(AtomicBool);
        impl std::task::Wake for Flag {
            fn wake(self: Arc<Self>) {
                self.0.store(true, Ordering::SeqCst);
            }
        }
        let flag = Arc::new(Flag(AtomicBool::new(false)));
        let waker = Waker::from(flag.clone());
        let t = Tracker::new();
        assert!(t.poll(&mut Context::from_waker(&waker)).is_pending());
        t.exited(ExitStatus::Exited(0), || {}).unwrap().wake();
        assert!(flag.0.load(Ordering::SeqCst));
    }
}
