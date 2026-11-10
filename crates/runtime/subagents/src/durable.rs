use super::{
    message::MAX_MESSAGE_BYTES,
    model::{AgentDescriptor, AgentId, AgentStatus, MessageDisposition, MessageId},
    runtime::{ChildSession, Completion, OutputContract, StoredRuntime},
};
use nanocode_agent_core::{AgentUsage, Lineage, ModelSelection, SessionCheckpoint, Thinking};
use nanocode_durability_core::EncodedPayload;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;
use std::{collections::HashMap, future::Future, pin::Pin, sync::Arc};

use nanocode_sync::Atom;

#[cfg(not(target_family = "wasm"))]
pub type SubagentStoreFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

#[cfg(target_family = "wasm")]
pub type SubagentStoreFuture<'a, T> = Pin<Box<dyn Future<Output = T> + 'a>>;

pub trait SubagentStore: Send + Sync {
    fn pending_turn<'a>(
        &'a self,
        _session_id: &'a str,
    ) -> SubagentStoreFuture<'a, std::io::Result<Option<(String, Value)>>> {
        Box::pin(async { Ok(None) })
    }

    fn cancelled_turns<'a>(
        &'a self,
        _session_id: &'a str,
        _operation_ids: Vec<String>,
    ) -> SubagentStoreFuture<'a, std::io::Result<Vec<String>>> {
        Box::pin(async { Ok(Vec::new()) })
    }

    fn load<'a>(
        &'a self,
        root_session_id: &'a str,
    ) -> SubagentStoreFuture<'a, std::io::Result<Option<String>>>;

    fn save<'a>(
        &'a self,
        root_session_id: &'a str,
        payload: String,
        records: Vec<Arc<str>>,
    ) -> SubagentStoreFuture<'a, std::io::Result<()>>;

    fn save_deferred<'a>(
        &'a self,
        _root_session_id: &'a str,
    ) -> SubagentStoreFuture<
        'a,
        std::io::Result<Option<nanocode_agent_core::backend::DeferredJournal>>,
    > {
        Box::pin(async { Ok(None) })
    }

    fn record_session<'a>(
        &'a self,
        _root_session_id: &'a str,
        _checkpoint: SessionCheckpoint,
    ) -> SubagentStoreFuture<'a, std::io::Result<()>> {
        Box::pin(async { Ok(()) })
    }

    fn records_sessions(&self) -> bool {
        true
    }

    fn load_record<'a>(
        &'a self,
        root_session_id: &'a str,
        key: &'a str,
    ) -> SubagentStoreFuture<'a, std::io::Result<String>>;

    fn load_records<'a>(
        &'a self,
        root_session_id: &'a str,
        keys: Vec<String>,
    ) -> SubagentStoreFuture<'a, std::io::Result<Vec<String>>> {
        Box::pin(async move {
            let mut records = Vec::with_capacity(keys.len());
            for key in &keys {
                records.push(self.load_record(root_session_id, key).await?);
            }
            Ok(records)
        })
    }
}

#[derive(Clone, Default)]
pub struct MemorySubagentStore {
    values: Arc<Atom<HashMap<String, Arc<str>>>>,
    sessions: Arc<Atom<HashMap<String, Arc<SessionCheckpoint>>>>,
    records: Arc<Atom<MemoryRecords>>,
}

type MemoryRecords = HashMap<(String, String), Arc<str>>;

impl MemorySubagentStore {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn session(&self, session_id: &str) -> Option<SessionCheckpoint> {
        self.sessions.read(|sessions| {
            sessions
                .get(session_id)
                .map(|session| SessionCheckpoint::clone(session))
        })
    }

    #[must_use]
    pub fn sessions(&self) -> Vec<String> {
        let mut ids = self
            .sessions
            .read(|sessions| sessions.keys().cloned().collect::<Vec<_>>());
        ids.sort();
        ids
    }
}

impl SubagentStore for MemorySubagentStore {
    fn load<'a>(
        &'a self,
        root_session_id: &'a str,
    ) -> SubagentStoreFuture<'a, std::io::Result<Option<String>>> {
        Box::pin(async move {
            Ok(self
                .values
                .read(|values| values.get(root_session_id).map(|value| value.to_string())))
        })
    }

    fn save<'a>(
        &'a self,
        root_session_id: &'a str,
        payload: String,
        records: Vec<Arc<str>>,
    ) -> SubagentStoreFuture<'a, std::io::Result<()>> {
        Box::pin(async move {
            let keyed = records
                .into_iter()
                .map(|record| {
                    (
                        (root_session_id.to_owned(), checkpoint_key(&record)),
                        record,
                    )
                })
                .collect::<Vec<_>>();
            self.records.update(|stored| {
                stored.extend(keyed.iter().cloned());
            });
            let payload: Arc<str> = payload.into();
            self.values.update(|values| {
                values.insert(root_session_id.to_owned(), Arc::clone(&payload));
            });
            Ok(())
        })
    }

    fn record_session<'a>(
        &'a self,
        _root_session_id: &'a str,
        checkpoint: SessionCheckpoint,
    ) -> SubagentStoreFuture<'a, std::io::Result<()>> {
        Box::pin(async move {
            let checkpoint = Arc::new(checkpoint);
            self.sessions.update(|sessions| {
                sessions.insert(checkpoint.session_id().to_owned(), Arc::clone(&checkpoint));
            });
            Ok(())
        })
    }

    fn load_record<'a>(
        &'a self,
        root_session_id: &'a str,
        key: &'a str,
    ) -> SubagentStoreFuture<'a, std::io::Result<String>> {
        Box::pin(async move {
            self.records
                .read(|records| {
                    records
                        .get(&(root_session_id.to_owned(), key.to_owned()))
                        .map(|record| record.to_string())
                })
                .ok_or_else(|| std::io::Error::other(format!("missing subagent checkpoint {key}")))
        })
    }
}

pub(super) const JOURNAL_VERSION: u32 = 7;

pub(super) const LAZY_CHECKPOINT_VERSION: u32 = 3;

pub(super) const PAGE_MESSAGES: usize = 64;

const PAGE_AGENT_BITS: u32 = PAGE_MESSAGES.trailing_zeros();

const LEAF_AGENT_BITS: u32 = 4;

pub(super) fn leaf_agents(leaf: u64) -> impl Iterator<Item = AgentId> {
    let first = leaf << LEAF_AGENT_BITS;
    (first..=first | ((1 << LEAF_AGENT_BITS) - 1)).map(AgentId::new)
}

pub(super) const MIN_JOURNAL_VERSION: u32 = 1;

#[must_use]
pub fn checkpoint_key(json: &str) -> String {
    record_key(json.into()).to_string()
}

fn record_key(json: Arc<str>) -> Arc<str> {
    EncodedPayload::from_json(json).key().into()
}

fn key_matches(json: &str, key: &str) -> bool {
    EncodedPayload::from_json(json.into()).matches_key(key)
}

#[derive(Clone)]
pub(super) struct JournalCheckpoint {
    pub(super) key: Arc<str>,

    pub(super) pending: Option<Arc<str>>,

    pub(super) checkpoint: Option<Arc<SessionCheckpoint>>,
}

impl JournalCheckpoint {
    pub(super) fn encode(checkpoint: SessionCheckpoint, retain: bool) -> std::io::Result<Self> {
        let (key, json) = encoded(&checkpoint)?;
        Ok(Self {
            key,
            pending: Some(json),
            checkpoint: retain.then(|| Arc::new(checkpoint)),
        })
    }

    pub(super) fn stored(key: Arc<str>, checkpoint: Option<SessionCheckpoint>) -> Self {
        Self {
            key,
            pending: None,
            checkpoint: checkpoint.map(Arc::new),
        }
    }
}

pub(super) fn encoded<T: Serialize>(value: &T) -> std::io::Result<(Arc<str>, Arc<str>)> {
    let json: Arc<str> = serde_json::to_string(value)
        .map_err(std::io::Error::other)?
        .into();
    Ok((record_key(Arc::clone(&json)), json))
}

pub(super) async fn load_verified(
    store: &dyn SubagentStore,
    root_session_id: &str,
    key: &str,
) -> std::io::Result<String> {
    let json = store.load_record(root_session_id, key).await?;
    if !key_matches(&json, key) {
        return Err(std::io::Error::other(format!(
            "subagent journal record {key} does not match its content"
        )));
    }
    Ok(json)
}

pub(super) fn decode_checkpoint(json: &str, key: &str) -> std::io::Result<SessionCheckpoint> {
    if !key_matches(json, key) {
        return Err(std::io::Error::other(format!(
            "subagent checkpoint {key} does not match its record"
        )));
    }
    let checkpoint: SessionCheckpoint = serde_json::from_str(json).map_err(|error| {
        std::io::Error::other(format!("invalid subagent checkpoint {key}: {error}"))
    })?;
    checkpoint.validate().map_err(std::io::Error::other)?;
    Ok(checkpoint)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyCheckpointRecord {
    #[serde(default)]
    checkpoint: Option<Value>,
    #[serde(default)]
    native_checkpoint: Option<PersistedNative>,
}

#[derive(Serialize, Deserialize)]
pub(super) struct PersistedScope {
    pub(super) version: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) agents: Vec<PersistedAgent>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) agent_records: Vec<(AgentId, Arc<str>)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) agent_pages: Option<AgentPages>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) agent_leaves: Option<AgentPages>,
    #[serde(default)]
    pub(super) messages: PersistedMessages,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) deltas: Vec<Vec<Arc<str>>>,
}

const DELTA_PAGE: usize = 16;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DeltaPage {
    deltas: Vec<Arc<str>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct JournalDelta {
    #[serde(default)]
    agents: Vec<(u64, AgentRecord)>,
    #[serde(default)]
    messages: Vec<super::model::AgentMessage>,
}

fn agents_json(agents: &[(u64, Arc<str>)]) -> String {
    let mut json = String::with_capacity(
        16 + agents
            .iter()
            .map(|(_, record)| record.len() + 24)
            .sum::<usize>(),
    );
    json.push_str("{\"agents\":[");
    for (index, (id, record)) in agents.iter().enumerate() {
        if index > 0 {
            json.push(',');
        }
        json.push('[');
        json.push_str(&id.to_string());
        json.push(',');
        json.push_str(record);
        json.push(']');
    }
    json.push(']');
    json
}

fn agent_leaf(agents: &[(u64, Arc<str>)]) -> (Arc<str>, Arc<str>) {
    let mut json = agents_json(agents);
    json.push('}');
    let json: Arc<str> = json.into();
    (record_key(Arc::clone(&json)), json)
}

fn journal_delta(agents: &[(u64, Arc<str>)], messages: &[Arc<str>]) -> (Arc<str>, Arc<str>) {
    let mut json = agents_json(agents);
    json.push_str(",\"messages\":[");
    for (index, message) in messages.iter().enumerate() {
        if index > 0 {
            json.push(',');
        }
        json.push_str(message);
    }
    json.push_str("]}");
    let json: Arc<str> = json.into();
    (record_key(Arc::clone(&json)), json)
}

pub(super) struct Deltas {
    pub(super) agents: Vec<(AgentId, Arc<str>, AgentRecord)>,
    pub(super) messages: Vec<super::model::AgentMessage>,
}

pub(super) async fn load_deltas(
    store: &dyn SubagentStore,
    root_session_id: &str,
    mut levels: Vec<Vec<Arc<str>>>,
) -> std::io::Result<Deltas> {
    let mut keys = Vec::<Arc<str>>::new();
    while let Some(level) = levels.pop() {
        let pages = verified(
            store,
            root_session_id,
            keys.iter().map(|key| key.to_string()).collect(),
        )
        .await?;
        keys = Vec::with_capacity(pages.len() * DELTA_PAGE + level.len());
        for json in pages {
            let page: DeltaPage = serde_json::from_str(&json).map_err(|error| {
                std::io::Error::other(format!("invalid subagent journal delta page: {error}"))
            })?;
            keys.extend(page.deltas);
        }
        keys.extend(level);
    }
    let records = verified(
        store,
        root_session_id,
        keys.iter().map(|key| key.to_string()).collect(),
    )
    .await?;
    let mut deltas = Deltas {
        agents: Vec::new(),
        messages: Vec::new(),
    };
    for (key, json) in keys.into_iter().zip(records) {
        let delta: JournalDelta = serde_json::from_str(&json).map_err(|error| {
            std::io::Error::other(format!("invalid subagent journal delta {key}: {error}"))
        })?;
        deltas.agents.extend(
            delta
                .agents
                .into_iter()
                .map(|(id, record)| (AgentId::new(id), Arc::clone(&key), record)),
        );
        deltas.messages.extend(delta.messages);
    }
    Ok(deltas)
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AgentPages {
    height: u32,
    pages: Vec<(u64, Arc<str>)>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AgentPage {
    entries: Vec<(u64, Arc<str>)>,
}

impl AgentPages {
    pub(super) async fn agent_records(
        self,
        store: &dyn SubagentStore,
        root_session_id: &str,
    ) -> std::io::Result<Vec<(AgentId, Arc<str>)>> {
        Ok(self
            .entries(store, root_session_id)
            .await?
            .into_iter()
            .map(|(id, key)| (AgentId::new(id), key))
            .collect())
    }

    async fn entries(
        self,
        store: &dyn SubagentStore,
        root_session_id: &str,
    ) -> std::io::Result<Vec<(u64, Arc<str>)>> {
        let mut entries = self.pages;
        for _ in 0..self.height {
            let keys = entries.iter().map(|(_, key)| key.to_string()).collect();
            let pages = verified(store, root_session_id, keys).await?;
            entries = Vec::with_capacity(pages.len() * PAGE_MESSAGES);
            for json in pages {
                let page: AgentPage = serde_json::from_str(&json).map_err(|error| {
                    std::io::Error::other(format!("invalid subagent journal page: {error}"))
                })?;
                entries.extend(page.entries);
            }
        }
        Ok(entries)
    }
}

#[derive(Clone, Default)]
struct TreePage {
    entries: imbl::OrdMap<u64, Arc<str>>,
    key: Option<Arc<str>>,
}

#[derive(Clone, Default)]
struct AgentTree {
    levels: Vec<imbl::OrdMap<u64, TreePage>>,
}

impl AgentTree {
    fn get(&self, level: usize, index: u64) -> Option<&Arc<str>> {
        self.levels
            .get(level)?
            .get(&(index >> PAGE_AGENT_BITS))?
            .entries
            .get(&index)
    }

    fn truncate(&mut self, mut keep: u64) {
        for pages in &mut self.levels {
            let kept = keep.div_ceil(1 << PAGE_AGENT_BITS);
            *pages = std::mem::take(pages).split(&kept).0;
            if let Some(last) = kept.checked_sub(1)
                && let Some(page) = pages.get_mut(&last)
            {
                let before = page.entries.len();
                page.entries = std::mem::take(&mut page.entries).split(&keep).0;
                if page.entries.len() != before {
                    page.key = None;
                }
            }
            keep = kept;
        }
        while self.levels.last().is_some_and(imbl::OrdMap::is_empty) {
            self.levels.pop();
        }
    }

    fn set(&mut self, level: usize, index: u64, key: Arc<str>) {
        if self.levels.len() <= level {
            self.levels.resize_with(level + 1, imbl::OrdMap::new);
        }
        let page = self.levels[level]
            .entry(index >> PAGE_AGENT_BITS)
            .or_default();
        if page.entries.get(&index) != Some(&key) {
            page.entries.insert(index, key);
            page.key = None;
        }
    }

    fn encode(
        &mut self,
        records: &mut Vec<(Arc<str>, Arc<str>)>,
    ) -> std::io::Result<Option<AgentPages>> {
        let mut level = 0;
        while let Some(pages) = self.levels.get(level) {
            let top = self.levels.len() == level + 1;
            let dirty = pages
                .iter()
                .filter(|(_, page)| page.key.is_none())
                .map(|(index, page)| (*index, page.entries.clone()))
                .collect::<Vec<_>>();
            for (index, entries) in dirty {
                let (key, json) = encoded(&AgentPage {
                    entries: entries
                        .iter()
                        .map(|(id, key)| (*id, Arc::clone(key)))
                        .collect(),
                })?;
                records.push((Arc::clone(&key), json));
                if let Some(page) = self.levels[level].get_mut(&index) {
                    page.key = Some(Arc::clone(&key));
                }
                if !top {
                    self.set(level + 1, index, key);
                }
            }
            if top {
                let pages = self.levels[level]
                    .iter()
                    .map(|(index, page)| (*index, page.key.clone()))
                    .collect::<Vec<_>>();
                if pages
                    .last()
                    .is_some_and(|(index, _)| *index >> PAGE_AGENT_BITS == 0)
                {
                    return Ok(Some(AgentPages {
                        height: u32::try_from(level + 1).map_err(std::io::Error::other)?,
                        pages: pages
                            .into_iter()
                            .map(|(index, key)| {
                                key.map(|key| (index, key)).ok_or_else(|| {
                                    std::io::Error::other("subagent journal page was not encoded")
                                })
                            })
                            .collect::<std::io::Result<_>>()?,
                    }));
                }
                for (index, key) in pages {
                    let key = key.ok_or_else(|| {
                        std::io::Error::other("subagent journal page was not encoded")
                    })?;
                    self.set(level + 1, index, key);
                }
            }
            level += 1;
        }
        Ok(None)
    }
}

#[derive(Default, Serialize, Deserialize)]
pub(super) struct PersistedMessages {
    pub(super) next_message_id: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) records: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) tail: Option<Arc<str>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) leaves: Option<AgentPages>,
    pub(super) pending: Vec<super::model::MessageId>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MessagePage {
    previous: Option<Arc<str>>,
    messages: Vec<Arc<str>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MessageLeaf {
    messages: Vec<super::model::AgentMessage>,
}

fn message_leaf(messages: &imbl::Vector<Arc<str>>) -> (Arc<str>, Arc<str>) {
    let mut json = String::with_capacity(
        16 + messages
            .iter()
            .map(|message| message.len() + 1)
            .sum::<usize>(),
    );
    json.push_str("{\"messages\":[");
    for (index, message) in messages.iter().enumerate() {
        if index > 0 {
            json.push(',');
        }
        json.push_str(message);
    }
    json.push_str("]}");
    let json: Arc<str> = json.into();
    (record_key(Arc::clone(&json)), json)
}

impl PersistedMessages {
    pub(super) async fn hydrate(
        self,
        store: &dyn SubagentStore,
        root_session_id: &str,
        appended: Vec<super::model::AgentMessage>,
    ) -> std::io::Result<super::message::MessageThreads> {
        let mut restored = Vec::new();
        if let Some(leaves) = self.leaves {
            let keys = leaves
                .entries(store, root_session_id)
                .await?
                .into_iter()
                .map(|(_, key)| key.to_string())
                .collect();
            for json in verified(store, root_session_id, keys).await? {
                let leaf: MessageLeaf = serde_json::from_str(&json).map_err(|error| {
                    std::io::Error::other(format!("invalid subagent message leaf: {error}"))
                })?;
                restored.extend(leaf.messages);
            }
        }
        let mut keys = self.records;
        let mut pages = Vec::new();
        let mut next = self.tail;
        while let Some(key) = next {
            let json = load_verified(store, root_session_id, &key).await?;
            let page: MessagePage = serde_json::from_str(&json).map_err(std::io::Error::other)?;
            next = page.previous;
            pages.push(page.messages);
        }
        keys.extend(pages.into_iter().rev().flatten().map(|key| key.to_string()));
        for json in verified(store, root_session_id, keys).await? {
            restored.push(serde_json::from_str(&json).map_err(std::io::Error::other)?);
        }
        restored.extend(appended);
        super::message::MessageThreads::restore(self.next_message_id, restored, self.pending)
    }
}

#[derive(Clone)]
pub(super) struct JournalCache {
    full: bool,
    dirty: std::collections::BTreeSet<AgentId>,
    agents: imbl::HashMap<AgentId, CachedAgent>,
    tree: AgentTree,
    messages: AgentTree,
    logged_ids: imbl::Vector<super::model::MessageId>,
    message_tail: imbl::Vector<Arc<str>>,
    logged: Option<super::message::MessageLog>,
    unfolded: imbl::OrdSet<AgentId>,
    unfolded_messages: imbl::Vector<Arc<str>>,
    deltas: Vec<imbl::Vector<Arc<str>>>,
    delta_bytes: usize,
    leaf_bytes: imbl::OrdMap<u64, usize>,
    folded_bytes: usize,
}

impl Default for JournalCache {
    fn default() -> Self {
        Self {
            full: true,
            dirty: std::collections::BTreeSet::new(),
            agents: imbl::HashMap::new(),
            tree: AgentTree::default(),
            messages: AgentTree::default(),
            logged_ids: imbl::Vector::new(),
            message_tail: imbl::Vector::new(),
            logged: None,
            unfolded: imbl::OrdSet::new(),
            unfolded_messages: imbl::Vector::new(),
            deltas: Vec::new(),
            delta_bytes: 0,
            leaf_bytes: imbl::OrdMap::new(),
            folded_bytes: 0,
        }
    }
}

pub(super) enum Tracked {
    Leaves(Vec<u64>),
    Agents(Vec<AgentId>),
}

#[derive(Clone)]
pub(super) struct CachedAgent {
    generation: u64,
    checkpoint: Option<Arc<str>>,
    pub(super) checkpoint_ref: Option<Arc<str>>,
    profile: Arc<str>,
    record: Arc<str>,
}

pub(super) struct AgentEntry {
    pub(super) generation: u64,
    pub(super) checkpoint: Option<Arc<str>>,
    pub(super) unchanged: bool,
    pub(super) agent: PersistedAgent,
}

pub(super) struct MessagesSnapshot {
    pub(super) kept: Option<usize>,
    pub(super) messages: Vec<super::model::AgentMessage>,
}

pub(super) struct JournalSnapshot {
    pub(super) fold: bool,
    pub(super) agents: Vec<(u64, Vec<(AgentId, AgentEntry)>)>,
    pub(super) messages: MessagesSnapshot,
    pub(super) logged: super::message::MessageLog,
    pub(super) next_message_id: u64,
    pub(super) pending: Vec<super::model::MessageId>,
    pub(super) records: Vec<(Arc<str>, Arc<str>)>,
    pub(super) acknowledged: Vec<(AgentId, Arc<str>)>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AgentProfile {
    descriptor: AgentDescriptor,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    binding_task: Option<String>,
    output_schema: Arc<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    host_context: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AgentLeaf {
    agents: Vec<(u64, AgentRecord)>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AgentRecord {
    profile: Arc<str>,
    status: AgentStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_output: Option<Value>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    output_in_status: bool,
    #[serde(default)]
    next_instruction_revision: u64,
    #[serde(default)]
    turn_in_flight: bool,
    #[serde(default, skip_serializing_if = "is_zero")]
    resume_attempts: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    checkpoint_ref: Option<Arc<str>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    in_flight_calls: Vec<InFlightCall>,
    #[serde(default, skip_serializing_if = "is_zero")]
    in_flight_omitted: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    accepted_output: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    accepted_revision: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pending_completion: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    settled_revision: Option<u64>,
    #[serde(default, skip_serializing_if = "AgentUsage::is_empty")]
    usage: AgentUsage,
    #[serde(default, skip_serializing_if = "is_zero")]
    completion_attempt: u32,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    completion_parked: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cancelled_revision: Option<u64>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    conversation: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    keeps_result: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    waited: Vec<Waited>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    sent: Vec<Sent>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    spawned: Option<Spawned>,
}

fn agent_record(
    agent: PersistedAgent,
    stored: Option<Arc<str>>,
    unchanged: bool,
    records: &mut Vec<(Arc<str>, Arc<str>)>,
) -> std::io::Result<AgentRecord> {
    let profile = match stored {
        Some(profile) if unchanged => profile,
        stored => {
            let (profile, json) = encoded(&AgentProfile {
                descriptor: agent.descriptor,
                binding_task: agent.binding_task,
                output_schema: agent.output_schema,
                host_context: agent.host_context,
            })?;
            if stored.as_ref() != Some(&profile) {
                records.push((Arc::clone(&profile), json));
            }
            profile
        }
    };
    Ok(AgentRecord {
        profile,
        status: agent.status,
        last_output: agent.last_output,
        output_in_status: agent.output_in_status,
        next_instruction_revision: agent.next_instruction_revision,
        turn_in_flight: agent.turn_in_flight,
        resume_attempts: agent.resume_attempts,
        checkpoint_ref: agent.checkpoint_ref,
        in_flight_calls: agent.in_flight_calls,
        in_flight_omitted: agent.in_flight_omitted,
        accepted_output: agent.accepted_output,
        accepted_revision: agent.accepted_revision,
        pending_completion: agent.pending_completion,
        settled_revision: agent.settled_revision,
        usage: agent.usage,
        completion_attempt: agent.completion_attempt,
        completion_parked: agent.completion_parked,
        cancelled_revision: agent.cancelled_revision,
        conversation: agent.conversation,
        keeps_result: agent.keeps_result,
        waited: agent.waited,
        sent: agent.sent,
        spawned: agent.spawned,
    })
}

pub(super) async fn load_agents(
    store: &dyn SubagentStore,
    root_session_id: &str,
    agents: Vec<(AgentId, Arc<str>)>,
) -> std::io::Result<Vec<PersistedAgent>> {
    let keys = agents
        .iter()
        .map(|(_, key)| key.to_string())
        .collect::<Vec<_>>();
    let records = verified(store, root_session_id, keys).await?;
    let records = agents
        .into_iter()
        .zip(&records)
        .map(
            |((id, key), json)| match serde_json::from_str::<AgentRecord>(json) {
                Ok(record) => Ok((id, key, record)),
                Err(error) => Err(std::io::Error::other(format!(
                    "invalid subagent journal record {key}: {error}"
                ))),
            },
        )
        .collect::<std::io::Result<Vec<_>>>()?;
    with_profiles(store, root_session_id, records).await
}

pub(super) async fn load_leaves(
    store: &dyn SubagentStore,
    root_session_id: &str,
    leaves: Option<AgentPages>,
    changed: Vec<(AgentId, Arc<str>, AgentRecord)>,
) -> std::io::Result<Vec<PersistedAgent>> {
    let leaves = match leaves {
        Some(leaves) => leaves.entries(store, root_session_id).await?,
        None => Vec::new(),
    };
    let keys = leaves
        .iter()
        .map(|(_, key)| key.to_string())
        .collect::<Vec<_>>();
    let mut records = Vec::new();
    for ((index, key), json) in leaves
        .into_iter()
        .zip(verified(store, root_session_id, keys).await?)
    {
        let leaf: AgentLeaf = serde_json::from_str(&json).map_err(|error| {
            std::io::Error::other(format!("invalid subagent journal leaf {key}: {error}"))
        })?;
        for (id, record) in leaf.agents {
            if id >> LEAF_AGENT_BITS != index {
                return Err(std::io::Error::other(format!(
                    "subagent journal leaf {key} holds agent {id} of another leaf"
                )));
            }
            records.push((AgentId::new(id), Arc::clone(&key), record));
        }
    }
    if !changed.is_empty() {
        let mut latest = records
            .into_iter()
            .map(|(id, key, record)| (id, (key, record)))
            .collect::<std::collections::BTreeMap<_, _>>();
        for (id, key, record) in changed {
            latest.insert(id, (key, record));
        }
        records = latest
            .into_iter()
            .map(|(id, (key, record))| (id, key, record))
            .collect();
    }
    with_profiles(store, root_session_id, records).await
}

async fn with_profiles(
    store: &dyn SubagentStore,
    root_session_id: &str,
    records: Vec<(AgentId, Arc<str>, AgentRecord)>,
) -> std::io::Result<Vec<PersistedAgent>> {
    let profiles = verified(
        store,
        root_session_id,
        records
            .iter()
            .map(|(_, _, record)| record.profile.to_string())
            .collect(),
    )
    .await?;
    records
        .into_iter()
        .zip(profiles)
        .map(|((id, key, record), profile)| {
            let profile = serde_json::from_str(&profile).map_err(|error| {
                std::io::Error::other(format!("invalid subagent journal record {key}: {error}"))
            })?;
            agent_from(id, &key, record, profile)
        })
        .collect()
}

async fn verified(
    store: &dyn SubagentStore,
    root_session_id: &str,
    keys: Vec<String>,
) -> std::io::Result<Vec<String>> {
    if keys.is_empty() {
        return Ok(Vec::new());
    }
    let records = store.load_records(root_session_id, keys.clone()).await?;
    if records.len() != keys.len() {
        return Err(std::io::Error::other(format!(
            "subagent journal store answered {} of {} records",
            records.len(),
            keys.len()
        )));
    }
    for (key, json) in keys.iter().zip(&records) {
        if !key_matches(json, key) {
            return Err(std::io::Error::other(format!(
                "subagent journal record {key} does not match its content"
            )));
        }
    }
    Ok(records)
}

fn agent_from(
    id: AgentId,
    key: &str,
    record: AgentRecord,
    profile: AgentProfile,
) -> std::io::Result<PersistedAgent> {
    if profile.descriptor.id != id {
        return Err(std::io::Error::other(format!(
            "subagent journal record {key} does not describe agent {id}"
        )));
    }
    let mut agent = PersistedAgent {
        descriptor: profile.descriptor,
        binding_task: profile.binding_task,
        status: record.status,
        output_schema: profile.output_schema,
        host_context: profile.host_context,
        last_output: record.last_output,
        output_in_status: record.output_in_status,
        next_instruction_revision: record.next_instruction_revision,
        turn_in_flight: record.turn_in_flight,
        resume_attempts: record.resume_attempts,
        checkpoint: None,
        native_checkpoint: None,
        checkpoint_ref: record.checkpoint_ref,
        in_flight_calls: record.in_flight_calls,
        in_flight_omitted: record.in_flight_omitted,
        accepted_output: record.accepted_output,
        accepted_revision: record.accepted_revision,
        pending_completion: record.pending_completion,
        settled_revision: record.settled_revision,
        usage: record.usage,
        completion_attempt: record.completion_attempt,
        completion_parked: record.completion_parked,
        cancelled_revision: record.cancelled_revision,
        conversation: record.conversation,
        keeps_result: record.keeps_result,
        waited: record.waited,
        sent: record.sent,
        spawned: record.spawned,
    };
    agent.expand();
    Ok(agent)
}

pub(super) struct EncodedJournal {
    pub(super) payload: String,
    pub(super) records: Vec<(Arc<str>, Arc<str>)>,
    pub(super) acknowledged: Vec<(AgentId, Arc<str>)>,
    update: JournalCache,
}

impl JournalCache {
    pub(super) fn agent(
        &self,
        id: AgentId,
        generation: u64,
        checkpoint: Option<&Arc<str>>,
    ) -> Option<&CachedAgent> {
        self.agents.get(&id).filter(|cached| {
            cached.generation == generation && cached.checkpoint.as_ref() == checkpoint
        })
    }

    pub(super) const fn logged_ids(&self) -> &imbl::Vector<super::model::MessageId> {
        &self.logged_ids
    }

    pub(super) fn sealed_messages(&self) -> usize {
        self.logged_ids.len() - self.message_tail.len() - self.unfolded_messages.len()
    }

    pub(super) const fn folds(&self, rewritten: bool) -> bool {
        self.full || rewritten || self.delta_bytes >= self.folded_bytes
    }

    pub(super) const fn logged(&self) -> Option<super::message::MessageLog> {
        self.logged
    }

    pub(super) fn track(&mut self, ids: impl IntoIterator<Item = AgentId>) {
        if !self.full {
            self.dirty.extend(ids);
        }
    }

    pub(super) fn tracked(&self, fold: bool, all: impl FnOnce() -> Vec<AgentId>) -> Tracked {
        if !fold {
            return Tracked::Agents(self.dirty.iter().copied().collect());
        }
        let ids = if self.full {
            all()
        } else {
            self.dirty
                .iter()
                .chain(&self.unfolded)
                .copied()
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect()
        };
        let mut leaves = ids
            .into_iter()
            .map(|id| id.get() >> LEAF_AGENT_BITS)
            .collect::<Vec<_>>();
        leaves.dedup();
        Tracked::Leaves(leaves)
    }

    fn push_delta(
        &mut self,
        key: Arc<str>,
        records: &mut Vec<(Arc<str>, Arc<str>)>,
    ) -> std::io::Result<()> {
        let mut entry = key;
        let mut level = 0;
        loop {
            if self.deltas.len() == level {
                self.deltas.push(imbl::Vector::new());
            }
            self.deltas[level].push_back(entry);
            if self.deltas[level].len() < DELTA_PAGE {
                return Ok(());
            }
            let deltas = std::mem::take(&mut self.deltas[level])
                .into_iter()
                .collect();
            let (key, json) = encoded(&DeltaPage { deltas })?;
            records.push((Arc::clone(&key), json));
            entry = key;
            level += 1;
        }
    }

    pub(super) fn committed(&mut self, encoded: EncodedJournal) {
        *self = encoded.update;
    }
}

impl JournalSnapshot {
    pub(super) fn encode(self, cache: &JournalCache) -> std::io::Result<EncodedJournal> {
        let mut records = self.records;
        let mut update = cache.clone();
        if update.full {
            update.agents = imbl::HashMap::new();
            update.tree = AgentTree::default();
            update.leaf_bytes = imbl::OrdMap::new();
            update.folded_bytes = 0;
        }
        update.full = false;
        update.dirty.clear();
        let mut changed = Vec::new();
        for (leaf, agents) in self.agents {
            let mut entries = Vec::with_capacity(agents.len());
            for (id, entry) in agents {
                let stored = update
                    .agents
                    .get(&id)
                    .map(|cached| Arc::clone(&cached.profile));
                let record = agent_record(entry.agent, stored, entry.unchanged, &mut records)?;
                let (key, json) = encoded(&record)?;
                let written = update.agents.insert(
                    id,
                    CachedAgent {
                        generation: entry.generation,
                        checkpoint: entry.checkpoint,
                        checkpoint_ref: record.checkpoint_ref,
                        profile: record.profile,
                        record: Arc::clone(&key),
                    },
                );
                if self.fold || written.is_none_or(|written| written.record != key) {
                    entries.push((id.get(), json));
                }
            }
            if !self.fold {
                update
                    .unfolded
                    .extend(entries.iter().map(|(id, _)| AgentId::new(*id)));
                changed.extend(entries);
                continue;
            }
            if entries.is_empty() {
                continue;
            }
            let (key, json) = agent_leaf(&entries);
            if update.tree.get(0, leaf) != Some(&key) {
                let replaced = update.leaf_bytes.insert(leaf, json.len()).unwrap_or(0);
                update.folded_bytes = update.folded_bytes + json.len() - replaced;
                records.push((Arc::clone(&key), json));
                update.tree.set(0, leaf, key);
            }
        }
        let agent_leaves = update.tree.encode(&mut records)?;
        let MessagesSnapshot { kept, messages } = self.messages;
        let mut appended = Vec::new();
        if self.fold {
            let mut sealed = update.sealed_messages();
            let mut unfolded = std::mem::take(&mut update.unfolded_messages);
            if let Some(kept) = kept {
                sealed = kept * PAGE_MESSAGES;
                unfolded = imbl::Vector::new();
                update.logged_ids = update.logged_ids.take(sealed);
                update.message_tail = imbl::Vector::new();
                update
                    .messages
                    .truncate(u64::try_from(kept).map_err(std::io::Error::other)?);
            }
            let written = !unfolded.is_empty() || !messages.is_empty();
            let mut logged = Vec::with_capacity(messages.len());
            for message in messages {
                logged.push(message.id);
                unfolded.push_back(
                    serde_json::to_string(&message)
                        .map_err(std::io::Error::other)?
                        .into(),
                );
            }
            update.logged_ids.extend(logged);
            for json in unfolded {
                update.message_tail.push_back(json);
                if update.message_tail.len() == PAGE_MESSAGES {
                    update.write_message_leaf(sealed, &mut records)?;
                    update.message_tail = imbl::Vector::new();
                    sealed += PAGE_MESSAGES;
                }
            }
            if written && !update.message_tail.is_empty() {
                update.write_message_leaf(sealed, &mut records)?;
            }
            update.unfolded = imbl::OrdSet::new();
            update.deltas.clear();
            update.delta_bytes = 0;
        } else {
            if kept.is_some() {
                return Err(std::io::Error::other(
                    "a subagent journal delta cannot rewrite the message log",
                ));
            }
            for message in messages {
                let json: Arc<str> = serde_json::to_string(&message)
                    .map_err(std::io::Error::other)?
                    .into();
                update.logged_ids.push_back(message.id);
                update.unfolded_messages.push_back(Arc::clone(&json));
                appended.push(json);
            }
        }
        let message_leaves = update.messages.encode(&mut records)?;
        if !changed.is_empty() || !appended.is_empty() {
            let (key, json) = journal_delta(&changed, &appended);
            update.delta_bytes += json.len();
            records.push((Arc::clone(&key), json));
            update.push_delta(key, &mut records)?;
        }
        update.logged = Some(self.logged);
        let payload = serde_json::to_string(&PersistedScope {
            version: JOURNAL_VERSION,
            agents: Vec::new(),
            agent_records: Vec::new(),
            agent_pages: None,
            agent_leaves,
            messages: PersistedMessages {
                next_message_id: self.next_message_id,
                records: Vec::new(),
                tail: None,
                leaves: message_leaves,
                pending: self.pending,
            },
            deltas: update
                .deltas
                .iter()
                .map(|level| level.iter().cloned().collect())
                .collect(),
        })
        .map_err(std::io::Error::other)?;
        Ok(EncodedJournal {
            payload,
            records,
            acknowledged: self.acknowledged,
            update,
        })
    }
}

impl JournalCache {
    fn write_message_leaf(
        &mut self,
        sealed: usize,
        records: &mut Vec<(Arc<str>, Arc<str>)>,
    ) -> std::io::Result<()> {
        let leaf = u64::try_from(sealed / PAGE_MESSAGES).map_err(std::io::Error::other)?;
        let (key, json) = message_leaf(&self.message_tail);
        if self.messages.get(0, leaf) != Some(&key) {
            records.push((Arc::clone(&key), json));
            self.messages.set(0, leaf, key);
        }
        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
pub(super) struct PersistedAgent {
    pub(super) descriptor: AgentDescriptor,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) binding_task: Option<String>,
    pub(super) status: AgentStatus,
    pub(super) output_schema: Arc<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) host_context: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) last_output: Option<Value>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(super) output_in_status: bool,
    #[serde(default)]
    pub(super) next_instruction_revision: u64,

    #[serde(default)]
    pub(super) turn_in_flight: bool,

    #[serde(default, skip_serializing_if = "is_zero")]
    pub(super) resume_attempts: u32,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) checkpoint: Option<JournaledCheckpoint>,

    #[serde(default, skip_serializing)]
    pub(super) native_checkpoint: Option<PersistedNative>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) checkpoint_ref: Option<Arc<str>>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) in_flight_calls: Vec<InFlightCall>,

    #[serde(default, skip_serializing_if = "is_zero")]
    pub(super) in_flight_omitted: u32,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) accepted_output: Option<Value>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) accepted_revision: Option<u64>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) pending_completion: Option<u64>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) settled_revision: Option<u64>,

    #[serde(default, skip_serializing_if = "AgentUsage::is_empty")]
    pub(super) usage: AgentUsage,

    #[serde(default, skip_serializing_if = "is_zero")]
    pub(super) completion_attempt: u32,

    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(super) completion_parked: bool,

    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(super) conversation: bool,

    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(super) keeps_result: bool,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) cancelled_revision: Option<u64>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) waited: Vec<Waited>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) sent: Vec<Sent>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) spawned: Option<Spawned>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Waited {
    pub(super) session: String,
    pub(super) call: String,
    pub(super) mark: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Sent {
    pub(super) session: String,
    pub(super) call: String,
    pub(super) message: MessageId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) disposition: Option<MessageDisposition>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Spawned {
    pub(super) session: String,
    pub(super) call: String,
}

pub(super) fn cell_of(call: &str) -> &str {
    call.rsplit_once("/code-").map_or(call, |(cell, _)| cell)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "JournaledCall", into = "JournaledCall")]
pub(super) struct InFlightCall {
    pub(super) call_id: String,
    pub(super) tool: String,
    pub(super) arguments: String,
    pub(super) state: CallState,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum CallState {
    Restored {
        result_recorded: bool,
        yielded: bool,
    },
    Live {
        parent_call_id: Option<String>,
        progress: CallProgress,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CallProgress {
    Started,
    Recorded,
    Yielded,
    Settled,
    Committable,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct JournaledCall {
    call_id: String,
    tool: String,
    arguments: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    result_recorded: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    yielded: bool,
}

impl From<JournaledCall> for InFlightCall {
    fn from(call: JournaledCall) -> Self {
        Self {
            call_id: call.call_id,
            tool: call.tool,
            arguments: call.arguments,
            state: CallState::Restored {
                result_recorded: call.result_recorded,
                yielded: call.yielded,
            },
        }
    }
}

impl From<InFlightCall> for JournaledCall {
    fn from(call: InFlightCall) -> Self {
        Self {
            result_recorded: call.result_recorded(),
            yielded: call.yielded(),
            call_id: call.call_id,
            tool: call.tool,
            arguments: call.arguments,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub(super) struct OmittedProgress {
    pub(super) settled: u32,

    pub(super) committable: u32,

    pub(super) nested: HashMap<String, u32>,
}

impl InFlightCall {
    pub(super) fn new(
        call_id: &str,
        tool: &str,
        arguments: Option<&Value>,
        parent_call_id: Option<&str>,
    ) -> Self {
        let arguments = match arguments {
            Some(Value::String(text)) => text.clone(),
            Some(value) => value.to_string(),
            None => String::new(),
        };
        Self {
            call_id: call_id.to_owned(),
            tool: tool.to_owned(),
            arguments,
            state: CallState::Live {
                parent_call_id: parent_call_id.map(str::to_owned),
                progress: CallProgress::Started,
            },
        }
    }

    pub(super) const fn result_recorded(&self) -> bool {
        match &self.state {
            CallState::Restored {
                result_recorded, ..
            } => *result_recorded,
            CallState::Live { progress, .. } => !matches!(progress, CallProgress::Started),
        }
    }

    pub(super) const fn yielded(&self) -> bool {
        match &self.state {
            CallState::Restored { yielded, .. } => *yielded,
            CallState::Live { progress, .. } => matches!(progress, CallProgress::Yielded),
        }
    }

    pub(super) fn live(&self, progress: CallProgress) -> bool {
        matches!(&self.state, CallState::Live { progress: current, .. } if *current == progress)
    }
}

pub(super) fn retain_call(calls: &mut Vec<InFlightCall>, call: InFlightCall) {
    calls.retain(|existing| existing.call_id != call.call_id);
    calls.push(call);
}

pub(super) fn in_flight_evidence(calls: &[InFlightCall], omitted: u32) -> Option<String> {
    if calls.is_empty() && omitted == 0 {
        return None;
    }
    let mut lines = calls
        .iter()
        .map(|call| {
            let state = if call.yielded() {
                "yielded; still running at the restart"
            } else if call.result_recorded() {
                "a result was observed before the restart"
            } else {
                "started; no result was observed"
            };
            format!(
                "- {} (call_id {}; {state}): {}",
                call.tool, call.call_id, call.arguments
            )
        })
        .collect::<Vec<_>>();
    if omitted > 0 {
        lines.push(format!(
            "- {omitted} additional observed call(s) omitted; this is not a complete history."
        ));
    }
    Some(format!(
        "Tool calls observed during the interrupted turn whose results are not known to be in \
         your restored history (calls already committed to it are not listed); any that are \
         absent from it may or may not have taken effect. This summary is \
         not a receipt: consult your retained history and reconcile tool receipts or external \
         effects before repeating any of them:\n{}",
        lines.join("\n")
    ))
}

const RESUME_ARGUMENT_BYTES: usize = 2 * 1024;

const CUT_TASK: &str = "\n\n[The task is cut here to fit this message; your history begins with \
                        it whole.]";

const RESUME_TASK_BYTES: usize = 16 * 1024;

pub(super) fn resume_message(calls: &[InFlightCall], omitted: u32, task: Option<&str>) -> String {
    let compose = |calls: &[InFlightCall], omitted: u32, task: Option<&str>| {
        let mut message = RESUME_MESSAGE.to_owned();
        if let Some(evidence) = in_flight_evidence(calls, omitted) {
            message.push_str("\n\n");
            message.push_str(&evidence);
        }
        if let Some(task) = task {
            message.push_str("\n\nDelegated task:\n");
            message.push_str(task);
        }
        message
    };
    let whole = compose(calls, omitted, task);
    if whole.len() <= MAX_MESSAGE_BYTES {
        return whole;
    }
    let quoted = calls
        .iter()
        .map(|call| InFlightCall {
            arguments: clip(&call.arguments, RESUME_ARGUMENT_BYTES),
            ..call.clone()
        })
        .collect::<Vec<_>>();
    let short = task.map(|task| cut_task(task, RESUME_TASK_BYTES));
    for dropped in 0..=quoted.len() {
        let more = u32::try_from(dropped).unwrap_or(u32::MAX);
        let message = compose(
            &quoted[dropped..],
            omitted.saturating_add(more),
            short.as_deref(),
        );
        if message.len() <= MAX_MESSAGE_BYTES {
            return message;
        }
    }
    let more = u32::try_from(quoted.len()).unwrap_or(u32::MAX);
    let head = compose(&[], omitted.saturating_add(more), Some(""));
    let room = MAX_MESSAGE_BYTES.saturating_sub(head.len());
    format!("{head}{}", cut_task(task.unwrap_or_default(), room))
}

fn cut_task(task: &str, max: usize) -> String {
    if task.len() <= max {
        return task.to_owned();
    }
    let mut end = max.saturating_sub(CUT_TASK.len());
    while !task.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{CUT_TASK}", &task[..end])
}

fn clip(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_owned();
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!(
        "{}… ({} more bytes not shown)",
        &text[..end],
        text.len() - end
    )
}

pub(super) enum JournaledCheckpoint {
    Current(SessionCheckpoint),

    LegacyCodex(Value),
}

impl Serialize for JournaledCheckpoint {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Current(checkpoint) => checkpoint.serialize(serializer),
            Self::LegacyCodex(value) => value.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for JournaledCheckpoint {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = Value::deserialize(deserializer)?;
        if value.get("format").is_some() {
            serde_json::from_value(value)
                .map(Self::Current)
                .map_err(serde::de::Error::custom)
        } else {
            Ok(Self::LegacyCodex(value))
        }
    }
}

impl JournaledCheckpoint {
    fn checkpoint(&self, lineage: &Lineage) -> std::io::Result<SessionCheckpoint> {
        match self {
            Self::Current(checkpoint) => {
                checkpoint.validate().map_err(std::io::Error::other)?;
                Ok(checkpoint.clone())
            }
            Self::LegacyCodex(value) => Ok(legacy_codex_checkpoint(value, lineage)?),
        }
    }
}

#[derive(Debug)]
pub(super) enum LegacyCheckpointError {
    Codex(String),

    Native(String),
}

impl std::fmt::Display for LegacyCheckpointError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Codex(detail) => {
                write!(formatter, "invalid journaled Codex checkpoint: {detail}")
            }
            Self::Native(detail) => {
                write!(formatter, "invalid journaled native checkpoint: {detail}")
            }
        }
    }
}

impl std::error::Error for LegacyCheckpointError {}

impl From<LegacyCheckpointError> for std::io::Error {
    fn from(error: LegacyCheckpointError) -> Self {
        Self::new(std::io::ErrorKind::InvalidData, error)
    }
}

fn legacy_field<T: serde::de::DeserializeOwned>(
    fields: &mut serde_json::Map<String, Value>,
    name: &str,
) -> Result<T, LegacyCheckpointError> {
    let value = fields
        .remove(name)
        .ok_or_else(|| LegacyCheckpointError::Codex(format!("missing {name}")))?;
    serde_json::from_value(value)
        .map_err(|error| LegacyCheckpointError::Codex(format!("invalid {name}: {error}")))
}

fn legacy_codex_checkpoint(
    value: &Value,
    lineage: &Lineage,
) -> Result<SessionCheckpoint, LegacyCheckpointError> {
    let Value::Object(fields) = value else {
        return Err(LegacyCheckpointError::Codex("expected an object".into()));
    };
    let mut payload = fields.clone();
    let session_id: String = legacy_field(&mut payload, "session_id")?;
    let model: nanocode_agent_core::Model = legacy_field(&mut payload, "model")?;
    let thinking: Thinking = legacy_field(&mut payload, "thinking")?;
    let conversation = payload
        .get("conversation")
        .filter(|conversation| !conversation.is_null());
    let has_conversation = conversation.is_some();
    let conversation_id = match conversation {
        Some(conversation) => conversation
            .get("lineage_id")
            .and_then(Value::as_str)
            .ok_or_else(|| LegacyCheckpointError::Codex("conversation has no lineage".into()))?
            .to_owned(),
        None => session_id.clone(),
    };
    Ok(SessionCheckpoint::native(
        session_id,
        ModelSelection::Codex(model),
        thinking,
        lineage.clone(),
        conversation_id,
        None,
        has_conversation,
        Value::Object(payload),
    ))
}

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct PersistedNative {
    model: String,
    session_id: String,
    thinking: Thinking,
    payload: String,
    has_conversation: bool,
}

impl PersistedNative {
    fn into_checkpoint(
        self,
        lineage: &Lineage,
    ) -> Result<SessionCheckpoint, LegacyCheckpointError> {
        let model = self.model.parse::<ModelSelection>().map_err(|error| {
            LegacyCheckpointError::Native(format!("invalid journaled model: {error}"))
        })?;
        if self.session_id.trim().is_empty() {
            return Err(LegacyCheckpointError::Native("empty session ID".into()));
        }
        let payload = serde_json::from_str(&self.payload).map_err(|error| {
            LegacyCheckpointError::Native(format!("payload is not JSON: {error}"))
        })?;
        Ok(SessionCheckpoint::native(
            self.session_id.clone(),
            model,
            self.thinking,
            lineage.clone(),
            self.session_id,
            None,
            self.has_conversation,
            payload,
        ))
    }
}

pub(super) struct JournalStore(pub(super) Arc<dyn nanocode_agent_core::backend::ChildJournalStore>);

impl SubagentStore for JournalStore {
    fn pending_turn<'a>(
        &'a self,
        session_id: &'a str,
    ) -> SubagentStoreFuture<'a, std::io::Result<Option<(String, Value)>>> {
        self.0.pending_turn(session_id.to_owned())
    }

    fn cancelled_turns<'a>(
        &'a self,
        session_id: &'a str,
        operation_ids: Vec<String>,
    ) -> SubagentStoreFuture<'a, std::io::Result<Vec<String>>> {
        self.0.cancelled_turns(session_id.to_owned(), operation_ids)
    }

    fn load<'a>(
        &'a self,
        _root_session_id: &'a str,
    ) -> SubagentStoreFuture<'a, std::io::Result<Option<String>>> {
        self.0.load()
    }

    fn save<'a>(
        &'a self,
        _root_session_id: &'a str,
        payload: String,
        records: Vec<Arc<str>>,
    ) -> SubagentStoreFuture<'a, std::io::Result<()>> {
        self.0.save(payload, records)
    }

    fn save_deferred<'a>(
        &'a self,
        _root_session_id: &'a str,
    ) -> SubagentStoreFuture<
        'a,
        std::io::Result<Option<nanocode_agent_core::backend::DeferredJournal>>,
    > {
        self.0.save_deferred()
    }

    fn load_record<'a>(
        &'a self,
        _root_session_id: &'a str,
        key: &'a str,
    ) -> SubagentStoreFuture<'a, std::io::Result<String>> {
        self.0.load_record(key.to_owned())
    }

    fn load_records<'a>(
        &'a self,
        _root_session_id: &'a str,
        keys: Vec<String>,
    ) -> SubagentStoreFuture<'a, std::io::Result<Vec<String>>> {
        self.0.load_records(keys)
    }

    fn record_session<'a>(
        &'a self,
        _root_session_id: &'a str,
        checkpoint: SessionCheckpoint,
    ) -> SubagentStoreFuture<'a, std::io::Result<()>> {
        self.0.record_child(checkpoint)
    }

    fn records_sessions(&self) -> bool {
        self.0.records_children()
    }
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct RestoreReport {
    pub restored: usize,

    pub interrupted: Vec<AgentId>,

    pub unrecoverable: Vec<AgentId>,

    pub completed: Vec<AgentId>,
}

pub(super) const RESUME_MESSAGE: &str = "The sub-agent runtime restarted while your previous \
turn was running. Your conversation was restored from its latest committed checkpoint: every \
step in your history completed, but a tool call that was still running at the restart is not \
shown and may or may not have taken effect. Inspect the current workspace state where that \
matters, do not repeat side effects that already happened, and continue your delegated task \
from where your history ends. Submit a result only once the whole delegated task is complete.";

pub(super) fn checkpoint_ref(
    session: &ChildSession,
    checkpoint: Option<&JournalCheckpoint>,
    records: &mut Vec<(Arc<str>, Arc<str>)>,
) -> std::io::Result<Option<Arc<str>>> {
    Ok(match (checkpoint, session.stored_runtime.checkpoint()) {
        (Some(checkpoint), _) => {
            if let Some(json) = &checkpoint.pending {
                records.push((Arc::clone(&checkpoint.key), Arc::clone(json)));
            }
            Some(Arc::clone(&checkpoint.key))
        }
        (None, Some(snapshot)) => {
            let (key, json) = encoded(snapshot)?;
            records.push((Arc::clone(&key), json));
            Some(key)
        }
        (None, None) => None,
    })
}

pub(super) fn persist_agent(
    session: &ChildSession,
    checkpoint_ref: Option<Arc<str>>,
) -> PersistedAgent {
    let output_in_status = matches!(
        &session.status,
        AgentStatus::Completed { output } if session.last_output.as_ref() == Some(output)
    );
    PersistedAgent {
        descriptor: session.descriptor.clone(),
        binding_task: (session.binding_task != session.descriptor.task)
            .then(|| session.binding_task.clone()),
        status: session.status.clone(),
        output_schema: session.output_schema.clone(),
        host_context: session.host_context.as_deref().map(str::to_owned),
        last_output: (!output_in_status)
            .then(|| session.last_output.clone())
            .flatten(),
        output_in_status,
        next_instruction_revision: session.next_instruction_revision,
        turn_in_flight: session.active() || matches!(session.status, AgentStatus::Running),
        resume_attempts: session.resume_attempts,
        checkpoint: None,
        native_checkpoint: None,
        checkpoint_ref,
        in_flight_calls: session.in_flight_calls.clone(),
        in_flight_omitted: session.in_flight_omitted,
        accepted_output: session.submitted_output().cloned(),
        accepted_revision: session
            .submitted_output()
            .and_then(|_| session.turn_revision()),
        pending_completion: session.completion.revision(),
        settled_revision: session.settled_revision,
        usage: session.usage.clone(),
        completion_attempt: session.completion.attempt(),
        completion_parked: session.completion.parked(),
        cancelled_revision: session.cancelled_revision,
        conversation: session.conversation,
        keeps_result: session.follow_up_output.is_some(),
        waited: session.waited.clone(),
        sent: session.sent.clone(),
        spawned: session.spawned.clone(),
    }
}

impl PersistedAgent {
    pub(super) fn expand(&mut self) {
        if std::mem::take(&mut self.output_in_status)
            && let AgentStatus::Completed { output } = &self.status
        {
            self.last_output = Some(output.clone());
        }
    }

    const fn running(&self) -> bool {
        !matches!(self.status, AgentStatus::Closing | AgentStatus::Closed)
            && (self.turn_in_flight
                || matches!(self.status, AgentStatus::Running | AgentStatus::Pending)
                || (matches!(self.status, AgentStatus::Interrupted) && self.resume_attempts > 0))
    }

    pub(super) fn unmarked_spawning_turn(&self) -> Option<&str> {
        (self.descriptor.parent.is_none() && self.cancelled_revision.is_none() && self.running())
            .then_some(self.host_context.as_deref())
            .flatten()
    }

    pub(super) async fn hydrate(
        &mut self,
        store: &dyn SubagentStore,
        root_session_id: &str,
    ) -> std::io::Result<Option<JournalCheckpoint>> {
        let Some(key) = self.checkpoint_ref.take() else {
            return Ok(None);
        };
        let json = store.load_record(root_session_id, &key).await?;
        if !key_matches(&json, &key) {
            return Err(std::io::Error::other(format!(
                "subagent checkpoint {key} does not match its record"
            )));
        }
        let invalid = |error: serde_json::Error| {
            std::io::Error::other(format!("invalid subagent checkpoint {key}: {error}"))
        };
        let record: Value = serde_json::from_str(&json).map_err(invalid)?;
        if record.get("format").is_some() {
            let checkpoint: SessionCheckpoint = serde_json::from_value(record).map_err(invalid)?;
            checkpoint.validate().map_err(std::io::Error::other)?;
            self.checkpoint = Some(JournaledCheckpoint::Current(checkpoint.clone()));
            return Ok(Some(JournalCheckpoint::stored(key, Some(checkpoint))));
        }
        let record: LegacyCheckpointRecord = serde_json::from_value(record).map_err(invalid)?;
        self.checkpoint = record.checkpoint.map(JournaledCheckpoint::LegacyCodex);
        self.native_checkpoint = record.native_checkpoint;
        Ok(None)
    }

    pub(super) fn snapshot(&self, lineage: &Lineage) -> std::io::Result<Option<SessionCheckpoint>> {
        if let Some(checkpoint) = &self.checkpoint {
            return checkpoint.checkpoint(lineage).map(Some);
        }
        Ok(self
            .native_checkpoint
            .clone()
            .map(|native| native.into_checkpoint(lineage))
            .transpose()?)
    }
}

pub(super) fn journaled_lineages(root_session_id: &str, agents: &[PersistedAgent]) -> Vec<Lineage> {
    let by_id = agents
        .iter()
        .map(|agent| (agent.descriptor.id, agent))
        .collect::<HashMap<_, _>>();
    let parent_of = |id: AgentId| by_id.get(&id).and_then(|agent| agent.descriptor.parent);
    agents
        .iter()
        .map(|agent| {
            let id = agent.descriptor.id;
            let parent_session_id = parent_of(id)
                .and_then(|parent| by_id.get(&parent))
                .map_or_else(
                    || root_session_id.to_owned(),
                    |parent| parent.descriptor.session_id.clone(),
                );
            let mut depth = 1;
            let mut current = parent_of(id);
            while let Some(parent) = current
                && depth <= agents.len()
            {
                depth += 1;
                current = parent_of(parent);
            }
            Lineage::new(
                root_session_id,
                Some(parent_session_id),
                nanocode_agent_core::Origin::Subagent,
                u32::try_from(depth).unwrap_or(u32::MAX),
            )
        })
        .collect()
}

const fn is_zero(value: &u32) -> bool {
    *value == 0
}

pub(super) fn restored_session(
    agent: PersistedAgent,
    snapshot: Option<SessionCheckpoint>,
) -> std::io::Result<(ChildSession, bool, bool)> {
    let contract = OutputContract::compile(&agent.output_schema)?;
    let accepted_revision = agent.accepted_revision;
    let pending_completion = agent.pending_completion;
    let settled_revision = agent.settled_revision;
    let root_child = agent.descriptor.parent.is_none();

    let journaled = snapshot.is_none() && agent.checkpoint_ref.is_some();
    let recoverable = snapshot.is_some() || journaled;
    let terminal = matches!(agent.status, AgentStatus::Closing | AgentStatus::Closed);
    let running = agent.running();

    let accepted = running.then_some(agent.accepted_output).flatten();
    let in_flight = running && accepted.is_none();
    let kept = (in_flight && agent.keeps_result)
        .then(|| agent.last_output.clone())
        .flatten();
    let completed_by_acceptance = accepted.is_some();
    let evidence = in_flight_evidence(&agent.in_flight_calls, agent.in_flight_omitted);
    let status = if terminal {
        AgentStatus::Closed
    } else if in_flight && !recoverable {
        let mut error = "subagent could not be restored after a runtime restart: no portable \
                         checkpoint was available"
            .to_owned();
        if let Some(evidence) = &evidence {
            error.push_str("\n\n");
            error.push_str(evidence);
        }
        AgentStatus::Failed { error }
    } else if in_flight {
        AgentStatus::Interrupted
    } else if let Some(output) = &accepted {
        AgentStatus::Completed {
            output: output.clone(),
        }
    } else {
        agent.status
    };
    let resume = in_flight && recoverable;
    let resume_attempts = if resume {
        agent
            .resume_attempts
            .checked_add(1)
            .ok_or_else(|| std::io::Error::other("subagent recovery attempt counter overflow"))?
    } else {
        0
    };
    let binding_task = agent.binding_task;
    let usage = agent.usage;
    let mut session = ChildSession::restored(
        agent.descriptor,
        agent.host_context.map(Arc::from),
        status,
        contract,
        match snapshot {
            Some(snapshot) => StoredRuntime::Checkpoint(Box::new(snapshot)),
            None if journaled => StoredRuntime::Journaled,
            None => StoredRuntime::None,
        },
        agent.next_instruction_revision,
        accepted.or(agent.last_output),
    );
    if let Some(task) = binding_task {
        session.binding_task = task;
    }
    session.resume_attempts = resume_attempts;
    session.usage = usage;
    session.conversation = agent.conversation;
    session.follow_up_output = kept;
    session.waited = agent.waited;
    session.sent = agent.sent;
    session.spawned = agent.spawned;

    let accepted_revision = accepted_revision.unwrap_or(agent.next_instruction_revision);
    session.settled_revision = if completed_by_acceptance {
        Some(accepted_revision)
    } else {
        settled_revision
    };
    if root_child {
        session.cancelled_revision = agent.cancelled_revision;
        if completed_by_acceptance {
            session.settle_completion(accepted_revision);
        } else if pending_completion.is_some() {
            session.completion = Completion::journaled(
                pending_completion,
                agent.completion_attempt,
                agent.completion_parked,
            );
        }
    }

    if in_flight {
        session.in_flight_calls = agent.in_flight_calls;
        session.in_flight_omitted = agent.in_flight_omitted;
    }
    Ok((session, resume, !recoverable && !terminal))
}
