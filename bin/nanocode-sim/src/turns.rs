use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use crate::{
    disk::Disk,
    faults::Profile,
    onelog::{
        Call, Class, Decision, EventId, Kind, Log, OneLog, Origin, Outcome, Phase, Point,
        Protections, Record, Reply, Row, Scan, ScopeState, Status, Tool, TurnDurability, decode,
    },
    rng::{FNV_OFFSET, Rng, fnv},
    turnlog::TurnLogPolicy,
};

pub const SESSIONS_MAX: u64 = 3;
pub const SESSION_TURNS_MAX: u64 = 3;
pub const CLIENTLESS_MAX: u64 = 2;
pub const TURN_STEPS_MAX: u64 = 4;
pub const CALLS_MAX: u64 = 3;
pub const STEP_LATENCY_MS_MIN: u64 = 50;
pub const STEP_LATENCY_MS_MAX: u64 = 20_000;
pub const LONG_CALL_MS: u64 = 5_000;
pub const DEPLOY_PPM: u64 = 200_000;
pub const DEPLOYS_MAX: u64 = 2;
pub const TOOL_LATENCY_MS_MIN: u64 = 5;
pub const TOOL_LATENCY_MS_MAX: u64 = 3_000;
pub const NET_MS_MIN: u64 = 1;
pub const NET_MS_MAX: u64 = 80;
pub const RESTART_MS: u64 = 1_000;
pub const RECONNECT_MS: u64 = 500;
pub const ACK_TIMEOUT_MS: u64 = 5_000;
pub const REDELIVER_MS: u64 = 5_000;
pub const BUSY_RETRY_MS: u64 = 1_000;
pub const THINK_MS_MAX: u64 = 3_000;
pub const CANCEL_PPM: u64 = 150_000;
pub const POISON_PPM: u64 = 80_000;
pub const CRASH_BEFORE_COMMIT_PPM_MAX: u64 = 80_000;
pub const CRASH_AFTER_COMMIT_PPM_MAX: u64 = 40_000;
pub const TIMED_CRASHES_MAX: u64 = 3;
pub const LIVENESS_RESTARTS: u64 = 16;
pub const TICKS_MAX: u64 = 200_000;
const TOOLS: [Tool; 5] = [
    Tool::Charge,
    Tool::Hand,
    Tool::Read,
    Tool::Write,
    Tool::Spawn,
];
const CHILD_TOOLS: usize = 4;
const ORIGINS: [Origin; 2] = [Origin::Alarm, Origin::Completion];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TurnMutation {
    None,
    DurableSeqBeforeCommit,
    FinishBeforeRow,
    GateReleasesEarly,
    IdempotentReportedUnknown,
    RecoverySkipsClientless,
    SealSkipped,
    TickNotAtomic,
}

impl TurnMutation {
    const ALL: [Self; 8] = [
        Self::None,
        Self::DurableSeqBeforeCommit,
        Self::FinishBeforeRow,
        Self::GateReleasesEarly,
        Self::IdempotentReportedUnknown,
        Self::RecoverySkipsClientless,
        Self::SealSkipped,
        Self::TickNotAtomic,
    ];

    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|mutation| mutation.name() == name)
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::DurableSeqBeforeCommit => "durable_seq_before_commit",
            Self::FinishBeforeRow => "finish_before_row",
            Self::GateReleasesEarly => "gate_releases_early",
            Self::IdempotentReportedUnknown => "idempotent_reported_unknown",
            Self::RecoverySkipsClientless => "recovery_skips_clientless",
            Self::SealSkipped => "seal_skipped",
            Self::TickNotAtomic => "tick_not_atomic",
        }
    }

    pub fn names() -> String {
        Self::ALL.map(Self::name).join("|")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Invariant {
    ExternalOnce,
    FinishOnce,
    IdempotentKnown,
    InObjectOnce,
    Liveness,
    Panic,
    SeqUnique,
}

impl Invariant {
    pub const fn name(self) -> &'static str {
        match self {
            Self::ExternalOnce => "external_once",
            Self::FinishOnce => "finish_once",
            Self::IdempotentKnown => "idempotent_known",
            Self::InObjectOnce => "in_object_once",
            Self::Liveness => "liveness",
            Self::Panic => "panic",
            Self::SeqUnique => "seq_unique",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct TurnOptions {
    pub calm: bool,
    pub mutation: TurnMutation,
    pub policy: PolicyKind,
    pub row_limit: Option<usize>,
    pub protections: Protections,
    pub verbose: bool,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct TurnStats {
    pub seeds: u64,
    pub turns: u64,
    pub finished: u64,
    pub failed: u64,
    pub cancelled: u64,
    pub capped: u64,
    pub capped_clean: u64,
    pub lost: u64,
    pub rows_admission: u64,
    pub rows_call: u64,
    pub rows_result: u64,
    pub rows_restart: u64,
    pub rows_finish: u64,
    pub rows_snapshot: u64,
    pub rows_floor: u64,
    pub rows_progress: u64,
    pub rows_part: u64,
    pub rows_appended: u64,
    pub brief_rows: u64,
    pub bytes: u64,
    pub cold_reads: u64,
    pub model_calls: u64,
    pub replayed: u64,
    pub starts: u64,
    pub crashes: u64,
    pub poison_resets: u64,
    pub deploys: u64,
    pub commit_faults: u64,
    pub read_faults: u64,
    pub recoveries: u64,
    pub unknown: u64,
    pub unknown_completed: u64,
    pub idempotent_resent: u64,
    pub idempotent_reapplied: u64,
    pub external_applied: u64,
}

impl TurnStats {
    pub const fn add(&mut self, other: &Self) {
        self.seeds += other.seeds;
        self.turns += other.turns;
        self.finished += other.finished;
        self.failed += other.failed;
        self.cancelled += other.cancelled;
        self.capped += other.capped;
        self.capped_clean += other.capped_clean;
        self.lost += other.lost;
        self.rows_admission += other.rows_admission;
        self.rows_call += other.rows_call;
        self.rows_result += other.rows_result;
        self.rows_restart += other.rows_restart;
        self.rows_finish += other.rows_finish;
        self.rows_snapshot += other.rows_snapshot;
        self.rows_floor += other.rows_floor;
        self.rows_progress += other.rows_progress;
        self.rows_part += other.rows_part;
        self.rows_appended += other.rows_appended;
        self.brief_rows += other.brief_rows;
        self.bytes += other.bytes;
        self.cold_reads += other.cold_reads;
        self.model_calls += other.model_calls;
        self.replayed += other.replayed;
        self.starts += other.starts;
        self.crashes += other.crashes;
        self.poison_resets += other.poison_resets;
        self.deploys += other.deploys;
        self.commit_faults += other.commit_faults;
        self.read_faults += other.read_faults;
        self.recoveries += other.recoveries;
        self.unknown += other.unknown;
        self.unknown_completed += other.unknown_completed;
        self.idempotent_resent += other.idempotent_resent;
        self.idempotent_reapplied += other.idempotent_reapplied;
        self.external_applied += other.external_applied;
    }

    pub const fn rows(&self) -> u64 {
        self.rows_admission
            + self.rows_call
            + self.rows_result
            + self.rows_restart
            + self.rows_finish
            + self.rows_snapshot
            + self.rows_floor
            + self.rows_progress
            + self.rows_part
    }

    pub fn summary(&self) -> String {
        let per_turn = |count: u64| {
            if self.turns == 0 {
                return "0".to_owned();
            }
            let hundredths = count * 100 / self.turns;
            format!("{}.{:02}", hundredths / 100, hundredths % 100)
        };
        format!(
            "rows/turn={} (A={} C={} T={} R={} F={} S={} L={} P={} X={}) appended/turn={} brief_rows/turn={} model_calls/turn={} replayed/turn={} unknown_completed={} capped={} capped_clean={} lost={}",
            per_turn(self.rows()),
            per_turn(self.rows_admission),
            per_turn(self.rows_call),
            per_turn(self.rows_result),
            per_turn(self.rows_restart),
            per_turn(self.rows_finish),
            per_turn(self.rows_snapshot),
            per_turn(self.rows_floor),
            per_turn(self.rows_progress),
            per_turn(self.rows_part),
            per_turn(self.rows_appended),
            per_turn(self.brief_rows),
            per_turn(self.model_calls),
            per_turn(self.replayed),
            self.unknown_completed,
            self.capped,
            self.capped_clean,
            self.lost
        )
    }
}

pub struct TurnOutcome {
    pub digest: u64,
    pub lines: u64,
    pub shape: String,
    pub stats: TurnStats,
    pub wrong: Option<(Invariant, String)>,
}

struct Plan {
    poison: Option<u64>,
    steps: Vec<Vec<Tool>>,
}

fn plan(seed: u64, request: &str, child: bool, calm: bool) -> Plan {
    let mut rng = Rng::derive(seed, &format!("plan {request}"));
    let count = rng.range(1, TURN_STEPS_MAX);
    let tools = if child {
        &TOOLS[..CHILD_TOOLS]
    } else {
        &TOOLS[..]
    };
    let mut steps = Vec::new();
    for _ in 1..count {
        let calls = rng.range(1, CALLS_MAX);
        steps.push((0..calls).map(|_| tools[rng.pick(tools.len())]).collect());
    }
    steps.push(Vec::new());
    let poison = rng
        .chance(POISON_PPM)
        .then(|| rng.below(count))
        .filter(|_| !calm);
    Plan { poison, steps }
}

#[derive(Clone)]
struct ClientTurn {
    cancel_after_ms: Option<u64>,
    request: String,
}

struct Shape {
    clientless: Vec<(Origin, u64)>,
    crash_after_ppm: u64,
    crash_before_ppm: u64,
    latency_ms_max: u64,
    sessions: Vec<Vec<ClientTurn>>,
    timed_crashes: Vec<u64>,
}

impl Shape {
    fn of(seed: u64, calm: bool) -> Self {
        let mut rng = Rng::derive(seed, "turns-shape");
        let latency_ms_max = rng.range(STEP_LATENCY_MS_MIN, STEP_LATENCY_MS_MAX);
        let window = latency_ms_max * TURN_STEPS_MAX * SESSION_TURNS_MAX;
        let sessions = (0..rng.range(1, SESSIONS_MAX))
            .map(|session| {
                (0..rng.range(1, SESSION_TURNS_MAX))
                    .map(|turn| ClientTurn {
                        cancel_after_ms: rng
                            .chance(CANCEL_PPM)
                            .then(|| rng.range(0, latency_ms_max * TURN_STEPS_MAX)),
                        request: format!("s{session}.{turn}"),
                    })
                    .collect()
            })
            .collect();
        let clientless = (0..rng.range(0, CLIENTLESS_MAX))
            .map(|_| (ORIGINS[rng.pick(ORIGINS.len())], rng.range(0, window)))
            .collect();
        let crash_before_ppm = rng.range(0, CRASH_BEFORE_COMMIT_PPM_MAX);
        let crash_after_ppm = rng.range(0, CRASH_AFTER_COMMIT_PPM_MAX);
        let timed_crashes = (0..rng.range(0, TIMED_CRASHES_MAX))
            .map(|_| rng.range(0, window))
            .collect();
        if calm {
            return Self {
                clientless,
                crash_after_ppm: 0,
                crash_before_ppm: 0,
                latency_ms_max,
                sessions,
                timed_crashes: Vec::new(),
            };
        }
        Self {
            clientless,
            crash_after_ppm,
            crash_before_ppm,
            latency_ms_max,
            sessions,
            timed_crashes,
        }
    }

    fn describe(&self, profile: &Profile) -> String {
        let sessions: Vec<usize> = self.sessions.iter().map(Vec::len).collect();
        format!(
            "scenario=turns profile={} sessions={sessions:?} clientless={} latency_ms<={} crash_before_ppm={} crash_after_ppm={} timed_crashes={}",
            profile.name,
            self.clientless.len(),
            self.latency_ms_max,
            self.crash_before_ppm,
            self.crash_after_ppm,
            self.timed_crashes.len()
        )
    }
}

#[derive(Clone, Debug)]
enum Message {
    Accepted {
        request: String,
    },
    Busy {
        request: String,
    },
    Durable {
        id: EventId,
        scope: String,
        text: String,
    },
    Ended {
        request: String,
        text: String,
    },
    Live,
    Lost {
        request: String,
    },
}

#[derive(Debug)]
enum Output {
    Client {
        client: usize,
        message: Message,
    },
    Model {
        request: String,
        scope: String,
        step: u64,
    },
    SourceAck {
        source: usize,
    },
    Tool {
        call: Call,
        scope: String,
    },
}

#[derive(Debug)]
struct Gated {
    output: Output,
    ungated: bool,
}

#[derive(Debug)]
enum Ev {
    ClientCancel {
        client: usize,
        request: String,
    },
    ClientReceive {
        client: usize,
        message: Message,
    },
    ClientReconnect {
        client: usize,
    },
    ClientSubmit {
        client: usize,
    },
    ClientTimeout {
        client: usize,
        generation: u64,
    },
    Crash {
        cause: &'static str,
    },
    ModelResponse {
        incarnation: u64,
        nonce: u64,
        request: String,
        scope: String,
        step: u64,
    },
    SourceAcked {
        source: usize,
    },
    SourceDeliver {
        source: usize,
    },
    ToolApply {
        call: Call,
    },
    ToolResponse {
        call: Call,
        incarnation: u64,
        scope: String,
    },
    Wake,
}

#[derive(Default)]
struct Storage {
    children: Vec<(String, String)>,
    files: Vec<String>,
}

#[derive(Default)]
struct Tick {
    children: Vec<(String, String)>,
    files: Vec<String>,
    kinds: Vec<Kind>,
    outputs: Vec<Gated>,
}

impl Tick {
    const fn writes(&self) -> bool {
        !self.children.is_empty() || !self.files.is_empty()
    }
}

struct TickLog<'a> {
    disk: &'a Disk,
}

impl Log for TickLog<'_> {
    fn append(&mut self, scope: &str, body: String) -> u64 {
        self.disk.with(|rows| rows.append(scope, body))
    }

    fn scan(&mut self, scope: &str) -> Scan {
        self.disk.with(|rows| {
            let snap = rows.snapshot(scope).map(|(seq, body)| Row { body, seq });
            let after = snap.as_ref().map_or(0, |snap| snap.seq);
            let rows = rows
                .scan(scope, after)
                .into_iter()
                .map(|(seq, body)| Row { body, seq })
                .collect();
            Scan { rows, snap }
        })
    }

    fn snapshot(&mut self, scope: &str, seq: u64, body: String) {
        self.disk.with(|rows| rows.stage(scope, seq, body));
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PolicyKind {
    Reference,
    Turnlog,
}

impl PolicyKind {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Reference => "reference",
            Self::Turnlog => "turnlog",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        [Self::Reference, Self::Turnlog]
            .into_iter()
            .find(|policy| policy.name() == name)
    }
}

enum Seen {
    Admitted {
        request: String,
    },
    Ended {
        request: String,
        text: String,
        status: Status,
    },
    Event {
        id: EventId,
        text: String,
    },
    Other,
    Restart,
}

fn seen(policy: PolicyKind, seq: u64, body: &str) -> Vec<Seen> {
    match policy {
        PolicyKind::Reference => {
            let mut index = 0;
            decode(body)
                .into_iter()
                .map(|record| match record {
                    Record::Admitted { request, .. } => Seen::Admitted { request },
                    Record::Ended {
                        nonce,
                        request,
                        status,
                    } => Seen::Ended {
                        request,
                        text: format!("{}:{nonce}", status.code()),
                        status,
                    },
                    Record::Event { id, text } => {
                        let id = id.unwrap_or((seq, index));
                        index += 1;
                        Seen::Event { id, text }
                    }
                    Record::Restart { .. } => Seen::Restart,
                    _ => Seen::Other,
                })
                .collect()
        }
        PolicyKind::Turnlog => nanocode_turnlog::decode(body)
            .unwrap_or_else(|error| panic!("a committed turn log row does not decode: {error}"))
            .into_iter()
            .map(|record| match record {
                nanocode_turnlog::Record::Admitted { turn, .. } => Seen::Admitted { request: turn },
                nanocode_turnlog::Record::Status {
                    status:
                        nanocode_turnlog::Status::Finished {
                            outcome, result, ..
                        },
                    turn,
                } => Seen::Ended {
                    request: turn,
                    text: crate::turnlog::text(&result),
                    status: match outcome {
                        nanocode_turnlog::Outcome::Cancelled => Status::Cancelled,
                        nanocode_turnlog::Outcome::Completed => Status::Finished,
                        nanocode_turnlog::Outcome::Failed => Status::Failed,
                    },
                },
                nanocode_turnlog::Record::Event { event, seq, .. } => Seen::Event {
                    id: (0, seq),
                    text: crate::turnlog::text(&event),
                },
                nanocode_turnlog::Record::Recovery { .. } => Seen::Restart,
                _ => Seen::Other,
            })
            .collect(),
    }
}

struct Object {
    incarnation: u64,
    policy: Box<dyn TurnDurability>,
    scopes: BTreeMap<String, ScopeState>,
    subscribers: BTreeMap<String, usize>,
    tick: Tick,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ClientPhase {
    Done,
    Idle,
    Submitting,
    Waiting,
}

struct Client {
    at: usize,
    connected: u64,
    cursor: Option<EventId>,
    generation: u64,
    phase: ClientPhase,
    scope: String,
    turns: Vec<ClientTurn>,
}

impl Client {
    fn request(&self) -> Option<&str> {
        self.turns.get(self.at).map(|turn| turn.request.as_str())
    }
}

struct Source {
    acked: bool,
    origin: Origin,
    request: String,
    scope: String,
}

#[derive(Default)]
struct Oracle {
    active: BTreeMap<String, String>,
    applied_external: BTreeMap<String, u64>,
    asked: BTreeMap<(String, u64), u64>,
    finishes: BTreeMap<String, String>,
    idempotent: BTreeMap<String, String>,
    idempotent_logical: BTreeMap<String, u64>,
    no_progress: BTreeMap<String, u64>,
    observed: BTreeMap<(String, EventId), String>,
    progress: BTreeMap<String, u64>,
    progress_seen: BTreeMap<String, u64>,
    submitted: BTreeSet<String>,
    terminal: BTreeMap<String, Status>,
    unknown_idempotent: BTreeSet<String>,
    wrong: Option<(Invariant, String)>,
}

struct Rngs {
    crash: Rng,
    faults: Rng,
    model: Rng,
    net: Rng,
    tools: Rng,
}

struct Sim {
    clients: Vec<Client>,
    deploys_left: u64,
    digest: u64,
    disk: Arc<Disk>,
    incarnation: u64,
    lines: u64,
    nonce: u64,
    now: u64,
    object: Option<Object>,
    options: TurnOptions,
    oracle: Oracle,
    order: u64,
    profile: Profile,
    queue: BTreeMap<(u64, u64), Ev>,
    rng: Rngs,
    seed: u64,
    shape: Shape,
    sources: Vec<Source>,
    stats: TurnStats,
    storage: Storage,
    ticks: u64,
    wake_pending: bool,
}

impl Sim {
    fn new(seed: u64, options: TurnOptions) -> Self {
        let shape = Shape::of(seed, options.calm);
        let mut profile = Profile::of(seed);
        if options.calm {
            profile.before_ppm = 0;
            profile.after_ppm = 0;
            profile.read_ppm = 0;
        }
        let clients = shape
            .sessions
            .iter()
            .enumerate()
            .map(|(session, turns)| Client {
                at: 0,
                connected: 0,
                cursor: None,
                generation: 0,
                phase: ClientPhase::Idle,
                scope: format!("s{session}"),
                turns: turns.clone(),
            })
            .collect();
        let sources = shape
            .clientless
            .iter()
            .enumerate()
            .map(|(index, (origin, _))| Source {
                acked: false,
                origin: *origin,
                request: format!("q{index}"),
                scope: format!("q{index}"),
            })
            .collect();
        Self {
            clients,
            deploys_left: if options.calm { 0 } else { DEPLOYS_MAX },
            digest: FNV_OFFSET,
            disk: Disk::new(),
            incarnation: 0,
            lines: 0,
            nonce: 0,
            now: 0,
            object: None,
            options,
            oracle: Oracle::default(),
            order: 0,
            profile,
            queue: BTreeMap::new(),
            rng: Rngs {
                crash: Rng::derive(seed, "turns-crash"),
                faults: Rng::derive(seed, "turns-faults"),
                model: Rng::derive(seed, "turns-model"),
                net: Rng::derive(seed, "turns-net"),
                tools: Rng::derive(seed, "turns-tools"),
            },
            seed,
            shape,
            sources,
            stats: TurnStats {
                seeds: 1,
                ..TurnStats::default()
            },
            storage: Storage::default(),
            ticks: 0,
            wake_pending: false,
        }
    }

    fn trace(&mut self, line: &str) {
        let line = format!("{:>9} {line}", self.now);
        self.digest = fnv(fnv(self.digest, line.as_bytes()), b"\n");
        self.lines += 1;
        if self.options.verbose {
            println!("{line}");
        }
    }

    fn wrong(&mut self, invariant: Invariant, message: String) {
        self.trace(&format!("WRONG {}: {message}", invariant.name()));
        if self.oracle.wrong.is_none() {
            self.oracle.wrong = Some((invariant, message));
        }
    }

    fn schedule(&mut self, delay_ms: u64, ev: Ev) {
        self.queue.insert((self.now + delay_ms, self.order), ev);
        self.order += 1;
    }

    fn net(&mut self) -> u64 {
        self.rng.net.range(NET_MS_MIN, NET_MS_MAX)
    }

    fn run(&mut self) {
        let think: Vec<u64> = self
            .clients
            .iter()
            .map(|_| self.rng.net.range(0, THINK_MS_MAX))
            .collect();
        for (client, at) in think.into_iter().enumerate() {
            self.schedule(at, Ev::ClientSubmit { client });
        }
        let deliveries: Vec<u64> = self.shape.clientless.iter().map(|(_, at)| *at).collect();
        for (source, at) in deliveries.into_iter().enumerate() {
            self.schedule(at, Ev::SourceDeliver { source });
        }
        for at in self.shape.timed_crashes.clone() {
            self.schedule(
                at,
                Ev::Crash {
                    cause: "timed crash",
                },
            );
        }
        self.drain();
        if self.oracle.wrong.is_none() {
            self.trace("final restart");
            if self.object.is_some() {
                self.down("final restart");
            }
            if !self.wake_pending {
                self.wake_pending = true;
                self.schedule(RESTART_MS, Ev::Wake);
            }
            self.drain();
        }
        if self.oracle.wrong.is_none() {
            self.judge_liveness();
        }
        if self.oracle.wrong.is_none() {
            self.judge_observed();
        }
        self.tally();
    }

    fn drain(&mut self) {
        while self.oracle.wrong.is_none() {
            let Some(((at, _), ev)) = self.queue.pop_first() else {
                return;
            };
            self.now = at;
            self.ticks += 1;
            if self.ticks > TICKS_MAX {
                self.wrong(
                    Invariant::Liveness,
                    format!("the object did not settle within {TICKS_MAX} ticks"),
                );
                return;
            }
            self.handle(ev);
        }
    }

    fn judge_liveness(&mut self) {
        let open: Vec<String> = self
            .oracle
            .submitted
            .iter()
            .filter(|request| !self.oracle.terminal.contains_key(*request))
            .cloned()
            .collect();
        if let Some(request) = open.first() {
            self.wrong(
                Invariant::Liveness,
                format!(
                    "{request} was accepted but its end is not durable after the final restart ({} such turns)",
                    open.len()
                ),
            );
        }
    }

    fn turnlog_rows(&self, scope: &str) -> Vec<nanocode_turnlog::Row> {
        let committed = self.disk.with(|rows| rows.committed(scope));
        nanocode_turnlog::rows(&committed)
            .unwrap_or_else(|error| panic!("a committed turn log row does not decode: {error}"))
    }

    fn judge_observed(&mut self) {
        let mut durable: BTreeMap<(String, EventId), String> = BTreeMap::new();
        let policy = self.options.policy;
        let log = self.disk.with(|rows| rows.log.clone());
        for (seq, (scope, body)) in &log {
            for record in seen(policy, *seq, body) {
                if let Seen::Event { id, text } = record {
                    durable.insert((scope.clone(), id), text);
                }
            }
        }
        let scopes = self.disk.with(|rows| {
            rows.tlog
                .keys()
                .map(|(scope, _)| scope.clone())
                .collect::<BTreeSet<_>>()
        });
        for scope in scopes {
            for row in self.turnlog_rows(&scope) {
                for record in seen(policy, row.seq, &row.plain) {
                    if let Seen::Event { id, text } = record {
                        durable.insert((scope.clone(), id), text);
                    }
                }
            }
        }
        let diverged = self
            .oracle
            .observed
            .iter()
            .find(|((scope, id), text)| {
                let held = durable.get(&(scope.clone(), *id));
                let reused = id.0 > 0 && log.contains_key(&id.0);
                held.is_some_and(|held| held != *text) || (reused && held.is_none())
            })
            .map(|((scope, id), text)| {
                let held = durable
                    .get(&(scope.clone(), *id))
                    .map_or("nothing", String::as_str);
                format!(
                    "a client saw event {id:?} of {scope} as {text}, but the durable log holds {held} there"
                )
            });
        if let Some(message) = diverged {
            self.wrong(Invariant::SeqUnique, message);
        }
    }

    fn tally(&mut self) {
        let seed = self.seed;
        let calm = self.options.calm;
        let requests: Vec<(String, bool)> = self
            .oracle
            .submitted
            .iter()
            .map(|request| (request.clone(), request.starts_with("k.")))
            .collect();
        self.stats.turns = u64::try_from(requests.len()).expect("a turn count fits in u64");
        for (request, child) in requests {
            let plan = plan(seed, &request, child, calm);
            let writing = plan
                .steps
                .iter()
                .filter(|tools| {
                    tools
                        .iter()
                        .any(|tool| matches!(tool.class(), Class::External | Class::InObject))
                })
                .count();
            self.stats.brief_rows += 2 + u64::try_from(writing).expect("a step count fits in u64");
        }
        for status in self.oracle.terminal.values() {
            match status {
                Status::Cancelled => self.stats.cancelled += 1,
                Status::Failed => self.stats.failed += 1,
                Status::Finished => self.stats.finished += 1,
            }
        }
    }

    fn handle(&mut self, ev: Ev) {
        match ev {
            Ev::ClientCancel { client, request } => self.client_cancel(client, &request),
            Ev::ClientReceive { client, message } => self.client_receive(client, message),
            Ev::ClientReconnect { client } => {
                if self.clients[client].phase == ClientPhase::Waiting {
                    self.client_resume(client);
                }
            }
            Ev::ClientSubmit { client } => self.client_submit(client),
            Ev::ClientTimeout { client, generation } => {
                let waiting = &self.clients[client];
                if waiting.phase == ClientPhase::Submitting && waiting.generation == generation {
                    self.client_submit(client);
                }
            }
            Ev::Crash { cause } => {
                if self.object.is_some() {
                    if cause == "deploy" {
                        self.stats.deploys += 1;
                    }
                    self.down(cause);
                }
            }
            Ev::ModelResponse {
                incarnation,
                nonce,
                request,
                scope,
                step,
            } => self.on_model(incarnation, nonce, &request, &scope, step),
            Ev::SourceAcked { source } => {
                self.sources[source].acked = true;
                let request = self.sources[source].request.clone();
                self.oracle.submitted.insert(request);
            }
            Ev::SourceDeliver { source } => self.source_deliver(source),
            Ev::ToolApply { call } => self.on_apply(&call),
            Ev::ToolResponse {
                call,
                incarnation,
                scope,
            } => self.on_tool(&call, incarnation, &scope),
            Ev::Wake => {
                self.wake_pending = false;
                if self.object.is_none() {
                    self.trace("alarm");
                    self.start();
                }
            }
        }
    }

    fn up(&mut self) -> bool {
        self.object.is_some() || self.start()
    }

    fn start(&mut self) -> bool {
        self.incarnation += 1;
        self.stats.starts += 1;
        let incarnation = self.incarnation;
        self.trace(&format!("start incarnation {incarnation}"));
        if self.rng.faults.chance(self.profile.read_ppm) {
            self.stats.read_faults += 1;
            self.down("read fault at load");
            return false;
        }
        self.judge_progress();
        if self.oracle.wrong.is_some() {
            return false;
        }
        let policy: Box<dyn TurnDurability> = match self.options.policy {
            PolicyKind::Reference => Box::new(OneLog::new(self.options.protections)),
            PolicyKind::Turnlog => Box::new(TurnLogPolicy::new(
                Arc::clone(&self.disk),
                self.options.protections,
                self.options.row_limit,
            )),
        };
        let mut object = Object {
            incarnation,
            policy,
            scopes: BTreeMap::new(),
            subscribers: BTreeMap::new(),
            tick: Tick::default(),
        };
        let scopes = self.disk.with(|rows| {
            rows.reads = 0;
            rows.scopes()
        });
        for scope in scopes {
            let mut log = TickLog { disk: &self.disk };
            let state = object.policy.load(&mut log, &scope);
            object.scopes.insert(scope, state);
        }
        self.stats.cold_reads += self.disk.with(|rows| std::mem::take(&mut rows.reads));
        let active: Vec<String> = object
            .scopes
            .iter()
            .filter(|(_, state)| state.active.is_some())
            .map(|(scope, _)| scope.clone())
            .collect();
        self.object = Some(object);
        for scope in active {
            self.recover(&scope);
        }
        self.commit()
    }

    fn judge_progress(&mut self) {
        let active: Vec<(String, String)> = self
            .oracle
            .active
            .iter()
            .map(|(scope, request)| (scope.clone(), request.clone()))
            .collect();
        for (scope, request) in active {
            let progress = self.oracle.progress.get(&scope).copied().unwrap_or(0);
            let seen = self.oracle.progress_seen.insert(scope.clone(), progress);
            let count = self.oracle.no_progress.entry(scope.clone()).or_default();
            if seen == Some(progress) {
                *count += 1;
            } else {
                *count = 0;
            }
            let count = *count;
            if count > LIVENESS_RESTARTS {
                self.wrong(
                    Invariant::Liveness,
                    format!(
                        "{request} in {scope} restarted {count} times without its log advancing"
                    ),
                );
            }
        }
    }

    fn down(&mut self, cause: &str) {
        self.object = None;
        self.disk.with(|rows| {
            rows.appends.clear();
            rows.staged.clear();
            rows.tappends.clear();
        });
        self.stats.crashes += 1;
        self.trace(&format!("down: {cause}"));
        let waiting: Vec<usize> = self
            .clients
            .iter()
            .enumerate()
            .filter(|(_, client)| client.phase == ClientPhase::Waiting)
            .map(|(index, _)| index)
            .collect();
        for client in waiting {
            self.schedule(RECONNECT_MS, Ev::ClientReconnect { client });
        }
        if !self.wake_pending && !self.oracle.active.is_empty() {
            self.wake_pending = true;
            self.schedule(RESTART_MS, Ev::Wake);
        }
    }

    const fn object(&mut self) -> &mut Object {
        self.object
            .as_mut()
            .expect("the object runs while it handles an input")
    }

    fn push(&mut self, scope: &str, record: Record) {
        let object = self.object();
        object
            .scopes
            .entry(scope.to_owned())
            .or_default()
            .apply(&record);
        object.policy.record(scope, record);
    }

    fn output(&mut self, output: Output, ungated: bool) {
        let ungated = ungated || self.options.mutation == TurnMutation::GateReleasesEarly;
        self.object().tick.outputs.push(Gated { output, ungated });
    }

    fn emit(&mut self, scope: &str, text: String) {
        let mutated = self.options.mutation == TurnMutation::DurableSeqBeforeCommit;
        let object = self.object();
        let state = object.scopes.entry(scope.to_owned()).or_default();
        let id = object.policy.event_id(scope, state);
        let early = mutated && id.is_some();
        let client = object.subscribers.get(scope).copied();
        self.push(
            scope,
            Record::Event {
                id,
                text: text.clone(),
            },
        );
        if let Some(client) = client {
            let message = match id {
                Some(id) => Message::Durable {
                    id,
                    scope: scope.to_owned(),
                    text,
                },
                None => Message::Live,
            };
            self.output(Output::Client { client, message }, early);
        }
    }

    fn point(&mut self, scope: &str, point: Point) {
        let object = self
            .object
            .as_mut()
            .expect("the object runs while it handles an input");
        let mut log = TickLog { disk: &self.disk };
        let state = object.scopes.entry(scope.to_owned()).or_default();
        let Some(appended) = object.policy.point(&mut log, scope, point, state) else {
            return;
        };
        object.tick.kinds.push(appended.kind);
        for _ in 0..appended.parts {
            object.tick.kinds.push(Kind::Part);
        }
        if appended.snapshot {
            object.tick.kinds.push(Kind::Snapshot);
        }
        let client = object.subscribers.get(scope).copied();
        let early = self.options.mutation == TurnMutation::DurableSeqBeforeCommit;
        if let Some(client) = client {
            for (id, text) in appended.events {
                let message = Message::Durable {
                    id,
                    scope: scope.to_owned(),
                    text,
                };
                self.output(Output::Client { client, message }, early);
            }
        }
        let seq = appended.seq;
        self.trace(&format!("  row {seq} {scope} {point:?}"));
    }

    fn active(&self, scope: &str) -> Option<crate::onelog::Active> {
        self.object.as_ref()?.scopes.get(scope)?.active.clone()
    }

    fn admit(&mut self, scope: &str, request: &str, origin: Origin) {
        self.trace(&format!("  admit {request} in {scope} ({origin:?})"));
        self.push(
            scope,
            Record::Admitted {
                origin,
                request: request.to_owned(),
            },
        );
        self.point(scope, Point::Admission);
        self.ask(scope);
    }

    fn ask(&mut self, scope: &str) {
        let Some(active) = self.active(scope) else {
            return;
        };
        if active.phase != Phase::Model {
            return;
        }
        self.output(
            Output::Model {
                request: active.request,
                scope: scope.to_owned(),
                step: active.step,
            },
            false,
        );
    }

    fn end(&mut self, scope: &str, status: Status, nonce: u64) {
        let Some(active) = self.active(scope) else {
            return;
        };
        let request = active.request;
        self.trace(&format!("  end {request} {}", status.code()));
        self.push(
            scope,
            Record::Ended {
                nonce,
                request: request.clone(),
                status,
            },
        );
        let text = format!("{}:{nonce}", status.code());
        self.emit(scope, format!("f:{request}:{text}"));
        self.point(scope, Point::Finish);
        let client = self.object().subscribers.get(scope).copied();
        if let Some(client) = client {
            let early = self.options.mutation == TurnMutation::FinishBeforeRow;
            self.output(
                Output::Client {
                    client,
                    message: Message::Ended { request, text },
                },
                early,
            );
        }
    }

    fn result(&mut self, scope: &str, call: &Call, text: &str) {
        self.push(
            scope,
            Record::Result {
                call: call.id.clone(),
                outcome: Outcome::Done(text.to_owned()),
            },
        );
        self.emit(scope, format!("r:{}:{text}", call.id));
    }

    fn unknown(&mut self, scope: &str, call: &Call) {
        self.stats.unknown += 1;
        self.trace(&format!("  unknown {}", call.id));
        if call.tool.class() == Class::External
            && self.oracle.applied_external.contains_key(call.logical())
        {
            self.stats.unknown_completed += 1;
        }
        if call.tool.class() == Class::Idempotent {
            self.oracle.unknown_idempotent.insert(call.id.clone());
            if self.oracle.idempotent.contains_key(&call.id) {
                self.wrong(
                    Invariant::IdempotentKnown,
                    format!(
                        "{} was reported unknown though the hand completed it",
                        call.id
                    ),
                );
            }
        }
        self.push(
            scope,
            Record::Result {
                call: call.id.clone(),
                outcome: Outcome::Unknown,
            },
        );
        self.emit(scope, format!("r:{}:unknown", call.id));
    }

    fn recover(&mut self, scope: &str) {
        let Some(active) = self.active(scope) else {
            return;
        };
        if self.options.mutation == TurnMutation::RecoverySkipsClientless
            && active.origin != Origin::Client
        {
            return;
        }
        let object = self.object();
        let decision = object.policy.restart(scope, &object.scopes[scope]);
        match decision {
            Decision::Fail => {
                self.stats.capped += 1;
                let child = active.origin == Origin::Child;
                if plan(self.seed, &active.request, child, self.options.calm)
                    .poison
                    .is_none()
                {
                    self.stats.capped_clean += 1;
                }
                self.end(scope, Status::Failed, 0);
                return;
            }
            Decision::Resume(record) => {
                if let Some(record) = record {
                    self.push(scope, record);
                }
            }
        }
        self.stats.recoveries += 1;
        self.trace(&format!(
            "  recover {} at step {}",
            active.request, active.step
        ));
        if let Phase::Tools { calls, results } = &active.phase {
            for call in calls.iter().filter(|call| !results.contains_key(&call.id)) {
                match call.tool.class() {
                    Class::External => self.unknown(scope, call),
                    Class::Idempotent
                        if self.options.mutation == TurnMutation::IdempotentReportedUnknown =>
                    {
                        self.unknown(scope, call);
                    }
                    Class::Idempotent => {
                        self.stats.idempotent_resent += 1;
                        self.output(
                            Output::Tool {
                                call: call.clone(),
                                scope: scope.to_owned(),
                            },
                            false,
                        );
                    }
                    Class::InObject | Class::None => panic!(
                        "{} ({:?}) is pending at a restart, though its result is recorded in the tick that ran it",
                        call.id, call.tool
                    ),
                }
            }
        }
        self.point(scope, Point::Restart);
        self.ask(scope);
    }

    fn commit(&mut self) -> bool {
        let Some(object) = self.object.as_mut() else {
            return false;
        };
        let unsealed = object.policy.unsealed();
        if self.options.mutation != TurnMutation::SealSkipped {
            for scope in unsealed {
                self.point(&scope, Point::Seal);
            }
        }
        let Some(object) = self.object.as_mut() else {
            return false;
        };
        let tick = std::mem::take(&mut object.tick);
        let (appends, staged, tappends) = self.disk.with(|rows| {
            (
                std::mem::take(&mut rows.appends),
                std::mem::take(&mut rows.staged),
                std::mem::take(&mut rows.tappends),
            )
        });
        if !tick.writes() && appends.is_empty() && staged.is_empty() && tappends.is_empty() {
            self.release(tick.outputs, true);
            return true;
        }
        if self.rng.crash.chance(self.shape.crash_before_ppm) {
            self.drop_tick(tick, "crash before commit");
            return false;
        }
        if self.rng.faults.chance(self.profile.before_ppm) {
            self.stats.commit_faults += 1;
            self.drop_tick(tick, "commit failed");
            return false;
        }
        self.apply(&tick, &appends, &staged, &tappends);
        let after = if self.rng.crash.chance(self.shape.crash_after_ppm) {
            Some("crash after commit")
        } else if self.rng.faults.chance(self.profile.after_ppm) {
            self.stats.commit_faults += 1;
            Some("commit confirmation lost")
        } else {
            None
        };
        if let Some(cause) = after {
            self.release(tick.outputs, false);
            self.down(cause);
            return false;
        }
        self.release(tick.outputs, true);
        true
    }

    fn drop_tick(&mut self, tick: Tick, cause: &str) {
        if self.options.mutation == TurnMutation::TickNotAtomic {
            let partial = Tick {
                children: tick.children.clone(),
                files: tick.files.clone(),
                ..Tick::default()
            };
            self.apply(&partial, &[], &[], &[]);
        }
        self.release(tick.outputs, false);
        self.down(cause);
    }

    fn apply(
        &mut self,
        tick: &Tick,
        appends: &[(u64, String, String)],
        staged: &[(String, u64, String)],
        tappends: &[(String, u64, String)],
    ) {
        let rows: Vec<String> = appends
            .iter()
            .map(|(seq, scope, _)| format!("{seq}:{scope}"))
            .collect();
        self.trace(&format!("commit rows={rows:?} kinds={:?}", tick.kinds));
        self.disk.with(|rows| {
            for (seq, scope, body) in appends {
                rows.log.insert(*seq, (scope.clone(), body.clone()));
            }
            for (scope, seq, body) in staged {
                rows.snaps.insert(scope.clone(), (*seq, body.clone()));
            }
            for (scope, seq, body) in tappends {
                rows.tlog.insert((scope.clone(), *seq), body.clone());
            }
        });
        let fresh: BTreeSet<(String, u64)> = tappends
            .iter()
            .map(|(scope, seq, _)| (scope.clone(), *seq))
            .collect();
        let touched: BTreeSet<String> = fresh.iter().map(|(scope, _)| scope.clone()).collect();
        for scope in touched {
            for row in self.turnlog_rows(&scope) {
                if fresh.contains(&(scope.clone(), row.seq)) {
                    self.observe_row(row.seq, &scope, &row.plain);
                }
            }
        }
        for (_, _, body) in tappends {
            self.stats.rows_appended += 1;
            self.stats.bytes += u64::try_from(body.len()).expect("a body length fits in u64");
        }
        for (seq, scope, body) in appends {
            self.stats.rows_appended += 1;
            self.stats.bytes += u64::try_from(body.len()).expect("a body length fits in u64");
            self.observe_row(*seq, scope, body);
        }
        for (_, _, body) in staged {
            self.stats.rows_appended += 1;
            self.stats.bytes += u64::try_from(body.len()).expect("a body length fits in u64");
        }
        for kind in &tick.kinds {
            match kind {
                Kind::Admission => self.stats.rows_admission += 1,
                Kind::Call => self.stats.rows_call += 1,
                Kind::Finish => self.stats.rows_finish += 1,
                Kind::Floor => self.stats.rows_floor += 1,
                Kind::Part => self.stats.rows_part += 1,
                Kind::Progress => self.stats.rows_progress += 1,
                Kind::Restart => self.stats.rows_restart += 1,
                Kind::Result => self.stats.rows_result += 1,
                Kind::Snapshot => self.stats.rows_snapshot += 1,
            }
        }
        for logical in &tick.files {
            self.storage.files.push(logical.clone());
            let count = self
                .storage
                .files
                .iter()
                .filter(|written| *written == logical)
                .count();
            if count > 1 {
                self.wrong(
                    Invariant::InObjectOnce,
                    format!("the workspace write {logical} was applied {count} times"),
                );
            }
        }
        for (logical, child) in &tick.children {
            self.storage.children.push((logical.clone(), child.clone()));
            self.oracle.submitted.insert(child.clone());
            let count = self
                .storage
                .children
                .iter()
                .filter(|(spawned, _)| spawned == logical)
                .count();
            if count > 1 {
                self.wrong(
                    Invariant::InObjectOnce,
                    format!("the spawn {logical} created {count} subagents"),
                );
            }
        }
    }

    fn observe_row(&mut self, seq: u64, scope: &str, body: &str) {
        let records = seen(self.options.policy, seq, body);
        if !records.iter().any(|record| matches!(record, Seen::Restart)) {
            self.oracle.progress.insert(scope.to_owned(), seq);
        }
        for record in records {
            match record {
                Seen::Admitted { request } => {
                    if !self.oracle.terminal.contains_key(&request) {
                        self.oracle.active.insert(scope.to_owned(), request);
                    }
                }
                Seen::Ended {
                    request,
                    status,
                    text,
                } => {
                    if self.oracle.active.get(scope) == Some(&request) {
                        self.oracle.active.remove(scope);
                    }
                    self.oracle.terminal.insert(request.clone(), status);
                    self.finish(&request, &text);
                }
                Seen::Event { .. } | Seen::Other | Seen::Restart => {}
            }
        }
    }

    fn finish(&mut self, request: &str, text: &str) {
        match self.oracle.finishes.get(request) {
            Some(first) if first != text => {
                let first = first.clone();
                self.wrong(
                    Invariant::FinishOnce,
                    format!("{request} ended as {first} and then as {text}"),
                );
            }
            Some(_) => {}
            None => {
                self.oracle
                    .finishes
                    .insert(request.to_owned(), text.to_owned());
            }
        }
    }

    fn release(&mut self, outputs: Vec<Gated>, all: bool) {
        for gated in outputs {
            if all || gated.ungated {
                self.send(gated.output);
            }
        }
    }

    fn send(&mut self, output: Output) {
        match output {
            Output::Client { client, message } => {
                let delay = self.net();
                self.schedule(delay, Ev::ClientReceive { client, message });
            }
            Output::Model {
                request,
                scope,
                step,
            } => {
                self.nonce += 1;
                let nonce = self.nonce;
                self.stats.model_calls += 1;
                let asked = self
                    .oracle
                    .asked
                    .entry((request.clone(), step))
                    .or_default();
                *asked += 1;
                if *asked > 1 {
                    self.stats.replayed += 1;
                }
                let latency = self
                    .rng
                    .model
                    .range(STEP_LATENCY_MS_MIN, self.shape.latency_ms_max);
                if latency >= LONG_CALL_MS
                    && self.deploys_left > 0
                    && self.rng.model.chance(DEPLOY_PPM)
                {
                    self.deploys_left -= 1;
                    let at = self.rng.model.below(latency);
                    self.schedule(at, Ev::Crash { cause: "deploy" });
                }
                self.trace(&format!("  ask {request} step {step} nonce {nonce}"));
                let incarnation = self.incarnation;
                self.schedule(
                    latency,
                    Ev::ModelResponse {
                        incarnation,
                        nonce,
                        request,
                        scope,
                        step,
                    },
                );
            }
            Output::SourceAck { source } => {
                let delay = self.net();
                self.schedule(delay, Ev::SourceAcked { source });
            }
            Output::Tool { call, scope } => {
                let latency = self
                    .rng
                    .tools
                    .range(TOOL_LATENCY_MS_MIN, TOOL_LATENCY_MS_MAX);
                self.trace(&format!("  dispatch {}", call.id));
                let incarnation = self.incarnation;
                self.schedule(latency / 2, Ev::ToolApply { call: call.clone() });
                self.schedule(
                    latency,
                    Ev::ToolResponse {
                        call,
                        incarnation,
                        scope,
                    },
                );
            }
        }
    }

    fn running(&self, incarnation: u64) -> bool {
        self.object
            .as_ref()
            .is_some_and(|object| object.incarnation == incarnation)
    }

    fn on_model(&mut self, incarnation: u64, nonce: u64, request: &str, scope: &str, step: u64) {
        if !self.running(incarnation) {
            return;
        }
        let Some(active) = self.active(scope) else {
            return;
        };
        if active.request != request || active.phase != Phase::Model || active.step != step {
            return;
        }
        let plan = plan(
            self.seed,
            request,
            active.origin == Origin::Child,
            self.options.calm,
        );
        self.trace(&format!("model {request} step {step} nonce {nonce}"));
        if plan.poison == Some(step) {
            self.stats.poison_resets += 1;
            self.object().tick = Tick::default();
            self.down("memory limit");
            return;
        }
        let index = usize::try_from(step).expect("a step fits in usize");
        let tools = plan.steps[index].clone();
        if tools.is_empty() {
            self.push(
                scope,
                Record::Model {
                    nonce,
                    reply: Reply::Final,
                    request: request.to_owned(),
                    step,
                },
            );
            self.emit(scope, format!("m:{request}:{step}:{nonce}"));
            self.end(scope, Status::Finished, nonce);
            self.commit();
            return;
        }
        let calls: Vec<Call> = (0_u64..)
            .zip(tools)
            .map(|(index, tool)| Call {
                id: format!("{request}/{step}/{index}/{nonce}"),
                index,
                tool,
            })
            .collect();
        self.push(
            scope,
            Record::Model {
                nonce,
                reply: Reply::Calls(calls.clone()),
                request: request.to_owned(),
                step,
            },
        );
        self.emit(scope, format!("m:{request}:{step}:{nonce}"));
        let (mut external, mut in_object) = (false, false);
        for call in calls {
            match call.tool {
                Tool::Read => self.result(scope, &call, call.logical()),
                Tool::Write => {
                    in_object = true;
                    self.object().tick.files.push(call.logical().to_owned());
                    self.result(scope, &call, call.logical());
                }
                Tool::Spawn => {
                    in_object = true;
                    let child = format!("k.{}", call.id);
                    self.object()
                        .tick
                        .children
                        .push((call.logical().to_owned(), child.clone()));
                    self.result(scope, &call, &child);
                    self.admit(&child, &child, Origin::Child);
                }
                Tool::Hand | Tool::Charge => {
                    external |= call.tool == Tool::Charge;
                    self.output(
                        Output::Tool {
                            call,
                            scope: scope.to_owned(),
                        },
                        false,
                    );
                }
            }
        }
        self.point(
            scope,
            Point::Calls {
                external,
                in_object,
            },
        );
        self.ask(scope);
        self.commit();
    }

    fn on_apply(&mut self, call: &Call) {
        let logical = call.logical().to_owned();
        match call.tool.class() {
            Class::External => {
                self.stats.external_applied += 1;
                let count = self
                    .oracle
                    .applied_external
                    .entry(logical.clone())
                    .or_default();
                *count += 1;
                let count = *count;
                self.trace(&format!("ledger applies {}", call.id));
                if count > 1 {
                    self.wrong(
                        Invariant::ExternalOnce,
                        format!(
                            "the charge {logical} was applied {count} times, the last as {}",
                            call.id
                        ),
                    );
                }
            }
            Class::Idempotent => {
                if self.oracle.idempotent.contains_key(&call.id) {
                    return;
                }
                self.oracle
                    .idempotent
                    .insert(call.id.clone(), logical.clone());
                let count = self.oracle.idempotent_logical.entry(logical).or_default();
                *count += 1;
                if *count > 1 {
                    self.stats.idempotent_reapplied += 1;
                }
                if self.oracle.unknown_idempotent.contains(&call.id) {
                    self.wrong(
                        Invariant::IdempotentKnown,
                        format!("{} completed after the turn reported it unknown", call.id),
                    );
                }
            }
            Class::InObject | Class::None => {
                panic!("{} ({:?}) never leaves the object", call.id, call.tool)
            }
        }
    }

    fn on_tool(&mut self, call: &Call, incarnation: u64, scope: &str) {
        if !self.running(incarnation) {
            return;
        }
        let Some(active) = self.active(scope) else {
            return;
        };
        let Phase::Tools { calls, results } = &active.phase else {
            return;
        };
        if results.contains_key(&call.id) || !calls.iter().any(|known| known.id == call.id) {
            return;
        }
        let external = calls
            .iter()
            .any(|known| known.tool.class() == Class::External);
        self.trace(&format!("tool result {}", call.id));
        self.result(scope, call, call.logical());
        if self
            .active(scope)
            .is_some_and(|active| active.phase == Phase::Model)
        {
            self.point(scope, Point::Results { external });
            self.ask(scope);
        }
        self.commit();
    }

    fn client_submit(&mut self, client: usize) {
        let Some(request) = self.clients[client].request().map(str::to_owned) else {
            return;
        };
        if matches!(
            self.clients[client].phase,
            ClientPhase::Done | ClientPhase::Waiting
        ) {
            return;
        }
        let submitting = &mut self.clients[client];
        submitting.phase = ClientPhase::Submitting;
        submitting.generation += 1;
        let generation = submitting.generation;
        self.schedule(ACK_TIMEOUT_MS, Ev::ClientTimeout { client, generation });
        self.trace(&format!("client {client} submits {request}"));
        if !self.up() {
            return;
        }
        let scope = self.clients[client].scope.clone();
        self.clients[client].connected = self.incarnation;
        self.object().subscribers.insert(scope.clone(), client);
        let state = self.object().scopes.entry(scope.clone()).or_default();
        let message = if let Some((status, nonce)) = state.ended.get(&request) {
            Message::Ended {
                text: format!("{}:{nonce}", status.code()),
                request,
            }
        } else if state.knows(&request) {
            Message::Accepted { request }
        } else if state.active.is_some() {
            Message::Busy { request }
        } else {
            self.admit(&scope, &request, Origin::Client);
            Message::Accepted { request }
        };
        self.output(Output::Client { client, message }, false);
        self.commit();
    }

    fn client_resume(&mut self, client: usize) {
        let Some(request) = self.clients[client].request().map(str::to_owned) else {
            return;
        };
        self.trace(&format!("client {client} resumes {request}"));
        if !self.up() {
            return;
        }
        let scope = self.clients[client].scope.clone();
        let cursor = self.clients[client].cursor;
        self.clients[client].connected = self.incarnation;
        self.object().subscribers.insert(scope.clone(), client);
        let state = self
            .object()
            .scopes
            .entry(scope.clone())
            .or_default()
            .clone();
        for (id, text) in &state.events {
            let message = match id {
                Some(id) if cursor.is_none_or(|cursor| *id > cursor) => Message::Durable {
                    id: *id,
                    scope: scope.clone(),
                    text: text.clone(),
                },
                Some(_) => continue,
                None => Message::Live,
            };
            self.output(Output::Client { client, message }, false);
        }
        if let Some((status, nonce)) = state.ended.get(&request) {
            let message = Message::Ended {
                text: format!("{}:{nonce}", status.code()),
                request,
            };
            self.output(Output::Client { client, message }, false);
        } else if !state.knows(&request) {
            self.output(
                Output::Client {
                    client,
                    message: Message::Lost { request },
                },
                false,
            );
        }
        self.commit();
    }

    fn client_cancel(&mut self, client: usize, request: &str) {
        let cancelling = &self.clients[client];
        if cancelling.phase != ClientPhase::Waiting || cancelling.request() != Some(request) {
            return;
        }
        self.schedule(
            ACK_TIMEOUT_MS,
            Ev::ClientCancel {
                client,
                request: request.to_owned(),
            },
        );
        self.trace(&format!("client {client} cancels {request}"));
        if !self.up() {
            return;
        }
        let scope = self.clients[client].scope.clone();
        let ended = self
            .object()
            .scopes
            .get(&scope)
            .and_then(|state| state.ended.get(request).copied());
        if let Some((status, nonce)) = ended {
            let message = Message::Ended {
                request: request.to_owned(),
                text: format!("{}:{nonce}", status.code()),
            };
            self.output(Output::Client { client, message }, false);
        } else if self
            .active(&scope)
            .is_some_and(|active| active.request == request)
        {
            self.end(&scope, Status::Cancelled, 0);
        }
        self.commit();
    }

    fn advance(&mut self, client: usize) {
        let think = self.rng.net.range(0, THINK_MS_MAX);
        let advancing = &mut self.clients[client];
        advancing.at += 1;
        if advancing.at < advancing.turns.len() {
            advancing.phase = ClientPhase::Idle;
            self.schedule(think, Ev::ClientSubmit { client });
        } else {
            advancing.phase = ClientPhase::Done;
        }
    }

    fn client_receive(&mut self, client: usize, message: Message) {
        let current = self.clients[client].request().map(str::to_owned);
        let phase = self.clients[client].phase;
        match message {
            Message::Accepted { request } => {
                if phase == ClientPhase::Submitting && current.as_deref() == Some(&request) {
                    self.clients[client].phase = ClientPhase::Waiting;
                    if !self.running(self.clients[client].connected) {
                        self.schedule(RECONNECT_MS, Ev::ClientReconnect { client });
                    }
                    self.oracle.submitted.insert(request.clone());
                    let cancel =
                        self.clients[client].turns[self.clients[client].at].cancel_after_ms;
                    if let Some(after) = cancel {
                        self.schedule(after, Ev::ClientCancel { client, request });
                    }
                }
            }
            Message::Busy { request } => {
                if phase == ClientPhase::Submitting && current.as_deref() == Some(&request) {
                    self.schedule(BUSY_RETRY_MS, Ev::ClientSubmit { client });
                }
            }
            Message::Durable { id, scope, text } => {
                let key = (scope.clone(), id);
                match self.oracle.observed.get(&key) {
                    Some(first) if *first != text => {
                        let first = first.clone();
                        self.wrong(
                            Invariant::SeqUnique,
                            format!(
                                "client {client} saw event {id:?} of {scope} as {first} and then as {text}"
                            ),
                        );
                    }
                    Some(_) => {}
                    None => {
                        self.oracle.observed.insert(key, text);
                    }
                }
                let cursor = &mut self.clients[client].cursor;
                *cursor = Some(cursor.map_or(id, |cursor| cursor.max(id)));
            }
            Message::Ended { request, text } => {
                self.finish(&request, &text);
                if current.as_deref() == Some(&request)
                    && matches!(phase, ClientPhase::Submitting | ClientPhase::Waiting)
                {
                    self.oracle.submitted.insert(request);
                    self.advance(client);
                }
            }
            Message::Live => {}
            Message::Lost { request } => {
                if phase == ClientPhase::Waiting && current.as_deref() == Some(&request) {
                    self.stats.lost += 1;
                    self.trace(&format!("client {client} lost {request}"));
                    self.advance(client);
                }
            }
        }
    }

    fn source_deliver(&mut self, source: usize) {
        if self.sources[source].acked {
            return;
        }
        self.schedule(REDELIVER_MS, Ev::SourceDeliver { source });
        let delivering = &self.sources[source];
        let (scope, request, origin) = (
            delivering.scope.clone(),
            delivering.request.clone(),
            delivering.origin,
        );
        self.trace(&format!("{origin:?} delivers {request}"));
        if !self.up() {
            return;
        }
        let state = self.object().scopes.entry(scope.clone()).or_default();
        if state.knows(&request) {
            self.output(Output::SourceAck { source }, false);
        } else if state.active.is_none() {
            self.admit(&scope, &request, origin);
            self.output(Output::SourceAck { source }, false);
        }
        self.commit();
    }
}

pub fn simulate(seed: u64, options: TurnOptions) -> TurnOutcome {
    let mut sim = Sim::new(seed, options);
    let shape = sim.shape.describe(&sim.profile);
    sim.trace(&format!(
        "seed {seed} {shape} policy={} calm={} protections_off={:?} live_seqs={} mutation={}",
        options.policy.name(),
        options.calm,
        options.protections.off(),
        options.protections.live_seqs.name(),
        options.mutation.name()
    ));
    sim.run();
    TurnOutcome {
        digest: sim.digest,
        lines: sim.lines,
        shape,
        stats: sim.stats,
        wrong: sim.oracle.wrong,
    }
}

pub fn run_seed(seed: u64, options: TurnOptions) -> TurnOutcome {
    std::panic::catch_unwind(|| simulate(seed, options)).unwrap_or_else(|panic| {
        let message = panic
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| panic.downcast_ref::<&str>().map(|s| (*s).to_owned()))
            .unwrap_or_default();
        TurnOutcome {
            digest: 0,
            lines: 0,
            shape: String::new(),
            stats: TurnStats::default(),
            wrong: Some((Invariant::Panic, message)),
        }
    })
}
