use nanocode_sync::oneshot;
use nanocode_sync::{Atom, Mailbox};
use std::{
    collections::VecDeque,
    future::Future,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};

#[derive(Clone)]
pub(super) struct Capacity {
    state: Arc<Atom<CapacityState>>,
    waiting: Mailbox<Waiting>,
}

#[derive(Default)]
struct Waiting {
    resident: VecDeque<oneshot::Sender<()>>,
    spawns: VecDeque<oneshot::Sender<()>>,
}

#[derive(Clone, Copy)]
pub(super) enum Waiter {
    Resident,
    Spawn,
}

#[derive(Clone, Copy)]
struct CapacityState {
    active: usize,
    limit: usize,
}

pub(super) struct TurnCapacity {
    capacity: Capacity,
}

pub(super) struct Acquire {
    capacity: Capacity,
    granted: Option<oneshot::Receiver<()>>,
}

fn exhausted(limit: usize) -> std::io::Error {
    std::io::Error::other(format!(
        "sub-agent concurrency limit of {limit} has been reached; try delegation again later"
    ))
}

impl CapacityState {
    fn take(&mut self, count: usize) -> std::io::Result<()> {
        if self.limit != usize::MAX && count > self.limit.saturating_sub(self.active) {
            return Err(exhausted(self.limit));
        }
        self.active += count;
        Ok(())
    }
}

impl Capacity {
    pub(super) fn new(limit: usize) -> Self {
        Self {
            state: Arc::new(Atom::new(CapacityState { active: 0, limit })),
            waiting: Mailbox::new(Waiting::default()),
        }
    }

    fn turn(&self) -> TurnCapacity {
        TurnCapacity {
            capacity: self.clone(),
        }
    }

    pub(super) fn reserve(&self) -> std::io::Result<TurnCapacity> {
        self.state.modify(|state| state.take(1))?;
        Ok(self.turn())
    }

    pub(super) fn reserve_many(&self, count: usize) -> std::io::Result<Vec<TurnCapacity>> {
        self.state.modify(|state| state.take(count))?;
        Ok((0..count).map(|_| self.turn()).collect())
    }

    pub(super) fn overdraw(&self) -> TurnCapacity {
        self.state.update(|state| state.active += 1);
        self.turn()
    }

    pub(super) fn acquire(&self, waiter: Waiter) -> Acquire {
        let (grant, granted) = oneshot::channel();
        let state = Arc::clone(&self.state);
        self.waiting.send(move |queue| {
            if state.modify(|state| state.take(1)).is_err() {
                match waiter {
                    Waiter::Resident => queue.resident.push_back(grant),
                    Waiter::Spawn => queue.spawns.push_back(grant),
                }
            } else if grant.send(()).is_err() {
                hand_over(&state, queue);
            }
        });
        Acquire {
            capacity: self.clone(),
            granted: Some(granted),
        }
    }

    pub(super) fn set_limit(&self, limit: usize) {
        let state = Arc::clone(&self.state);
        self.waiting.send(move |queue| {
            state.update(|state| state.limit = limit);
            while !(queue.resident.is_empty() && queue.spawns.is_empty())
                && state.modify(|state| state.take(1)).is_ok()
            {
                hand_over(&state, queue);
            }
        });
    }

    fn release(&self) {
        let state = Arc::clone(&self.state);
        self.waiting.send(move |queue| hand_over(&state, queue));
    }
}

fn hand_over(state: &Atom<CapacityState>, queue: &mut Waiting) {
    let overdrawn = state.update(|state| {
        let overdrawn = state.active > state.limit;
        if overdrawn {
            state.active -= 1;
        }
        overdrawn
    });
    if overdrawn {
        return;
    }
    while let Some(waiter) = queue
        .resident
        .pop_front()
        .or_else(|| queue.spawns.pop_front())
    {
        if waiter.send(()).is_ok() {
            return;
        }
    }
    state.update(|state| state.active -= 1);
}

impl Future for Acquire {
    type Output = TurnCapacity;

    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<TurnCapacity> {
        let Some(granted) = self.granted.as_mut() else {
            return Poll::Ready(self.capacity.turn());
        };
        match Pin::new(granted).poll(context) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(_) => {
                self.granted = None;
                Poll::Ready(self.capacity.turn())
            }
        }
    }
}

impl Drop for TurnCapacity {
    fn drop(&mut self) {
        self.capacity.release();
    }
}

impl Drop for Acquire {
    fn drop(&mut self) {
        if let Some(mut granted) = self.granted.take() {
            granted.close();
            if granted.try_recv().is_ok() {
                self.capacity.release();
            }
        }
    }
}
