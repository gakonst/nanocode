use super::{
    model::{AgentDescriptor, AgentId, AgentStatus},
    runtime::{ChildSession, OutputContract},
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

pub(super) const JOURNAL_VERSION: u32 = 4;

pub(super) const LAZY_CHECKPOINT_VERSION: u32 = 3;

const PAGE_MESSAGES: usize = 64;

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

    pub(super) fn stored(key: String, checkpoint: Option<SessionCheckpoint>) -> Self {
        Self {
            key: key.into(),
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
    #[serde(default)]
    pub(super) messages: PersistedMessages,
}

#[derive(Default, Serialize, Deserialize)]
pub(super) struct PersistedMessages {
    pub(super) next_message_id: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) records: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) tail: Option<Arc<str>>,
    pub(super) pending: Vec<super::model::MessageId>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MessagePage {
    previous: Option<Arc<str>>,
    messages: Vec<Arc<str>>,
}

impl PersistedMessages {
    pub(super) async fn hydrate(
        self,
        store: &dyn SubagentStore,
        root_session_id: &str,
    ) -> std::io::Result<super::message::MessageThreads> {
        let mut keys: Vec<Arc<str>> = self.records.into_iter().map(Arc::from).collect();
        let mut pages = Vec::new();
        let mut next = self.tail;
        while let Some(key) = next {
            let json = load_verified(store, root_session_id, &key).await?;
            let page: MessagePage = serde_json::from_str(&json).map_err(std::io::Error::other)?;
            next = page.previous;
            pages.push(page.messages);
        }
        keys.extend(pages.into_iter().rev().flatten());
        let mut messages = Vec::with_capacity(keys.len());
        for key in keys {
            let json = load_verified(store, root_session_id, &key).await?;
            messages.push(serde_json::from_str(&json).map_err(std::io::Error::other)?);
        }
        super::message::MessageThreads::restore(self.next_message_id, messages, self.pending)
    }
}

#[derive(Default)]
pub(super) struct JournalCache {
    agents: HashMap<AgentId, CachedAgent>,
    messages: HashMap<super::model::MessageId, Arc<str>>,
    pages: Vec<Arc<str>>,
    paged: Vec<Arc<str>>,
}

struct CachedAgent {
    generation: u64,
    checkpoint: Option<Arc<str>>,
    key: Arc<str>,
}

pub(super) enum AgentEntry {
    Cached(Arc<str>),
    Changed {
        generation: u64,
        checkpoint: Option<Arc<str>>,
        agent: Box<PersistedAgent>,
    },
}

pub(super) enum MessageEntry {
    Cached(Arc<str>),
    New(super::model::AgentMessage),
}

pub(super) struct JournalSnapshot {
    pub(super) agents: Vec<(AgentId, AgentEntry)>,
    pub(super) messages: Vec<(super::model::MessageId, MessageEntry)>,
    pub(super) next_message_id: u64,
    pub(super) pending: Vec<super::model::MessageId>,
    pub(super) records: Vec<(Arc<str>, Arc<str>)>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AgentProfile {
    descriptor: AgentDescriptor,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    binding_task: Option<String>,
    output_schema: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    host_context: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AgentRecord {
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
    checkpoint_ref: Option<String>,
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
}

fn encode_agent(
    agent: PersistedAgent,
    records: &mut Vec<(Arc<str>, Arc<str>)>,
) -> std::io::Result<Arc<str>> {
    let (profile, json) = encoded(&AgentProfile {
        descriptor: agent.descriptor,
        binding_task: agent.binding_task,
        output_schema: agent.output_schema,
        host_context: agent.host_context,
    })?;
    records.push((Arc::clone(&profile), json));
    let (key, json) = encoded(&AgentRecord {
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
    })?;
    records.push((Arc::clone(&key), json));
    Ok(key)
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
        .iter()
        .zip(&records)
        .map(|((_, key), json)| {
            serde_json::from_str::<AgentRecord>(json).map_err(|error| {
                std::io::Error::other(format!("invalid subagent journal record {key}: {error}"))
            })
        })
        .collect::<std::io::Result<Vec<_>>>()?;
    let profiles = verified(
        store,
        root_session_id,
        records
            .iter()
            .map(|record| record.profile.to_string())
            .collect(),
    )
    .await?;
    agents
        .into_iter()
        .zip(records)
        .zip(profiles)
        .map(|(((id, key), record), profile)| {
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
    };
    agent.expand();
    Ok(agent)
}

pub(super) struct EncodedJournal {
    pub(super) payload: String,
    pub(super) records: Vec<(Arc<str>, Arc<str>)>,
    update: JournalCache,
}

impl JournalCache {
    pub(super) fn agent(
        &self,
        id: AgentId,
        generation: u64,
        checkpoint: Option<&Arc<str>>,
    ) -> Option<&Arc<str>> {
        self.agents
            .get(&id)
            .filter(|cached| {
                cached.generation == generation && cached.checkpoint.as_ref() == checkpoint
            })
            .map(|cached| &cached.key)
    }

    pub(super) fn message(&self, id: super::model::MessageId) -> Option<&Arc<str>> {
        self.messages.get(&id)
    }

    pub(super) fn committed(&mut self, encoded: EncodedJournal) {
        *self = encoded.update;
    }
}

impl JournalSnapshot {
    pub(super) fn encode(self, cache: &JournalCache) -> std::io::Result<EncodedJournal> {
        let mut records = self.records;
        let mut update = JournalCache::default();
        let mut agent_records = Vec::with_capacity(self.agents.len());
        for (id, entry) in self.agents {
            let key = match entry {
                AgentEntry::Cached(key) => {
                    if let Some(cached) = cache.agents.get(&id) {
                        update.agents.insert(
                            id,
                            CachedAgent {
                                generation: cached.generation,
                                checkpoint: cached.checkpoint.clone(),
                                key: Arc::clone(&key),
                            },
                        );
                    }
                    key
                }
                AgentEntry::Changed {
                    generation,
                    checkpoint,
                    agent,
                } => {
                    let key = encode_agent(*agent, &mut records)?;
                    update.agents.insert(
                        id,
                        CachedAgent {
                            generation,
                            checkpoint,
                            key: Arc::clone(&key),
                        },
                    );
                    key
                }
            };
            agent_records.push((id, key));
        }
        let mut keys = Vec::with_capacity(self.messages.len());
        for (id, entry) in self.messages {
            let key = match entry {
                MessageEntry::Cached(key) => key,
                MessageEntry::New(message) => {
                    let (key, json) = encoded(&message)?;
                    records.push((Arc::clone(&key), json));
                    key
                }
            };
            update.messages.insert(id, Arc::clone(&key));
            keys.push(key);
        }
        let shared = keys
            .iter()
            .zip(&cache.paged)
            .take_while(|(current, cached)| current == cached)
            .count();
        let reused = (shared / PAGE_MESSAGES).min(cache.pages.len());
        update.pages = cache.pages[..reused].to_vec();
        for chunk in keys[reused * PAGE_MESSAGES..].chunks(PAGE_MESSAGES) {
            let (key, json) = encoded(&MessagePage {
                previous: update.pages.last().cloned(),
                messages: chunk.to_vec(),
            })?;
            records.push((Arc::clone(&key), json));
            update.pages.push(key);
        }
        let tail = update.pages.last().cloned();
        update.paged = keys;
        let payload = serde_json::to_string(&PersistedScope {
            version: JOURNAL_VERSION,
            agents: Vec::new(),
            agent_records,
            messages: PersistedMessages {
                next_message_id: self.next_message_id,
                records: Vec::new(),
                tail,
                pending: self.pending,
            },
        })
        .map_err(std::io::Error::other)?;
        Ok(EncodedJournal {
            payload,
            records,
            update,
        })
    }
}

#[derive(Serialize, Deserialize)]
pub(super) struct PersistedAgent {
    pub(super) descriptor: AgentDescriptor,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) binding_task: Option<String>,
    pub(super) status: AgentStatus,
    pub(super) output_schema: Value,
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
    pub(super) checkpoint_ref: Option<String>,

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

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) cancelled_revision: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct InFlightCall {
    pub(super) call_id: String,
    pub(super) tool: String,

    pub(super) arguments: String,

    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(super) result_recorded: bool,

    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(super) yielded: bool,

    #[serde(skip)]
    pub(super) parent_call_id: Option<String>,

    #[serde(skip)]
    pub(super) live: bool,

    #[serde(skip)]
    pub(super) settled: bool,

    #[serde(skip)]
    pub(super) committable: bool,
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
            result_recorded: false,
            yielded: false,
            parent_call_id: parent_call_id.map(str::to_owned),
            live: true,
            settled: false,
            committable: false,
        }
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
            let state = if call.yielded {
                "yielded; still running at the restart"
            } else if call.result_recorded {
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

pub(super) fn persist_agent(
    session: &ChildSession,
    checkpoint: Option<&JournalCheckpoint>,
    records: &mut Vec<(Arc<str>, Arc<str>)>,
) -> std::io::Result<PersistedAgent> {
    let checkpoint_ref = match (checkpoint, &session.stored_runtime) {
        (Some(checkpoint), _) => {
            if let Some(json) = &checkpoint.pending {
                records.push((Arc::clone(&checkpoint.key), Arc::clone(json)));
            }
            Some(checkpoint.key.to_string())
        }
        (None, Some(snapshot)) => {
            let (key, json) = encoded(snapshot)?;
            records.push((Arc::clone(&key), json));
            Some(key.to_string())
        }
        (None, None) => None,
    };
    let output_in_status = matches!(
        &session.status,
        AgentStatus::Completed { output } if session.last_output.as_ref() == Some(output)
    );
    Ok(PersistedAgent {
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
        turn_in_flight: session.active || matches!(session.status, AgentStatus::Running),
        resume_attempts: session.resume_attempts,
        checkpoint: None,
        native_checkpoint: None,
        checkpoint_ref,
        in_flight_calls: session.in_flight_calls.clone(),
        in_flight_omitted: session.in_flight_omitted,
        accepted_output: session
            .active
            .then(|| session.submitted_output.clone())
            .flatten(),
        accepted_revision: (session.active && session.submitted_output.is_some())
            .then_some(session.active_instruction_revision)
            .flatten(),
        pending_completion: session.pending_completion,
        settled_revision: session.settled_revision,
        usage: session.usage.clone(),
        completion_attempt: session.completion_attempt,
        completion_parked: session.completion_parked,
        cancelled_revision: session.cancelled_revision,
    })
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

pub(super) fn journaled_lineage(
    root_session_id: &str,
    agents: &[PersistedAgent],
    id: AgentId,
) -> Lineage {
    let parent_of = |id: AgentId| {
        agents
            .iter()
            .find(|agent| agent.descriptor.id == id)
            .and_then(|agent| agent.descriptor.parent)
    };
    let parent_session_id = parent_of(id)
        .and_then(|parent| agents.iter().find(|agent| agent.descriptor.id == parent))
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
        agent.output_schema,
        snapshot,
        agent.next_instruction_revision,
        accepted.or(agent.last_output),
    );
    if let Some(task) = binding_task {
        session.binding_task = task;
    }
    session.resume_attempts = resume_attempts;
    session.usage = usage;

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
            session.pending_completion = pending_completion;
            session.completion_attempt = agent.completion_attempt;
            session.completion_parked = agent.completion_parked;
        }
    }
    session.journaled_runtime = journaled;

    if in_flight {
        session.in_flight_calls = agent.in_flight_calls;
        session.in_flight_omitted = agent.in_flight_omitted;
    }
    Ok((session, resume, !recoverable && !terminal))
}
