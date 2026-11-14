use crate::{Atom, OrdMap};
use std::task::Waker;

#[derive(Clone, Default)]
struct List {
    next: u64,
    wakers: OrdMap<u64, Waker>,
}

#[derive(Default)]
pub(crate) struct Waiters(Atom<List>);

impl Waiters {
    pub(crate) fn register(&self, ticket: Option<u64>, waker: &Waker) -> u64 {
        if let Some(ticket) = ticket
            && self.0.read(|list| {
                list.wakers
                    .get(&ticket)
                    .is_some_and(|held| held.will_wake(waker))
            })
        {
            return ticket;
        }
        self.0.update(|list| {
            let ticket = ticket.unwrap_or_else(|| {
                let fresh = list.next;
                list.next += 1;
                fresh
            });
            list.wakers.insert(ticket, waker.clone());
            ticket
        })
    }

    pub(crate) fn remove(&self, ticket: u64) {
        if self.0.read(|list| list.wakers.contains_key(&ticket)) {
            self.0.update(|list| list.wakers.remove(&ticket));
        }
    }

    pub(crate) fn wake_all(&self) {
        if self.0.read(|list| list.wakers.get_min().is_none()) {
            return;
        }
        let woken = self.0.update(|list| std::mem::take(&mut list.wakers));
        for waker in woken.values() {
            waker.wake_by_ref();
        }
    }
}
