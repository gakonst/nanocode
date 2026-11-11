use std::{
    borrow::Cow,
    collections::{BTreeSet, HashMap, HashSet, VecDeque},
    sync::{Arc, OnceLock, Weak},
    time::Duration,
};

use djs_embed::{Data, HostCall, HostFn, Snapshot, Source, Throw};
use nanocode_platform::Instant;
use nanocode_sync::Atom;
use serde::{Deserialize, Serialize, Serializer};
use serde_json::{Value, value::RawValue};

use nanocode_tool_contracts::ToolOutputContent;

use crate::journal::{CellJournaling, JournalEntry, JournalWriter};

pub enum RuntimeEvent {
    ToolCall {
        cell_id: u64,
        id: u64,
        name: String,
        input: Value,
        host: bool,
    },
    Notify {
        cell_id: u64,
        text: String,
    },
    Content {
        cell_id: u64,
        content: ToolOutputContent,
    },
    Yielded {
        cell_id: u64,
    },
    Done {
        cell_id: u64,
        stored: HashMap<String, Arc<RawValue>>,
    },
    Error {
        cell_id: u64,
        message: String,
        stored: HashMap<String, Arc<RawValue>>,
    },
    Held {
        cell_id: u64,
    },
    Waiting {
        cell_id: u64,
    },
    Replayed {
        cell_id: u64,
    },
    Diverged {
        cell_id: u64,
        reason: String,
    },
}

impl RuntimeEvent {
    pub const fn cell_id(&self) -> u64 {
        match self {
            Self::ToolCall { cell_id, .. }
            | Self::Notify { cell_id, .. }
            | Self::Content { cell_id, .. }
            | Self::Yielded { cell_id, .. }
            | Self::Done { cell_id, .. }
            | Self::Error { cell_id, .. }
            | Self::Held { cell_id }
            | Self::Waiting { cell_id }
            | Self::Replayed { cell_id }
            | Self::Diverged { cell_id, .. } => *cell_id,
        }
    }
}

const BOOTSTRAP: &str = include_str!("bootstrap.js");
const CODE_TOOLS: &str = include_str!("code-tools.mjs");
const CODE_VALUES: &str = include_str!("code-values.mjs");

pub(crate) const BOOTSTRAP_MODULE: &str = "nanocode:bootstrap";
pub(crate) const RUN_CELL: &str = "runCell";
const PRELUDE_SCRIPT: &str = "nanocode:prelude";

const TOOL: u32 = 1;
const CONTENT: u32 = 2;
const TEXT: u32 = 3;
const LOAD: u32 = 4;
const NOTIFY: u32 = 5;
const YIELD: u32 = 6;
const SET_TIMEOUT: u32 = 7;
const CLEAR_TIMEOUT: u32 = 8;
const VALUE: u32 = 9;
const SETTLED: u32 = 10;
pub(crate) const NEXT: u32 = 11;
const CATALOG: u32 = 12;

const HOST_FNS: [(u32, &str); 11] = [
    (TOOL, "__nanocodeTool"),
    (CONTENT, "__nanocodeContent"),
    (TEXT, "__nanocodeText"),
    (LOAD, "__nanocodeLoad"),
    (NOTIFY, "__nanocodeNotify"),
    (YIELD, "__nanocodeYield"),
    (SET_TIMEOUT, "__nanocodeSetTimeout"),
    (CLEAR_TIMEOUT, "__nanocodeClearTimeout"),
    (VALUE, "__nanocodeValue"),
    (SETTLED, "__nanocodeSettled"),
    (CATALOG, "__nanocodeCatalog"),
];

#[derive(Clone)]
pub(crate) enum Unprepared {
    Prelude(String),
    Bootstrap(String),
}

fn build(tools: &CellTools, prelude: Option<&str>) -> Result<Snapshot, Unprepared> {
    let values = CODE_VALUES
        .split_once("export const")
        .ok_or_else(|| {
            Unprepared::Bootstrap("shared value helper export marker is missing".to_owned())
        })?
        .0;
    let mut builder = Snapshot::builder()
        .frozen_builtins(false)
        .global(
            "__nanocodeDefinitions",
            Data::from(tools.definitions.as_str()),
        )
        .host_fn(HostFn::asynchronous(NEXT, "__nanocodeNext"));
    for (id, name) in HOST_FNS {
        builder = builder.host_fn(HostFn::sync(id, name));
    }
    if let Some(source) = prelude {
        for (id, name) in crate::bytes::HOST_FNS {
            builder = builder.host_fn(HostFn::sync(id, name));
        }
        builder = builder.source(Source::script(
            PRELUDE_SCRIPT,
            format!("globalThis.__nanocodePrelude = function (tools, call) {{\n{source}\n}};\n"),
        ));
    }
    let bootstrap = format!(
        "{CODE_TOOLS}\n{values}\nexport const {RUN_CELL} = {}(createCodeTools, createValueHelpers());\n",
        BOOTSTRAP.trim_end()
    );
    builder
        .source(Source::module(BOOTSTRAP_MODULE, bootstrap))
        .build()
        .map_err(|error| match error {
            djs_embed::Error::Syntax { source, message } if source == PRELUDE_SCRIPT => {
                Unprepared::Prelude(format!(
                    "failed to compile the Code Mode prelude: {message}"
                ))
            }
            error => Unprepared::Bootstrap(format!(
                "failed to compile the embedded Code Mode bootstrap: {error}"
            )),
        })
}

struct Prepared {
    tools: Weak<CellTools>,
    prelude: Option<Arc<str>>,
    snapshot: Snapshot,
}

pub(crate) fn snapshot(
    tools: &Arc<CellTools>,
    prelude: Option<&Arc<str>>,
) -> Result<Snapshot, Unprepared> {
    type Snapshots = Vec<Arc<Prepared>>;
    static SNAPSHOTS: OnceLock<Atom<Snapshots>> = OnceLock::new();
    let snapshots = SNAPSHOTS.get_or_init(Atom::default);
    let find = |list: &Snapshots| {
        list.iter()
            .find(|prepared| {
                std::ptr::eq(prepared.tools.as_ptr(), Arc::as_ptr(tools))
                    && prepared.prelude.as_deref() == prelude.map(|prelude| &**prelude)
            })
            .map(|prepared| prepared.snapshot.clone())
    };
    if let Some(snapshot) = snapshots.read(find) {
        return Ok(snapshot);
    }
    let prepared = Arc::new(Prepared {
        tools: Arc::downgrade(tools),
        prelude: prelude.cloned(),
        snapshot: build(tools, prelude.map(|prelude| &**prelude))?,
    });
    Ok(snapshots.update(|list| {
        find(list).unwrap_or_else(|| {
            list.retain(|prepared| prepared.tools.strong_count() > 0);
            list.push(Arc::clone(&prepared));
            prepared.snapshot.clone()
        })
    }))
}

pub(crate) enum Reply<'a> {
    Nothing,
    Number(f64),
    Text(Cow<'a, str>),
    Lossy(Vec<u8>),
    Bytes(Vec<u8>),
}

pub(crate) struct ExecutionState {
    pub(crate) execution_id: u64,
    pub(crate) events: VecDeque<RuntimeEvent>,
    stored: HashMap<String, Arc<RawValue>>,
    tools: Option<Arc<CellTools>>,
    pending_tools: HashSet<u64>,
    pending_timeouts: HashMap<u64, PendingTimeout>,
    timeout_order: BTreeSet<(Instant, u64)>,
    next_tool_id: u64,
    next_timeout_id: u64,
    pending_text: Option<String>,
    journal: Option<JournalWriter>,
    replay: Option<Replay>,
    settling: bool,
    started: Instant,
}

struct Replay {
    entries: Vec<JournalEntry>,
    pos: usize,
    diverged: Option<String>,
}

enum Expectation {
    Live(JournalEntry),
    Replayed(JournalEntry),
    Diverged,
}

pub(crate) enum Replaying {
    Live,
    Input(JournalEntry),
    Diverged(String),
}

const PENDING_TEXT_BYTES: usize = 64 * 1024;

impl ExecutionState {
    pub(crate) fn new() -> Self {
        Self {
            execution_id: 0,
            events: VecDeque::new(),
            stored: HashMap::new(),
            tools: None,
            pending_tools: HashSet::new(),
            pending_timeouts: HashMap::new(),
            timeout_order: BTreeSet::new(),
            next_tool_id: 1,
            next_timeout_id: 1,
            pending_text: None,
            journal: None,
            replay: None,
            settling: false,
            started: Instant::now(),
        }
    }

    pub(crate) fn begin(
        &mut self,
        execution_id: u64,
        stored: HashMap<String, Arc<RawValue>>,
        tools: Arc<CellTools>,
        journaling: Option<CellJournaling>,
    ) {
        let (journal, replay) = journaling.map_or((None, None), |journaling| {
            (
                Some(journaling.journal.writer()),
                journaling.replay.map(|entries| Replay {
                    entries,
                    pos: 0,
                    diverged: None,
                }),
            )
        });
        *self = Self {
            execution_id,
            events: std::mem::take(&mut self.events),
            stored,
            tools: Some(tools),
            journal,
            replay,
            ..Self::new()
        };
    }

    pub(crate) fn clear(&mut self) {
        self.tools = None;
        self.pending_tools.clear();
        self.pending_timeouts.clear();
        self.timeout_order.clear();
        self.journal = None;
        self.replay = None;
    }

    const fn journaling(&self) -> bool {
        self.journal.is_some() || self.replay.is_some()
    }

    fn record(&mut self, entry: JournalEntry, weight: usize) {
        if let Some(journal) = &mut self.journal {
            journal.record(entry, weight);
        }
    }

    fn expect(&mut self, wanted: JournalEntry, weight: usize) -> Expectation {
        if self
            .replay
            .as_ref()
            .is_some_and(|replay| replay.diverged.is_none() && replay.pos >= replay.entries.len())
        {
            self.finish_replay();
        }
        let Some(replay) = self.replay.as_mut() else {
            if self.journal.is_some() {
                self.record(wanted.clone(), weight);
            }
            return Expectation::Live(wanted);
        };
        if replay.diverged.is_some() {
            return Expectation::Diverged;
        }
        match replay.entries.get(replay.pos).cloned() {
            Some(found) if found.same_step(&wanted) => {
                replay.pos += 1;
                self.record(found.clone(), weight);
                Expectation::Replayed(found)
            }
            found => {
                replay.diverged = Some(divergence(replay.pos, found.as_ref(), &wanted));
                Expectation::Diverged
            }
        }
    }

    fn finish_replay(&mut self) {
        if self.replay.take().is_none() {
            return;
        }
        let now = Instant::now();
        let wall = wall_ms();
        let mut scheduled = Vec::new();
        for (id, timeout) in &mut self.pending_timeouts {
            let Some((at_ms, delay_ms)) = timeout.planned.take() else {
                continue;
            };
            let due = at_ms.saturating_add(delay_ms).saturating_sub(wall);
            timeout.deadline = now.checked_add(Duration::from_millis(due));
            if let Some(deadline) = timeout.deadline {
                scheduled.push((deadline, *id));
            }
        }
        self.timeout_order.extend(scheduled);
        self.send(RuntimeEvent::Replayed {
            cell_id: self.execution_id,
        });
    }

    pub(crate) fn replaying(&mut self, finished: bool) -> Replaying {
        let Some(replay) = self.replay.as_ref() else {
            return Replaying::Live;
        };
        if let Some(reason) = &replay.diverged {
            return Replaying::Diverged(reason.clone());
        }
        let position = replay.pos;
        let Some(entry) = replay.entries.get(position).cloned() else {
            self.finish_replay();
            return Replaying::Live;
        };
        if finished {
            return Replaying::Diverged(format!(
                "the script finished before journal entry {position} ({})",
                entry.describe()
            ));
        }
        let known = match &entry {
            JournalEntry::Result { id, .. } => self.pending_tools.contains(id),
            JournalEntry::Timer { id } => self.pending_timeouts.contains_key(id),
            _ => false,
        };
        if !known {
            return Replaying::Diverged(format!(
                "the script did not reach {} that the journal recorded at entry {position}",
                entry.describe()
            ));
        }
        if let Some(replay) = self.replay.as_mut() {
            replay.pos += 1;
        }
        Replaying::Input(entry)
    }

    pub(crate) fn random_bytes(&mut self, live: Vec<u8>) -> Vec<u8> {
        if !self.journaling() {
            return live;
        }
        let length = live.len();
        match self.expect(JournalEntry::Bytes { data: live }, length) {
            Expectation::Live(JournalEntry::Bytes { data })
            | Expectation::Replayed(JournalEntry::Bytes { data }) => data,
            _ => vec![0; length],
        }
    }

    pub(crate) fn send(&mut self, event: RuntimeEvent) {
        self.flush_text();
        if let Some(journal) = &mut self.journal {
            journal.publish();
        }
        self.events.push_back(event);
    }

    pub(crate) fn flush_text(&mut self) {
        let Some(text) = self.pending_text.take() else {
            return;
        };
        self.events.push_back(RuntimeEvent::Content {
            cell_id: self.execution_id,
            content: ToolOutputContent::InputText { text },
        });
    }

    fn push_text(&mut self, text: String) {
        match self.pending_text.as_mut() {
            Some(buffered) => {
                buffered.push('\n');
                buffered.push_str(&text);
            }
            None => self.pending_text = Some(text),
        }
        if self
            .pending_text
            .as_ref()
            .is_some_and(|buffered| buffered.len() >= PENDING_TEXT_BYTES)
        {
            self.flush_text();
        }
    }

    pub(crate) fn settling(&mut self) -> bool {
        std::mem::take(&mut self.settling)
    }

    pub(crate) fn host_call(&mut self, call: &HostCall<'_>) -> Result<Reply<'_>, Throw> {
        match call.fn_id() {
            TOOL => {
                let name = text(call, 0)?;
                let input_json = text(call, 1)?;
                let host = call.arg(2).as_bool().unwrap_or(false);
                self.tool(name, &input_json, host)
                    .map(|id| Reply::Number(id as f64))
            }
            CONTENT => self.content(&text(call, 0)?).map(|()| Reply::Nothing),
            TEXT => {
                self.push_text(text(call, 0)?);
                Ok(Reply::Nothing)
            }
            LOAD => {
                let key = text(call, 0)?;
                Ok(self.stored.get(&key).map_or(Reply::Nothing, |value| {
                    Reply::Text(Cow::Borrowed(value.get()))
                }))
            }
            NOTIFY => {
                let text = text(call, 0)?;
                self.send(RuntimeEvent::Notify {
                    cell_id: self.execution_id,
                    text,
                });
                Ok(Reply::Nothing)
            }
            YIELD => {
                self.send(RuntimeEvent::Yielded {
                    cell_id: self.execution_id,
                });
                Ok(Reply::Nothing)
            }
            SET_TIMEOUT => {
                let delay_ms = normalize_delay_ms(call.arg(0).as_number().unwrap_or(0.0));
                Ok(Reply::Number(self.set_timeout(delay_ms) as f64))
            }
            CLEAR_TIMEOUT => {
                let id = normalize_delay_ms(call.arg(0).as_number().unwrap_or(0.0));
                if let Some(deadline) = self
                    .pending_timeouts
                    .remove(&id)
                    .and_then(|timeout| timeout.deadline)
                {
                    self.timeout_order.remove(&(deadline, id));
                }
                Ok(Reply::Nothing)
            }
            VALUE => {
                let kind = call.arg(0).as_number().unwrap_or(0.0) as u8;
                let live = match kind {
                    0 => wall_ms() as f64,
                    1 => self.started.elapsed().as_secs_f64() * 1000.0,
                    _ => call.arg(1).as_number().unwrap_or(0.0),
                };
                Ok(Reply::Number(self.value(kind, live)))
            }
            SETTLED => {
                self.settling = true;
                Ok(Reply::Nothing)
            }
            CATALOG => {
                let schemas = call.arg(0).as_number().is_some_and(|kind| kind != 0.0);
                Ok(self.tools.as_ref().map_or(Reply::Nothing, |tools| {
                    Reply::Text(Cow::Borrowed(if schemas {
                        &tools.schemas
                    } else {
                        &tools.descriptions
                    }))
                }))
            }
            _ => crate::bytes::host_call(self, call),
        }
    }

    fn tool(&mut self, name: String, input_json: &str, host: bool) -> Result<u64, Throw> {
        let input = serde_json::from_str(input_json)
            .map_err(|error| Throw::type_error(format!("invalid tool input: {error}")))?;
        let id = self.next_tool_id;
        self.next_tool_id = self.next_tool_id.saturating_add(1);
        self.pending_tools.insert(id);
        let cell_id = self.execution_id;
        if !self.journaling() {
            self.send(RuntimeEvent::ToolCall {
                cell_id,
                id,
                name,
                input,
                host,
            });
            return Ok(id);
        }
        let weight = name.len().saturating_add(input_json.len());
        match self.expect(
            JournalEntry::Call {
                id,
                name,
                input,
                host,
            },
            weight,
        ) {
            Expectation::Live(JournalEntry::Call {
                id,
                name,
                input,
                host,
            }) => self.send(RuntimeEvent::ToolCall {
                cell_id,
                id,
                name,
                input,
                host,
            }),
            Expectation::Live(_) | Expectation::Replayed(_) => self.flush_text(),
            Expectation::Diverged => {}
        }
        Ok(id)
    }

    fn content(&mut self, content_json: &str) -> Result<(), Throw> {
        let content: ToolOutputContent = serde_json::from_str(content_json)
            .map_err(|error| Throw::type_error(format!("invalid output content: {error}")))?;
        let content = match content {
            ToolOutputContent::InputAudio { ref audio_url }
                if crate::audio::wav_duration_seconds(audio_url)
                    .is_some_and(|duration| duration < 0.025) =>
            {
                ToolOutputContent::InputText {
                    text: "Audio output omitted because the clip is shorter than 25 ms; use a longer clip.".to_owned(),
                }
            }
            content => content,
        };
        match content {
            ToolOutputContent::InputText { text } => self.push_text(text),
            content => self.send(RuntimeEvent::Content {
                cell_id: self.execution_id,
                content,
            }),
        }
        Ok(())
    }

    fn set_timeout(&mut self, delay_ms: u64) -> u64 {
        let id = self.next_timeout_id;
        self.next_timeout_id = self.next_timeout_id.saturating_add(1);
        let mut deadline = Instant::now().checked_add(Duration::from_millis(delay_ms));
        let mut planned = None;
        if self.journaling() {
            let wanted = JournalEntry::Timeout {
                id,
                delay_ms,
                at_ms: wall_ms(),
            };
            match self.expect(wanted, 0) {
                Expectation::Replayed(JournalEntry::Timeout { at_ms, .. }) => {
                    planned = Some((at_ms, delay_ms));
                    deadline = None;
                }
                Expectation::Diverged => deadline = None,
                Expectation::Live(_) | Expectation::Replayed(_) => {}
            }
        }
        if let Some(deadline) = deadline {
            self.timeout_order.insert((deadline, id));
        }
        self.pending_timeouts
            .insert(id, PendingTimeout { deadline, planned });
        id
    }

    fn value(&mut self, kind: u8, live: f64) -> f64 {
        if !self.journaling() {
            return live;
        }
        let wanted = match kind {
            0 => JournalEntry::Clock { value: live },
            1 => JournalEntry::Monotonic { value: live },
            _ => JournalEntry::Random { value: live },
        };
        match self.expect(wanted, 0) {
            Expectation::Replayed(
                JournalEntry::Clock { value }
                | JournalEntry::Monotonic { value }
                | JournalEntry::Random { value },
            ) => value,
            _ => live,
        }
    }

    pub(crate) fn resolve_tool(
        &mut self,
        id: u64,
        encoded: &str,
        success: bool,
    ) -> Result<(), String> {
        if !self.pending_tools.remove(&id) {
            return Err(format!(
                "embedded Code Mode received a result for unknown tool call {id}"
            ));
        }
        if self.journal.is_some() {
            self.record(
                JournalEntry::Result {
                    id,
                    encoded: encoded.to_owned(),
                    success,
                },
                encoded.len(),
            );
        }
        Ok(())
    }

    pub(crate) fn fire_timeout(&mut self, id: u64) -> bool {
        let Some(timeout) = self.pending_timeouts.remove(&id) else {
            return false;
        };
        if let Some(deadline) = timeout.deadline {
            self.timeout_order.remove(&(deadline, id));
        }
        self.record(JournalEntry::Timer { id }, 0);
        true
    }

    pub(crate) fn next_timeout_wait(&self) -> Option<Duration> {
        let now = Instant::now();
        self.timeout_order
            .first()
            .map(|(deadline, _)| deadline.saturating_duration_since(now))
    }

    pub(crate) fn next_due_timeout(&self, at: Instant) -> Option<u64> {
        self.timeout_order
            .first()
            .filter(|(deadline, _)| *deadline <= at)
            .map(|(_, id)| *id)
    }

    pub(crate) fn send_terminal(&mut self, encoded: &str) -> Result<(), String> {
        let terminal: ExecutionTerminal = serde_json::from_str(encoded)
            .map_err(|error| format!("embedded Code Mode returned an invalid result: {error}"))?;
        let cell_id = self.execution_id;
        let stored = terminal
            .stored
            .into_iter()
            .map(|(key, value)| (key, Arc::from(value)))
            .collect();
        let event = match (terminal.kind, terminal.message) {
            (TerminalKind::Done, _) => RuntimeEvent::Done { cell_id, stored },
            (TerminalKind::Error, Some(message)) => RuntimeEvent::Error {
                cell_id,
                message,
                stored,
            },
            (TerminalKind::Error, None) => {
                return Err(
                    "embedded Code Mode returned an error result without a message".to_owned(),
                );
            }
        };
        self.send(event);
        Ok(())
    }

    pub(crate) fn send_waiting(&mut self) {
        if self.journal.is_some() {
            self.send(RuntimeEvent::Waiting {
                cell_id: self.execution_id,
            });
        }
    }

    pub(crate) fn send_checkpoint(&mut self) {
        if self.replay.is_none()
            && self
                .journal
                .as_ref()
                .is_some_and(JournalWriter::unpublished)
        {
            self.send(RuntimeEvent::Waiting {
                cell_id: self.execution_id,
            });
        }
    }
}

struct PendingTimeout {
    deadline: Option<Instant>,
    planned: Option<(u64, u64)>,
}

pub(crate) fn text(call: &HostCall<'_>, index: usize) -> Result<String, Throw> {
    call.arg(index)
        .as_str()
        .map(Cow::into_owned)
        .ok_or_else(|| Throw::type_error(format!("argument {index} must be a string")))
}

fn wall_ms() -> u64 {
    nanocode_platform::system_time()
        .duration_since(nanocode_platform::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}

fn divergence(position: usize, found: Option<&JournalEntry>, wanted: &JournalEntry) -> String {
    match found {
        Some(found) => format!(
            "at journal entry {position} the script reached {} where the journal recorded {}",
            wanted.describe(),
            found.describe()
        ),
        None => format!(
            "the script reached {} past the journal's end",
            wanted.describe()
        ),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum TerminalKind {
    Done,
    Error,
}

#[derive(Deserialize)]
struct ExecutionTerminal {
    #[serde(rename = "type")]
    kind: TerminalKind,
    message: Option<String>,
    #[serde(default)]
    stored: HashMap<String, Box<RawValue>>,
}

pub struct CellTools {
    definitions: String,
    descriptions: String,
    schemas: String,
}

const DESCRIPTION_FIELD: &str = "description";
const SCHEMA_FIELDS: [&str; 2] = ["input_schema", "output_schema"];

impl CellTools {
    pub fn encode(tools: &[Value]) -> Result<Self, String> {
        struct Definition<'a>(&'a Value);

        impl Serialize for Definition<'_> {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                match self.0 {
                    Value::Object(fields) => {
                        serializer.collect_map(fields.iter().filter(|(key, _)| {
                            *key != DESCRIPTION_FIELD && !SCHEMA_FIELDS.contains(&key.as_str())
                        }))
                    }
                    other => other.serialize(serializer),
                }
            }
        }

        let definitions = serde_json::to_string(&tools.iter().map(Definition).collect::<Vec<_>>())
            .map_err(|error| format!("failed to encode Code Mode tool metadata: {error}"))?;
        let descriptions = serde_json::to_string(
            &tools
                .iter()
                .map(|tool| tool.get(DESCRIPTION_FIELD).unwrap_or(&Value::Null))
                .collect::<Vec<_>>(),
        )
        .map_err(|error| format!("failed to encode Code Mode tool descriptions: {error}"))?;
        let schemas = serde_json::to_string(
            &tools
                .iter()
                .map(|tool| SCHEMA_FIELDS.map(|field| tool.get(field).unwrap_or(&Value::Null)))
                .collect::<Vec<_>>(),
        )
        .map_err(|error| format!("failed to encode Code Mode tool schemas: {error}"))?;
        Ok(Self {
            definitions,
            descriptions,
            schemas,
        })
    }
}

fn normalize_delay_ms(value: f64) -> u64 {
    if !value.is_finite() || value <= 0.0 {
        0
    } else {
        value.trunc() as u64
    }
}
