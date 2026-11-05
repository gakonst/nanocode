use crate::Atom;
use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    thread,
    thread::ThreadId,
    time::{Duration, Instant},
};
use tokio::{
    sync::Notify,
    task::{Id, try_id},
    time::timeout_at,
};

#[derive(Clone, Copy, Debug)]
pub struct Reentrant;

impl std::fmt::Display for Reentrant {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("this task already holds a claim it waited to see released")
    }
}

impl std::error::Error for Reentrant {}

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

    pub async fn unclaimed(&self, wait: Duration) -> Result<Option<Duration>, Reentrant> {
        let owner = owner();
        let started = Instant::now();
        let mut deadline = started + wait;
        let mut seen = None;
        let mut waited = false;
        loop {
            let held = self.0.owners.update(|owners| {
                owners.get_mut(&owner).map(|held| {
                    let wake = Arc::clone(held.wake.get_or_insert_with(Arc::default));
                    (held.claim, wake)
                })
            });
            let Some((claim, wake)) = held else {
                return Ok(waited.then(|| started.elapsed()));
            };
            let woken = wake.notified();
            tokio::pin!(woken);
            woken.as_mut().enable();
            if self.held_here() != Some(claim) {
                continue;
            }
            if seen != Some(claim) {
                seen = Some(claim);
                deadline = Instant::now() + wait;
            } else if Instant::now() >= deadline {
                return Err(Reentrant);
            }
            waited = true;
            drop(timeout_at(deadline.into(), woken).await);
        }
    }

    #[must_use]
    pub fn claim(&self) -> Claim {
        let owner = owner();
        let claim = self.0.next.fetch_add(1, Ordering::Relaxed) + 1;
        let wake = self.0.owners.update(|owners| {
            let held = owners.entry(owner).or_insert(Held {
                claim,
                count: 0,
                wake: None,
            });
            held.claim = claim;
            held.count += 1;
            held.wake.clone()
        });
        if let Some(wake) = wake {
            wake.notify_waiters();
        }
        Claim(self.clone(), owner)
    }
}

pub struct Claim(Reentry, Owner);

impl Drop for Claim {
    fn drop(&mut self) {
        let Self(holder, owner) = self;
        let released = holder.0.owners.update(|owners| {
            let held = owners.get_mut(owner)?;
            held.count = held.count.saturating_sub(1);
            if held.count > 0 {
                return None;
            }
            owners.remove(owner).and_then(|held| held.wake)
        });
        if let Some(wake) = released {
            wake.notify_waiters();
        }
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
    wake: Option<Arc<Notify>>,
}

#[derive(Default)]
struct Claims {
    owners: Atom<HashMap<Owner, Held>>,
    next: AtomicU64,
}

fn owner() -> Owner {
    try_id().map_or_else(|| Owner::Thread(thread::current().id()), Owner::Task)
}
