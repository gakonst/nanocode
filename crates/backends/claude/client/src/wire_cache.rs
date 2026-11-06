use std::sync::{Arc, LazyLock, Weak};

use nanocode_sync::Atom;
use serde::Serialize;
use serde_json::{Value, value::RawValue};

use crate::{ClaudeError, ClaudeToolSpec};

pub(crate) struct Encoded<T> {
    value: T,
    pub(crate) raw: Box<RawValue>,
    pub(crate) digest: blake3::Hash,
    pub(crate) fingerprint: String,
}

type Entries<T> = Atom<Vec<Weak<Encoded<T>>>>;

#[derive(Default)]
pub struct WireCache {
    tools: Atom<Option<Arc<Encoded<Vec<ClaudeToolSpec>>>>>,
    system: Atom<Option<Arc<Encoded<Value>>>>,
}

impl std::fmt::Debug for WireCache {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("WireCache").finish_non_exhaustive()
    }
}

impl WireCache {
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

    pub(crate) fn system(&self, system: &Value) -> Result<Arc<Encoded<Value>>, ClaudeError> {
        static LIVE: LazyLock<Entries<Value>> = LazyLock::new(Atom::default);
        remembered(
            &self.system,
            &LIVE,
            system,
            |value| value == system,
            || system.clone(),
        )
    }

    pub fn tools_fingerprint(&self, tools: &[ClaudeToolSpec]) -> Result<String, ClaudeError> {
        Ok(self.tools(tools)?.fingerprint.clone())
    }

    pub fn system_fingerprint(&self, system: Option<&Value>) -> Result<String, ClaudeError> {
        match system {
            Some(system) => Ok(self.system(system)?.fingerprint.clone()),
            None => Ok(crate::fingerprint(&system)),
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
                digest: blake3::hash(raw.get().as_bytes()),
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
