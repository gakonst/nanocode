use std::collections::{BTreeMap, BTreeSet};

pub const RESTART_CAP: u64 = 3;
pub const RESTART_SEQ_GAP: u64 = 1 << 14;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Charge,
    Hand,
    Read,
    Spawn,
    Write,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    External,
    Idempotent,
    InObject,
    None,
}

impl Tool {
    pub const fn class(self) -> Class {
        match self {
            Self::Charge => Class::External,
            Self::Hand => Class::Idempotent,
            Self::Read => Class::None,
            Self::Spawn | Self::Write => Class::InObject,
        }
    }

    const fn code(self) -> &'static str {
        match self {
            Self::Charge => "charge",
            Self::Hand => "hand",
            Self::Read => "read",
            Self::Spawn => "spawn",
            Self::Write => "write",
        }
    }

    fn parse(code: &str) -> Option<Self> {
        [
            Self::Charge,
            Self::Hand,
            Self::Read,
            Self::Spawn,
            Self::Write,
        ]
        .into_iter()
        .find(|tool| tool.code() == code)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    Alarm,
    Child,
    Client,
    Completion,
}

impl Origin {
    const fn code(self) -> &'static str {
        match self {
            Self::Alarm => "alarm",
            Self::Child => "child",
            Self::Client => "client",
            Self::Completion => "completion",
        }
    }

    fn parse(code: &str) -> Option<Self> {
        [Self::Alarm, Self::Child, Self::Client, Self::Completion]
            .into_iter()
            .find(|origin| origin.code() == code)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Call {
    pub id: String,
    pub index: u64,
    pub tool: Tool,
}

impl Call {
    pub fn logical(&self) -> &str {
        self.id
            .rsplit_once('/')
            .map_or(self.id.as_str(), |(logical, _)| logical)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reply {
    Calls(Vec<Call>),
    Final,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Done(String),
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Cancelled,
    Failed,
    Finished,
}

impl Status {
    pub const fn code(self) -> &'static str {
        match self {
            Self::Cancelled => "cancelled",
            Self::Failed => "failed",
            Self::Finished => "finished",
        }
    }

    fn parse(code: &str) -> Option<Self> {
        [Self::Cancelled, Self::Failed, Self::Finished]
            .into_iter()
            .find(|status| status.code() == code)
    }
}

pub type EventId = (u64, u64);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Record {
    Admitted {
        origin: Origin,
        request: String,
    },
    Ended {
        nonce: u64,
        request: String,
        status: Status,
    },
    Event {
        id: Option<EventId>,
        text: String,
    },
    Floor {
        seq: u64,
    },
    Model {
        nonce: u64,
        reply: Reply,
        request: String,
        step: u64,
    },
    Restart {
        attempt: u64,
        floor: u64,
    },
    Result {
        call: String,
        outcome: Outcome,
    },
}

fn encode_record(record: &Record, out: &mut String) {
    match record {
        Record::Admitted { origin, request } => {
            out.push_str(&format!("A {request} {}", origin.code()));
        }
        Record::Ended {
            nonce,
            request,
            status,
        } => out.push_str(&format!("X {request} {} {nonce}", status.code())),
        Record::Event { id, text } => match id {
            Some((high, low)) => out.push_str(&format!("E {high}.{low} {text}")),
            None => out.push_str(&format!("E - {text}")),
        },
        Record::Model {
            nonce,
            reply,
            request,
            step,
        } => {
            out.push_str(&format!("M {request} {step} {nonce}"));
            match reply {
                Reply::Final => out.push_str(" F"),
                Reply::Calls(calls) if calls.is_empty() => out.push_str(" C"),
                Reply::Calls(calls) => {
                    out.push_str(" C ");
                    let listed: Vec<String> = calls
                        .iter()
                        .map(|call| format!("{}:{}:{}", call.id, call.tool.code(), call.index))
                        .collect();
                    out.push_str(&listed.join(","));
                }
            }
        }
        Record::Floor { seq } => out.push_str(&format!("L {seq}")),
        Record::Restart { attempt, floor } => out.push_str(&format!("R {attempt} {floor}")),
        Record::Result { call, outcome } => match outcome {
            Outcome::Done(text) => out.push_str(&format!("T {call} D {text}")),
            Outcome::Unknown => out.push_str(&format!("T {call} U")),
        },
    }
    out.push('\n');
}

pub fn encode(records: &[Record]) -> String {
    let mut out = String::new();
    for record in records {
        encode_record(record, &mut out);
    }
    out
}

fn decode_call(text: &str) -> Option<Call> {
    let mut parts = text.split(':');
    let id = parts.next()?.to_owned();
    let tool = Tool::parse(parts.next()?)?;
    let index = parts.next()?.parse().ok()?;
    Some(Call { id, index, tool })
}

fn decode_record(line: &str) -> Option<Record> {
    let mut parts = line.split(' ');
    let record = match parts.next()? {
        "A" => Record::Admitted {
            request: parts.next()?.to_owned(),
            origin: Origin::parse(parts.next()?)?,
        },
        "X" => Record::Ended {
            request: parts.next()?.to_owned(),
            status: Status::parse(parts.next()?)?,
            nonce: parts.next()?.parse().ok()?,
        },
        "E" => {
            let id = match parts.next()? {
                "-" => None,
                id => {
                    let (high, low) = id.split_once('.')?;
                    Some((high.parse().ok()?, low.parse().ok()?))
                }
            };
            Record::Event {
                id,
                text: parts.next()?.to_owned(),
            }
        }
        "M" => {
            let request = parts.next()?.to_owned();
            let step = parts.next()?.parse().ok()?;
            let nonce = parts.next()?.parse().ok()?;
            let reply = match parts.next()? {
                "F" => Reply::Final,
                "C" => Reply::Calls(match parts.next() {
                    Some(listed) => listed
                        .split(',')
                        .map(decode_call)
                        .collect::<Option<Vec<_>>>()?,
                    None => Vec::new(),
                }),
                _ => return None,
            };
            Record::Model {
                nonce,
                reply,
                request,
                step,
            }
        }
        "L" => Record::Floor {
            seq: parts.next()?.parse().ok()?,
        },
        "R" => Record::Restart {
            attempt: parts.next()?.parse().ok()?,
            floor: parts.next()?.parse().ok()?,
        },
        "T" => {
            let call = parts.next()?.to_owned();
            let outcome = match parts.next()? {
                "D" => Outcome::Done(parts.next()?.to_owned()),
                "U" => Outcome::Unknown,
                _ => return None,
            };
            Record::Result { call, outcome }
        }
        _ => return None,
    };
    Some(record)
}

pub fn decode(body: &str) -> Vec<Record> {
    body.lines()
        .filter(|line| !line.starts_with('N'))
        .map(|line| decode_record(line).unwrap_or_else(|| panic!("an undecodable record: {line}")))
        .collect()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Phase {
    Model,
    Tools {
        calls: Vec<Call>,
        results: BTreeMap<String, Outcome>,
    },
}

#[derive(Clone, Debug)]
pub struct Active {
    pub nonce: u64,
    pub origin: Origin,
    pub phase: Phase,
    pub request: String,
    pub step: u64,
}

#[derive(Clone, Debug, Default)]
pub struct ScopeState {
    pub active: Option<Active>,
    pub ended: BTreeMap<String, (Status, u64)>,
    pub events: Vec<(Option<EventId>, String)>,
    pub floor: u64,
    pub next_event: u64,
    pub restarts: u64,
}

impl ScopeState {
    pub fn knows(&self, request: &str) -> bool {
        self.ended.contains_key(request)
            || self
                .active
                .as_ref()
                .is_some_and(|active| active.request == request)
    }

    pub fn apply(&mut self, record: &Record) {
        match record {
            Record::Admitted { origin, request } => {
                self.active = Some(Active {
                    nonce: 0,
                    origin: *origin,
                    phase: Phase::Model,
                    request: request.clone(),
                    step: 0,
                });
            }
            Record::Ended {
                nonce,
                request,
                status,
            } => {
                self.ended.insert(request.clone(), (*status, *nonce));
                if self
                    .active
                    .as_ref()
                    .is_some_and(|active| &active.request == request)
                {
                    self.active = None;
                }
            }
            Record::Event { id, text } => {
                if let Some((0, low)) = id {
                    self.next_event = self.next_event.max(low + 1);
                }
                self.events.push((*id, text.clone()));
            }
            Record::Model {
                nonce, reply, step, ..
            } => {
                if let Some(active) = &mut self.active {
                    active.nonce = *nonce;
                    active.step = *step;
                    active.phase = match reply {
                        Reply::Final => Phase::Model,
                        Reply::Calls(calls) => Phase::Tools {
                            calls: calls.clone(),
                            results: BTreeMap::new(),
                        },
                    };
                }
            }
            Record::Floor { seq: floor } | Record::Restart { floor, .. } => {
                self.floor = self.floor.max(*floor);
            }
            Record::Result { call, outcome } => {
                let Some(active) = &mut self.active else {
                    return;
                };
                let Phase::Tools { calls, results } = &mut active.phase else {
                    return;
                };
                results.insert(call.clone(), outcome.clone());
                if calls.iter().all(|call| results.contains_key(&call.id)) {
                    active.step += 1;
                    active.phase = Phase::Model;
                }
            }
        }
    }

    pub fn apply_row(&mut self, seq: u64, records: &[Record]) {
        let mut index = 0;
        for record in records {
            match record {
                Record::Event { id: None, text } => {
                    self.events.push((Some((seq, index)), text.clone()));
                    index += 1;
                }
                Record::Event { .. } => {
                    self.apply(record);
                    index += 1;
                }
                _ => self.apply(record),
            }
        }
        self.count_row(records);
    }

    const fn count_row(&mut self, records: &[Record]) {
        if matches!(records.first(), Some(Record::Restart { .. })) {
            self.restarts += 1;
        } else if !records.is_empty() {
            self.restarts = 0;
        }
    }

    pub fn snapshot(&self) -> String {
        let mut records = Vec::new();
        for (request, (status, nonce)) in &self.ended {
            records.push(Record::Ended {
                nonce: *nonce,
                request: request.clone(),
                status: *status,
            });
        }
        for (id, text) in &self.events {
            records.push(Record::Event {
                id: *id,
                text: text.clone(),
            });
        }
        if let Some(active) = &self.active {
            records.push(Record::Admitted {
                origin: active.origin,
                request: active.request.clone(),
            });
            if let Phase::Tools { calls, results } = &active.phase {
                records.push(Record::Model {
                    nonce: active.nonce,
                    reply: Reply::Calls(calls.clone()),
                    request: active.request.clone(),
                    step: active.step,
                });
                for (call, outcome) in results {
                    records.push(Record::Result {
                        call: call.clone(),
                        outcome: outcome.clone(),
                    });
                }
            } else if active.step > 0 || active.nonce > 0 {
                records.push(Record::Model {
                    nonce: active.nonce,
                    reply: Reply::Calls(Vec::new()),
                    request: active.request.clone(),
                    step: active.step,
                });
            }
        }
        format!(
            "N {} {} {}\n{}",
            self.restarts,
            self.next_event,
            self.floor,
            encode(&records)
        )
    }

    pub fn restore(body: &str) -> Self {
        let header = body
            .lines()
            .next()
            .and_then(|line| line.strip_prefix("N "))
            .expect("a snapshot starts with its header");
        let fields: Vec<u64> = header
            .split(' ')
            .map(|field| field.parse().expect("a snapshot header holds numbers"))
            .collect();
        let mut state = Self::default();
        for record in decode(body) {
            match &record {
                Record::Model {
                    nonce,
                    reply: Reply::Calls(calls),
                    step,
                    ..
                } if calls.is_empty() => {
                    if let Some(active) = &mut state.active {
                        active.nonce = *nonce;
                        active.step = *step;
                    }
                }
                _ => state.apply(&record),
            }
        }
        state.restarts = fields[0];
        state.next_event = state.next_event.max(fields[1]);
        state.floor = fields[2];
        state
    }
}

#[derive(Clone, Debug)]
pub struct Row {
    pub body: String,
    pub seq: u64,
}

#[derive(Clone, Debug, Default)]
pub struct Scan {
    pub rows: Vec<Row>,
    pub snap: Option<Row>,
}

pub trait Log {
    fn append(&mut self, scope: &str, body: String) -> u64;
    fn scan(&mut self, scope: &str) -> Scan;
    fn snapshot(&mut self, scope: &str, seq: u64, body: String);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Point {
    Admission,
    Calls { external: bool, in_object: bool },
    Finish,
    Restart,
    Results { external: bool },
    Seal,
}

impl Point {
    pub const fn kind(self) -> Kind {
        match self {
            Self::Admission => Kind::Admission,
            Self::Calls { .. } => Kind::Call,
            Self::Finish => Kind::Finish,
            Self::Restart => Kind::Restart,
            Self::Results { .. } => Kind::Result,
            Self::Seal => Kind::Floor,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    Admission,
    Call,
    Finish,
    Floor,
    Part,
    Progress,
    Restart,
    Result,
    Snapshot,
}

pub enum Decision {
    Fail,
    Resume(Option<Record>),
}

pub struct Appended {
    pub events: Vec<(EventId, String)>,
    pub kind: Kind,
    pub parts: u64,
    pub seq: u64,
    pub snapshot: bool,
}

pub trait TurnDurability {
    fn event_id(&mut self, scope: &str, state: &mut ScopeState) -> Option<EventId>;
    fn load(&mut self, log: &mut dyn Log, scope: &str) -> ScopeState;
    fn point(
        &mut self,
        log: &mut dyn Log,
        scope: &str,
        point: Point,
        state: &mut ScopeState,
    ) -> Option<Appended>;
    fn record(&mut self, scope: &str, record: Record);
    fn restart(&mut self, scope: &str, state: &ScopeState) -> Decision;
    fn unsealed(&mut self) -> Vec<String>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LiveSeqs {
    Floor,
    Gap,
    Reuse,
    Temporary,
}

impl LiveSeqs {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Floor => "floor",
            Self::Gap => "gap",
            Self::Reuse => "reuse",
            Self::Temporary => "temporary",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        [Self::Floor, Self::Gap, Self::Reuse, Self::Temporary]
            .into_iter()
            .find(|seqs| seqs.name() == name)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Protections {
    pub admission: bool,
    pub call_external: bool,
    pub call_in_object: bool,
    pub finish: bool,
    pub live_seqs: LiveSeqs,
    pub progress: bool,
    pub restart: bool,
    pub result: bool,
    pub snapshot: bool,
}

pub const PROTECTIONS: [&str; 8] = [
    "admission",
    "call_external",
    "call_in_object",
    "finish",
    "progress",
    "restart",
    "result",
    "snapshot",
];

impl Protections {
    pub const ALL: Self = Self {
        admission: true,
        call_external: true,
        call_in_object: true,
        finish: true,
        live_seqs: LiveSeqs::Temporary,
        progress: true,
        restart: true,
        result: true,
        snapshot: true,
    };

    pub fn without(mut self, name: &str) -> Option<Self> {
        match name {
            "admission" => self.admission = false,
            "call_external" => self.call_external = false,
            "call_in_object" => self.call_in_object = false,
            "finish" => self.finish = false,
            "progress" => self.progress = false,
            "restart" => self.restart = false,
            "result" => self.result = false,
            "snapshot" => self.snapshot = false,
            _ => return None,
        }
        Some(self)
    }

    pub fn off(&self) -> Vec<&'static str> {
        let on = [
            self.admission,
            self.call_external,
            self.call_in_object,
            self.finish,
            self.progress,
            self.restart,
            self.result,
            self.snapshot,
        ];
        PROTECTIONS
            .into_iter()
            .zip(on)
            .filter(|(_, on)| !on)
            .map(|(name, _)| name)
            .collect()
    }

    const fn writes(&self, point: Point, recovering: bool) -> bool {
        if recovering
            && self.progress
            && matches!(point, Point::Calls { .. } | Point::Results { .. })
        {
            return true;
        }
        match point {
            Point::Admission => self.admission,
            Point::Calls {
                external,
                in_object,
            } => (external && self.call_external) || (in_object && self.call_in_object),
            Point::Finish => self.finish,
            Point::Restart => self.restart,
            Point::Results { external } => external && self.result,
            Point::Seal => false,
        }
    }
}

#[derive(Default)]
struct Written {
    since_snapshot: u64,
    snapshot: Option<u64>,
}

pub struct OneLog {
    buffered: BTreeMap<String, Vec<Record>>,
    pending: BTreeMap<String, u64>,
    protections: Protections,
    seals: bool,
    unsealed: BTreeSet<String>,
    written: BTreeMap<String, Written>,
}

impl OneLog {
    pub const fn new(protections: Protections, seals: bool) -> Self {
        Self {
            buffered: BTreeMap::new(),
            pending: BTreeMap::new(),
            protections,
            seals,
            unsealed: BTreeSet::new(),
            written: BTreeMap::new(),
        }
    }
}

fn len(body: &str) -> u64 {
    u64::try_from(body.len()).expect("a body length fits in u64")
}

impl TurnDurability for OneLog {
    fn event_id(&mut self, scope: &str, state: &mut ScopeState) -> Option<EventId> {
        if self.protections.live_seqs == LiveSeqs::Temporary {
            return None;
        }
        if self.pending.contains_key(scope) {
            self.unsealed.insert(scope.to_owned());
        }
        let id = (0, state.next_event);
        state.next_event += 1;
        Some(id)
    }

    fn load(&mut self, log: &mut dyn Log, scope: &str) -> ScopeState {
        let scan = log.scan(scope);
        let mut written = Written::default();
        let mut state = scan.snap.as_ref().map_or_else(ScopeState::default, |snap| {
            written.snapshot = Some(len(&snap.body));
            ScopeState::restore(&snap.body)
        });
        for row in &scan.rows {
            written.since_snapshot += len(&row.body);
            state.apply_row(row.seq, &decode(&row.body));
        }
        if matches!(self.protections.live_seqs, LiveSeqs::Floor | LiveSeqs::Gap) {
            state.next_event = state.next_event.max(state.floor) + RESTART_SEQ_GAP;
        }
        if self.protections.live_seqs == LiveSeqs::Floor {
            self.pending.insert(scope.to_owned(), state.next_event);
        }
        self.written.insert(scope.to_owned(), written);
        state
    }

    fn point(
        &mut self,
        log: &mut dyn Log,
        scope: &str,
        point: Point,
        state: &mut ScopeState,
    ) -> Option<Appended> {
        let floor = self.pending.get(scope).copied().filter(|_| self.seals);
        let writes = match point {
            Point::Seal => floor.is_some(),
            _ => self.protections.writes(point, state.restarts > 0),
        };
        let kind = if point != Point::Seal && !self.protections.writes(point, false) {
            Kind::Progress
        } else {
            point.kind()
        };
        if !writes {
            return None;
        }
        let mut records = self.buffered.remove(scope).unwrap_or_default();
        if let Some(seq) = floor {
            records.push(Record::Floor { seq });
            self.pending.remove(scope);
            self.unsealed.remove(scope);
        }
        if records.is_empty() {
            return None;
        }
        let body = encode(&records);
        let body_len = len(&body);
        let seq = log.append(scope, body);
        let mut events = Vec::new();
        let unnumbered = state.events.iter().filter(|(id, _)| id.is_none()).count();
        let mut index = 0;
        for record in &records {
            if let Record::Event { id, text } = record {
                if id.is_none() {
                    events.push(((seq, index), text.clone()));
                }
                index += 1;
            }
        }
        let first = state.events.len() - unnumbered;
        for (slot, (id, _)) in state.events[first..].iter_mut().zip(&events) {
            *slot = (Some(*id), slot.1.clone());
        }
        state.count_row(&records);
        let written = self.written.entry(scope.to_owned()).or_default();
        written.since_snapshot += body_len;
        let mut snapshot = false;
        if self.protections.snapshot {
            let body = state.snapshot();
            let cold = written.snapshot.unwrap_or(0) + written.since_snapshot;
            if cold > 2 * len(&body) {
                written.snapshot = Some(len(&body));
                written.since_snapshot = 0;
                log.snapshot(scope, seq, body);
                snapshot = true;
            }
        }
        Some(Appended {
            events,
            kind,
            parts: 0,
            seq,
            snapshot,
        })
    }

    fn record(&mut self, scope: &str, record: Record) {
        self.buffered
            .entry(scope.to_owned())
            .or_default()
            .push(record);
    }

    fn restart(&mut self, _scope: &str, state: &ScopeState) -> Decision {
        if !self.protections.restart {
            return Decision::Resume(None);
        }
        if state.restarts >= RESTART_CAP {
            return Decision::Fail;
        }
        Decision::Resume(Some(Record::Restart {
            attempt: state.restarts + 1,
            floor: state.next_event,
        }))
    }

    fn unsealed(&mut self) -> Vec<String> {
        std::mem::take(&mut self.unsealed).into_iter().collect()
    }
}
