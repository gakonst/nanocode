use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    future::Future,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    task::{Context, Poll},
    time::Duration,
};

use nanocode_durability_core::{KeyValue, StoreError, StoreFuture, Versioned};
use nanocode_sync::Slot;

use crate::{
    executor::Clock,
    faults::{CACHE_WRITES, Durability, Profile},
    rng::{FNV_OFFSET, Rng, fnv},
};

const ACKED_WRITES_DROPPED_MAX: u64 = 3;
const CAS_IGNORES_EXPECTED_PPM: u64 = 500_000;
const RECORD_INSERT_DROPS_PPM: u64 = 250_000;
const RECORD_INSERT_OVERWRITES_PPM: u64 = 500_000;
const RECORD_PREFIX: &str = "record/";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mutation {
    None,
    CasIgnoresExpected,
    RecordInsertDrops,
    PowerLossDropsAcked,
    RecordInsertOverwrites,
    CacheKeepsBlank,
    StagedRefusesRewrite,
}

impl Mutation {
    const ALL: [Self; 7] = [
        Self::None,
        Self::CasIgnoresExpected,
        Self::RecordInsertDrops,
        Self::PowerLossDropsAcked,
        Self::RecordInsertOverwrites,
        Self::CacheKeepsBlank,
        Self::StagedRefusesRewrite,
    ];

    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|mutation| mutation.name() == name)
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::CasIgnoresExpected => "cas_ignores_expected",
            Self::RecordInsertDrops => "record_insert_drops",
            Self::PowerLossDropsAcked => "power_loss_drops_acked",
            Self::RecordInsertOverwrites => "record_insert_overwrites",
            Self::CacheKeepsBlank => "cache_keeps_blank",
            Self::StagedRefusesRewrite => "staged_refuses_rewrite",
        }
    }

    pub fn names() -> String {
        Self::ALL.map(Self::name).join("|")
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub version: String,
    pub value: String,
    pub seq: u64,
}

#[derive(Clone, Debug)]
pub struct CasEvent {
    pub key: String,
    pub prev: Option<String>,
    pub prev_seq: Option<u64>,
    pub new: String,
    pub value: String,
    pub swapped: bool,
    pub after_fault: bool,
    pub seq: u64,
}

#[derive(Default, Debug)]
pub struct OpLog {
    pub faulted: bool,
    pub cas: Vec<CasEvent>,
}

pub struct PowerLoss {
    pub reverted: u64,
    pub survived_seq: u64,
}

type Recent = (String, Option<Entry>, Entry);

pub struct Kv {
    rng: Rng,
    profile: Profile,
    mutation: Mutation,
    pub faults_on: bool,
    pending: BTreeMap<String, Entry>,
    durable: BTreeMap<String, Entry>,
    unsynced: VecDeque<(u64, String, Entry)>,
    recent: VecDeque<Recent>,
    seq: u64,
    logs: BTreeMap<u64, OpLog>,
    pub digest: u64,
    verbose: bool,
    pub faults: u64,
}

fn injected(what: &str, when: &str) -> StoreError {
    StoreError::Backend(format!("sim: injected {what} fault {when}"))
}

impl Kv {
    pub fn trace(&mut self, line: &str) {
        self.digest = fnv(fnv(self.digest, line.as_bytes()), b"\n");
        if self.verbose {
            println!("{line}");
        }
    }

    fn fault(&mut self, op: u64, ppm: u64, what: &str) -> bool {
        if !self.faults_on || !self.rng.chance(ppm) {
            return false;
        }
        self.faults += 1;
        self.logs.entry(op).or_default().faulted = true;
        self.trace(&format!("  kv fault op={op} {what}"));
        true
    }

    pub const fn seq(&self) -> u64 {
        self.seq
    }

    fn write(&mut self, key: &str, mut entry: Entry) {
        self.seq += 1;
        entry.seq = self.seq;
        let line = format!(
            "  kv write #{} {key} = {:?} {:?}",
            self.seq, entry.version, entry.value
        );
        self.trace(&line);
        self.pending.insert(key.to_owned(), entry.clone());
        match self.profile.durability {
            Durability::OnAck | Durability::LyingCache { .. } => {
                let prev = self.durable.insert(key.to_owned(), entry.clone());
                self.recent.push_back((key.to_owned(), prev, entry));
                if self.recent.len() > CACHE_WRITES {
                    self.recent.pop_front();
                }
            }
            Durability::Async { sync_ppm } => {
                self.unsynced.push_back((self.seq, key.to_owned(), entry));
                if self.rng.chance(sync_ppm) {
                    self.sync();
                }
            }
        }
    }

    fn sync(&mut self) {
        for (_, key, entry) in self.unsynced.drain(..) {
            self.durable.insert(key, entry);
        }
        let line = format!("  kv sync through #{}", self.seq);
        self.trace(&line);
    }

    fn revert(&mut self, key: &str, prev: Option<Entry>) {
        match prev {
            Some(entry) => self.durable.insert(key.to_owned(), entry),
            None => self.durable.remove(key),
        };
    }

    pub fn power_loss(&mut self) -> PowerLoss {
        let (reverted, survived_seq) = match self.profile.durability {
            Durability::OnAck => (self.drop_acked(), self.seq),
            Durability::Async { .. } => self.lose_unsynced(),
            Durability::LyingCache { lost_ppm } => (self.lose_cached(lost_ppm), self.seq),
        };
        self.unsynced.clear();
        self.recent.clear();
        self.pending = self.durable.clone();
        let line = format!("  kv power loss reverted={reverted} survived_seq={survived_seq}");
        self.trace(&line);
        PowerLoss {
            reverted,
            survived_seq,
        }
    }

    fn drop_acked(&mut self) -> u64 {
        if self.mutation != Mutation::PowerLossDropsAcked || self.recent.is_empty() {
            return 0;
        }
        let recent = u64::try_from(self.recent.len()).expect("the cache window fits in u64");
        let count = self.rng.range(1, recent.min(ACKED_WRITES_DROPPED_MAX));
        let mut reverted = 0;
        for _ in 0..count {
            if let Some((key, prev, _)) = self.recent.pop_back() {
                self.revert(&key, prev);
                reverted += 1;
            }
        }
        reverted
    }

    fn lose_unsynced(&mut self) -> (u64, u64) {
        let unsynced = u64::try_from(self.unsynced.len()).expect("unsynced writes fit in u64");
        let keep = self.rng.below(unsynced + 1);
        let mut survived_seq = self
            .unsynced
            .front()
            .map_or(self.seq, |(first, ..)| first - 1);
        let mut reverted = 0;
        let writes: Vec<_> = self.unsynced.drain(..).collect();
        for (index, (seq, key, entry)) in (0..).zip(writes) {
            if index < keep {
                survived_seq = seq;
                self.durable.insert(key, entry);
            } else {
                reverted += 1;
            }
        }
        (reverted, survived_seq)
    }

    fn lose_cached(&mut self, lost_ppm: u64) -> u64 {
        let writes: Vec<_> = self.recent.drain(..).collect();
        let lost: Vec<bool> = writes.iter().map(|_| self.rng.chance(lost_ppm)).collect();
        let mut fixed = BTreeSet::new();
        let mut tentative: BTreeMap<String, Option<Entry>> = BTreeMap::new();
        for ((key, prev, _), lost) in writes.iter().zip(&lost).rev() {
            if fixed.contains(key) {
                continue;
            }
            if *lost {
                tentative.insert(key.clone(), prev.clone());
            } else {
                fixed.insert(key.clone());
                tentative.remove(key);
            }
        }
        let mut reverted = 0;
        for (key, prev) in tentative {
            if self.durable.get(&key) != prev.as_ref() {
                reverted += 1;
            }
            self.revert(&key, prev);
        }
        reverted
    }

    fn delay(&mut self) -> Duration {
        let spread = self.rng.below(self.profile.latency_spread_ns + 1);
        Duration::from_nanos(self.profile.latency_min_ns + spread)
    }

    fn get(&mut self, op: u64, key: &str) -> Result<Option<Versioned>, StoreError> {
        if self.fault(op, self.profile.read_ppm, "get") {
            return Err(injected("get", "before read"));
        }
        Ok(self.pending.get(key).map(|entry| Versioned {
            version: entry.version.clone(),
            value: entry.value.clone(),
        }))
    }

    fn get_values(&mut self, op: u64, keys: &[String]) -> Result<Vec<Option<String>>, StoreError> {
        if self.fault(op, self.profile.read_ppm, "get_values") {
            return Err(injected("get_values", "before read"));
        }
        Ok(keys
            .iter()
            .map(|key| self.pending.get(key).map(|entry| entry.value.clone()))
            .collect())
    }

    fn landed_count(&mut self, op: u64, count: usize) -> (usize, bool) {
        if count > 1 && self.fault(op, self.profile.torn_ppm, "put_values torn") {
            let more = u64::try_from(count - 1).expect("a batch length fits in u64");
            let prefix = usize::try_from(self.rng.below(more)).expect("a prefix fits in usize");
            return (1 + prefix, true);
        }
        (count, false)
    }

    fn put_values(&mut self, op: u64, entries: &[(String, String)]) -> Result<(), StoreError> {
        if self.fault(op, self.profile.before_ppm, "put_values before apply") {
            return Err(injected("put_values", "before apply"));
        }
        let (count, torn) = self.landed_count(op, entries.len());
        for (key, value) in &entries[..count] {
            let entry = Entry {
                version: String::new(),
                value: value.clone(),
                seq: 0,
            };
            self.write(key, entry);
        }
        if torn {
            return Err(injected("put_values", "torn after a prefix landed"));
        }
        if self.fault(op, self.profile.after_ppm, "put_values after apply") {
            return Err(injected("put_values", "after apply"));
        }
        Ok(())
    }

    fn compare_and_swap(
        &mut self,
        op: u64,
        key: &str,
        expected_version: Option<&str>,
        entry: Entry,
    ) -> Result<bool, StoreError> {
        if self.fault(op, self.profile.before_ppm, "cas before apply") {
            return Err(injected("compare_and_swap", "before apply"));
        }
        let prev = self.pending.get(key).map(|entry| entry.version.clone());
        let prev_seq = self.pending.get(key).map(|entry| entry.seq);
        let swapped =
            prev.as_deref() == expected_version || self.mutated_swap(key, expected_version);
        let (new, value) = (entry.version.clone(), entry.value.clone());
        if swapped && !self.drops_insert(key, expected_version) {
            self.write(key, entry);
        }
        let after_fault = self.fault(op, self.profile.after_ppm, "cas after apply");
        let event = CasEvent {
            key: key.to_owned(),
            prev,
            prev_seq,
            new,
            value,
            swapped,
            after_fault,
            seq: self.seq,
        };
        self.logs.entry(op).or_default().cas.push(event);
        if after_fault {
            return Err(injected("compare_and_swap", "after apply"));
        }
        Ok(swapped)
    }

    fn drops_insert(&mut self, key: &str, expected_version: Option<&str>) -> bool {
        self.mutation == Mutation::RecordInsertDrops
            && expected_version.is_none()
            && key.starts_with(RECORD_PREFIX)
            && self.rng.chance(RECORD_INSERT_DROPS_PPM)
    }

    fn mutated_swap(&mut self, key: &str, expected_version: Option<&str>) -> bool {
        match self.mutation {
            Mutation::CasIgnoresExpected => self.rng.chance(CAS_IGNORES_EXPECTED_PPM),
            Mutation::RecordInsertOverwrites => {
                expected_version.is_none()
                    && key.starts_with(RECORD_PREFIX)
                    && self.rng.chance(RECORD_INSERT_OVERWRITES_PPM)
            }
            _ => false,
        }
    }

    pub fn holds_other(&self, key: &str, value: &str) -> bool {
        self.pending
            .get(key)
            .is_some_and(|entry| !entry.value.is_empty() && entry.value != value)
    }

    fn list_keys(&mut self, op: u64, prefix: &str) -> Result<Vec<String>, StoreError> {
        if self.fault(op, self.profile.read_ppm, "list_keys") {
            return Err(injected("list_keys", "before read"));
        }
        Ok(self
            .pending
            .range(prefix.to_owned()..)
            .take_while(|(key, _)| key.starts_with(prefix))
            .map(|(key, _)| key.clone())
            .collect())
    }
}

struct Shared {
    kv: Slot<Kv>,
    op: AtomicU64,
    clock: Arc<Clock>,
}

#[derive(Clone)]
pub struct SimKv(Arc<Shared>);

pub struct Tagged<'a, F> {
    shared: &'a Shared,
    op: u64,
    future: F,
}

impl<F: Future + Unpin> Future for Tagged<'_, F> {
    type Output = F::Output;

    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<F::Output> {
        let tagged = &mut *self;
        let outer = tagged.shared.op.swap(tagged.op, Ordering::SeqCst);
        let polled = Pin::new(&mut tagged.future).poll(context);
        tagged.shared.op.store(outer, Ordering::SeqCst);
        polled
    }
}

impl SimKv {
    pub fn new(
        seed: u64,
        profile: Profile,
        mutation: Mutation,
        verbose: bool,
        clock: Arc<Clock>,
    ) -> Self {
        let kv = Kv {
            rng: Rng::derive(seed, "kv"),
            profile,
            mutation,
            faults_on: true,
            pending: BTreeMap::new(),
            durable: BTreeMap::new(),
            unsynced: VecDeque::new(),
            recent: VecDeque::new(),
            seq: 0,
            logs: BTreeMap::new(),
            digest: FNV_OFFSET,
            verbose,
            faults: 0,
        };
        Self(Arc::new(Shared {
            kv: Slot::new(Some(kv)),
            op: AtomicU64::new(0),
            clock,
        }))
    }

    pub fn with<T>(&self, each: impl FnOnce(&mut Kv) -> T) -> T {
        let mut kv =
            self.0.kv.take().expect(
                "the simulator polls one task at a time, so its store is never taken twice",
            );
        let result = each(&mut kv);
        assert!(
            self.0.kv.put(kv).is_none(),
            "the simulated store has one home"
        );
        result
    }

    pub fn trace(&self, line: &str) {
        self.with(|kv| kv.trace(line));
    }

    pub fn take_log(&self, op: u64) -> OpLog {
        self.with(|kv| kv.logs.remove(&op).unwrap_or_default())
    }

    pub fn tagged<F: Future + Unpin>(&self, op: u64, future: F) -> Tagged<'_, F> {
        Tagged {
            shared: &self.0,
            op,
            future,
        }
    }

    fn call<'a, T: Send + 'a>(
        &'a self,
        act: impl FnOnce(&mut Kv, u64) -> T + Send + 'a,
    ) -> StoreFuture<'a, T> {
        Box::pin(async move {
            let op = self.0.op.load(Ordering::SeqCst);
            let before = self.with(Kv::delay);
            self.0.clock.sleep(before).await;
            let result = self.with(|kv| act(kv, op));
            let after = self.with(Kv::delay);
            self.0.clock.sleep(after).await;
            result
        })
    }
}

impl KeyValue for SimKv {
    fn get<'a>(&'a self, key: &'a str) -> StoreFuture<'a, Result<Option<Versioned>, StoreError>> {
        self.call(move |kv, op| kv.get(op, key))
    }

    fn get_values<'a>(
        &'a self,
        keys: &'a [String],
    ) -> StoreFuture<'a, Result<Vec<Option<String>>, StoreError>> {
        self.call(move |kv, op| kv.get_values(op, keys))
    }

    fn put_values<'a>(
        &'a self,
        entries: &'a [(String, String)],
    ) -> StoreFuture<'a, Result<(), StoreError>> {
        self.call(move |kv, op| kv.put_values(op, entries))
    }

    fn compare_and_swap<'a>(
        &'a self,
        key: &'a str,
        expected_version: Option<&'a str>,
        version: &'a str,
        value: &'a str,
    ) -> StoreFuture<'a, Result<bool, StoreError>> {
        self.call(move |kv, op| {
            let entry = Entry {
                version: version.to_owned(),
                value: value.to_owned(),
                seq: 0,
            };
            kv.compare_and_swap(op, key, expected_version, entry)
        })
    }

    fn list_keys<'a>(
        &'a self,
        prefix: &'a str,
    ) -> StoreFuture<'a, Result<Vec<String>, StoreError>> {
        self.call(move |kv, op| kv.list_keys(op, prefix))
    }
}
