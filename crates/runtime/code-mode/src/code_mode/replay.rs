use std::{
    collections::{BTreeMap, HashMap, HashSet},
    sync::{Arc, Mutex, MutexGuard, PoisonError, Weak},
};

use serde::{Deserialize, Serialize};
use serde_json::{Value, value::RawValue};
use tokio::sync::watch;

use nanocode_code_mode_engine::{JournalEntry, SharedJournal};

pub const CELL_JOURNAL_VERSION: u32 = 1;

const UNKNOWN_OUTCOME: &str = "This nested call was running when its cell moved to another host; its outcome is unknown. Do not assume it did not run or automatically repeat it.";

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

impl CellJournal {
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
    cells: Mutex<Vec<Weak<CellRecord>>>,
    held: Mutex<HashSet<String>>,
    restored: Mutex<HashMap<String, Restored>>,
}

pub(super) struct Restored {
    pub(super) cells: Vec<CellJournal>,
    pub(super) fallback: Option<String>,
    pub(super) outcomes: HashMap<String, (Value, bool)>,
}

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl CellJournals {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub(super) fn register(&self, record: &Arc<CellRecord>) {
        let mut cells = locked(&self.0.cells);
        cells.retain(|cell| cell.strong_count() > 0);
        cells.push(Arc::downgrade(record));
    }

    #[must_use]
    pub fn holding(&self, session_id: &str) -> bool {
        locked(&self.0.held).contains(session_id)
    }

    fn live(&self, session_id: &str) -> Vec<Arc<CellRecord>> {
        locked(&self.0.cells)
            .iter()
            .filter_map(Weak::upgrade)
            .filter(|record| record.session_id == session_id && !record.status().retired)
            .collect()
    }

    pub fn hold(&self, session_id: &str) {
        locked(&self.0.held).insert(session_id.to_owned());
        for record in self.live(session_id) {
            record.hold.send_replace(true);
        }
    }

    pub fn release(&self, session_id: &str) {
        locked(&self.0.held).remove(session_id);
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
            let overflowed = record.overflowed();
            let status = record.status();
            state.cells += 1;
            if status.held || status.finished {
                state.held += 1;
            }
            state.running_calls += status
                .calls
                .values()
                .filter(|call| matches!(call.state, JournaledCallState::Running))
                .count();
            state.replayable &= !status.unsupported && !overflowed;
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
        locked(&self.0.restored).insert(
            session_id.to_owned(),
            Restored {
                cells,
                fallback,
                outcomes,
            },
        );
    }

    #[must_use]
    pub fn has_restored(&self, session_id: &str) -> bool {
        locked(&self.0.restored).contains_key(session_id)
    }

    pub(super) fn take_restored(&self, session_id: &str) -> Option<Restored> {
        locked(&self.0.restored).remove(session_id)
    }
}

pub(super) struct CellRecord {
    pub(super) cell_id: u64,
    pub(super) session_id: String,
    pub(super) origin_call_id: String,
    pub(super) script: String,
    pub(super) tools: Vec<Value>,
    pub(super) stored: HashMap<String, Arc<RawValue>>,
    pub(super) prelude: bool,
    pub(super) yield_time_ms: u64,
    pub(super) max_output_tokens: Option<usize>,
    pub(super) journal: SharedJournal,
    pub(super) status: Mutex<CellStatus>,
    pub(super) hold: watch::Sender<bool>,
}

#[derive(Default)]
pub(super) struct CellStatus {
    pub(super) observer_call_id: Option<String>,
    pub(super) held: bool,
    pub(super) replaying: bool,
    pub(super) finished: bool,
    pub(super) unsupported: bool,
    pub(super) retired: bool,
    pub(super) content_returned: u64,
    pub(super) calls: BTreeMap<u64, JournaledCall>,
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
                let (value, success) =
                    self.outcomes
                        .get(&call.call_id)
                        .cloned()
                        .unwrap_or_else(|| {
                            (
                                serde_json::json!({
                                    "error": UNKNOWN_OUTCOME,
                                    "code": "CODE_MODE_CALL_INTERRUPTED",
                                    "outcome": "unknown",
                                }),
                                false,
                            )
                        });
                (value.to_string(), success)
            }
        }
    }

    pub(super) fn divergence(&self, cell_id: u64, reason: &str) -> String {
        divergence_text(self.fallback.as_deref(), cell_id, reason)
    }
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
    pub(super) fn status(&self) -> MutexGuard<'_, CellStatus> {
        locked(&self.status)
    }

    fn overflowed(&self) -> bool {
        locked(&self.journal).overflowed()
    }

    fn snapshot(&self) -> Option<CellJournal> {
        let entries = locked(&self.journal).entries()?.to_vec();
        let status = self.status();
        if status.unsupported || !(status.held || status.finished) {
            return None;
        }
        Some(CellJournal {
            version: CELL_JOURNAL_VERSION,
            cell_id: self.cell_id,
            session_id: self.session_id.clone(),
            origin_call_id: self.origin_call_id.clone(),
            observer_call_id: status.observer_call_id.clone(),
            script: self.script.clone(),
            tools: self.tools.clone(),
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
            calls: status.calls.values().cloned().collect(),
        })
    }
}
