use std::{
    collections::{HashMap, VecDeque},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use nanocode_sync::{Atom, Signal};

type OriginKey = Option<Arc<str>>;

#[derive(Clone, Copy, Default)]
struct Origin {
    count: usize,
    epoch: u64,
    yielded: bool,
}

#[derive(Clone, Default)]
struct GateState {
    exclusive: Option<u64>,
    shared: usize,
    origins: HashMap<OriginKey, Origin>,
    granted: HashMap<u64, u64>,
    waiting: VecDeque<Waiter>,
    epochs: u64,
}

#[derive(Clone)]
struct Waiter {
    token: u64,
    exclusive: bool,
    origin: OriginKey,
    admitted: Arc<Signal>,
}

#[derive(Default)]
pub(super) struct AdmissionGate {
    state: Atom<GateState>,
    tokens: AtomicU64,
}

pub(super) struct Admission<'gate> {
    gate: &'gate AdmissionGate,
    token: u64,
    origin: OriginKey,
    epoch: Option<u64>,
}

impl Drop for Admission<'_> {
    fn drop(&mut self) {
        self.gate.release(self.token, &self.origin, self.epoch);
    }
}

enum Entry {
    Admitted(Option<u64>),
    Wait,
}

impl GateState {
    const fn admits(&self, exclusive: bool) -> bool {
        self.exclusive.is_none() && (!exclusive || self.shared == 0)
    }

    const fn next_epoch(&mut self) -> u64 {
        self.epochs += 1;
        self.epochs
    }

    fn admit(&mut self, token: u64, exclusive: bool, origin: &OriginKey) -> Option<u64> {
        if exclusive {
            self.exclusive = Some(token);
            return None;
        }
        if !self.origins.contains_key(origin) {
            let epoch = self.next_epoch();
            self.origins.insert(
                origin.clone(),
                Origin {
                    epoch,
                    ..Origin::default()
                },
            );
        }
        let entry = self.origins.get_mut(origin)?;
        if entry.yielded {
            return None;
        }
        entry.count += 1;
        self.shared += 1;
        Some(entry.epoch)
    }

    fn release_shared(&mut self, origin: &OriginKey, epoch: u64) {
        let Some(entry) = self.origins.get_mut(origin) else {
            return;
        };
        if entry.epoch != epoch || entry.count == 0 {
            return;
        }
        entry.count -= 1;
        self.shared -= 1;
        if entry.count == 0 && !entry.yielded {
            self.origins.remove(origin);
        }
    }

    fn advance(&mut self) -> Vec<Arc<Signal>> {
        let mut admitted = Vec::new();
        while self
            .waiting
            .front()
            .is_some_and(|next| self.admits(next.exclusive))
        {
            let Some(next) = self.waiting.pop_front() else {
                break;
            };
            if let Some(epoch) = self.admit(next.token, next.exclusive, &next.origin) {
                self.granted.insert(next.token, epoch);
            }
            admitted.push(next.admitted);
        }
        admitted
    }
}

impl AdmissionGate {
    pub(super) async fn admit(&self, exclusive: bool, origin: Option<&str>) -> Admission<'_> {
        let token = self.tokens.fetch_add(1, Ordering::Relaxed);
        let origin: OriginKey = origin.map(Arc::from);
        let immediate = self.state.update(|state| {
            if state.waiting.is_empty() && state.admits(exclusive) {
                Entry::Admitted(state.admit(token, exclusive, &origin))
            } else {
                Entry::Wait
            }
        });
        if let Entry::Admitted(epoch) = immediate {
            return Admission {
                gate: self,
                token,
                origin,
                epoch,
            };
        }
        let admitted = Arc::new(Signal::new());
        let mut admission = Admission {
            gate: self,
            token,
            origin: origin.clone(),
            epoch: None,
        };
        let entry = self.state.update(|state| {
            if state.waiting.is_empty() && state.admits(exclusive) {
                Entry::Admitted(state.admit(token, exclusive, &origin))
            } else {
                state.waiting.push_back(Waiter {
                    token,
                    exclusive,
                    origin: origin.clone(),
                    admitted: Arc::clone(&admitted),
                });
                Entry::Wait
            }
        });
        match entry {
            Entry::Admitted(epoch) => {
                admission.epoch = epoch;
                admission
            }
            Entry::Wait => {
                admitted.wait().await;
                admission
            }
        }
    }

    pub(super) fn observed(&self, origin: &str, observed: bool) {
        let key: OriginKey = Some(Arc::from(origin));
        let admitted = self.state.update(|state| {
            if observed {
                if let Some(entry) = state.origins.get_mut(&key) {
                    entry.yielded = false;
                    if entry.count == 0 {
                        state.origins.remove(&key);
                    }
                }
                return Vec::new();
            }
            let epoch = state.next_epoch();
            let entry = state.origins.entry(key.clone()).or_default();
            let released = entry.count;
            *entry = Origin {
                count: 0,
                epoch,
                yielded: true,
            };
            state.shared -= released;
            state.advance()
        });
        admitted.iter().for_each(|admitted| admitted.fire());
    }

    fn release(&self, token: u64, origin: &OriginKey, epoch: Option<u64>) {
        let admitted = self.state.update(|state| {
            state.waiting.retain(|waiter| waiter.token != token);
            if state.exclusive == Some(token) {
                state.exclusive = None;
            }
            if let Some(epoch) = epoch.or_else(|| state.granted.remove(&token)) {
                state.release_shared(origin, epoch);
            }
            state.advance()
        });
        admitted.iter().for_each(|admitted| admitted.fire());
    }
}
