use crate::{ClaudeError, ContentBlock, Message, WireMessage};
use nanocode_durability::EncodedPayload;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::value::RawValue;
use std::{
    borrow::Cow,
    collections::HashMap,
    ops::Deref,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
};

#[derive(Clone)]
pub(super) struct HistoryMessage(Arc<Entry>);

struct Entry {
    message: Message,
    wire: OnceLock<Box<str>>,
    json: OnceLock<Arc<str>>,
    record: OnceLock<EncodedPayload>,
}

impl Clone for Entry {
    fn clone(&self) -> Self {
        Self {
            message: self.message.clone(),
            wire: OnceLock::new(),
            json: OnceLock::new(),
            record: OnceLock::new(),
        }
    }
}

impl HistoryMessage {
    pub(super) fn make_mut(&mut self) -> &mut Message {
        let entry = Arc::make_mut(&mut self.0);
        entry.wire = OnceLock::new();
        entry.json = OnceLock::new();
        entry.record = OnceLock::new();
        &mut entry.message
    }

    pub(super) fn ptr_eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }

    pub(super) fn into_message(self) -> Message {
        Arc::try_unwrap(self.0).map_or_else(|entry| entry.message.clone(), |entry| entry.message)
    }

    pub(super) fn cache_json(&self) -> serde_json::Result<()> {
        self.json().map(drop)
    }

    fn json(&self) -> serde_json::Result<&Arc<str>> {
        if let Some(json) = self.0.json.get() {
            return Ok(json);
        }
        let encoded = Arc::from(serde_json::to_string(&self.0.message)?);
        Ok(self.0.json.get_or_init(|| encoded))
    }

    pub(super) fn record(&self) -> serde_json::Result<EncodedPayload> {
        if let Some(record) = self.0.record.get() {
            return Ok(record.clone());
        }
        let record = EncodedPayload::from_json(Arc::clone(self.json()?));
        Ok(self.0.record.get_or_init(|| record).clone())
    }
}

impl From<Message> for HistoryMessage {
    fn from(message: Message) -> Self {
        Self(Arc::new(Entry {
            message,
            wire: OnceLock::new(),
            json: OnceLock::new(),
            record: OnceLock::new(),
        }))
    }
}

impl Deref for HistoryMessage {
    type Target = Message;

    fn deref(&self) -> &Message {
        &self.0.message
    }
}

impl PartialEq for HistoryMessage {
    fn eq(&self, other: &Self) -> bool {
        self.ptr_eq(other) || self.0.message == other.0.message
    }
}

impl Serialize for HistoryMessage {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self.0.json.get() {
            Some(json) => serde_json::from_str::<&RawValue>(json)
                .map_err(serde::ser::Error::custom)?
                .serialize(serializer),
            None => self.0.message.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for HistoryMessage {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Message::deserialize(deserializer).map(Self::from)
    }
}

impl WireMessage for HistoryMessage {
    fn message(&self) -> &Message {
        &self.0.message
    }

    fn encoded<'a>(
        &'a self,
        subscription: bool,
        encode: &dyn Fn(&Message) -> Result<String, ClaudeError>,
    ) -> Result<Cow<'a, str>, ClaudeError> {
        if subscription {
            let wire = match self.0.wire.get() {
                Some(wire) => wire,
                None => {
                    let encoded = encode(&self.0.message)?.into_boxed_str();
                    self.0.wire.get_or_init(|| encoded)
                }
            };
            return Ok(Cow::Borrowed(wire));
        }
        let json = match self.0.json.get() {
            Some(json) => json,
            None => {
                let encoded = Arc::from(encode(&self.0.message)?);
                self.0.json.get_or_init(|| encoded)
            }
        };
        Ok(Cow::Borrowed(json))
    }
}

#[derive(Clone)]
pub(super) struct CodeRound(Arc<Round>);

struct Round {
    value: serde_json::Value,
    record: OnceLock<EncodedPayload>,
}

impl CodeRound {
    pub(super) fn ptr_eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }

    pub(super) fn record(&self) -> serde_json::Result<EncodedPayload> {
        if let Some(record) = self.0.record.get() {
            return Ok(record.clone());
        }
        let record = EncodedPayload::from_json(serde_json::to_string(&self.0.value)?.into());
        Ok(self.0.record.get_or_init(|| record).clone())
    }
}

impl From<serde_json::Value> for CodeRound {
    fn from(value: serde_json::Value) -> Self {
        Self(Arc::new(Round {
            value,
            record: OnceLock::new(),
        }))
    }
}

impl Deref for CodeRound {
    type Target = serde_json::Value;

    fn deref(&self) -> &serde_json::Value {
        &self.0.value
    }
}

impl PartialEq for CodeRound {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0) || self.0.value == other.0.value
    }
}

impl Serialize for CodeRound {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.value.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for CodeRound {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        serde_json::Value::deserialize(deserializer).map(Self::from)
    }
}

static ADMITTED_EPOCH: AtomicU64 = AtomicU64::new(1);

pub(super) struct AdmittedIds {
    order: Vec<Arc<str>>,
    positions: HashMap<Arc<str>, usize>,
    epoch: u64,
}

impl Default for AdmittedIds {
    fn default() -> Self {
        Self {
            order: Vec::new(),
            positions: HashMap::new(),
            epoch: ADMITTED_EPOCH.fetch_add(1, Ordering::Relaxed),
        }
    }
}

impl Clone for AdmittedIds {
    fn clone(&self) -> Self {
        Self {
            order: self.order.clone(),
            positions: self.positions.clone(),
            ..Self::default()
        }
    }
}

impl PartialEq for AdmittedIds {
    fn eq(&self, other: &Self) -> bool {
        self.order.len() == other.order.len()
            && self.order.iter().all(|id| other.positions.contains_key(id))
    }
}

impl AdmittedIds {
    pub(super) fn contains(&self, id: &str) -> bool {
        self.positions.contains_key(id)
    }

    pub(super) fn position(&self, id: &str) -> Option<usize> {
        self.positions.get(id).copied()
    }

    pub(super) const fn is_empty(&self) -> bool {
        self.order.is_empty()
    }

    pub(super) const fn len(&self) -> usize {
        self.order.len()
    }

    pub(super) const fn epoch(&self) -> u64 {
        self.epoch
    }

    pub(super) fn ordered(&self) -> &[Arc<str>] {
        &self.order
    }

    pub(super) fn insert(&mut self, id: Arc<str>) {
        if !self.positions.contains_key(&id) {
            self.positions.insert(Arc::clone(&id), self.order.len());
            self.order.push(id);
        }
    }
}

impl Extend<String> for AdmittedIds {
    fn extend<I: IntoIterator<Item = String>>(&mut self, ids: I) {
        for id in ids {
            self.insert(id.into());
        }
    }
}

impl Extend<Arc<str>> for AdmittedIds {
    fn extend<I: IntoIterator<Item = Arc<str>>>(&mut self, ids: I) {
        for id in ids {
            self.insert(id);
        }
    }
}

impl IntoIterator for AdmittedIds {
    type Item = Arc<str>;
    type IntoIter = std::vec::IntoIter<Arc<str>>;

    fn into_iter(self) -> Self::IntoIter {
        self.order.into_iter()
    }
}

impl Serialize for AdmittedIds {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(&self.order)
    }
}

impl<'de> Deserialize<'de> for AdmittedIds {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let mut ids = Self::default();
        ids.extend(Vec::<String>::deserialize(deserializer)?);
        Ok(ids)
    }
}

pub(super) fn strip_thinking(messages: &mut Vec<HistoryMessage>) {
    messages.retain_mut(|message| {
        if message.content.iter().any(|block| {
            matches!(
                block,
                ContentBlock::Thinking { .. } | ContentBlock::RedactedThinking { .. }
            )
        }) {
            message.make_mut().content.retain(|block| {
                !matches!(
                    block,
                    ContentBlock::Thinking { .. } | ContentBlock::RedactedThinking { .. }
                )
            });
        }
        !message.content.is_empty()
    });
}
