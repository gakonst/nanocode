use crate::Atom;
use nanocode_executor::{Id, try_id};
use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    thread,
    thread::ThreadId,
};

#[derive(Clone, Default)]
pub struct Reentry(Arc<Claims>);

impl Reentry {
    #[must_use]
    pub fn held_here(&self) -> Option<u64> {
        let owner = owner();
        self.0
            .owners
            .read(|owners| owners.get(&owner).map(|held| held.claim))
    }

    #[must_use]
    pub fn claim(&self) -> Claim {
        let owner = owner();
        let claim = self.0.next.fetch_add(1, Ordering::Relaxed) + 1;
        self.0.owners.update(|owners| {
            let held = owners.entry(owner).or_insert(Held { claim, count: 0 });
            held.claim = claim;
            held.count += 1;
        });
        Claim(self.clone(), owner)
    }
}

pub struct Claim(Reentry, Owner);

impl Drop for Claim {
    fn drop(&mut self) {
        let Self(holder, owner) = self;
        holder.0.owners.update(|owners| {
            if let Some(held) = owners.get_mut(owner) {
                held.count = held.count.saturating_sub(1);
                if held.count == 0 {
                    owners.remove(owner);
                }
            }
        });
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Owner {
    Task(Id),
    Thread(ThreadId),
}

#[derive(Clone)]
struct Held {
    claim: u64,
    count: usize,
}

#[derive(Default)]
struct Claims {
    owners: Atom<HashMap<Owner, Held>>,
    next: AtomicU64,
}

fn owner() -> Owner {
    try_id().map_or_else(|| Owner::Thread(thread::current().id()), Owner::Task)
}
