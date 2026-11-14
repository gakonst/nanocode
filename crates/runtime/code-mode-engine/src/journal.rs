use std::sync::Arc;

use nanocode_sync::Atom;
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

#[derive(Clone, Default)]
pub struct SharedJournal(Arc<Atom<Published>>);

#[derive(Default)]
enum Published {
    #[default]
    Empty,
    Pages(Arc<Page>),
    Overflowed,
}

struct Page {
    entries: Vec<JournalEntry>,
    end: usize,
    previous: Option<Arc<Self>>,
}

impl Drop for Page {
    fn drop(&mut self) {
        let mut previous = self.previous.take();
        while let Some(page) = previous {
            previous = Arc::try_unwrap(page)
                .ok()
                .and_then(|mut page| page.previous.take());
        }
    }
}

impl SharedJournal {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn overflowed(&self) -> bool {
        self.0
            .read(|published| matches!(published, Published::Overflowed))
    }

    #[must_use]
    pub fn entries(&self) -> Option<Vec<JournalEntry>> {
        self.entries_from(0)
    }

    #[must_use]
    pub fn entries_from(&self, from: usize) -> Option<Vec<JournalEntry>> {
        let head = match &*self.0.get() {
            Published::Empty => return Some(Vec::new()),
            Published::Overflowed => return None,
            Published::Pages(head) => Arc::clone(head),
        };
        let mut pages = Vec::new();
        let mut page = Some(&head);
        while let Some(current) = page.filter(|current| current.end > from) {
            pages.push(current);
            page = current.previous.as_ref();
        }
        let mut entries = Vec::with_capacity(head.end.saturating_sub(from));
        for page in pages.into_iter().rev() {
            let start = page.end - page.entries.len();
            entries.extend_from_slice(&page.entries[from.saturating_sub(start)..]);
        }
        Some(entries)
    }

    pub(crate) fn writer(&self) -> JournalWriter {
        JournalWriter {
            shared: self.clone(),
            state: Writing::Recording {
                pending: Vec::new(),
                head: None,
                len: 0,
                bytes: 0,
            },
        }
    }
}

pub(crate) struct JournalWriter {
    shared: SharedJournal,
    state: Writing,
}

enum Writing {
    Recording {
        pending: Vec<JournalEntry>,
        head: Option<Arc<Page>>,
        len: usize,
        bytes: usize,
    },
    Overflowed {
        published: bool,
    },
}

impl JournalWriter {
    pub(crate) fn record(&mut self, entry: JournalEntry, weight: usize) {
        let Writing::Recording {
            pending,
            len,
            bytes,
            ..
        } = &mut self.state
        else {
            return;
        };
        *bytes = bytes.saturating_add(weight.saturating_add(ENTRY_BYTES));
        if *bytes > JOURNAL_LIMIT_BYTES {
            self.state = Writing::Overflowed { published: false };
            return;
        }
        pending.push(entry);
        *len += 1;
    }

    pub(crate) fn unpublished(&self) -> bool {
        match &self.state {
            Writing::Recording { pending, .. } => !pending.is_empty(),
            Writing::Overflowed { published } => !published,
        }
    }

    pub(crate) fn publish(&mut self) {
        match &mut self.state {
            Writing::Recording {
                pending, head, len, ..
            } => {
                if pending.is_empty() {
                    return;
                }
                let page = Arc::new(Page {
                    entries: std::mem::take(pending),
                    end: *len,
                    previous: head.take(),
                });
                *head = Some(Arc::clone(&page));
                self.shared.0.set(Published::Pages(page));
            }
            Writing::Overflowed { published } => {
                if !*published {
                    *published = true;
                    self.shared.0.set(Published::Overflowed);
                }
            }
        }
    }
}

pub struct CellJournaling {
    pub journal: SharedJournal,
    pub replay: Option<Vec<JournalEntry>>,
    pub started_at_ms: Option<u64>,
}
