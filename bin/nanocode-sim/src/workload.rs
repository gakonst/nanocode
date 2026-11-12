use std::{
    cell::{Cell, RefCell},
    collections::{BTreeMap, BTreeSet},
    fmt::Debug,
    sync::Arc,
    time::Duration,
};

use nanocode_durability::{CachedStore, RecordCache};
use nanocode_durability_core::{
    KeyValueStore, OwnedState, OwnerId, OwnerToken, Settle, StagedStore, StateCommit, StateStore,
    StoreError, StoreRecord, SummaryUpdate,
};

use crate::{
    executor::Clock,
    model::{Attempt, Ctx, Model, Observed, Status, parse_version, state_id, version},
    rng::{FNV_OFFSET, Rng, fnv},
    store::SimKv,
};

const THINK_NS_MAX: u64 = 200_000;
const RECORD_CACHE_BYTES_MAX: u64 = 4096;
const INHERITED_REFS_MAX: usize = 4;
const READ_AFTER_ACQUIRE_PPM: u64 = 500_000;
const INHERIT_REF_PPM: u64 = 500_000;
const STEP_ROLLS: u64 = 100;
const ACQUIRE_ROLLS_END: u64 = 25;
const COMMIT_ROLLS_END: u64 = 60;
const PEEK_ROLLS_END: u64 = 80;
const OWNER_ROLLS_END: u64 = 88;
const LIST_ROLLS_END: u64 = 92;
const CHECKER_PROC: usize = 99;

pub struct World {
    pub kv: SimKv,
    pub clock: Arc<Clock>,
    pub model: RefCell<Model>,
    pub ops_left: Cell<u64>,
    pub states: usize,
    pub records_max: u64,
    pub slots: bool,
}

impl World {
    fn begin(&self, what: &str) -> Ctx {
        let mut model = self.model.borrow_mut();
        model.clock += 1;
        model.next_op += 1;
        model.stats.ops += 1;
        let op = model.next_op;
        self.kv.trace(&format!("{op} > {what}"));
        Ctx {
            op,
            start: model.clock,
            end: 0,
            log: crate::store::OpLog::default(),
        }
    }

    fn end<T: Debug>(&self, mut ctx: Ctx, result: &T) -> Ctx {
        let mut model = self.model.borrow_mut();
        model.clock += 1;
        ctx.end = model.clock;
        ctx.log = self.kv.take_log(ctx.op);
        self.kv.trace(&format!("{} < {result:?}", ctx.op));
        ctx
    }

    pub fn failed(&self) -> bool {
        self.model.borrow().wrong.is_some()
    }
}

#[derive(Clone)]
struct Held {
    token: OwnerToken,
    revision: u64,
    refs: Vec<String>,
}

struct Staging {
    staged: StagedStore<KeyValueStore<SimKv>>,
    base: KeyValueStore<SimKv>,
    bases: BTreeMap<usize, (u64, u64)>,
    revisions: BTreeMap<usize, u64>,
    pending: Vec<usize>,
}

impl Staging {
    fn fresh(kv: &SimKv) -> Box<Self> {
        Box::new(Self {
            staged: StagedStore::new(KeyValueStore::new(kv.clone())),
            base: KeyValueStore::new(kv.clone()),
            bases: BTreeMap::new(),
            revisions: BTreeMap::new(),
            pending: Vec::new(),
        })
    }
}

enum Mode {
    Plain(KeyValueStore<SimKv>),
    Cached(CachedStore<KeyValueStore<SimKv>>),
    Staged(Box<Staging>),
}

pub struct Proc {
    id: usize,
    owner: OwnerId,
    rng: Rng,
    mode: Mode,
    held: BTreeMap<usize, Held>,
    kv: SimKv,
}

fn pick_mode(rng: &mut Rng, kv: &SimKv, slots: bool) -> Mode {
    match rng.below(if slots { 2 } else { 3 }) {
        0 => Mode::Plain(KeyValueStore::new(kv.clone())),
        1 => {
            let budget = usize::try_from(rng.below(RECORD_CACHE_BYTES_MAX))
                .expect("a cache budget below 4096 fits in usize");
            let cache = RecordCache::new(budget).expect("a record cache needs no I/O");
            Mode::Cached(cache.wrap(KeyValueStore::new(kv.clone())))
        }
        _ => Mode::Staged(Staging::fresh(kv)),
    }
}

const fn definite(error: &StoreError) -> bool {
    matches!(error, StoreError::Fenced | StoreError::Conflict { .. })
}

impl Proc {
    pub fn new(kv: &SimKv, seed: u64, epoch: u64, id: usize, slots: bool) -> Self {
        let mut rng = Rng::derive(seed, &format!("proc/{epoch}/{id}"));
        let mode = pick_mode(&mut rng, kv, slots);
        Self {
            id,
            owner: OwnerId::from(format!("p{id}e{epoch}")),
            rng,
            mode,
            held: BTreeMap::new(),
            kv: kv.clone(),
        }
    }

    fn checker(world: &World, label: &str) -> Self {
        Self {
            id: CHECKER_PROC,
            owner: OwnerId::from(format!("checker{label}")),
            rng: Rng::derive(0, label),
            mode: Mode::Plain(KeyValueStore::new(world.kv.clone())),
            held: BTreeMap::new(),
            kv: world.kv.clone(),
        }
    }

    pub const fn mode_name(&self) -> &'static str {
        match self.mode {
            Mode::Plain(_) => "plain",
            Mode::Cached(_) => "cached",
            Mode::Staged(_) => "staged",
        }
    }

    fn writer(&mut self) -> &mut dyn StateStore {
        match &mut self.mode {
            Mode::Plain(store) => store,
            Mode::Cached(store) => store,
            Mode::Staged(staging) => &mut staging.staged,
        }
    }

    fn reader(&mut self) -> &mut dyn StateStore {
        match &mut self.mode {
            Mode::Plain(store) => store,
            Mode::Cached(store) => store,
            Mode::Staged(staging) => &mut staging.base,
        }
    }

    pub async fn run(&mut self, world: &World) {
        while !world.failed() {
            let left = world.ops_left.get();
            if left == 0 {
                return;
            }
            world.ops_left.set(left - 1);
            let think = self.rng.below(THINK_NS_MAX);
            world.clock.sleep(Duration::from_nanos(think)).await;
            self.step(world).await;
        }
    }

    async fn step(&mut self, world: &World) {
        let roll = self.rng.below(STEP_ROLLS);
        let states = u64::try_from(world.states).expect("a state count fits in u64");
        let state = usize::try_from(self.rng.below(states)).expect("a state index fits in usize");
        let staged = matches!(self.mode, Mode::Staged(_));
        match roll {
            _ if roll < ACQUIRE_ROLLS_END => self.acquire(world, state).await,
            _ if roll < COMMIT_ROLLS_END => self.commit_held(world, state).await,
            _ if roll < PEEK_ROLLS_END => self.peek(world, state).await,
            _ if roll < OWNER_ROLLS_END => self.owner_of(world, state).await,
            _ if roll < LIST_ROLLS_END => self.list(world).await,
            _ if staged => self.commit_to(world).await,
            _ => self.peek(world, state).await,
        }
    }

    async fn commit_held(&mut self, world: &World, state: usize) {
        let held: Vec<usize> = self.held.keys().copied().collect();
        if held.is_empty() {
            self.acquire(world, state).await;
        } else {
            let pick = held[self.rng.pick(held.len())];
            self.commit(world, pick).await;
        }
    }

    async fn acquire(&mut self, world: &World, state: usize) {
        let id = state_id(state);
        let owner = self.owner.clone();
        let what = format!("p{} {} acquire {id}", self.id, self.mode_name());
        let ctx = world.begin(&what);
        let result = world
            .kv
            .tagged(ctx.op, self.writer().acquire(&id, owner))
            .await;
        let ctx = world.end(ctx, &result);
        let observed = {
            let mut model = world.model.borrow_mut();
            match &result {
                Ok(owned) => self.judge_acquired(&mut model, state, owned, &ctx, &what),
                Err(error) => {
                    model.failed(error, &ctx, &what);
                    model.register(&ctx);
                    None
                }
            }
        };
        self.hold_acquired(state, result, observed.clone());
        if let Some(observed) = observed
            && !observed.refs.is_empty()
            && self.rng.chance(READ_AFTER_ACQUIRE_PPM)
        {
            self.read(world, state, observed).await;
        }
    }

    fn judge_acquired(
        &mut self,
        model: &mut Model,
        state: usize,
        owned: &OwnedState,
        ctx: &Ctx,
        what: &str,
    ) -> Option<Observed> {
        match &mut self.mode {
            Mode::Staged(staging) => judge_staged_acquired(model, staging, state, owned, ctx, what),
            _ => judge_plain_acquired(model, self.owner.as_str(), state, owned, ctx, what),
        }
    }

    fn hold_acquired(
        &mut self,
        state: usize,
        result: Result<OwnedState, StoreError>,
        observed: Option<Observed>,
    ) {
        let Ok(owned) = result else {
            self.held.remove(&state);
            return;
        };
        let refs = observed.map(|observed| observed.refs).unwrap_or_else(|| {
            self.held
                .get(&state)
                .map(|held| held.refs.clone())
                .unwrap_or_default()
        });
        let held = Held {
            token: owned.owner,
            revision: owned.state.revision,
            refs,
        };
        self.held.insert(state, held);
    }

    fn plan_records(
        &mut self,
        world: &World,
        attempt: usize,
        held: &Held,
    ) -> (Vec<StoreRecord>, Vec<String>) {
        let count = self.rng.below(world.records_max + 1);
        let mut records = Vec::new();
        let mut refs = Vec::new();
        for index in 0..count {
            let value = format!("v{attempt}.{index}.{:x}", self.rng.draw());
            let key = if world.slots {
                format!("slot{index}")
            } else {
                format!("k{:016x}", fnv(FNV_OFFSET, value.as_bytes()))
            };
            refs.push(key.clone());
            records.push(StoreRecord { key, value });
        }
        for inherited in &held.refs {
            if !world.slots
                && refs.len() < INHERITED_REFS_MAX
                && self.rng.chance(INHERIT_REF_PPM)
                && !refs.contains(inherited)
            {
                refs.push(inherited.clone());
            }
        }
        (records, refs)
    }

    async fn commit(&mut self, world: &World, state: usize) {
        let Some(held) = self.held.get(&state).cloned() else {
            return;
        };
        let staged = matches!(self.mode, Mode::Staged(_));
        let attempt = world.model.borrow().attempts.len();
        let (records, refs) = self.plan_records(world, attempt, &held);
        let payload = format!("a{attempt};{}", refs.join(","));
        let revision = held.revision + 1;
        {
            let mut model = world.model.borrow_mut();
            for record in &records {
                model
                    .records
                    .insert(record.key.clone(), record.value.clone());
            }
            model.attempts.push(Attempt {
                state,
                revision,
                refs: refs.clone(),
                payload: payload.clone(),
                status: if staged {
                    Status::Staged
                } else {
                    Status::InFlight
                },
                rejected: None,
            });
        }
        let id = state_id(state);
        let what = format!(
            "p{} {} commit {id} fence={} rev {}->{revision} attempt {attempt} records={}",
            self.id,
            self.mode_name(),
            held.token.fence(),
            held.revision,
            records.len()
        );
        let commit = StateCommit {
            payload: &payload,
            records: &records,
            events: &[],
            summary: &SummaryUpdate::Unchanged,
            settle: Settle::Durable,
        };
        let ctx = world.begin(&what);
        let writing = self
            .writer()
            .commit(&id, &held.token, held.revision, commit);
        let result = world.kv.tagged(ctx.op, writing).await;
        let ctx = world.end(ctx, &result);
        let judged = Judged {
            state,
            attempt,
            revision,
            payload: &payload,
            records: &records,
            what: &what,
        };
        match &result {
            Ok(committed) => self.judge_committed(world, &judged, &held, committed.revision, &ctx),
            Err(error) => judge_commit_failed(world, &judged, staged, error, &ctx),
        }
        match result {
            Ok(_) => {
                let token = held.token;
                self.held.insert(
                    state,
                    Held {
                        token,
                        revision,
                        refs,
                    },
                );
            }
            Err(_) => {
                self.held.remove(&state);
            }
        }
    }

    fn judge_committed(
        &mut self,
        world: &World,
        judged: &Judged<'_>,
        held: &Held,
        acknowledged: u64,
        ctx: &Ctx,
    ) {
        let mut model = world.model.borrow_mut();
        let what = judged.what;
        if acknowledged != judged.revision {
            model.wrong(format!("{what}: acknowledged revision {acknowledged}"));
        }
        if let Mode::Staged(staging) = &mut self.mode {
            if ctx.log.cas.iter().any(|cas| cas.swapped) {
                model.wrong(format!("{what}: staged commit wrote to the base store"));
            }
            staging.revisions.insert(judged.state, judged.revision);
            staging.pending.push(judged.attempt);
            return;
        }
        judge_plain_committed(&mut model, judged, held, self.owner.as_str(), ctx);
    }

    async fn commit_to(&mut self, world: &World) {
        let Mode::Staged(staging) = &mut self.mode else {
            return;
        };
        if staging.pending.is_empty() {
            return;
        }
        let pending = staging.pending.clone();
        let what = format!("p{} staged commit_to attempts {pending:?}", self.id);
        {
            let mut model = world.model.borrow_mut();
            for attempt in &pending {
                model.attempts[*attempt].status = Status::InFlight;
            }
        }
        let writes = staging.staged.writes();
        let ctx = world.begin(&what);
        let result = world
            .kv
            .tagged(ctx.op, Box::pin(writes.commit_to(&mut staging.base)))
            .await;
        let ctx = world.end(ctx, &result);
        match &result {
            Ok(()) => judge_committed_to(world, staging, &pending, &ctx, &what),
            Err(error) => judge_commit_to_failed(world, &self.kv, &pending, error, &ctx, &what),
        }
        self.mode = Mode::Staged(Staging::fresh(&self.kv));
        self.held.clear();
    }

    async fn peek(&mut self, world: &World, state: usize) {
        let id = state_id(state);
        let what = format!("p{} {} peek {id}", self.id, self.mode_name());
        let ctx = world.begin(&what);
        let result = world.kv.tagged(ctx.op, self.reader().peek(&id)).await;
        let ctx = world.end(ctx, &result);
        let observed = {
            let mut model = world.model.borrow_mut();
            match &result {
                Ok(stored) => model.observe(state, stored, &ctx, &what),
                Err(error) => {
                    model.failed(error, &ctx, &what);
                    None
                }
            }
        };
        if let Some(observed) = observed
            && !observed.refs.is_empty()
        {
            self.read(world, state, observed).await;
        }
    }

    async fn read(&mut self, world: &World, state: usize, observed: Observed) {
        let Observed { attempt, refs } = observed;
        let id = state_id(state);
        let what = format!(
            "p{} {} read_records {id} {refs:?}",
            self.id,
            self.mode_name()
        );
        let ctx = world.begin(&what);
        let result = world
            .kv
            .tagged(ctx.op, self.reader().read_records(&id, &refs))
            .await;
        let ctx = world.end(ctx, &result);
        let mut model = world.model.borrow_mut();
        let values = match &result {
            Ok(values) => values,
            Err(error) => return model.failed(error, &ctx, &what),
        };
        if values.len() != refs.len() {
            return model.wrong(format!(
                "{what}: answered {} of {} records",
                values.len(),
                refs.len()
            ));
        }
        for (key, value) in refs.iter().zip(values) {
            if world.slots {
                judge_slot_value(&mut model, state, attempt, key, value.as_deref(), &what);
            } else {
                judge_content_value(&mut model, key, value.as_deref(), &what);
            }
        }
    }

    async fn owner_of(&mut self, world: &World, state: usize) {
        let id = state_id(state);
        let what = format!("p{} {} owner {id}", self.id, self.mode_name());
        let ctx = world.begin(&what);
        let result = world.kv.tagged(ctx.op, self.reader().owner(&id)).await;
        let ctx = world.end(ctx, &result);
        let mut model = world.model.borrow_mut();
        let floor = model.states[state].fence_floor(ctx.start);
        match (&result, floor) {
            (Ok(Some(token)), Some(floor)) if token.fence() < floor => {
                model.wrong(format!(
                    "{what}: fence {} after fence {floor} was granted",
                    token.fence()
                ));
            }
            (Ok(None), Some(floor)) => {
                model.wrong(format!("{what}: no owner after fence {floor} was granted"));
            }
            (Err(error), _) => model.failed(error, &ctx, &what),
            _ => {}
        }
    }

    async fn list(&mut self, world: &World) {
        let what = format!("p{} {} list_states", self.id, self.mode_name());
        let ctx = world.begin(&what);
        let result = world
            .kv
            .tagged(ctx.op, self.reader().list_states(usize::MAX))
            .await;
        let ctx = world.end(ctx, &result);
        let mut model = world.model.borrow_mut();
        match &result {
            Ok(ids) => judge_listed(&mut model, world.states, ids, &ctx, &what),
            Err(error) => model.failed(error, &ctx, &what),
        }
    }
}

struct Judged<'a> {
    state: usize,
    attempt: usize,
    revision: u64,
    payload: &'a str,
    records: &'a [StoreRecord],
    what: &'a str,
}

fn judge_staged_acquired(
    model: &mut Model,
    staging: &mut Staging,
    state: usize,
    owned: &OwnedState,
    ctx: &Ctx,
    what: &str,
) -> Option<Observed> {
    if ctx.log.cas.iter().any(|cas| cas.swapped) {
        model.wrong(format!(
            "{what}: staged acquisition wrote to the base store"
        ));
    }
    if let Some(revision) = staging.revisions.get(&state) {
        if owned.state.revision != *revision {
            model.wrong(format!(
                "{what}: staged acquisition returned revision {} but revision {revision} is staged",
                owned.state.revision
            ));
        }
        return None;
    }
    let observed = model.observe(state, &owned.state, ctx, what);
    staging
        .bases
        .insert(state, (owned.owner.fence() - 1, owned.state.revision));
    staging.revisions.insert(state, owned.state.revision);
    observed
}

fn judge_plain_acquired(
    model: &mut Model,
    owner: &str,
    state: usize,
    owned: &OwnedState,
    ctx: &Ctx,
    what: &str,
) -> Option<Observed> {
    let fence = owned.owner.fence();
    let prior = model.states[state].fence_floor(ctx.start);
    let reused = model.states[state].fences.iter().any(|f| f.1 == fence);
    if let Some(prior) = prior
        && fence <= prior
    {
        model.wrong(format!(
            "{what}: granted fence {fence} after fence {prior} was granted"
        ));
    } else if reused {
        model.wrong(format!("{what}: granted fence {fence} twice"));
    }
    let expected = version(fence, owned.state.revision, owner);
    if !ctx
        .log
        .cas
        .iter()
        .any(|cas| cas.swapped && cas.new == expected)
    {
        model.wrong(format!(
            "{what}: acknowledged ownership {expected} that was never written"
        ));
    }
    let observed = model.observe(state, &owned.state, ctx, what);
    model.register(ctx);
    observed
}

fn judge_plain_committed(
    model: &mut Model,
    judged: &Judged<'_>,
    held: &Held,
    owner: &str,
    ctx: &Ctx,
) {
    let (state, revision, what) = (judged.state, judged.revision, judged.what);
    let fence = held.token.fence();
    if let Some(newer) = model.states[state].fence_floor(ctx.start)
        && newer > fence
    {
        model.wrong(format!(
            "{what}: fenced-out owner (fence {fence}) committed after fence {newer} was granted"
        ));
    }
    if model.states[state].acked.iter().any(|a| a.1 == revision) {
        model.wrong(format!(
            "{what}: revision {revision} of s{state} was acknowledged twice"
        ));
    }
    let expected = version(fence, revision, owner);
    let landed = ctx
        .log
        .cas
        .iter()
        .find(|cas| cas.swapped && cas.value == judged.payload);
    match landed {
        Some(cas) if cas.new == expected => {
            model.states[state].acked.push((ctx.end, revision, cas.seq))
        }
        _ => model.wrong(format!(
            "{what}: acknowledged but head {expected} was never written"
        )),
    }
    model.attempts[judged.attempt].status = Status::Acked;
    model.stats.acked_commits += 1;
    model.register(ctx);
}

fn judge_commit_failed(
    world: &World,
    judged: &Judged<'_>,
    staged: bool,
    error: &StoreError,
    ctx: &Ctx,
) {
    let refused = conflicting_records(world, judged, error);
    let mut model = world.model.borrow_mut();
    let what = judged.what;
    if refused {
        model.stats.record_conflicts += 1;
    } else {
        model.failed(error, ctx, what);
    }
    let landed = ctx
        .log
        .cas
        .iter()
        .find(|cas| cas.swapped && cas.value == judged.payload);
    if let Some(cas) = landed {
        if definite(error) || refused {
            model.wrong(format!(
                "{what}: reported `{error}` but its head write landed"
            ));
        } else if !cas.after_fault {
            model.wrong(format!(
                "{what}: reported `{error}` although its head write landed without an uncertain fault"
            ));
        }
    }
    let attempt = &mut model.attempts[judged.attempt];
    attempt.status = if staged {
        Status::Failed
    } else if definite(error) || refused {
        attempt.rejected = Some(error.to_string());
        Status::Rejected
    } else {
        Status::Uncertain
    };
    model.register(ctx);
}

fn record_key(state: usize, key: &str) -> String {
    let id = state_id(state);
    format!("record/{}/{id}/{key}", id.len())
}

fn conflicting_records(world: &World, judged: &Judged<'_>, error: &StoreError) -> bool {
    let StoreError::NotCommitted(message) = error else {
        return false;
    };
    world.slots
        && message.contains("conflicting content")
        && world.kv.with(|kv| {
            judged
                .records
                .iter()
                .any(|record| kv.holds_other(&record_key(judged.state, &record.key), &record.value))
        })
}

fn judge_committed_to(world: &World, staging: &Staging, pending: &[usize], ctx: &Ctx, what: &str) {
    let mut model = world.model.borrow_mut();
    let states: BTreeSet<usize> = pending.iter().map(|a| model.attempts[*a].state).collect();
    for state in &states {
        let (base_fence, base_revision) = staging.bases[state];
        let key = format!("state/{}", state_id(*state));
        let acquired = ctx.log.cas.iter().find(|cas| cas.swapped && cas.key == key);
        let from = acquired
            .and_then(|cas| cas.prev.as_deref().and_then(parse_version))
            .map_or((0, 0), |prev| (prev.0, prev.1));
        if acquired.is_none() || from != (base_fence, base_revision) {
            model.wrong(format!(
                "{what}: committed s{state} over head {from:?} although it was staged on fence {base_fence} revision {base_revision}"
            ));
        }
    }
    for attempt in pending {
        acknowledge_staged(&mut model, *attempt, ctx, what);
    }
    model.register(ctx);
}

fn acknowledge_staged(model: &mut Model, attempt: usize, ctx: &Ctx, what: &str) {
    let (state, revision, payload) = {
        let a = &model.attempts[attempt];
        (a.state, a.revision, a.payload.clone())
    };
    let landed = ctx
        .log
        .cas
        .iter()
        .find(|cas| {
            cas.swapped
                && cas.value == payload
                && parse_version(&cas.new).map(|v| v.1) == Some(revision)
        })
        .map(|cas| cas.seq);
    match landed {
        Some(seq) => {
            if model.states[state].acked.iter().any(|a| a.1 == revision) {
                model.wrong(format!("{what}: revision {revision} of s{state} was acknowledged twice"));
            }
            model.states[state].acked.push((ctx.end, revision, seq));
        }
        None => model.wrong(format!(
            "{what}: acknowledged attempt {attempt} (s{state} revision {revision}) that never landed"
        )),
    }
    model.attempts[attempt].status = Status::Acked;
    model.stats.acked_commits += 1;
}

fn judge_commit_to_failed(
    world: &World,
    kv: &SimKv,
    pending: &[usize],
    error: &StoreError,
    ctx: &Ctx,
    what: &str,
) {
    let mut model = world.model.borrow_mut();
    model.failed(error, ctx, what);
    for attempt in pending {
        let payload = model.attempts[*attempt].payload.clone();
        let landed = ctx
            .log
            .cas
            .iter()
            .any(|cas| cas.swapped && cas.value == payload);
        if landed && definite(error) {
            model.stats.partial_commit_to += 1;
            kv.trace(&format!(
                "  partial commit_to: attempt {attempt} landed but reported {error}"
            ));
        }
        model.attempts[*attempt].status = if landed {
            Status::Uncertain
        } else {
            Status::Failed
        };
    }
    model.register(ctx);
}

fn writer_of(value: &str) -> Option<usize> {
    value.strip_prefix('v')?.split('.').next()?.parse().ok()
}

fn judge_slot_value(
    model: &mut Model,
    state: usize,
    head: Option<usize>,
    key: &str,
    value: Option<&str>,
    what: &str,
) {
    let Some(value) = value else {
        if model.damaged {
            model.stats.damaged_reads += 1;
        } else {
            model.wrong(format!(
                "{what}: record {key} referenced by a committed revision is missing"
            ));
        }
        return;
    };
    let writer = writer_of(value).filter(|writer| *writer < model.attempts.len());
    let Some(writer) = writer else {
        return model.wrong(format!("{what}: record {key} read unknown {value:?}"));
    };
    if Some(writer) == head {
        return;
    }
    if model.damaged {
        model.stats.damaged_reads += 1;
        return;
    }
    let attempt = &model.attempts[writer];
    let message = format!(
        "{what}: record {key} of s{state} revision {} (attempt {}) holds the value written by attempt {writer} for s{} revision {}, which is {:?}{}",
        head.map_or(0, |head| model.attempts[head].revision),
        head.map_or_else(|| "none".to_owned(), |head| head.to_string()),
        attempt.state,
        attempt.revision,
        attempt.status,
        attempt
            .rejected
            .as_ref()
            .map_or_else(String::new, |rejected| format!(" ({rejected})")),
    );
    model.wrong(message);
}

fn judge_content_value(model: &mut Model, key: &str, value: Option<&str>, what: &str) {
    let expected = model.records.get(key).cloned();
    match value {
        Some(value) if Some(value) == expected.as_deref() => {}
        Some(value) => model.wrong(format!(
            "{what}: record {key} read {value:?}, expected {expected:?}"
        )),
        None if model.damaged => model.stats.damaged_reads += 1,
        None => model.wrong(format!(
            "{what}: record {key} referenced by a committed revision is missing"
        )),
    }
}

fn judge_listed(model: &mut Model, states: usize, ids: &[String], ctx: &Ctx, what: &str) {
    for id in ids {
        let known = id
            .strip_prefix('s')
            .and_then(|n| n.parse::<usize>().ok())
            .is_some_and(|n| n < states);
        if !known {
            model.wrong(format!("{what}: listed unknown state {id:?}"));
        }
    }
    for state in 0..states {
        if model.states[state].fence_floor(ctx.start).is_some() && !ids.contains(&state_id(state)) {
            model.wrong(format!("{what}: s{state} has an owner but was not listed"));
        }
    }
}

pub async fn reopen_check(world: &World, label: &str) {
    world.kv.with(|kv| kv.faults_on = false);
    let mut checker = Proc::checker(world, label);
    for state in 0..world.states {
        let id = state_id(state);
        let what = format!("check {label} peek {id}");
        let ctx = world.begin(&what);
        let result = world.kv.tagged(ctx.op, checker.reader().peek(&id)).await;
        let ctx = world.end(ctx, &result);
        let observed = {
            let mut model = world.model.borrow_mut();
            match &result {
                Ok(stored) => {
                    let most = model.max_attempted(state);
                    if stored.revision > most {
                        model.wrong(format!(
                            "{what}: reopened at revision {} beyond the last attempted revision {most}",
                            stored.revision
                        ));
                    }
                    model.observe(state, stored, &ctx, &what)
                }
                Err(error) => {
                    model.wrong(format!("{what}: failed with `{error}` with faults off"));
                    None
                }
            }
        };
        if let Some(observed) = observed
            && !observed.refs.is_empty()
        {
            checker.read(world, state, observed).await;
        }
        checker.owner_of(world, state).await;
    }
    checker.list(world).await;
    world.kv.with(|kv| kv.faults_on = true);
}
