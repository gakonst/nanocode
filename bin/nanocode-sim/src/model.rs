use std::collections::BTreeMap;

use nanocode_durability_core::{StoreError, StoredState};

use crate::{
    faults::Durability,
    store::{OpLog, PowerLoss},
};

pub fn state_id(state: usize) -> String {
    format!("s{state}")
}

pub fn head_state(key: &str) -> Option<usize> {
    key.strip_prefix("state/s")?.parse().ok()
}

pub fn parse_version(version: &str) -> Option<(u64, u64, &str)> {
    let mut parts = version.splitn(3, '.');
    Some((
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
        parts.next()?,
    ))
}

pub fn version(fence: u64, revision: u64, owner: &str) -> String {
    format!("{fence}.{revision}.{owner}")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Staged,
    InFlight,
    Acked,
    Failed,
    Rejected,
    Uncertain,
}

pub struct Attempt {
    pub state: usize,
    pub revision: u64,
    pub refs: Vec<String>,
    pub payload: String,
    pub status: Status,
    pub rejected: Option<String>,
}

#[derive(Default)]
pub struct StateModel {
    pub fences: Vec<(u64, u64)>,
    pub applied: Vec<(u64, u64)>,
    pub acked: Vec<(u64, u64, u64)>,
    pub observed: Vec<(u64, u64)>,
    pub floor: u64,
}

impl StateModel {
    fn reset(&mut self, floor: u64) {
        self.fences.clear();
        self.applied.clear();
        self.acked.clear();
        self.observed.clear();
        self.floor = floor;
    }

    pub fn revision_floor(&self, before: u64) -> u64 {
        let acked = self.acked.iter().filter(|a| a.0 < before).map(|a| a.1);
        let applied = self.applied.iter().filter(|a| a.0 < before).map(|a| a.1);
        let observed = self.observed.iter().filter(|a| a.0 < before).map(|a| a.1);
        acked
            .chain(applied)
            .chain(observed)
            .fold(self.floor, u64::max)
    }

    pub fn fence_floor(&self, before: u64) -> Option<u64> {
        self.fences
            .iter()
            .filter(|f| f.0 < before)
            .map(|f| f.1)
            .max()
    }
}

#[derive(Default, Clone, Debug)]
pub struct Stats {
    pub ops: u64,
    pub store_errors: u64,
    pub fenced: u64,
    pub conflicts: u64,
    pub acked_commits: u64,
    pub restarts: u64,
    pub power_losses: u64,
    pub reverted_writes: u64,
    pub damaged_reads: u64,
    pub partial_commit_to: u64,
    pub record_conflicts: u64,
    pub kv_faults: u64,
    pub epochs: u64,
}

impl Stats {
    pub const fn add(&mut self, other: &Self) {
        self.ops += other.ops;
        self.store_errors += other.store_errors;
        self.fenced += other.fenced;
        self.conflicts += other.conflicts;
        self.acked_commits += other.acked_commits;
        self.restarts += other.restarts;
        self.power_losses += other.power_losses;
        self.reverted_writes += other.reverted_writes;
        self.damaged_reads += other.damaged_reads;
        self.partial_commit_to += other.partial_commit_to;
        self.record_conflicts += other.record_conflicts;
        self.kv_faults += other.kv_faults;
        self.epochs += other.epochs;
    }
}

#[derive(Clone, Default)]
pub struct Observed {
    pub attempt: Option<usize>,
    pub refs: Vec<String>,
}

pub struct Ctx {
    pub op: u64,
    pub start: u64,
    pub end: u64,
    pub log: OpLog,
}

pub struct Model {
    pub clock: u64,
    pub next_op: u64,
    pub durability: Durability,
    pub attempts: Vec<Attempt>,
    pub states: Vec<StateModel>,
    pub records: BTreeMap<String, String>,
    pub damaged: bool,
    pub wrong: Option<String>,
    pub stats: Stats,
}

impl Model {
    pub fn new(durability: Durability, states: usize) -> Self {
        Self {
            clock: 0,
            next_op: 0,
            durability,
            attempts: Vec::new(),
            states: (0..states).map(|_| StateModel::default()).collect(),
            records: BTreeMap::new(),
            damaged: false,
            wrong: None,
            stats: Stats::default(),
        }
    }

    pub fn wrong(&mut self, message: String) {
        if self.wrong.is_none() {
            self.wrong = Some(message);
        }
    }

    fn attempt_of(&self, payload: &str) -> Option<usize> {
        let id: usize = payload.strip_prefix('a')?.split(';').next()?.parse().ok()?;
        (id < self.attempts.len()).then_some(id)
    }

    fn payload_refs(&mut self, state: usize, stored: &StoredState, what: &str) -> Option<Observed> {
        let revision = stored.revision;
        match &stored.payload {
            None if revision == 0 => Some(Observed::default()),
            Some(payload) if revision > 0 => self.attempt_refs(state, revision, payload, what),
            payload => {
                self.wrong(format!(
                    "{what} of s{state} returned revision {revision} with payload {payload:?}"
                ));
                None
            }
        }
    }

    fn attempt_refs(
        &mut self,
        state: usize,
        revision: u64,
        payload: &str,
        what: &str,
    ) -> Option<Observed> {
        let Some(id) = self.attempt_of(payload) else {
            self.wrong(format!(
                "{what} of s{state} returned an unknown payload {payload:?} at revision {revision}"
            ));
            return None;
        };
        let attempt = &self.attempts[id];
        if attempt.state != state || attempt.revision != revision || attempt.payload != payload {
            let message = format!(
                "{what} of s{state} returned revision {revision} carrying attempt {id} written for s{} revision {}",
                attempt.state, attempt.revision
            );
            self.wrong(message);
            return None;
        }
        if matches!(
            attempt.status,
            Status::Failed | Status::Rejected | Status::Staged
        ) {
            let message = format!(
                "{what} of s{state} returned revision {revision} from attempt {id}, which is {:?} (reported not written)",
                attempt.status
            );
            self.wrong(message);
            return None;
        }
        Some(Observed {
            attempt: Some(id),
            refs: attempt.refs.clone(),
        })
    }

    pub fn observe(
        &mut self,
        state: usize,
        stored: &StoredState,
        ctx: &Ctx,
        what: &str,
    ) -> Option<Observed> {
        let observed = self.payload_refs(state, stored, what)?;
        let floor = self.states[state].revision_floor(ctx.start);
        if stored.revision < floor {
            self.wrong(format!(
                "{what} of s{state} returned revision {} after revision {floor} was acknowledged or observed",
                stored.revision
            ));
            return None;
        }
        self.states[state].observed.push((ctx.end, stored.revision));
        Some(observed)
    }

    pub fn register(&mut self, ctx: &Ctx) {
        for cas in ctx.log.cas.iter().filter(|cas| cas.swapped) {
            let Some(state) = head_state(&cas.key) else {
                continue;
            };
            let Some((fence, revision, _)) = parse_version(&cas.new) else {
                continue;
            };
            let prev = cas.prev.as_deref().and_then(parse_version);
            if prev.map(|p| p.0) != Some(fence) {
                self.states[state].fences.push((ctx.end, fence));
            }
            if prev.map_or(0, |p| p.1) != revision {
                self.states[state].applied.push((ctx.end, revision));
            }
        }
    }

    pub fn failed(&mut self, error: &StoreError, ctx: &Ctx, what: &str) {
        match error {
            StoreError::Fenced => self.stats.fenced += 1,
            StoreError::Conflict { .. } => self.stats.conflicts += 1,
            _ => {
                self.stats.store_errors += 1;
                if !ctx.log.faulted {
                    self.wrong(format!(
                        "{what} failed with `{error}` although no fault was injected"
                    ));
                }
            }
        }
    }

    pub fn crash(&mut self, power: Option<&PowerLoss>) {
        for attempt in &mut self.attempts {
            attempt.status = match attempt.status {
                Status::InFlight => Status::Uncertain,
                Status::Staged => Status::Failed,
                status => status,
            };
        }
        let Some(power) = power else {
            self.stats.restarts += 1;
            return;
        };
        self.stats.power_losses += 1;
        self.stats.reverted_writes += power.reverted;
        match self.durability {
            Durability::OnAck => {}
            Durability::Async { .. } => self.lose_unsynced(power.survived_seq),
            Durability::LyingCache { .. } if power.reverted > 0 => {
                self.damaged = true;
                for state in &mut self.states {
                    state.reset(0);
                }
            }
            Durability::LyingCache { .. } => {}
        }
    }

    fn lose_unsynced(&mut self, survived_seq: u64) {
        for state in &mut self.states {
            let floor = state
                .acked
                .iter()
                .filter(|a| a.2 <= survived_seq)
                .map(|a| a.1)
                .max()
                .unwrap_or(0);
            state.reset(floor);
        }
    }

    pub fn max_attempted(&self, state: usize) -> u64 {
        self.attempts
            .iter()
            .filter(|a| {
                a.state == state
                    && matches!(
                        a.status,
                        Status::Acked | Status::Uncertain | Status::InFlight
                    )
            })
            .map(|a| a.revision)
            .max()
            .unwrap_or(0)
    }
}
