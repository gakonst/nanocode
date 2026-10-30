use std::sync::{Arc, Mutex, PoisonError};

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const JOURNAL_LIMIT_BYTES: usize = 32 * 1024 * 1024;

const ENTRY_BYTES: usize = 32;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum JournalEntry {
    Call {
        id: u64,
        name: String,
        input: Value,
        host: bool,
    },
    Result {
        id: u64,
        encoded: String,
        success: bool,
    },
    Timeout {
        id: u64,
        delay_ms: u64,
        at_ms: u64,
    },
    Timer {
        id: u64,
    },
    Clock {
        value: f64,
    },
    Monotonic {
        value: f64,
    },
    Random {
        value: f64,
    },
    Bytes {
        data: Vec<u8>,
    },
}

impl JournalEntry {
    #[must_use]
    pub const fn is_input(&self) -> bool {
        matches!(self, Self::Result { .. } | Self::Timer { .. })
    }

    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            Self::Call { name, id, .. } => format!("call {name} (nested call {id})"),
            Self::Result { id, .. } => format!("the result of nested call {id}"),
            Self::Timeout { id, delay_ms, .. } => format!("timer {id} set for {delay_ms} ms"),
            Self::Timer { id } => format!("timer {id} firing"),
            Self::Clock { .. } => "a clock read".to_owned(),
            Self::Monotonic { .. } => "a performance.now() read".to_owned(),
            Self::Random { .. } => "a random number".to_owned(),
            Self::Bytes { data } => format!("{} random bytes", data.len()),
        }
    }

    pub(crate) fn same_step(&self, other: &Self) -> bool {
        match (self, other) {
            (
                Self::Timeout {
                    id: left,
                    delay_ms: left_delay,
                    ..
                },
                Self::Timeout {
                    id: right,
                    delay_ms: right_delay,
                    ..
                },
            ) => left == right && left_delay == right_delay,
            (Self::Clock { .. }, Self::Clock { .. })
            | (Self::Monotonic { .. }, Self::Monotonic { .. })
            | (Self::Random { .. }, Self::Random { .. }) => true,
            (Self::Bytes { data: left }, Self::Bytes { data: right }) => left.len() == right.len(),
            (left, right) => left == right,
        }
    }
}

#[derive(Debug, Default)]
pub struct Journal {
    entries: Vec<JournalEntry>,
    bytes: usize,
    overflowed: bool,
}

pub type SharedJournal = Arc<Mutex<Journal>>;

impl Journal {
    #[must_use]
    pub fn shared() -> SharedJournal {
        Arc::default()
    }

    pub(crate) fn record(&mut self, entry: JournalEntry, weight: usize) {
        if self.overflowed {
            return;
        }
        self.bytes = self
            .bytes
            .saturating_add(weight.saturating_add(ENTRY_BYTES));
        if self.bytes > JOURNAL_LIMIT_BYTES {
            self.overflowed = true;
            self.entries = Vec::new();
            return;
        }
        self.entries.push(entry);
    }

    #[must_use]
    pub const fn overflowed(&self) -> bool {
        self.overflowed
    }

    #[must_use]
    pub fn entries(&self) -> Option<&[JournalEntry]> {
        (!self.overflowed).then_some(self.entries.as_slice())
    }

    #[must_use]
    pub const fn len(&self) -> usize {
        self.entries.len()
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    #[must_use]
    pub const fn bytes(&self) -> usize {
        self.bytes
    }
}

pub(crate) fn lock(journal: &SharedJournal) -> std::sync::MutexGuard<'_, Journal> {
    journal.lock().unwrap_or_else(PoisonError::into_inner)
}

pub struct CellJournaling {
    pub journal: SharedJournal,
    pub replay: Option<Vec<JournalEntry>>,
}
