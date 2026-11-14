use std::{
    collections::{BTreeMap, BTreeSet},
    future::Future,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

use futures_util::FutureExt;
use nanocode::{
    backend::Prompt,
    session_host::{Submission, TurnState},
    turnlog::{LogHost, OneLog, OneLogStore, Policy},
};
use nanocode_code_mode::code_mode::{
    CELL_JOURNAL_VERSION, CellJournal, CellJournalHeader, CellJournalSink, CellJournalUpdate,
    JournalEntry, JournaledCall, JournaledCallState,
};
use nanocode_durability_core::{
    OwnerId, OwnerToken, Settle, StateCommit, StateStore, StoreError, StoreFuture, SummaryUpdate,
};
use nanocode_sync::Atom;
use serde_json::{Map, Value, json, value::to_raw_value};

use crate::{
    executor::Clock,
    onelog::RESTART_CAP,
    rng::{FNV_OFFSET, PPM, Rng, fnv},
    turns::{
        Invariant, SESSION_TURNS_MAX, STEP_LATENCY_MS_MAX, STEP_LATENCY_MS_MIN,
        TOOL_LATENCY_MS_MAX, TOOL_LATENCY_MS_MIN, TurnOptions,
    },
};

const CHARGE: &str = "charge";
const CRASHES_MAX: u64 = RESTART_CAP - 1;
const READ: &str = "Read";
const SCOPE: &str = "cells";
const SESSION: &str = "cells-session";
const STATE: &str = "cells-state";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Sync,
    Timer,
    External,
    ReadOnly,
}

impl Kind {
    const ALL: [Self; 4] = [Self::Sync, Self::Timer, Self::External, Self::ReadOnly];

    const fn rows(self) -> u64 {
        match self {
            Self::Sync | Self::ReadOnly => 2,
            Self::Timer | Self::External => 3,
        }
    }

    const fn tool(self) -> &'static str {
        match self {
            Self::External => CHARGE,
            Self::ReadOnly => READ,
            Self::Sync | Self::Timer => "",
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct CellStats {
    pub charges: u64,
    pub clean: [u64; 4],
    pub crashes: u64,
    pub opens: u64,
    pub reasks: u64,
    pub rows: [u64; 4],
    pub turns: u64,
    pub unknown: u64,
}

impl CellStats {
    pub const fn add(&mut self, other: &Self) {
        self.charges += other.charges;
        self.crashes += other.crashes;
        self.opens += other.opens;
        self.reasks += other.reasks;
        self.turns += other.turns;
        self.unknown += other.unknown;
        let mut kind = 0;
        while kind < Kind::ALL.len() {
            self.clean[kind] += other.clean[kind];
            self.rows[kind] += other.rows[kind];
            kind += 1;
        }
    }

    pub fn summary(&self) -> String {
        let per_turn = |kind: Kind| {
            let index = kind as usize;
            if self.clean[index] == 0 {
                return "-".to_owned();
            }
            let hundredths = self.rows[index] * 100 / self.clean[index];
            format!(
                "{}.{:02}/{}",
                hundredths / 100,
                hundredths % 100,
                self.clean[index]
            )
        };
        format!(
            "cell_turns={} cell_crashes={} cell_opens={} cell_reasks={} cell_unknown={} cell_charges={} cell_rows/clean_turn sync={} timer={} external={} read={}",
            self.turns,
            self.crashes,
            self.opens,
            self.reasks,
            self.unknown,
            self.charges,
            per_turn(Kind::Sync),
            per_turn(Kind::Timer),
            per_turn(Kind::External),
            per_turn(Kind::ReadOnly)
        )
    }
}

pub struct CellOutcome {
    pub digest: u64,
    pub lines: u64,
    pub shape: String,
    pub stats: CellStats,
    pub wrong: Option<(Invariant, String)>,
}

#[derive(Default)]
struct CellHost {
    appends: AtomicU64,
    armed: AtomicBool,
    dead: AtomicBool,
    rows: Atom<BTreeMap<u64, String>>,
}

impl CellHost {
    fn kill(&self) {
        self.armed.store(false, Ordering::SeqCst);
        self.dead.store(true, Ordering::SeqCst);
    }

    fn revive(&self) {
        self.armed.store(false, Ordering::SeqCst);
        self.dead.store(false, Ordering::SeqCst);
    }

    fn dead(&self) -> bool {
        self.dead.load(Ordering::SeqCst)
    }

    fn appends(&self) -> u64 {
        self.appends.load(Ordering::SeqCst)
    }
}

impl LogHost for CellHost {
    fn scan<'a>(
        &'a self,
        _scope: &'a str,
        from: u64,
        to: u64,
    ) -> StoreFuture<'a, Result<Vec<(u64, String)>, StoreError>> {
        let rows = self.rows.read(|rows| {
            rows.range(from..to)
                .map(|(seq, body)| (*seq, body.clone()))
                .collect()
        });
        Box::pin(async move { Ok(rows) })
    }

    fn load<'a>(
        &'a self,
        _scope: &'a str,
    ) -> StoreFuture<'a, Result<Vec<(u64, String)>, StoreError>> {
        let rows = self.rows.read(|rows| {
            let start = rows
                .iter()
                .rev()
                .find(|(_, body)| body.starts_with('s'))
                .map_or(0, |(seq, _)| *seq);
            rows.range(start..)
                .map(|(seq, body)| (*seq, body.clone()))
                .collect()
        });
        Box::pin(async move { Ok(rows) })
    }

    fn append<'a>(
        &'a self,
        _scope: &'a str,
        rows: &'a [(u64, String)],
    ) -> StoreFuture<'a, Result<(), StoreError>> {
        let refused = self.dead() || self.armed.swap(false, Ordering::SeqCst);
        if refused {
            self.kill();
            return Box::pin(async {
                Err(StoreError::Backend(
                    "the simulated host died before this row landed".to_owned(),
                ))
            });
        }
        let taken = self.rows.update(|stored| {
            if rows.iter().any(|(seq, _)| stored.contains_key(seq)) {
                return true;
            }
            stored.extend(rows.iter().cloned());
            false
        });
        if !taken {
            self.appends.fetch_add(1, Ordering::SeqCst);
        }
        Box::pin(async move {
            if taken {
                Err(StoreError::Fenced)
            } else {
                Ok(())
            }
        })
    }
}

struct Turn {
    delay_ms: u64,
    id: String,
    kind: Kind,
    split: bool,
}

#[derive(Clone, Debug)]
enum Op {
    Ask { cell: u64, step: u64 },
    Call,
    Finish,
    Fire,
    Forget(u64),
    Save(CellJournalUpdate),
    Start,
    Submit,
    Unknown(CellJournalUpdate),
}

impl Op {
    fn describe(&self) -> String {
        match self {
            Self::Save(update) | Self::Unknown(update) => format!(
                "{} cell={} seq={} header={} entries={} calls={} finished={}",
                if matches!(self, Self::Unknown(_)) {
                    "unknown"
                } else {
                    "save"
                },
                update.cell_id,
                update.seq,
                update.header.is_some(),
                update.entries.len(),
                update.calls.len(),
                update.finished
            ),
            other => format!("{other:?}"),
        }
    }
}

struct Writer {
    cell: u64,
    entries: usize,
    seq: u64,
}

impl Writer {
    fn update(
        &mut self,
        header: Option<CellJournalHeader>,
        entries: Vec<JournalEntry>,
        calls: Vec<JournaledCall>,
        finished: bool,
    ) -> CellJournalUpdate {
        let update = CellJournalUpdate {
            session_id: SESSION.to_owned(),
            cell_id: self.cell,
            seq: self.seq,
            header,
            entries_from: self.entries,
            entries,
            calls: calls.into_iter().map(Arc::new).collect(),
            observer_call_id: None,
            observer_yield_ms: None,
            observer_since_ms: None,
            content_returned: 0,
            finished,
        };
        self.seq += 1;
        self.entries += update.entries.len();
        update
    }

    fn save(
        &mut self,
        header: Option<CellJournalHeader>,
        entries: Vec<JournalEntry>,
        calls: Vec<JournaledCall>,
        finished: bool,
    ) -> Op {
        Op::Save(self.update(header, entries, calls, finished))
    }
}

struct Live {
    log: Arc<OneLog>,
    owner: OwnerToken,
    sink: Arc<dyn CellJournalSink>,
    store: OneLogStore,
}

struct Cells {
    asks: BTreeMap<String, u64>,
    charges: BTreeMap<String, u64>,
    digest: u64,
    finished: BTreeSet<String>,
    host: Arc<CellHost>,
    lines: u64,
    live: Option<Live>,
    next_cell: u64,
    options: TurnOptions,
    rng: Rng,
    started: BTreeSet<(String, u64)>,
    stats: CellStats,
    unknown: BTreeSet<String>,
    wrong: Option<(Invariant, String)>,
    yield_ms: u64,
}

fn ready<T>(future: impl Future<Output = T>) -> T {
    future
        .now_or_never()
        .expect("the simulated cell host answers every call at once")
}

fn refused<T, E: std::fmt::Display>(result: Result<T, E>) -> T {
    result.unwrap_or_else(|error| panic!("the one log refused a live host: {error}"))
}

fn origin(turn: &Turn, cell: u64) -> String {
    format!("{}/cell-{cell}", turn.id)
}

fn call(turn: &Turn, cell: u64, state: JournaledCallState) -> JournaledCall {
    JournaledCall {
        id: 0,
        call_id: format!("{}/code-0", origin(turn, cell)),
        name: turn.kind.tool().to_owned(),
        input: json!({ "turn": turn.id }),
        host: false,
        state,
    }
}

fn call_entry(turn: &Turn) -> JournalEntry {
    JournalEntry::Call {
        id: 0,
        name: turn.kind.tool().to_owned(),
        input: json!({ "turn": turn.id }),
        host: false,
    }
}

fn completed(turn: &Turn, cell: u64, encoded: &str) -> (JournalEntry, JournaledCall) {
    (
        JournalEntry::Result {
            id: 0,
            encoded: encoded.to_owned(),
            success: true,
        },
        call(
            turn,
            cell,
            JournaledCallState::Completed {
                encoded: encoded.to_owned(),
                success: true,
            },
        ),
    )
}

const fn timeout(turn: &Turn) -> JournalEntry {
    JournalEntry::Timeout {
        id: 0,
        delay_ms: turn.delay_ms,
        at_ms: turn.delay_ms,
    }
}

fn waits(update: &CellJournalUpdate) -> bool {
    update
        .entries
        .iter()
        .any(|entry| matches!(entry, JournalEntry::Timeout { .. }))
        || update
            .calls
            .iter()
            .any(|call| call.name == CHARGE && matches!(call.state, JournaledCallState::Running))
}

impl Cells {
    fn trace(&mut self, line: &str) {
        self.digest = fnv(fnv(self.digest, line.as_bytes()), b"\n");
        self.lines += 1;
        if self.options.verbose {
            println!("    cells {line}");
        }
    }

    fn wrong(&mut self, invariant: Invariant, message: String) {
        self.trace(&format!("WRONG {}: {message}", invariant.name()));
        if self.wrong.is_none() {
            self.wrong = Some((invariant, message));
        }
    }

    fn live(&mut self) -> &mut Live {
        if self.live.is_none() {
            let policy = Policy {
                max_row_bytes: self.options.row_limit,
                restarts: RESTART_CAP,
                retain_finished: self.options.retain,
                snapshot_ratio: self.options.snapshot_ratio,
                ..Policy::default()
            };
            let host: Arc<dyn LogHost> = Arc::clone(&self.host) as Arc<dyn LogHost>;
            let log = refused(ready(OneLog::open(host, SCOPE, policy)));
            drop(refused(ready(log.journals())));
            let mut store = log.store();
            let owner = refused(ready(store.acquire(STATE, OwnerId::new()))).owner;
            let sink = log.sink();
            self.stats.opens += 1;
            self.live = Some(Live {
                log,
                owner,
                sink,
                store,
            });
            let rows = self.host.rows.read(BTreeMap::len);
            self.trace(&format!("open rows={rows}"));
        }
        self.live
            .as_mut()
            .expect("the live log was opened just above")
    }

    fn header(&self, turn: &Turn, cell: u64) -> CellJournalHeader {
        CellJournalHeader {
            version: CELL_JOURNAL_VERSION,
            origin_call_id: origin(turn, cell),
            script: format!("{:?} cell of {}", turn.kind, turn.id),
            tools: vec![json!({ "name": CHARGE }), json!({ "name": READ })].into(),
            stored: BTreeMap::new(),
            prelude: false,
            yield_time_ms: self.yield_ms,
            max_output_tokens: None,
        }
    }

    fn ask_fresh(&mut self, turn: &Turn) -> Vec<Op> {
        let cell = self.next_cell;
        self.next_cell += 1;
        let mut ops = vec![Op::Ask { cell, step: 1 }];
        ops.extend(self.fresh(turn, cell));
        ops
    }

    fn fresh(&self, turn: &Turn, cell: u64) -> Vec<Op> {
        let header = Some(self.header(turn, cell));
        let mut writer = Writer {
            cell,
            entries: 0,
            seq: 0,
        };
        let mut ops = Vec::new();
        match turn.kind {
            Kind::Sync => {
                ops.push(writer.save(header, Vec::new(), Vec::new(), false));
                ops.push(writer.save(None, Vec::new(), Vec::new(), true));
            }
            Kind::Timer => {
                if turn.split {
                    ops.push(writer.save(header, Vec::new(), Vec::new(), false));
                    ops.push(writer.save(None, vec![timeout(turn)], Vec::new(), false));
                } else {
                    ops.push(writer.save(header, vec![timeout(turn)], Vec::new(), false));
                }
                ops.push(Op::Fire);
                ops.push(writer.save(None, vec![JournalEntry::Timer { id: 0 }], Vec::new(), true));
            }
            Kind::External | Kind::ReadOnly => {
                let running = call(turn, cell, JournaledCallState::Running);
                if turn.split {
                    ops.push(writer.save(header, Vec::new(), Vec::new(), false));
                    ops.push(writer.save(None, vec![call_entry(turn)], vec![running], false));
                } else {
                    ops.push(writer.save(header, vec![call_entry(turn)], vec![running], false));
                }
                ops.push(Op::Call);
                let (entry, done) = completed(turn, cell, "\"ok\"");
                ops.push(writer.save(None, vec![entry], vec![done], false));
                ops.push(writer.save(None, Vec::new(), Vec::new(), true));
            }
        }
        ops.extend(Self::close(cell));
        ops
    }

    const fn close(cell: u64) -> [Op; 3] {
        [Op::Ask { cell, step: 2 }, Op::Forget(cell), Op::Finish]
    }

    fn resume(turn: &Turn, cell: u64, journal: &CellJournal, saved: u64) -> Vec<Op> {
        let mut writer = Writer {
            cell,
            entries: journal.entries.len(),
            seq: saved,
        };
        let has = |matches: fn(&JournalEntry) -> bool| journal.entries.iter().any(matches);
        let mut ops = Vec::new();
        if !journal.finished {
            match turn.kind {
                Kind::Sync => ops.push(writer.save(None, Vec::new(), Vec::new(), true)),
                Kind::Timer => {
                    if !has(|entry| matches!(entry, JournalEntry::Timeout { .. })) {
                        ops.push(writer.save(None, vec![timeout(turn)], Vec::new(), false));
                    }
                    if has(|entry| matches!(entry, JournalEntry::Timer { .. })) {
                        ops.push(writer.save(None, Vec::new(), Vec::new(), true));
                    } else {
                        ops.push(Op::Fire);
                        ops.push(writer.save(
                            None,
                            vec![JournalEntry::Timer { id: 0 }],
                            Vec::new(),
                            true,
                        ));
                    }
                }
                Kind::External | Kind::ReadOnly => {
                    let running = journal.unknown_calls().next().is_some();
                    if has(|entry| matches!(entry, JournalEntry::Result { .. })) {
                        ops.push(writer.save(None, Vec::new(), Vec::new(), true));
                    } else if running && turn.kind == Kind::External {
                        let (entry, done) = completed(turn, cell, "\"unknown\"");
                        ops.push(Op::Unknown(writer.update(
                            None,
                            vec![entry],
                            vec![done],
                            false,
                        )));
                        ops.push(writer.save(None, Vec::new(), Vec::new(), true));
                    } else {
                        let entries = if has(|entry| matches!(entry, JournalEntry::Call { .. })) {
                            Vec::new()
                        } else {
                            vec![call_entry(turn)]
                        };
                        let calls = vec![call(turn, cell, JournaledCallState::Running)];
                        ops.push(writer.save(None, entries, calls, false));
                        ops.push(Op::Call);
                        let (entry, done) = completed(turn, cell, "\"ok\"");
                        ops.push(writer.save(None, vec![entry], vec![done], false));
                        ops.push(writer.save(None, Vec::new(), Vec::new(), true));
                    }
                }
            }
        }
        ops.extend(Self::close(cell));
        ops
    }

    fn checkpoint(&mut self, turn: &Turn) -> Option<(u64, u64)> {
        let live = self.live();
        let state = refused(ready(live.store.peek(STATE)));
        let payload: Value = serde_json::from_str(state.payload.as_deref()?)
            .expect("the simulated checkpoint is JSON");
        (payload["turn"] == turn.id.as_str()).then(|| {
            (
                payload["steps"].as_u64().expect("a checkpoint has steps"),
                payload["cell"].as_u64().expect("a checkpoint has its cell"),
            )
        })
    }

    fn journal(&mut self, cell: u64) -> Option<(CellJournal, u64)> {
        let live = self.live();
        let journals = refused(ready(live.sink.load(SESSION)));
        let saved = ready(live.log.read(|fold| {
            fold.cells()
                .find(|(session, id, _)| *session == SESSION && *id == cell)
                .map(|(_, _, updates)| updates.len())
        }));
        let journal = journals
            .into_iter()
            .find(|journal| journal.cell_id == cell)?;
        let saved = u64::try_from(saved.expect("a loaded journal has updates in the fold"))
            .expect("an update count fits in u64");
        Some((journal, saved))
    }

    fn plan(&mut self, turn: &Turn) -> Option<Vec<Op>> {
        if self.finished.contains(&turn.id) {
            return Some(Vec::new());
        }
        let live = self.live();
        let record = refused(ready(live.log.turn(&turn.id)));
        let Some(record) = record else {
            let mut ops = vec![Op::Submit, Op::Start];
            ops.extend(self.ask_fresh(turn));
            return Some(ops);
        };
        match record.state {
            TurnState::Completed => return Some(Vec::new()),
            TurnState::Cancelled | TurnState::Failed => {
                self.wrong(
                    Invariant::Liveness,
                    format!("turn {} ended {}", turn.id, record.state.as_str()),
                );
                return None;
            }
            TurnState::Queued | TurnState::Running => {}
        }
        let mut ops = vec![Op::Start];
        match self.checkpoint(turn) {
            None => ops.extend(self.ask_fresh(turn)),
            Some((1, cell)) => {
                let Some((journal, saved)) = self.journal(cell) else {
                    if self.started.contains(&(turn.id.clone(), cell)) {
                        self.wrong(
                            Invariant::CellLost,
                            format!(
                                "turn {} lost the journal of cell {cell}, which waited when the host died",
                                turn.id
                            ),
                        );
                        return None;
                    }
                    self.trace(&format!("rerun cell={cell}"));
                    ops.extend(self.fresh(turn, cell));
                    return Some(ops);
                };
                self.trace(&format!(
                    "journal cell={cell} saved={saved} entries={} calls={} finished={}",
                    journal.entries.len(),
                    journal.calls.len(),
                    journal.finished
                ));
                ops.extend(Self::resume(turn, cell, &journal, saved));
            }
            Some((_, cell)) => {
                if self.journal(cell).is_some() {
                    ops.push(Op::Forget(cell));
                }
                ops.push(Op::Finish);
            }
        }
        Some(ops)
    }

    fn settled<T, E: std::fmt::Display>(&self, result: Result<T, E>) -> Option<T> {
        match result {
            Ok(value) => Some(value),
            Err(_) if self.host.dead() => None,
            Err(error) => panic!("the one log refused a live host: {error}"),
        }
    }

    fn ask(&mut self, turn: &Turn, cell: u64, step: u64) {
        if step == 1 {
            let asks = self.asks.entry(turn.id.clone()).or_default();
            *asks += 1;
            if *asks > 1 {
                self.stats.reasks += 1;
                if self.started.iter().any(|(id, _)| *id == turn.id) {
                    self.wrong(
                        Invariant::CellModelOnce,
                        format!(
                            "turn {} asked the model again for the step whose cell waited when the host died",
                            turn.id
                        ),
                    );
                    return;
                }
            }
        }
        let payload = json!({ "cell": cell, "steps": step, "turn": turn.id }).to_string();
        let live = self.live();
        let revision = refused(ready(live.store.peek(STATE))).revision;
        let commit = StateCommit {
            payload: &payload,
            records: &[],
            events: &[],
            summary: &SummaryUpdate::Unchanged,
            settle: Settle::Durable,
        };
        let result = ready(live.store.commit(STATE, &live.owner, revision, commit));
        self.settled(result);
    }

    fn save(&mut self, turn: &Turn, update: &CellJournalUpdate) {
        let live = self.live();
        let result = ready(live.sink.save(update.clone()));
        if self.settled(result).is_some() && waits(update) {
            self.started.insert((turn.id.clone(), update.cell_id));
        }
    }

    fn run_op(&mut self, turn: &Turn, op: &Op) {
        match op {
            Op::Submit => {
                let submission = Submission {
                    request_id: turn.id.clone(),
                    prompt: Prompt::new(format!("{:?} cell", turn.kind)),
                    hand: None,
                    metadata: to_raw_value(&Value::Null).expect("null encodes as JSON"),
                };
                let live = self.live();
                let result = ready(live.log.submit(&submission));
                self.settled(result);
            }
            Op::Start => {
                let live = self.live();
                let result = ready(live.log.start(&turn.id));
                self.settled(result);
            }
            Op::Ask { cell, step } => self.ask(turn, *cell, *step),
            Op::Save(update) => self.save(turn, update),
            Op::Unknown(update) => {
                self.stats.unknown += 1;
                self.unknown.insert(turn.id.clone());
                self.save(turn, update);
            }
            Op::Fire => {}
            Op::Call => {
                if turn.kind == Kind::External {
                    self.stats.charges += 1;
                    let charges = self.charges.entry(turn.id.clone()).or_default();
                    *charges += 1;
                    if *charges > 1 {
                        let charges = *charges;
                        self.wrong(
                            Invariant::CellExternalOnce,
                            format!("turn {} charged {charges} times", turn.id),
                        );
                    }
                }
            }
            Op::Forget(cell) => {
                let live = self.live();
                ready(live.sink.forget(SESSION, *cell));
            }
            Op::Finish => {
                let live = self.live();
                let result = ready(live.log.finish(&turn.id, TurnState::Completed, &Map::new()));
                if self.settled(result).flatten().is_some() {
                    self.finished.insert(turn.id.clone());
                }
            }
        }
    }

    fn turn(&mut self, turn: &Turn) {
        let mut crashes = self.rng.range(0, CRASHES_MAX);
        let clean = crashes == 0;
        let before = self.host.appends();
        self.stats.turns += 1;
        self.trace(&format!(
            "turn {} kind={:?} split={} crashes={crashes}",
            turn.id, turn.kind, turn.split
        ));
        loop {
            let Some(ops) = self.plan(turn) else {
                return;
            };
            if ops.is_empty() {
                break;
            }
            let crash = (crashes > 0).then(|| {
                crashes -= 1;
                let point = self.rng.below(2 * ops.len() as u64);
                (
                    usize::try_from(point / 2).expect("an op index fits in usize"),
                    point.is_multiple_of(2),
                )
            });
            self.trace(&format!(
                "plan ops={} crash={crash:?}: {}",
                ops.len(),
                ops.iter().map(Op::describe).collect::<Vec<_>>().join(", ")
            ));
            let mut crashed = false;
            for (index, op) in ops.iter().enumerate() {
                if crash == Some((index, true)) {
                    self.host.armed.store(true, Ordering::SeqCst);
                }
                self.run_op(turn, op);
                if self.wrong.is_some() {
                    return;
                }
                if self.host.dead() || crash.is_some_and(|(at, _)| at == index) {
                    let landed = !self.host.dead();
                    self.host.kill();
                    self.trace(&format!(
                        "crash at op {index} {} landed={landed} rows={}",
                        op.describe(),
                        self.host.appends()
                    ));
                    crashed = true;
                    break;
                }
            }
            if crashed {
                self.stats.crashes += 1;
                self.live = None;
                self.host.revive();
            }
        }
        if turn.kind == Kind::External
            && self.charges.get(&turn.id).copied().unwrap_or(0) == 0
            && !self.unknown.contains(&turn.id)
        {
            self.wrong(
                Invariant::CellExternalOnce,
                format!(
                    "turn {} completed with no charge and no unknown outcome",
                    turn.id
                ),
            );
            return;
        }
        if clean {
            let rows = self.host.appends() - before;
            let index = turn.kind as usize;
            self.stats.clean[index] += 1;
            self.stats.rows[index] += rows;
            if rows != turn.kind.rows() {
                self.wrong(
                    Invariant::CellRows,
                    format!(
                        "turn {} with a {:?} cell wrote {rows} rows, not {}",
                        turn.id,
                        turn.kind,
                        turn.kind.rows()
                    ),
                );
            }
        }
    }
}

pub fn run(seed: u64, options: TurnOptions) -> CellOutcome {
    let clock = Clock::new();
    crate::host::enter(seed, &clock);
    let mut rng = Rng::derive(seed, "cells");
    let count = rng.range(1, SESSION_TURNS_MAX);
    let yield_ms = rng.range(STEP_LATENCY_MS_MIN, STEP_LATENCY_MS_MAX);
    let turns: Vec<Turn> = (0..count)
        .map(|index| Turn {
            delay_ms: rng.range(TOOL_LATENCY_MS_MIN, TOOL_LATENCY_MS_MAX),
            id: format!("cell-turn-{index}"),
            kind: Kind::ALL[rng.pick(Kind::ALL.len())],
            split: rng.chance(PPM / 2),
        })
        .collect();
    let shape = format!(
        "cells=[{}]",
        turns
            .iter()
            .map(|turn| format!("{:?}{}", turn.kind, if turn.split { "/split" } else { "" }))
            .collect::<Vec<_>>()
            .join(",")
    );
    let mut cells = Cells {
        asks: BTreeMap::new(),
        charges: BTreeMap::new(),
        digest: FNV_OFFSET,
        finished: BTreeSet::new(),
        host: Arc::new(CellHost::default()),
        lines: 0,
        live: None,
        next_cell: 0,
        options,
        rng,
        started: BTreeSet::new(),
        stats: CellStats::default(),
        unknown: BTreeSet::new(),
        wrong: None,
        yield_ms,
    };
    for turn in &turns {
        cells.turn(turn);
        if cells.wrong.is_some() {
            break;
        }
    }
    let rows = cells.host.rows.read(|rows| {
        rows.values()
            .fold(FNV_OFFSET, |digest, body| fnv(digest, body.as_bytes()))
    });
    let count = cells.host.appends();
    cells.trace(&format!("end rows={count} disk={rows:016x}"));
    CellOutcome {
        digest: cells.digest,
        lines: cells.lines,
        shape,
        stats: cells.stats,
        wrong: cells.wrong,
    }
}
