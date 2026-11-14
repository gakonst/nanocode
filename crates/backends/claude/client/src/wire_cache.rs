use std::sync::{
    Arc, LazyLock, Weak,
    atomic::{AtomicUsize, Ordering},
};

use nanocode_sync::{Atom, Slot};
use serde::Serialize;
use serde_json::{Value, value::RawValue};

use crate::{ClaudeError, ClaudeToolSpec, MessageDigests, MessageLayout, WireMessage};

pub(crate) struct Encoded<T> {
    value: T,
    pub(crate) raw: Box<RawValue>,
    pub(crate) digest: nanocode_digest::Hash,
    pub(crate) fingerprint: String,
}

type Entries<T> = Atom<Vec<Weak<Encoded<T>>>>;

#[derive(Clone, Default)]
struct Chain {
    at: usize,
    read: nanocode_digest::Blake3,
    sent: nanocode_digest::Blake3,
}

#[derive(Default)]
pub(crate) struct Hashed {
    digests: Vec<MessageDigests>,
    ids: Vec<Option<u64>>,
    pub(crate) read: nanocode_digest::Blake3,
    pub(crate) sent: nanocode_digest::Blake3,
    marks: Vec<Chain>,
}

fn known<M: WireMessage>(ids: &[Option<u64>], messages: &[M]) -> usize {
    ids.iter()
        .zip(messages)
        .take_while(|(kept, message)| kept.is_some() && **kept == message.wire_id())
        .count()
}

impl Hashed {
    pub(crate) fn known<M: WireMessage>(&self, messages: &[M]) -> usize {
        known(&self.ids, messages)
    }

    pub(crate) fn shared(&self, known: usize, fresh: &[MessageDigests]) -> usize {
        known
            + self.digests[known..]
                .iter()
                .zip(fresh)
                .take_while(|(kept, message)| kept.sent == message.sent)
                .count()
    }

    pub(crate) fn resume(
        self,
        shared: usize,
        known: usize,
        ids: impl Iterator<Item = Option<u64>>,
        fresh: Vec<MessageDigests>,
    ) -> Self {
        let Self {
            mut digests,
            ids: mut kept,
            read,
            sent,
            marks,
        } = self;
        let start = if digests.len() <= shared {
            Chain {
                at: digests.len(),
                read,
                sent,
            }
        } else {
            marks
                .iter()
                .rev()
                .find(|mark| mark.at <= shared)
                .cloned()
                .unwrap_or_default()
        };
        digests.truncate(known);
        digests.extend(fresh);
        kept.truncate(known);
        kept.extend(ids);
        let mut marks = marks
            .into_iter()
            .filter(|mark| {
                mark.at < start.at && digests.get(mark.at).is_some_and(MessageDigests::marked)
            })
            .collect::<Vec<_>>();
        let Chain {
            at,
            mut read,
            mut sent,
        } = start;
        for (index, message) in digests.iter().enumerate().skip(at) {
            if message.marked() {
                marks.push(Chain {
                    at: index,
                    read: read.clone(),
                    sent: sent.clone(),
                });
            }
            read.update(message.read.as_bytes());
            sent.update(message.sent.as_bytes());
        }
        Self {
            digests,
            ids: kept,
            read,
            sent,
            marks,
        }
    }
}

#[derive(Default)]
struct Layouts {
    ids: Vec<Option<u64>>,
    prefixes: Vec<MessageLayout>,
}

pub(crate) struct Assembled {
    pub(crate) body: Arc<String>,
    pub(crate) start: usize,
    pub(crate) ends: Vec<usize>,
}

#[derive(Default)]
pub struct WireCache {
    tools: Atom<Option<Arc<Encoded<Vec<ClaudeToolSpec>>>>>,
    system: Atom<Option<Arc<Encoded<Arc<Value>>>>>,
    body: Slot<Assembled>,
    hashed: Slot<Hashed>,
    layouts: Slot<Layouts>,
    keepers: AtomicUsize,
}

pub struct KeptBody<'a>(&'a WireCache);

impl Drop for KeptBody<'_> {
    fn drop(&mut self) {
        if self.0.keepers.fetch_sub(1, Ordering::SeqCst) == 1 {
            self.0.body.take();
        }
    }
}

impl std::fmt::Debug for WireCache {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("WireCache").finish_non_exhaustive()
    }
}

impl WireCache {
    pub fn keep_body(&self) -> KeptBody<'_> {
        self.keepers.fetch_add(1, Ordering::SeqCst);
        KeptBody(self)
    }

    pub(crate) fn assembled(&self) -> Option<Assembled> {
        self.body.take()
    }

    pub fn layout<M: WireMessage>(&self, messages: &[M]) -> MessageLayout {
        let mut layouts = self.layouts.take().unwrap_or_default();
        let known = known(&layouts.ids, messages);
        layouts.ids.truncate(known);
        layouts.prefixes.truncate(known);
        let mut layout = layouts.prefixes.last().copied().unwrap_or_default();
        for message in &messages[known..] {
            layout = layout.then(MessageLayout::message(message.message()));
            layouts.ids.push(message.wire_id());
            layouts.prefixes.push(layout);
        }
        self.layouts.put(layouts);
        layout
    }

    pub(crate) fn hashed(&self) -> Option<Hashed> {
        self.hashed.take()
    }

    pub(crate) fn keep_hashed(&self, hashed: Hashed) {
        self.hashed.put(hashed);
    }

    pub(crate) fn assemble(&self, assembled: Assembled) {
        if self.keepers.load(Ordering::SeqCst) > 0 {
            self.body.put(assembled);
        }
        if self.keepers.load(Ordering::SeqCst) == 0 {
            self.body.take();
        }
    }

    pub(crate) fn tools(
        &self,
        tools: &[ClaudeToolSpec],
    ) -> Result<Arc<Encoded<Vec<ClaudeToolSpec>>>, ClaudeError> {
        static LIVE: LazyLock<Entries<Vec<ClaudeToolSpec>>> = LazyLock::new(Atom::default);
        remembered(
            &self.tools,
            &LIVE,
            tools,
            |value| value.as_slice() == tools,
            || tools.to_vec(),
        )
    }

    pub(crate) fn system(
        &self,
        system: &Arc<Value>,
    ) -> Result<Arc<Encoded<Arc<Value>>>, ClaudeError> {
        static LIVE: LazyLock<Entries<Arc<Value>>> = LazyLock::new(Atom::default);
        remembered(
            &self.system,
            &LIVE,
            &**system,
            |value| Arc::ptr_eq(value, system) || value == system,
            || Arc::clone(system),
        )
    }

    pub fn tools_fingerprint(&self, tools: &[ClaudeToolSpec]) -> Result<String, ClaudeError> {
        Ok(self.tools(tools)?.fingerprint.clone())
    }

    pub fn system_fingerprint(&self, system: Option<&Arc<Value>>) -> Result<String, ClaudeError> {
        match system {
            Some(system) => Ok(self.system(system)?.fingerprint.clone()),
            None => Ok(crate::fingerprint(&None::<&Value>)),
        }
    }
}

fn remembered<T, Q: Serialize + ?Sized>(
    last: &Atom<Option<Arc<Encoded<T>>>>,
    live: &Entries<T>,
    value: &Q,
    equal: impl Fn(&T) -> bool,
    owned: impl FnOnce() -> T,
) -> Result<Arc<Encoded<T>>, ClaudeError> {
    if let Some(entry) = last.read(|last| {
        last.as_ref()
            .filter(|entry| equal(&entry.value))
            .map(Arc::clone)
    }) {
        return Ok(entry);
    }
    let found = live.read(|entries| {
        entries
            .iter()
            .filter_map(Weak::upgrade)
            .find(|entry| equal(&entry.value))
    });
    let entry = match found {
        Some(entry) => entry,
        None => {
            let raw = serde_json::value::to_raw_value(value)?;
            let entry = Arc::new(Encoded {
                digest: nanocode_digest::blake3(raw.get().as_bytes()),
                fingerprint: crate::fingerprint_bytes(raw.get().as_bytes()),
                raw,
                value: owned(),
            });
            let weak = Arc::downgrade(&entry);
            live.update(|entries| {
                entries.retain(|entry| entry.strong_count() > 0);
                entries.push(Weak::clone(&weak));
            });
            entry
        }
    };
    last.update(|last| *last = Some(Arc::clone(&entry)));
    Ok(entry)
}
