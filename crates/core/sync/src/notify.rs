use crate::{Atom, OrdMap};
use nanocode_sys::AtomicWaker;
use std::{
    fmt,
    future::Future,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicU8, Ordering},
    },
    task::{Context, Poll, Waker},
};

const WAITING: u8 = 0;
const NOTIFIED_FIRST: u8 = 1;
const NOTIFIED_LAST: u8 = 2;
const NOTIFIED_ALL: u8 = 3;
const CANCELLED: u8 = 4;

struct Cell {
    state: AtomicU8,
    waker: AtomicWaker,
}

#[derive(Clone, Default)]
struct State {
    calls: u64,
    permit: bool,
    next: u64,
    waiters: OrdMap<u64, Arc<Cell>>,
}

#[derive(Default)]
pub struct Notify(Atom<State>);

impl fmt::Debug for Notify {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("Notify").finish_non_exhaustive()
    }
}

#[derive(Clone, Copy)]
enum Strategy {
    First,
    Last,
}

impl Strategy {
    const fn mark(self) -> u8 {
        match self {
            Self::First => NOTIFIED_FIRST,
            Self::Last => NOTIFIED_LAST,
        }
    }
}

enum Picked {
    Stored,
    Waiter(u64, Arc<Cell>),
}

impl Notify {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn notified(&self) -> Notified<'_> {
        Notified {
            notify: self,
            calls: self.0.read(|state| state.calls),
            phase: Phase::Init,
        }
    }

    pub fn notify_one(&self) {
        self.notify_with(Strategy::First);
    }

    pub fn notify_last(&self) {
        self.notify_with(Strategy::Last);
    }

    fn notify_with(&self, strategy: Strategy) {
        loop {
            let picked = self.0.modify(|state| {
                let chosen = match strategy {
                    Strategy::First => state.waiters.get_min(),
                    Strategy::Last => state.waiters.get_max(),
                };
                match chosen {
                    Some((ticket, cell)) => Err(Picked::Waiter(*ticket, Arc::clone(cell))),
                    None if state.permit => Err(Picked::Stored),
                    None => {
                        state.permit = true;
                        Ok(Picked::Stored)
                    }
                }
            });
            let (ticket, cell) = match picked {
                Ok(Picked::Stored) | Err(Picked::Stored) => return,
                Ok(Picked::Waiter(..)) => unreachable!("a stored permit picks no waiter"),
                Err(Picked::Waiter(ticket, cell)) => (ticket, cell),
            };
            let claimed = cell
                .state
                .compare_exchange(
                    WAITING,
                    strategy.mark(),
                    Ordering::AcqRel,
                    Ordering::Acquire,
                )
                .is_ok();
            self.0.update(|state| state.waiters.remove(&ticket));
            if claimed {
                cell.waker.wake();
                return;
            }
        }
    }

    pub fn notify_waiters(&self) {
        let woken = self.0.update(|state| {
            state.calls = state.calls.wrapping_add(1);
            std::mem::take(&mut state.waiters)
        });
        for cell in woken.values() {
            if cell
                .state
                .compare_exchange(WAITING, NOTIFIED_ALL, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                cell.waker.wake();
            }
        }
    }
}

enum Phase {
    Init,
    Waiting(u64, Arc<Cell>),
    Done,
}

pub struct Notified<'a> {
    notify: &'a Notify,
    calls: u64,
    phase: Phase,
}

impl fmt::Debug for Notified<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("Notified").finish_non_exhaustive()
    }
}

enum Entered {
    Done,
    Waiting(u64),
}

impl Notified<'_> {
    pub fn enable(self: Pin<&mut Self>) -> bool {
        self.get_mut().poll_with(None).is_ready()
    }

    fn poll_with(&mut self, waker: Option<&Waker>) -> Poll<()> {
        match &self.phase {
            Phase::Done => Poll::Ready(()),
            Phase::Init => self.enter(waker),
            Phase::Waiting(_, cell) => {
                let cell = Arc::clone(cell);
                if let Some(waker) = waker {
                    cell.waker.register(waker);
                }
                match cell.state.load(Ordering::Acquire) {
                    NOTIFIED_FIRST | NOTIFIED_LAST | NOTIFIED_ALL => {
                        self.phase = Phase::Done;
                        Poll::Ready(())
                    }
                    _ => Poll::Pending,
                }
            }
        }
    }

    fn enter(&mut self, waker: Option<&Waker>) -> Poll<()> {
        let calls = self.calls;
        if self.notify.0.read(|state| state.calls != calls) {
            self.phase = Phase::Done;
            return Poll::Ready(());
        }
        let cell = Arc::new(Cell {
            state: AtomicU8::new(WAITING),
            waker: AtomicWaker::new(),
        });
        if let Some(waker) = waker {
            cell.waker.register(waker);
        }
        let entered = self.notify.0.modify(|state| {
            if state.calls != calls {
                return Err(Entered::Done);
            }
            if state.permit {
                state.permit = false;
                return Ok(Entered::Done);
            }
            let ticket = state.next;
            state.next += 1;
            state.waiters.insert(ticket, Arc::clone(&cell));
            Ok(Entered::Waiting(ticket))
        });
        match entered {
            Ok(Entered::Done) | Err(Entered::Done) => {
                self.phase = Phase::Done;
                Poll::Ready(())
            }
            Ok(Entered::Waiting(ticket)) | Err(Entered::Waiting(ticket)) => {
                self.phase = Phase::Waiting(ticket, cell);
                Poll::Pending
            }
        }
    }
}

impl Future for Notified<'_> {
    type Output = ();

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<()> {
        self.get_mut().poll_with(Some(context.waker()))
    }
}

impl Drop for Notified<'_> {
    fn drop(&mut self) {
        let Phase::Waiting(ticket, cell) = &self.phase else {
            return;
        };
        let ticket = *ticket;
        let previous = match cell.state.compare_exchange(
            WAITING,
            CANCELLED,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(previous) | Err(previous) => previous,
        };
        if previous == WAITING {
            self.notify.0.update(|state| state.waiters.remove(&ticket));
            return;
        }
        match previous {
            NOTIFIED_FIRST => self.notify.notify_with(Strategy::First),
            NOTIFIED_LAST => self.notify.notify_with(Strategy::Last),
            _ => {}
        }
    }
}
