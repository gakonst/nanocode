use std::{
    collections::{BTreeMap, HashMap, HashSet},
    sync::{Arc, Weak},
};

use serde::{Deserialize, Serialize};
use serde_json::{Value, value::RawValue};
use tokio::sync::watch;

use nanocode_code_mode_engine::{JournalEntry, SharedJournal};
use nanocode_sync::{Atom, Owned};

use crate::embedded::HostFuture;

pub const CELL_JOURNAL_VERSION: u32 = 1;

pub(super) const UNKNOWN_OUTCOME: &str = "This nested call was running when its cell moved to another host; its outcome is unknown. Do not assume it did not run or automatically repeat it.";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CellJournal {
    pub version: u32,
    pub cell_id: u64,
    pub session_id: String,
    pub origin_call_id: String,
    #[serde(default)]
    pub observer_call_id: Option<String>,
    pub script: String,
    pub tools: Vec<Value>,
    #[serde(default)]
    pub stored: BTreeMap<String, Value>,
    #[serde(default)]
    pub prelude: bool,
    pub yield_time_ms: u64,
    #[serde(default)]
    pub max_output_tokens: Option<usize>,
    #[serde(default)]
    pub content_returned: u64,
    pub entries: Vec<JournalEntry>,
    #[serde(default)]
    pub calls: Vec<JournaledCall>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JournaledCall {
    pub id: u64,
    pub call_id: String,
    pub name: String,
    pub input: Value,
    #[serde(default)]
    pub host: bool,
    pub state: JournaledCallState,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum JournaledCallState {
    Queued,
    Running,
    Completed { encoded: String, success: bool },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CellJournalHeader {
    pub version: u32,
    pub origin_call_id: String,
    pub script: String,
    pub tools: Vec<Value>,
    #[serde(default)]
    pub stored: BTreeMap<String, Value>,
    #[serde(default)]
    pub prelude: bool,
    pub yield_time_ms: u64,
    #[serde(default)]
    pub max_output_tokens: Option<usize>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CellJournalUpdate {
    pub session_id: String,
    pub cell_id: u64,
    pub seq: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header: Option<CellJournalHeader>,
    pub entries_from: usize,
    #[serde(default)]
    pub entries: Vec<JournalEntry>,
    #[serde(default)]
    pub calls: Vec<JournaledCall>,
    #[serde(default)]
    pub observer_call_id: Option<String>,
    #[serde(default)]
    pub content_returned: u64,
}

pub trait CellJournalSink: Send + Sync + 'static {
    fn save(&self, update: CellJournalUpdate) -> HostFuture<'static, Result<(), String>>;

    fn forget(&self, session_id: &str, cell_id: u64) -> HostFuture<'static, ()>;
}

impl CellJournal {
    #[must_use]
    pub fn fold(updates: impl IntoIterator<Item = CellJournalUpdate>) -> Option<Self> {
        let mut updates = updates.into_iter().collect::<Vec<_>>();
        updates.sort_by_key(|update| update.seq);
        let start = updates.iter().position(|update| update.header.is_some())?;
        let mut updates = updates.into_iter().skip(start);
        let mut first = updates.next()?;
        let header = first.header.take()?;
        let mut journal = Self {
            version: header.version,
            cell_id: first.cell_id,
            session_id: first.session_id.clone(),
            origin_call_id: header.origin_call_id,
            observer_call_id: None,
            script: header.script,
            tools: header.tools,
            stored: header.stored,
            prelude: header.prelude,
            yield_time_ms: header.yield_time_ms,
            max_output_tokens: header.max_output_tokens,
            content_returned: 0,
            entries: Vec::new(),
            calls: Vec::new(),
        };
        let mut calls = BTreeMap::new();
        let mut seq = first.seq;
        for update in std::iter::once(first).chain(updates) {
            if update.seq != seq
                || update.cell_id != journal.cell_id
                || update.entries_from > journal.entries.len()
            {
                break;
            }
            seq = seq.saturating_add(1);
            let known = journal.entries.len() - update.entries_from;
            journal
                .entries
                .extend(update.entries.into_iter().skip(known));
            for call in update.calls {
                calls.insert(call.id, call);
            }
            if update.observer_call_id.is_some() {
                journal.observer_call_id = update.observer_call_id;
            }
            journal.content_returned = journal.content_returned.max(update.content_returned);
        }
        let delivered = journal
            .entries
            .iter()
            .filter_map(|entry| match entry {
                JournalEntry::Result { id, .. } => Some(*id),
                _ => None,
            })
            .collect::<HashSet<_>>();
        journal.calls = calls
            .into_values()
            .filter(|call| !delivered.contains(&call.id))
            .collect();
        Some(journal)
    }

    pub fn unknown_calls(&self) -> impl Iterator<Item = &JournaledCall> {
        self.calls
            .iter()
            .filter(|call| matches!(call.state, JournaledCallState::Running))
    }

    pub(super) fn stored_values(&self) -> HashMap<String, Arc<RawValue>> {
        self.stored
            .iter()
            .filter_map(|(key, value)| {
                serde_json::value::to_raw_value(value)
                    .ok()
                    .map(|raw| (key.clone(), Arc::from(raw)))
            })
            .collect()
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CellJournalState {
    pub cells: usize,
    pub held: usize,
    pub running_calls: usize,
    pub replayable: bool,
}

impl CellJournalState {
    #[must_use]
    pub const fn settled(&self) -> bool {
        self.replayable && self.held == self.cells && self.running_calls == 0
    }
}

#[derive(Clone, Default)]
pub struct CellJournals(Arc<Journals>);

#[derive(Default)]
struct Journals {
    cells: Atom<Vec<Weak<CellRecord>>>,
    held: Atom<HashSet<String>>,
    restored: Atom<HashMap<String, Arc<Restored>>>,
    sink: Option<Arc<dyn CellJournalSink>>,
}

pub(super) struct Persistence {
    sink: Arc<dyn CellJournalSink>,
    saving: Owned<Saving>,
}

enum Saving {
    Unsaved,
    Saved { persisted: usize, next_seq: u64 },
    Stopped,
}

#[derive(Clone)]
pub(super) struct Restored {
    pub(super) cells: Vec<CellJournal>,
    pub(super) fallback: Option<String>,
    pub(super) outcomes: HashMap<String, (Value, bool)>,
}

impl CellJournals {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn with_sink(sink: Arc<dyn CellJournalSink>) -> Self {
        Self(Arc::new(Journals {
            sink: Some(sink),
            ..Journals::default()
        }))
    }

    pub(super) fn persistence(&self) -> Option<Persistence> {
        self.0.sink.as_ref().map(|sink| Persistence {
            sink: Arc::clone(sink),
            saving: Owned::new(Saving::Unsaved),
        })
    }

    pub(super) fn register(&self, record: &Arc<CellRecord>) {
        let record = Arc::downgrade(record);
        self.0.cells.update(|cells| {
            cells.retain(|cell| cell.strong_count() > 0);
            cells.push(Weak::clone(&record));
        });
    }

    #[must_use]
    pub fn holding(&self, session_id: &str) -> bool {
        self.0.held.read(|held| held.contains(session_id))
    }

    fn live(&self, session_id: &str) -> Vec<Arc<CellRecord>> {
        self.0.cells.read(|cells| {
            cells
                .iter()
                .filter_map(Weak::upgrade)
                .filter(|record| {
                    record.session_id == session_id
                        && record.status(|status| status.life != CellLife::Retired)
                })
                .collect()
        })
    }

    pub fn hold(&self, session_id: &str) {
        self.0
            .held
            .update(|held| held.insert(session_id.to_owned()));
        for record in self.live(session_id) {
            record.hold.send_replace(true);
        }
    }

    pub fn release(&self, session_id: &str) {
        self.0.held.update(|held| held.remove(session_id));
        for record in self.live(session_id) {
            record.hold.send_replace(false);
        }
    }

    #[must_use]
    pub fn state(&self, session_id: &str) -> CellJournalState {
        let mut state = CellJournalState {
            replayable: true,
            ..CellJournalState::default()
        };
        for record in self.live(session_id) {
            let overflowed = record.journal.overflowed();
            record.status(|status| {
                state.cells += 1;
                if status.life.settled() {
                    state.held += 1;
                }
                state.running_calls += status
                    .calls
                    .values()
                    .filter(|call| matches!(call.state, JournaledCallState::Running))
                    .count();
                state.replayable &= !status.unsupported && !overflowed;
            });
        }
        state
    }

    #[must_use]
    pub fn journals(&self, session_id: &str) -> Option<Vec<CellJournal>> {
        let mut journals = Vec::new();
        for record in self.live(session_id) {
            journals.push(record.snapshot()?);
        }
        journals.sort_by_key(|journal| journal.cell_id);
        Some(journals)
    }

    pub fn restore(
        &self,
        session_id: &str,
        cells: Vec<CellJournal>,
        fallback: Option<String>,
        outcomes: HashMap<String, (Value, bool)>,
    ) {
        if cells.is_empty() {
            return;
        }
        let restored = Arc::new(Restored {
            cells,
            fallback,
            outcomes,
        });
        self.0.restored.update(|map| {
            map.insert(session_id.to_owned(), Arc::clone(&restored));
        });
    }

    #[must_use]
    pub fn has_restored(&self, session_id: &str) -> bool {
        self.0.restored.read(|map| map.contains_key(session_id))
    }

    pub(super) fn take_restored(&self, session_id: &str) -> Option<Restored> {
        self.0
            .restored
            .try_update(|map| map.remove(session_id))
            .map(Arc::unwrap_or_clone)
    }
}

pub(super) struct CellRecord {
    pub(super) cell_id: u64,
    pub(super) session_id: String,
    pub(super) origin_call_id: String,
    pub(super) script: String,
    pub(super) tools: Arc<[Value]>,
    pub(super) stored: HashMap<String, Arc<RawValue>>,
    pub(super) prelude: bool,
    pub(super) yield_time_ms: u64,
    pub(super) max_output_tokens: Option<usize>,
    pub(super) journal: SharedJournal,
    pub(super) status: Atom<CellStatus>,
    pub(super) hold: watch::Sender<bool>,
    pub(super) persistence: Option<Persistence>,
}

#[derive(Clone)]
pub(super) struct CellStatus {
    pub(super) observer_call_id: Option<String>,
    pub(super) life: CellLife,
    pub(super) unsupported: bool,
    pub(super) content_returned: u64,
    pub(super) calls: BTreeMap<u64, Arc<JournaledCall>>,
}

impl CellStatus {
    pub(super) const fn new(life: CellLife) -> Self {
        Self {
            observer_call_id: None,
            life,
            unsupported: false,
            content_returned: 0,
            calls: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CellLife {
    Active { replay: Replay, hold: Hold },
    Finished,
    Retired,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Replay {
    Replaying,
    Live,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Hold {
    Running,
    Held,
}

impl CellLife {
    pub(super) const fn started(replay: Replay) -> Self {
        Self::Active {
            replay,
            hold: Hold::Running,
        }
    }

    #[must_use]
    pub(super) const fn holding(self, hold: Hold) -> Self {
        match self {
            Self::Active { replay, .. } => Self::Active { replay, hold },
            ended => ended,
        }
    }

    #[must_use]
    pub(super) const fn replayed(self) -> Self {
        match self {
            Self::Active { hold, .. } => Self::Active {
                replay: Replay::Live,
                hold,
            },
            ended => ended,
        }
    }

    #[must_use]
    pub(super) const fn finished(self) -> Self {
        match self {
            Self::Active { .. } => Self::Finished,
            ended => ended,
        }
    }

    pub(super) const fn replaying(self) -> bool {
        matches!(
            self,
            Self::Active {
                replay: Replay::Replaying,
                ..
            }
        )
    }

    pub(super) const fn settled(self) -> bool {
        matches!(
            self,
            Self::Active {
                hold: Hold::Held,
                ..
            } | Self::Finished
        )
    }
}

pub(super) struct ReplayPlan {
    pub(super) entries: Vec<JournalEntry>,
    pub(super) calls: Vec<JournaledCall>,
    pub(super) fallback: Option<String>,
    pub(super) outcomes: HashMap<String, (Value, bool)>,
    pub(super) content_returned: u64,
}

impl ReplayPlan {
    pub(super) fn outcome(&self, call: &JournaledCall) -> (String, bool) {
        match &call.state {
            JournaledCallState::Completed { encoded, success } => (encoded.clone(), *success),
            JournaledCallState::Running | JournaledCallState::Queued => {
                let (value, success) = self
                    .outcomes
                    .get(&call.call_id)
                    .cloned()
                    .unwrap_or_else(|| (unknown_outcome(), false));
                (value.to_string(), success)
            }
        }
    }

    pub(super) fn divergence(&self, cell_id: u64, reason: &str) -> String {
        divergence_text(self.fallback.as_deref(), cell_id, reason)
    }
}

pub(super) fn unknown_outcome() -> Value {
    serde_json::json!({
        "error": UNKNOWN_OUTCOME,
        "code": "CODE_MODE_CALL_INTERRUPTED",
        "outcome": "unknown",
    })
}

pub(super) fn divergence_text(fallback: Option<&str>, cell_id: u64, reason: &str) -> String {
    fallback.map_or_else(
        || {
            format!(
                "This cell moved to another host and could not be resumed ({reason}), so it stopped and its outcome is unknown: nested calls it made may have run. Cell {cell_id} no longer exists: don't wait on it again."
            )
        },
        str::to_owned,
    )
}

impl CellRecord {
    pub(super) fn status<R>(&self, read: impl FnOnce(&CellStatus) -> R) -> R {
        self.status.read(read)
    }

    pub(super) fn update<R>(&self, change: impl FnMut(&mut CellStatus) -> R) -> R {
        self.status.update(change)
    }

    fn stored_json(&self) -> BTreeMap<String, Value> {
        self.stored
            .iter()
            .filter_map(|(key, value)| {
                serde_json::from_str(value.get())
                    .ok()
                    .map(|value| (key.clone(), value))
            })
            .collect()
    }

    pub(super) async fn persist(&self, calls: Vec<JournaledCall>) {
        let Some(persistence) = &self.persistence else {
            return;
        };
        let mut saving = persistence.saving.acquire().await;
        let (from, seq) = match *saving {
            Saving::Stopped => return,
            Saving::Unsaved => (0, 0),
            Saving::Saved {
                persisted,
                next_seq,
            } => (persisted, next_seq),
        };
        if self.status(|status| status.life.replaying()) {
            return;
        }
        let entries = self.journal.entries_from(from);
        let Some(entries) = entries else {
            *saving = Saving::Stopped;
            persistence
                .sink
                .forget(&self.session_id, self.cell_id)
                .await;
            return;
        };
        let (observer_call_id, content_returned) =
            self.status(|status| (status.observer_call_id.clone(), status.content_returned));
        let header = matches!(*saving, Saving::Unsaved).then(|| CellJournalHeader {
            version: CELL_JOURNAL_VERSION,
            origin_call_id: self.origin_call_id.clone(),
            script: self.script.clone(),
            tools: self.tools.to_vec(),
            stored: self.stored_json(),
            prelude: self.prelude,
            yield_time_ms: self.yield_time_ms,
            max_output_tokens: self.max_output_tokens,
        });
        let persisted = from + entries.len();
        let update = CellJournalUpdate {
            session_id: self.session_id.clone(),
            cell_id: self.cell_id,
            seq,
            header,
            entries_from: from,
            entries,
            calls,
            observer_call_id,
            content_returned,
        };
        match persistence.sink.save(update).await {
            Ok(()) => {
                *saving = Saving::Saved {
                    persisted,
                    next_seq: seq + 1,
                };
            }
            Err(error) => {
                tracing::warn!(
                    target: "nanocode_code_mode",
                    cell_id = self.cell_id,
                    %error,
                    "a Code Mode cell journal could not be saved, so the cell cannot be resumed after a restart"
                );
                *saving = Saving::Stopped;
                persistence
                    .sink
                    .forget(&self.session_id, self.cell_id)
                    .await;
            }
        }
    }

    pub(super) fn forget(self: &Arc<Self>) {
        if self.persistence.is_none() {
            return;
        }
        let record = Arc::clone(self);
        super::spawn(async move {
            let Some(persistence) = &record.persistence else {
                return;
            };
            *persistence.saving.acquire().await = Saving::Stopped;
            persistence
                .sink
                .forget(&record.session_id, record.cell_id)
                .await;
        });
    }

    fn snapshot(&self) -> Option<CellJournal> {
        let entries = self.journal.entries()?;
        let status = self.status.get();
        if status.unsupported || !status.life.settled() {
            return None;
        }
        Some(CellJournal {
            version: CELL_JOURNAL_VERSION,
            cell_id: self.cell_id,
            session_id: self.session_id.clone(),
            origin_call_id: self.origin_call_id.clone(),
            observer_call_id: status.observer_call_id.clone(),
            script: self.script.clone(),
            tools: self.tools.to_vec(),
            stored: self
                .stored
                .iter()
                .filter_map(|(key, value)| {
                    serde_json::from_str(value.get())
                        .ok()
                        .map(|value| (key.clone(), value))
                })
                .collect(),
            prelude: self.prelude,
            yield_time_ms: self.yield_time_ms,
            max_output_tokens: self.max_output_tokens,
            content_returned: status.content_returned,
            entries,
            calls: status
                .calls
                .values()
                .map(|call| JournaledCall::clone(call))
                .collect(),
        })
    }
}
