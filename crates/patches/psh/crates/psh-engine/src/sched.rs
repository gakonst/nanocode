//! The engine's scheduler: virtual processes and helper tasks are futures in a slab,
//! polled by [`crate::Shell::step`] on the embedder's thread (D1050).
//!
//! Every task has its own `Waker`. Waking it (from the engine thread, or from a host
//! thread that finished an operation) puts the task id on a shared queue and, unless a
//! step is running and will see the queue anyway, wakes the embedder's waker. So only
//! tasks that were woken are polled, and the embedder parks while nothing can run.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Wake, Waker};

pub(crate) type LocalBoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + 'a>>;

/// Control bits (ARCH.md "Control").
pub(crate) const CANCEL: u32 = 1;
pub(crate) const PAUSE: u32 = 2;

/// What crosses threads: the wake queue, the embedder's waker and the control word.
pub(crate) struct Shared {
    woken: Mutex<Vec<usize>>,
    outer: Mutex<Option<Waker>>,
    /// A step is running: it drains the queue before it returns, so a wake need not
    /// wake the embedder.
    in_step: AtomicBool,
    pub(crate) control: AtomicU32,
    /// Signals sent to the shell and not delivered yet (bit n = signal n, D1284).
    pub(crate) signals: std::sync::atomic::AtomicU64,
}

impl Shared {
    pub(crate) fn new() -> Shared {
        Shared {
            woken: Mutex::new(Vec::new()),
            outer: Mutex::new(None),
            in_step: AtomicBool::new(false),
            control: AtomicU32::new(0),
            signals: std::sync::atomic::AtomicU64::new(0),
        }
    }

    pub(crate) fn set_waker(&self, waker: Waker) {
        *self.outer.lock().unwrap_or_else(|e| e.into_inner()) = Some(waker);
    }

    pub(crate) fn wake_outer(&self) {
        let w = self.outer.lock().unwrap_or_else(|e| e.into_inner()).clone();
        if let Some(w) = w {
            w.wake();
        }
    }

    fn push(&self, id: usize) {
        self.woken
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(id);
        if !self.in_step.load(Ordering::SeqCst) {
            self.wake_outer();
        }
    }

    fn take(&self, into: &mut Vec<usize>) {
        into.append(&mut self.woken.lock().unwrap_or_else(|e| e.into_inner()));
    }
}

struct TaskWaker {
    id: usize,
    shared: Arc<Shared>,
}

impl Wake for TaskWaker {
    fn wake(self: Arc<Self>) {
        self.shared.push(self.id);
    }
    fn wake_by_ref(self: &Arc<Self>) {
        self.shared.push(self.id);
    }
}

struct Slot {
    fut: Option<LocalBoxFuture<'static, ()>>,
    waker: Waker,
    queued: bool,
    /// Taken out for polling (the slot is not free).
    busy: bool,
}

pub(crate) struct Sched {
    slots: RefCell<Vec<Option<Slot>>>,
    free: RefCell<Vec<usize>>,
    ready: RefCell<VecDeque<usize>>,
    pub(crate) shared: Arc<Shared>,
    budget: Cell<u32>,
    out_of_budget: Cell<bool>,
    /// Waits of the shell's own vproc that a signal ends (D1903).
    signal_waiters: RefCell<Vec<Waker>>,
}

/// What [`Sched::run`] stopped on.
pub(crate) enum RunEnd {
    /// No task is ready.
    Blocked,
    /// The budget is spent.
    Yield,
    /// The control word asks for attention.
    Control,
    /// `done()` returned true.
    Done,
}

impl Sched {
    pub(crate) fn new(shared: Arc<Shared>) -> Sched {
        Sched {
            slots: RefCell::new(Vec::new()),
            free: RefCell::new(Vec::new()),
            ready: RefCell::new(VecDeque::new()),
            shared,
            budget: Cell::new(0),
            out_of_budget: Cell::new(false),
            signal_waiters: RefCell::new(Vec::new()),
        }
    }

    /// Starts a task; it is polled on the next round. Returns its id.
    pub(crate) fn spawn_task(&self, fut: LocalBoxFuture<'static, ()>) -> usize {
        let id = match self.free.borrow_mut().pop() {
            Some(id) => id,
            None => {
                let mut slots = self.slots.borrow_mut();
                slots.push(None);
                slots.len() - 1
            }
        };
        let waker = Waker::from(Arc::new(TaskWaker {
            id,
            shared: self.shared.clone(),
        }));
        self.slots.borrow_mut()[id] = Some(Slot {
            fut: Some(fut),
            waker,
            queued: true,
            busy: false,
        });
        self.ready.borrow_mut().push_back(id);
        id
    }

    /// Drops task `id` now, as a cancel would (its host resources are released by
    /// `Drop`); false if it is the task being polled or gone. The id must still be the
    /// task the caller means (ids are reused once a task ends).
    pub(crate) fn drop_task(&self, id: usize) -> bool {
        let fut = {
            let mut slots = self.slots.borrow_mut();
            match slots.get_mut(id) {
                Some(Some(slot)) if !slot.busy => {
                    let fut = slot.fut.take();
                    slots[id] = None;
                    self.free.borrow_mut().push(id);
                    fut
                }
                _ => return false,
            }
        };
        drop(fut);
        true
    }

    /// Whether task `id` exists (not yet ended or dropped).
    pub(crate) fn has_task(&self, id: usize) -> bool {
        matches!(self.slots.borrow().get(id), Some(Some(_)))
    }

    /// Starts a task and returns a handle to await its result. Dropping the handle
    /// detaches the task.
    pub(crate) fn spawn<T: 'static>(&self, fut: impl Future<Output = T> + 'static) -> Join<T> {
        self.spawn_with_id(fut).1
    }

    /// [`Self::spawn`], also returning the task's id for [`Self::drop_task`]: the id is
    /// the task's while its handle is not done.
    pub(crate) fn spawn_with_id<T: 'static>(
        &self,
        fut: impl Future<Output = T> + 'static,
    ) -> (usize, Join<T>) {
        let state = Rc::new(RefCell::new(JoinState {
            result: None,
            waker: None,
        }));
        let st = state.clone();
        let id = self.spawn_task(Box::pin(async move {
            let r = fut.await;
            let waker = {
                let mut s = st.borrow_mut();
                s.result = Some(r);
                s.waker.take()
            };
            if let Some(w) = waker {
                w.wake();
            }
        }));
        (id, Join { state })
    }

    /// Whether a task other than the one being polled is ready to run, or was woken.
    pub(crate) fn has_ready(&self) -> bool {
        !self.ready.borrow().is_empty()
            || !self
                .shared
                .woken
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .is_empty()
    }

    /// The number of live tasks.
    /// Whether signals are pending (D1284).
    pub(crate) fn has_signals(&self) -> bool {
        self.shared.signals.load(Ordering::Relaxed) != 0
    }

    /// Sends signal `sig` to the shell from inside (`kill $$`, D1284).
    pub(crate) fn raise(&self, sig: i32) {
        if (1..64).contains(&sig) {
            self.shared.signals.fetch_or(1u64 << sig, Ordering::SeqCst);
        }
    }

    /// The pending signals, left pending (D1903).
    pub(crate) fn peek_signals(&self) -> u64 {
        self.shared.signals.load(Ordering::SeqCst)
    }

    /// Wakes `w` when a signal is sent to the shell (D1903).
    pub(crate) fn on_signal(&self, w: &Waker) {
        let mut ws = self.signal_waiters.borrow_mut();
        if !ws.iter().any(|x| x.will_wake(w)) {
            ws.push(w.clone());
        }
    }

    pub(crate) fn wake_signal_waiters(&self) {
        let ws: Vec<Waker> = self.signal_waiters.borrow_mut().drain(..).collect();
        for w in ws {
            w.wake();
        }
    }

    /// Takes the pending signals (D1284).
    pub(crate) fn take_signals(&self) -> u64 {
        if self.shared.signals.load(Ordering::SeqCst) == 0 {
            return 0;
        }
        self.shared.signals.swap(0, Ordering::SeqCst)
    }

    pub(crate) fn task_count(&self) -> usize {
        self.slots.borrow().iter().filter(|s| s.is_some()).count()
    }

    /// Drops every task: the cancel path. Dropping a task drops the host resources it
    /// holds (processes are abandoned, files closed).
    pub(crate) fn drop_all(&self) {
        // Futures may wake or touch the scheduler while they are dropped: take them out
        // first, then drop them outside the borrow. Dropping can't spawn (no task runs).
        let futs: Vec<_> = {
            let mut slots = self.slots.borrow_mut();
            let futs = slots
                .iter_mut()
                .filter_map(|s| s.take().and_then(|s| s.fut))
                .collect();
            slots.clear();
            futs
        };
        self.free.borrow_mut().clear();
        self.ready.borrow_mut().clear();
        drop(futs);
        let mut ignored = Vec::new();
        self.shared.take(&mut ignored);
    }

    fn collect_woken(&self) {
        let mut woken = Vec::new();
        self.shared.take(&mut woken);
        let mut slots = self.slots.borrow_mut();
        let mut ready = self.ready.borrow_mut();
        for id in woken {
            if let Some(Some(slot)) = slots.get_mut(id)
                && !slot.queued
            {
                slot.queued = true;
                ready.push_back(id);
            }
        }
    }

    /// Polls ready tasks until none is ready, the budget is spent, the control word is
    /// set or `done()` holds.
    pub(crate) fn run(&self, budget: u32, done: &dyn Fn() -> bool) -> RunEnd {
        self.budget.set(budget);
        self.out_of_budget.set(false);
        self.shared.in_step.store(true, Ordering::SeqCst);
        let end = loop {
            if self.shared.control.load(Ordering::SeqCst) != 0 {
                break RunEnd::Control;
            }
            self.collect_woken();
            let next = self.ready.borrow_mut().pop_front();
            let Some(id) = next else {
                // Nothing ready. A host thread may have woken a task after the last
                // look; once in_step is clear, later wakes go to the embedder.
                self.shared.in_step.store(false, Ordering::SeqCst);
                self.collect_woken();
                if self.ready.borrow().is_empty() {
                    break RunEnd::Blocked;
                }
                self.shared.in_step.store(true, Ordering::SeqCst);
                continue;
            };
            let taken = {
                let mut slots = self.slots.borrow_mut();
                match slots.get_mut(id) {
                    Some(Some(slot)) => {
                        slot.queued = false;
                        slot.busy = true;
                        slot.fut.take().map(|f| (f, slot.waker.clone()))
                    }
                    _ => None,
                }
            };
            let Some((mut fut, waker)) = taken else {
                continue;
            };
            let mut cx = Context::from_waker(&waker);
            let poll = fut.as_mut().poll(&mut cx);
            {
                let mut slots = self.slots.borrow_mut();
                match poll {
                    Poll::Ready(()) => {
                        slots[id] = None;
                        self.free.borrow_mut().push(id);
                    }
                    Poll::Pending => {
                        if let Some(Some(slot)) = slots.get_mut(id) {
                            slot.busy = false;
                            slot.fut = Some(fut);
                        }
                    }
                }
            }
            if done() {
                break RunEnd::Done;
            }
            if self.out_of_budget.get() {
                break RunEnd::Yield;
            }
        };
        self.shared.in_step.store(false, Ordering::SeqCst);
        end
    }

    /// Takes one unit of budget; false when it is spent (the caller yields).
    fn take_budget(&self) -> bool {
        let b = self.budget.get();
        if b == 0 {
            self.out_of_budget.set(true);
            false
        } else {
            self.budget.set(b - 1);
            true
        }
    }
}

struct JoinState<T> {
    result: Option<T>,
    waker: Option<Waker>,
}

/// The result of a spawned task.
pub(crate) struct Join<T> {
    state: Rc<RefCell<JoinState<T>>>,
}

impl<T> Join<T> {
    pub(crate) fn is_done(&self) -> bool {
        self.state.borrow().result.is_some()
    }

    pub(crate) fn try_take(&self) -> Option<T> {
        self.state.borrow_mut().result.take()
    }
}

impl<T> Future for Join<T> {
    type Output = T;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<T> {
        let mut s = self.state.borrow_mut();
        match s.result.take() {
            Some(r) => Poll::Ready(r),
            None => {
                s.waker = Some(cx.waker().clone());
                Poll::Pending
            }
        }
    }
}

/// A safepoint (before every command and loop iteration): yields when the step's
/// budget is spent or the control word is set, so that `step` can return or act.
pub(crate) fn safepoint(sched: &Sched) -> impl Future<Output = ()> + '_ {
    let mut yielded = false;
    std::future::poll_fn(move |cx| {
        if yielded {
            return Poll::Ready(());
        }
        if sched.shared.control.load(Ordering::SeqCst) != 0 || !sched.take_budget() {
            yielded = true;
            cx.waker().wake_by_ref();
            return Poll::Pending;
        }
        Poll::Ready(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tasks_run_and_join() {
        let sched = Rc::new(Sched::new(Arc::new(Shared::new())));
        let s2 = sched.clone();
        let out = Rc::new(Cell::new(0));
        let o2 = out.clone();
        sched.spawn_task(Box::pin(async move {
            let j = s2.spawn(async { 20 });
            o2.set(j.await + 1);
        }));
        assert!(matches!(sched.run(100, &|| false), RunEnd::Blocked));
        assert_eq!(out.get(), 21);
        assert_eq!(sched.task_count(), 0);
    }

    #[test]
    fn budget_yields() {
        let sched = Rc::new(Sched::new(Arc::new(Shared::new())));
        let s2 = sched.clone();
        let n = Rc::new(Cell::new(0));
        let n2 = n.clone();
        sched.spawn_task(Box::pin(async move {
            for _ in 0..10 {
                safepoint(&s2).await;
                n2.set(n2.get() + 1);
            }
        }));
        assert!(matches!(sched.run(3, &|| false), RunEnd::Yield));
        assert_eq!(n.get(), 3);
        assert!(matches!(sched.run(100, &|| false), RunEnd::Blocked));
        assert_eq!(n.get(), 10);
    }
}
