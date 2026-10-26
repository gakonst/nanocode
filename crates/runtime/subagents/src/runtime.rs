use super::diagnostics::{CompletionError, CompletionErrorCode};
use super::durable::{self, RestoreReport, SubagentStore};
use super::{
    capacity::{Capacity, TurnCapacity},
    harness::{self, HarnessHandle},
    message::MessageThreads,
    model::{
        AgentContext, AgentDescriptor, AgentId, AgentMessage, AgentMessageUpdate, AgentStatus,
        AgentThread, AgentUpdate, MessageDeliveryState, MessageDisposition, MessageId,
        MessagePriority, MessagePurpose, MessageSender, ScopedAgentUpdate, SubagentRuntimeId,
        ThreadId,
    },
    platform::{self, Task, timeout_at},
    task_tree::TaskTree,
};
use futures_util::future::join_all;
use nanocode_agent_core::{
    Agent, AgentEvents, AgentHandle, AgentUsage, ExecutionPolicyDisposition, ModelSelection,
    NanocodeError, PromptRoute, ReasoningMode, Result as AgentResult, SessionCheckpoint,
    SteerReceiptStatus, Thinking, TurnResult, WeakAgent,
    events::{AgentEventKind, RunStatus},
};
use nanocode_platform::Instant;
use nanocode_schema::Validator;
use nanocode_sync::{Atom, Owned, Sequencer, Turn};
use nanocode_tool_contracts::ToolReceipt;
use serde::Serialize;
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    sync::{
        Arc, Weak,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::sync::{mpsc, oneshot, watch};
use tracing::Instrument;

mod activity;
mod completion;
mod focus;

pub use activity::SubagentActivity;
pub use completion::{RunningSubagents, SubagentCompletion};

pub(super) struct ChildSession {
    pub(super) descriptor: AgentDescriptor,

    pub(super) binding_task: String,
    pub(super) host_context: Option<Arc<str>>,
    pub(super) event_task: Option<Task<()>>,
    pub(super) harness: Option<HarnessHandle>,
    pub(super) harness_task: Option<Task<()>>,
    pub(super) status: AgentStatus,
    pub(super) active: bool,
    pub(super) output_validator: Validator,
    pub(super) output_schema: Value,
    pub(super) stored_runtime: Option<SessionCheckpoint>,

    pub(super) journaled_runtime: bool,
    pub(super) next_instruction_revision: u64,
    pub(super) active_instruction_revision: Option<u64>,
    pub(super) steering: bool,
    pub(super) submitted_output: Option<Value>,
    pub(super) last_output: Option<Value>,
    pub(super) last_used: u64,
    pub(super) evicted: bool,

    pub(super) resume_attempts: u32,

    pub(super) in_flight_calls: Vec<durable::InFlightCall>,

    pub(super) in_flight_omitted: u32,

    pub(super) pending_completion: Option<u64>,

    pub(super) completion_attempt: u32,

    pub(super) completion_parked: bool,

    pub(super) cancelled_revision: Option<u64>,

    pub(super) in_spawning_turn: bool,

    pub(super) settled_revision: Option<u64>,

    pub(super) usage: AgentUsage,

    pub(super) in_flight_progress: durable::OmittedProgress,

    announce: bool,

    journal_generation: u64,
}

static JOURNAL_GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

fn next_generation() -> u64 {
    JOURNAL_GENERATION.fetch_add(1, Ordering::Relaxed)
}

#[derive(Default)]
pub(super) struct Sessions(HashMap<AgentId, ChildSession>);

impl Sessions {
    pub(super) fn get(&self, id: &AgentId) -> Option<&ChildSession> {
        self.0.get(id)
    }

    pub(super) fn get_mut(&mut self, id: &AgentId) -> Option<&mut ChildSession> {
        let session = self.0.get_mut(id)?;
        session.journal_generation = next_generation();
        Some(session)
    }

    fn insert(&mut self, id: AgentId, mut session: ChildSession) {
        session.journal_generation = next_generation();
        self.0.insert(id, session);
    }

    pub(super) fn iter(&self) -> impl Iterator<Item = (&AgentId, &ChildSession)> {
        self.0.iter()
    }

    pub(super) fn keys(&self) -> impl Iterator<Item = &AgentId> {
        self.0.keys()
    }

    pub(super) fn values(&self) -> impl Iterator<Item = &ChildSession> {
        self.0.values()
    }

    fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl std::ops::Index<&AgentId> for Sessions {
    type Output = ChildSession;

    fn index(&self, id: &AgentId) -> &ChildSession {
        &self.0[id]
    }
}

pub(super) struct OutputContract {
    validator: Validator,
    schema: String,
}

impl OutputContract {
    pub(super) fn compile(schema: &Value) -> std::io::Result<Self> {
        let validator = nanocode_schema::validator_for(schema)
            .map_err(|error| std::io::Error::other(format!("invalid output_schema: {error}")))?;
        let schema = serde_json::to_string(schema)
            .map_err(|error| std::io::Error::other(format!("could not render schema: {error}")))?;
        Ok(Self { validator, schema })
    }
}

pub const COMPLETION_INSTRUCTIONS_LEAD: &str = "Before finishing, call `submit_result`";

pub(super) fn completion_instructions(schema: &str) -> String {
    format!(
        "{COMPLETION_INSTRUCTIONS_LEAD} with `{{ output: ... }}` and a JSON value \
         matching the output schema below. Pass objects and arrays directly as JSON values. Use the callable entry in your actual tool catalog; \
         do not assume a Code Mode binding exists unless that catalog exposes it. \
         If validation rejects the value, correct it and retry. If the result is superseded, \
         incorporate the pending instructions and submit an updated result without repeating \
         completed task side effects. Finish after acceptance. A turn that ends without an \
         accepted result fails.\n\nOutput schema:\n{schema}"
    )
}

pub(super) fn completion_reminder() -> String {
    format!(
        "{COMPLETION_INSTRUCTIONS_LEAD} with output matching the output schema from your \
         assignment."
    )
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum SubmissionOutcome {
    Accepted { decoded_json_text: bool },
    Superseded,
}

#[derive(Clone)]
enum RestorationState {
    Loading,
    Resuming,
    Ready,
    Failed(String),
}

type RestorationOutcome = watch::Receiver<RestorationState>;

pub struct Registry {
    session_handles: Atom<HashMap<String, AgentHandle>>,
    spawn_router: Atom<Option<Arc<dyn crate::SpawnRouter>>>,
    id: SubagentRuntimeId,
    state: Owned<RegistryState>,
    pub(super) updates: mpsc::UnboundedSender<ScopedAgentUpdate>,
    revision: watch::Sender<u64>,
    capacity: Capacity,
    max_resident: AtomicUsize,

    max_resident_explicit: std::sync::atomic::AtomicBool,
    scope_locks: Atom<HashMap<String, Arc<ScopeLocks>>>,
    store: Atom<Option<Arc<dyn SubagentStore>>>,

    journals: Atom<HashMap<String, Arc<dyn SubagentStore>>>,

    claims: Atom<JournalClaims>,

    spawn_admission: Atom<HashMap<String, Sequencer>>,
    journal_writer: std::sync::atomic::AtomicBool,

    journal_writers: Atom<HashMap<String, mpsc::UnboundedSender<JournalRequest>>>,
    checkpoints: Atom<HashMap<(String, AgentId), durable::JournalCheckpoint>>,

    pending_checkpoints: Atom<HashMap<String, usize>>,

    progress_captures: Atom<HashMap<(String, AgentId), bool>>,

    completed_tools: Atom<std::collections::HashSet<(String, AgentId)>>,
    pending_resume: Atom<HashMap<String, Vec<AgentId>>>,

    wait_reported: Atom<HashMap<(String, AgentId), u64>>,

    idle_waits: Atom<HashMap<String, (Vec<AgentId>, u32)>>,

    this: std::sync::OnceLock<Weak<Self>>,

    status_queues: Atom<HashMap<(String, AgentId), mpsc::UnboundedSender<OrderedJob>>>,

    completion_retries: Atom<HashMap<(String, AgentId), (u32, bool)>>,
    completion_dispatch: Atom<HashMap<(String, AgentId), Sequencer>>,
    completion_routes: Atom<HashMap<(String, AgentId), u64>>,
    result_readers: Atom<HashMap<(String, AgentId), Readers>>,
    root_watches: Atom<HashSet<String>>,
    running_roots: Atom<HashSet<String>>,

    parent_resolver: Atom<Option<ParentResolver>>,

    pub(super) spawn_policy: Atom<crate::SpawnPolicy>,
}

#[derive(Clone, Default)]
struct JournalClaims {
    restored: HashMap<String, RestorationOutcome>,
    reserved: HashMap<String, nanocode_agent_core::backend::ChildJournal>,
}

pub type ParentResolver = Arc<dyn Fn(&str) -> Option<Agent> + Send + Sync>;

pub fn completion_continuation_prompt(id: AgentId, status: &AgentStatus) -> String {
    match status {
        AgentStatus::Failed { error } => format!(
            "Subagent {id} failed: {error}\n\nContinue the current task: decide whether to retry, \
             recover, or report the failure, perform any remaining work, and then respond to the \
             user. Do not repeat side effects the subagent already completed.\n\n\
             <subagent_completion agent_id=\"{id}\" status=\"failed\" />"
        ),
        AgentStatus::Completed { output } => format!(
            "Subagent {id} completed with this structured result:\n{output}\n\nContinue the \
             current task using only the result fields needed for the next step. Integrate or \
             verify them as appropriate, perform any remaining work, and then respond to the \
             user. Do not merely repeat the raw result.\n\n<subagent_completion \
             agent_id=\"{id}\" status=\"completed\" />"
        ),
        _ => format!(
            "Subagent {id} was interrupted before finishing. Continue the current task: call \
             list_agents with include_completed=true to inspect agent {id}, decide whether to \
             resume it with a message, replace it, or finish without it, and then respond to the \
             user.\n\n<subagent_completion agent_id=\"{id}\" status=\"interrupted\" />"
        ),
    }
}

fn completion_retry_delay(attempt: u32) -> Duration {
    Duration::from_millis(1000_u64 << attempt.clamp(1, 9)).min(Duration::from_secs(300))
}

#[derive(Clone, Copy)]
enum NoticeOutcome {
    Read,
    Unread,
    Undelivered,
    Pending,
    Cancelled,
    Stopped,
}

impl NoticeOutcome {
    fn of_turn(result: AgentResult<TurnResult>) -> Self {
        let Err(error) = result else {
            return Self::Read;
        };
        match (&error, error.execution_policy_disposition()) {
            (NanocodeError::TurnCancelled, _) => Self::Cancelled,
            (NanocodeError::AgentStopped | NanocodeError::TurnStopped, _)
            | (_, Some(ExecutionPolicyDisposition::Reopen)) => Self::Stopped,
            (_, Some(ExecutionPolicyDisposition::Retry)) => Self::Pending,
            (NanocodeError::Response(failure), _) if failure.retryable() => Self::Pending,
            _ => Self::Cancelled,
        }
    }

    async fn of_steer(
        mut events: AgentEvents,
        request_id: &str,
        prompt: String,
        parent: &WeakAgent,
    ) -> Self {
        let mut carrier = None;
        while let Some(event) = events.recv().await {
            let steered = event.kind == AgentEventKind::RunSteered;
            if !steered
                && !matches!(
                    event.kind,
                    AgentEventKind::RunCompleted | AgentEventKind::RunFailed
                )
            {
                continue;
            }
            let Ok(run) = event.decode_payload::<RootRunEvent>() else {
                continue;
            };
            if steered {
                if run.message_id.as_deref() == Some(request_id) {
                    carrier = run.turn_id;
                }
                continue;
            }
            if carrier.is_some() && carrier != run.turn_id {
                continue;
            }
            return match run.status {
                Some(RunStatus::Completed) if carrier.is_some() => Self::Read,
                Some(RunStatus::Completed) => match parent.upgrade() {
                    Some(parent)
                        if matches!(
                            parent.steer_receipt(request_id.to_owned(), prompt).await,
                            Ok(SteerReceiptStatus::Accepted)
                        ) =>
                    {
                        Self::Read
                    }
                    _ => Self::Unread,
                },
                Some(RunStatus::Cancelled) => Self::Cancelled,
                _ => Self::Unread,
            };
        }
        Self::Stopped
    }

    async fn of_tool_output(mut events: AgentEvents, readers: &Readers) -> Self {
        let mut carried = false;
        let mut carrying_call = None;
        while let Some(event) = events.recv().await {
            match event.kind {
                AgentEventKind::ToolResult => {
                    let Ok(result) = event.decode_payload::<ToolResultCall>() else {
                        continue;
                    };
                    if readers.read(|readers| {
                        readers
                            .iter()
                            .any(|receipt| receipt.carries(&result.call_id))
                    }) {
                        carried = true;
                        carrying_call = None;
                    }
                }
                AgentEventKind::ModelCallStarted if carried => {
                    carrying_call = event
                        .decode_payload::<ModelCall>()
                        .ok()
                        .map(|call| call.call_index);
                }
                AgentEventKind::ModelCallCompleted if carrying_call.is_some() => {
                    if event
                        .decode_payload::<ModelCall>()
                        .is_ok_and(|call| Some(call.call_index) == carrying_call)
                    {
                        return Self::Read;
                    }
                }
                AgentEventKind::RunCompleted | AgentEventKind::RunFailed => {
                    let cancelled = event
                        .decode_payload::<RootRunEvent>()
                        .is_ok_and(|run| run.status == Some(RunStatus::Cancelled));
                    return if cancelled {
                        Self::Cancelled
                    } else {
                        Self::Undelivered
                    };
                }
                _ => {}
            }
        }
        Self::Stopped
    }
}

struct ResultReceipt {
    carrier: Atom<Option<Arc<str>>>,
}

type Readers = Arc<Atom<Vec<Arc<ResultReceipt>>>>;

impl ResultReceipt {
    fn carries(&self, call_id: &str) -> bool {
        self.carrier
            .read(|carrier| carrier.as_deref() == Some(call_id))
    }
}

impl ToolReceipt for ResultReceipt {
    fn carried_by(&self, call_id: Option<&str>) {
        self.carrier.set(call_id.map(Arc::from));
    }
}

#[derive(serde::Deserialize)]
struct ToolResultCall {
    call_id: String,
}

#[derive(serde::Deserialize)]
struct ModelCall {
    call_index: u32,
}

#[derive(serde::Deserialize)]
struct RootRunEvent {
    #[serde(default)]
    turn_id: Option<String>,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    request_id: Option<String>,
    #[serde(default)]
    message_id: Option<String>,
    #[serde(default)]
    status: Option<RunStatus>,
}

impl RootRunEvent {
    fn starts_user_turn(&self) -> bool {
        self.kind.as_deref() == Some("prompt")
            && self
                .request_id
                .as_deref()
                .is_none_or(|id| !id.starts_with(crate::COMPLETION_REQUEST_PREFIX))
    }
}

#[derive(Default)]
pub(super) struct RegistryState {
    root_by_session: HashMap<String, String>,
    scopes: HashMap<String, AgentScope>,
    next_access: u64,
}

#[derive(Default)]
struct AgentScope {
    topology: TaskTree,
    sessions: Sessions,
    messages: MessageThreads,
    closing: bool,

    journal_frozen: bool,
}

struct CommittedCalls {
    revision: u64,
    call_ids: Vec<String>,
    omitted: u32,
}

enum OrderedJob {
    Update(AgentUpdate),

    Settle {
        status: AgentStatus,
        completion: Option<u64>,
        harness: Option<HarnessHandle>,
    },
}

const fn settled_update(id: AgentId, status: AgentStatus, completion: Option<u64>) -> AgentUpdate {
    match completion {
        Some(revision) => AgentUpdate::Completion {
            id,
            status,
            revision,
        },
        None => AgentUpdate::Status { id, status },
    }
}

struct JournalWrite {
    payload: String,
    records: Vec<(Arc<str>, Arc<str>)>,
}

enum JournalRequest {
    Persist {
        scope: bool,
        done: oneshot::Sender<std::io::Result<()>>,
    },
    Final {
        write: JournalWrite,
        done: oneshot::Sender<std::io::Result<()>>,
    },
}

#[derive(Default)]
pub(super) struct ScopeLocks {
    pub(super) residency: Sequencer,
    pub(super) message: Sequencer,
}

fn copy_error(error: &std::io::Error) -> std::io::Error {
    std::io::Error::new(error.kind(), error.to_string())
}

async fn journal_writer(
    registry: Weak<Registry>,
    root_session_id: String,
    mut requests: mpsc::UnboundedReceiver<JournalRequest>,
    mut revision: watch::Receiver<u64>,
) {
    let mut saved = None;
    let mut recorded = HashMap::new();
    let mut cache = durable::JournalCache::default();
    let mut pending = Vec::new();
    let mut failures = 0_u32;
    loop {
        for _ in 0..requests.len() {
            if let Ok(request) = requests.try_recv() {
                pending.push(request);
            }
        }
        let Some(live) = registry.upgrade() else {
            return;
        };
        let (finished, failed) = live
            .write_journal(
                &root_session_id,
                std::mem::take(&mut pending),
                &mut saved,
                &mut recorded,
                &mut cache,
            )
            .await;
        failures = if failed {
            failures.saturating_add(1)
        } else {
            0
        };
        if finished {
            while let Ok(request) = requests.try_recv() {
                pending.push(request);
            }
            if pending.is_empty() {
                requests.close();
                live.journal_writers.update(|writers| {
                    if writers
                        .get(&root_session_id)
                        .is_some_and(mpsc::UnboundedSender::is_closed)
                    {
                        writers.remove(&root_session_id);
                    }
                });
                while let Ok(request) = requests.try_recv() {
                    live.send_journal(&root_session_id, Some(request));
                }
                return;
            }
            continue;
        }
        drop(live);
        let retry = async {
            match failures {
                0 => std::future::pending().await,
                attempt => platform::sleep(completion_retry_delay(attempt)).await,
            }
        };
        tokio::select! {
            changed = revision.changed() => {
                if changed.is_err() {
                    return;
                }
            }
            request = requests.recv() => match request {
                Some(request) => pending.push(request),
                None => return,
            },
            () = retry => {}
        }
    }
}

impl AgentScope {
    fn journal_snapshot(
        &self,
        root_session_id: &str,
        checkpoints: &HashMap<(String, AgentId), durable::JournalCheckpoint>,
        cache: &durable::JournalCache,
    ) -> std::io::Result<durable::JournalSnapshot> {
        let mut ids = self.sessions.keys().copied().collect::<Vec<_>>();
        ids.sort_unstable();
        let mut records = Vec::new();
        let mut agents = Vec::with_capacity(ids.len());
        for id in ids {
            let Some(session) = self.sessions.get(&id) else {
                continue;
            };
            let checkpoint = checkpoints.get(&(root_session_id.to_owned(), id));
            let checkpoint_key = checkpoint.map(|checkpoint| &checkpoint.key);
            let entry = match cache.agent(id, session.journal_generation, checkpoint_key) {
                Some(key) => {
                    if let Some(checkpoint) = checkpoint
                        && let Some(json) = &checkpoint.pending
                    {
                        records.push((Arc::clone(&checkpoint.key), Arc::clone(json)));
                    }
                    durable::AgentEntry::Cached(Arc::clone(key))
                }
                None => durable::AgentEntry::Changed {
                    generation: session.journal_generation,
                    checkpoint: checkpoint_key.cloned(),
                    agent: Box::new(durable::persist_agent(session, checkpoint, &mut records)?),
                },
            };
            agents.push((id, entry));
        }
        let (next_message_id, messages, pending) = self.messages.snapshot(cache);
        Ok(durable::JournalSnapshot {
            agents,
            messages,
            next_message_id,
            pending,
            records,
        })
    }

    fn journal_payload(
        &self,
        root_session_id: &str,
        checkpoints: &HashMap<(String, AgentId), durable::JournalCheckpoint>,
    ) -> std::io::Result<JournalWrite> {
        let cache = durable::JournalCache::default();
        let encoded = self
            .journal_snapshot(root_session_id, checkpoints, &cache)?
            .encode(&cache)?;
        Ok(JournalWrite {
            payload: encoded.payload,
            records: encoded.records,
        })
    }
}

pub(super) struct AgentReservation {
    pub(super) root_session_id: String,
    pub(super) id: AgentId,
    pub(super) parent: Option<AgentId>,
}

pub(super) struct CloseRequest {
    pub(super) root_session_id: String,
    pub(super) ids: Vec<AgentId>,
    pub(super) harnesses: Vec<HarnessHandle>,
    pub(super) status_updates: Vec<(AgentId, AgentStatus)>,
}

pub(super) struct ClosedSessions {
    pub(super) summaries: Vec<AgentSummary>,
    pub(super) harness_tasks: Vec<Task<()>>,
    pub(super) event_tasks: Vec<Task<()>>,
}

pub(super) struct BatchStartup {
    registry: Arc<Registry>,
    cleanup: Option<(String, Vec<AgentId>)>,
}

impl BatchStartup {
    pub(super) fn track(&mut self, root_session_id: &str, id: AgentId) {
        let (_, ids) = self
            .cleanup
            .get_or_insert_with(|| (root_session_id.to_owned(), Vec::new()));
        ids.push(id);
    }

    pub(super) async fn rollback(mut self) -> std::io::Result<()> {
        let cleanup = self.cleanup.take();
        let registry = Arc::clone(&self.registry);
        drop(self);
        if let Some((root_session_id, ids)) = cleanup {
            registry.close_batch(&root_session_id, ids).await?;
        }
        Ok(())
    }

    pub(super) async fn fail(self, error: std::io::Error) -> std::io::Error {
        match self.rollback().await {
            Ok(()) => error,
            Err(cleanup) => std::io::Error::other(format!(
                "{error}; subagent startup cleanup failed: {cleanup}"
            )),
        }
    }

    pub(super) async fn commit(mut self) -> std::io::Result<()> {
        if let Some((root_session_id, _)) = &self.cleanup
            && self.registry.store_for(root_session_id).is_some()
            && let Err(error) = self.registry.persist_durably(root_session_id).await
        {
            return Err(self.fail(error).await);
        }
        self.cleanup = None;
        Ok(())
    }
}

impl Drop for BatchStartup {
    fn drop(&mut self) {
        let Some((root_session_id, ids)) = self.cleanup.take() else {
            return;
        };
        let registry = Arc::clone(&self.registry);
        drop(platform::spawn(async move {
            let _ = registry.close_batch(&root_session_id, ids).await;
        }));
    }
}

#[derive(Clone, Serialize)]
pub struct AgentSummary {
    pub agent_id: AgentId,
    pub role: String,
    pub task: String,
    pub parent_agent_id: Option<AgentId>,
    pub status: AgentStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelSelection>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<Thinking>,
    pub reasoning_mode: ReasoningMode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_output: Option<Value>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub completion_revision: Option<u64>,

    #[serde(skip_serializing_if = "AgentUsage::is_empty")]
    pub usage: AgentUsage,
}

#[derive(Serialize)]
pub struct AgentDirectoryEntry {
    pub agent_id: AgentId,
    pub role: String,
    pub task: String,
    pub parent_agent_id: Option<AgentId>,
    pub status: AgentStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_output: Option<Value>,
    pub can_message: bool,
    pub can_manage: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelSelection>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<Thinking>,
    pub reasoning_mode: ReasoningMode,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub completion_revision: Option<u64>,

    #[serde(skip_serializing_if = "AgentUsage::is_empty")]
    pub usage: AgentUsage,

    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub resuming: bool,
}

#[derive(Debug, Serialize)]
pub struct MessageReceipt {
    pub message_id: MessageId,
    pub thread_id: ThreadId,
    pub from: MessageSender,
    pub to_agent_id: AgentId,
    pub disposition: MessageDisposition,
}

struct PreparedMessage {
    root_session_id: String,
    message: AgentMessage,
    harness: HarnessHandle,
}

pub(super) struct DelegationChange {
    target: AgentId,
    previous_task: String,
}

pub(super) struct TurnSteer {
    id: AgentId,
    previous_revision: u64,
    revision: u64,
}

struct ResidentEviction {
    id: AgentId,
    harness: HarnessHandle,
}

impl TurnSteer {
    pub(super) const fn revision(&self) -> u64 {
        self.revision
    }
}

impl RegistryState {
    const fn next_access(&mut self) -> u64 {
        self.next_access = self.next_access.wrapping_add(1);
        self.next_access
    }

    fn insert_restored(
        &mut self,
        root_session_id: &str,
        session: ChildSession,
    ) -> std::io::Result<()> {
        let id = session.descriptor.id;
        let session_id = session.descriptor.session_id.clone();
        self.scope_mut(root_session_id).topology.insert(
            id,
            session_id.clone(),
            session.descriptor.parent,
        )?;
        self.root_by_session
            .insert(session_id, root_session_id.to_owned());
        let last_used = self.next_access();
        let mut session = session;
        session.last_used = last_used;
        self.scope_mut(root_session_id).sessions.insert(id, session);
        Ok(())
    }

    fn submit_result(
        &mut self,
        session_id: &str,
        instruction_revision: Option<u64>,
        output: Value,
    ) -> std::io::Result<SubmissionOutcome> {
        let root_session_id = self.root_session_id(session_id).to_owned();
        let scope = self.scopes.get_mut(&root_session_id).ok_or_else(|| {
            CompletionError::new(
                CompletionErrorCode::NotChild,
                false,
                "submit_result is only available to subagents",
                "Return root answers as assistant text.",
            )
        })?;
        let id = scope
            .topology
            .agent_for_session(session_id)
            .ok_or_else(|| {
                CompletionError::new(
                    CompletionErrorCode::NotChild,
                    false,
                    "submit_result is only available to subagents",
                    "Return root answers as assistant text.",
                )
            })?;
        let session = scope
            .sessions
            .get_mut(&id)
            .ok_or_else(|| std::io::Error::other("subagent session disappeared"))?;
        if !session.active {
            return Err(CompletionError::new(
                CompletionErrorCode::InactiveTurn,
                false,
                "submit_result is only available during an active subagent turn",
                "Start a new assigned turn before submitting.",
            )
            .into());
        }
        let Some(instruction_revision) = instruction_revision else {
            return Err(CompletionError::new(
                CompletionErrorCode::MissingInstructionRevision,
                false,
                "submit_result requires runtime-owned instruction context",
                "The execution host must preserve the originating model response context.",
            )
            .into());
        };

        if session.steering || session.active_instruction_revision != Some(instruction_revision) {
            return Ok(SubmissionOutcome::Superseded);
        }
        if session.submitted_output.is_some() {
            return Err(CompletionError::new(
                CompletionErrorCode::AlreadyAccepted,
                false,
                "submit_result already accepted one result for this turn",
                "Finish this turn; do not submit again.",
            )
            .into());
        }
        let (output, decoded_json_text) =
            validate_submitted_output(&session.output_validator, &session.output_schema, output)?;
        session.submitted_output = Some(output);
        Ok(SubmissionOutcome::Accepted { decoded_json_text })
    }

    fn begin_turn_steer(&mut self, root_session_id: &str, id: AgentId) -> Option<TurnSteer> {
        let session = self
            .scopes
            .get_mut(root_session_id)?
            .sessions
            .get_mut(&id)?;
        if !session.active || session.steering || session.submitted_output.is_some() {
            return None;
        }
        let previous_revision = session.active_instruction_revision?;
        let revision = session.next_instruction_revision.checked_add(1)?;
        session.next_instruction_revision = revision;
        session.active_instruction_revision = Some(revision);
        session.steering = true;
        Some(TurnSteer {
            id,
            previous_revision,
            revision,
        })
    }

    fn finish_turn_steer(&mut self, root_session_id: &str, steer: TurnSteer, committed: bool) {
        let Some(session) = self
            .scopes
            .get_mut(root_session_id)
            .and_then(|scope| scope.sessions.get_mut(&steer.id))
        else {
            return;
        };
        if session.active_instruction_revision != Some(steer.revision) {
            return;
        }
        if !committed {
            session.active_instruction_revision = Some(steer.previous_revision);
        }
        session.steering = false;
    }

    fn reserve_for(&mut self, session_id: &str) -> std::io::Result<AgentReservation> {
        let root_session_id = self.root_session_id(session_id).to_owned();
        if self
            .scopes
            .get(&root_session_id)
            .is_some_and(|scope| scope.closing)
        {
            return Err(std::io::Error::other(
                "subagent scope is closing and cannot spawn children",
            ));
        }
        let parent = self
            .scopes
            .get(&root_session_id)
            .and_then(|scope| scope.topology.agent_for_session(session_id));
        if let Some(parent) = parent {
            let parent_session = self
                .scopes
                .get(&root_session_id)
                .and_then(|scope| scope.sessions.get(&parent))
                .ok_or_else(|| std::io::Error::other("subagent parent disappeared"))?;
            if matches!(
                parent_session.status,
                AgentStatus::Closing | AgentStatus::Closed
            ) {
                return Err(std::io::Error::other(format!(
                    "agent {parent} is closing and cannot spawn children"
                )));
            }
        }
        self.reserve(&root_session_id, parent)
    }

    fn reserve(
        &mut self,
        session_id: &str,
        parent: Option<AgentId>,
    ) -> std::io::Result<AgentReservation> {
        let root_session_id = self.root_session_id(session_id).to_owned();
        if self
            .scopes
            .get(&root_session_id)
            .is_some_and(|scope| scope.closing)
        {
            return Err(std::io::Error::other(
                "subagent scope is closing and cannot reserve children",
            ));
        }
        let id = self.scope_mut(&root_session_id).topology.reserve(parent)?;
        Ok(AgentReservation {
            root_session_id,
            id,
            parent,
        })
    }

    fn insert(
        &mut self,
        root_session_id: String,
        id: AgentId,
        session_id: String,
        session: ChildSession,
    ) -> std::io::Result<()> {
        if let Some(parent) = session.descriptor.parent {
            let parent_session = self
                .scopes
                .get(&root_session_id)
                .and_then(|scope| scope.sessions.get(&parent))
                .ok_or_else(|| std::io::Error::other(format!("unknown parent agent {parent}")))?;
            if matches!(
                parent_session.status,
                AgentStatus::Closing | AgentStatus::Closed
            ) {
                return Err(std::io::Error::other(format!(
                    "agent {parent} stopped while spawning child {id}"
                )));
            }
        }
        self.scope_mut(&root_session_id).topology.insert(
            id,
            session_id.clone(),
            session.descriptor.parent,
        )?;
        self.root_by_session
            .insert(session_id, root_session_id.clone());
        let last_used = self.next_access();
        let mut session = session;
        session.last_used = last_used;
        self.scope_mut(&root_session_id)
            .sessions
            .insert(id, session);
        Ok(())
    }

    fn validate_insert(
        &self,
        root_session_id: &str,
        descriptor: &AgentDescriptor,
    ) -> std::io::Result<()> {
        if self
            .scopes
            .get(root_session_id)
            .is_some_and(|scope| scope.closing)
        {
            return Err(std::io::Error::other(
                "subagent scope stopped while spawning children",
            ));
        }
        if let Some(parent) = descriptor.parent {
            let parent_session = self
                .scopes
                .get(root_session_id)
                .and_then(|scope| scope.sessions.get(&parent))
                .ok_or_else(|| std::io::Error::other(format!("unknown parent agent {parent}")))?;
            if matches!(
                parent_session.status,
                AgentStatus::Closing | AgentStatus::Closed
            ) {
                return Err(std::io::Error::other(format!(
                    "agent {parent} stopped while spawning child {}",
                    descriptor.id
                )));
            }
        }
        Ok(())
    }

    fn harness_in_scope(
        &self,
        root_session_id: &str,
        id: AgentId,
    ) -> std::io::Result<HarnessHandle> {
        self.scopes
            .get(root_session_id)
            .and_then(|scope| scope.sessions.get(&id))
            .and_then(|session| session.harness.clone())
            .ok_or_else(|| std::io::Error::other(format!("agent {id} is closed")))
    }

    fn directory(
        &self,
        session_id: &str,
        include_completed: bool,
        include_self: bool,
    ) -> Vec<AgentDirectoryEntry> {
        let root_session_id = self.root_session_id(session_id);
        let Some(scope) = self.scopes.get(root_session_id) else {
            return Vec::new();
        };
        let caller = scope.topology.agent_for_session(session_id);
        let mut ids = scope.topology.ids();
        ids.sort_unstable();
        ids.into_iter()
            .filter(|id| include_self || caller != Some(*id))
            .filter_map(|id| {
                let session = scope.sessions.get(&id)?;

                let recoverable_interrupted = matches!(session.status, AgentStatus::Interrupted)
                    && (session.harness.is_some()
                        || session.stored_runtime.is_some()
                        || session.journaled_runtime);
                if !include_completed
                    && !matches!(session.status, AgentStatus::Pending | AgentStatus::Running)
                    && !recoverable_interrupted
                {
                    return None;
                }
                let can_message = caller != Some(id)
                    && (session.harness.is_some()
                        || session.stored_runtime.is_some()
                        || session.journaled_runtime)
                    && !matches!(
                        session.status,
                        AgentStatus::Pending | AgentStatus::Closing | AgentStatus::Closed
                    );
                let can_manage = caller != Some(id)
                    && !matches!(session.status, AgentStatus::Closing | AgentStatus::Closed)
                    && scope.topology.authorize(session_id, id).is_ok();
                Some(AgentDirectoryEntry {
                    agent_id: id,
                    role: bounded_summary(&session.descriptor.role),
                    task: bounded_summary(&session.descriptor.task),
                    parent_agent_id: session.descriptor.parent,
                    status: session.status.clone(),
                    last_output: if matches!(session.status, AgentStatus::Completed { .. }) {
                        None
                    } else {
                        session.last_output.clone()
                    },
                    can_message,
                    can_manage,
                    model: session.descriptor.model.clone(),
                    thinking: session.descriptor.thinking,
                    reasoning_mode: session.descriptor.reasoning_mode,
                    completion_revision: session.completion_revision(),
                    usage: session.usage.clone(),
                    resuming: false,
                })
            })
            .collect()
    }

    #[allow(clippy::too_many_arguments)]
    fn prepare_message(
        &mut self,
        session_id: &str,
        to: AgentId,
        priority: MessagePriority,
        purpose: MessagePurpose,
        in_reply_to: Option<MessageId>,
        body: String,
        user: bool,
    ) -> std::io::Result<PreparedMessage> {
        let root_session_id = self.root_session_id(session_id).to_owned();
        let scope = self
            .scopes
            .get_mut(&root_session_id)
            .ok_or_else(|| std::io::Error::other(format!("unknown agent_id {to}")))?;
        let from = if user {
            MessageSender::User
        } else {
            scope
                .topology
                .agent_for_session(session_id)
                .map_or(MessageSender::Root, |agent_id| MessageSender::Agent {
                    agent_id,
                })
        };
        if from.agent_id() == Some(to) {
            return Err(std::io::Error::other("agents cannot message themselves"));
        }
        if purpose == MessagePurpose::Delegate {
            scope.topology.authorize(session_id, to)?;
        }
        let target = scope
            .sessions
            .get_mut(&to)
            .ok_or_else(|| std::io::Error::other(format!("unknown agent_id {to}")))?;
        if matches!(target.status, AgentStatus::Pending) {
            return Err(std::io::Error::other(format!(
                "agent {to} has not started and cannot receive messages yet"
            )));
        }
        if matches!(target.status, AgentStatus::Closing | AgentStatus::Closed) {
            return Err(std::io::Error::other(format!(
                "agent {to} is {:?} and cannot receive messages",
                target.status
            )));
        }
        let harness = target.harness.clone().ok_or_else(|| {
            std::io::Error::other(format!(
                "agent {to} has no in-memory runtime history and cannot resume. Call list_agents with include_completed=true and select a recipient with can_message=true, or spawn a replacement agent. Do not retry this recipient while can_message=false."
            ))
        })?;
        self.next_access = self.next_access.wrapping_add(1);
        target.last_used = self.next_access;
        let message = scope
            .messages
            .prepare(from, to, priority, purpose, in_reply_to, body)?;
        Ok(PreparedMessage {
            root_session_id,
            message,
            harness,
        })
    }

    fn commit_message(
        &mut self,
        root_session_id: &str,
        message: AgentMessage,
    ) -> std::io::Result<AgentThread> {
        let scope = self
            .scopes
            .get_mut(root_session_id)
            .ok_or_else(|| std::io::Error::other("subagent scope disappeared"))?;
        Ok(scope.messages.commit(message))
    }

    fn rollback_message(&mut self, root_session_id: &str, id: MessageId) {
        if let Some(scope) = self.scopes.get_mut(root_session_id) {
            scope.messages.rollback(id);
        }
    }

    fn begin_delegation(
        &mut self,
        root_session_id: &str,
        id: MessageId,
    ) -> Option<(DelegationChange, AgentDescriptor)> {
        let scope = self.scopes.get_mut(root_session_id)?;
        let message = scope.messages.message(id)?;
        if message.purpose != MessagePurpose::Delegate {
            return None;
        }
        let target = scope.sessions.get_mut(&message.to)?;
        let previous_task = std::mem::replace(&mut target.descriptor.task, message.body);
        Some((
            DelegationChange {
                target: message.to,
                previous_task,
            },
            target.descriptor.clone(),
        ))
    }

    fn rollback_delegation(
        &mut self,
        root_session_id: &str,
        change: DelegationChange,
    ) -> Option<AgentDescriptor> {
        let target = self
            .scopes
            .get_mut(root_session_id)?
            .sessions
            .get_mut(&change.target)?;
        target.descriptor.task = change.previous_task;
        Some(target.descriptor.clone())
    }

    fn thread_for_message(&self, root_session_id: &str, id: MessageId) -> Option<AgentThread> {
        self.scopes
            .get(root_session_id)
            .and_then(|scope| scope.messages.thread_for_message(id))
    }

    fn mark_message_admitted(
        &mut self,
        root_session_id: &str,
        id: MessageId,
        disposition: MessageDisposition,
    ) {
        if let Some(scope) = self.scopes.get_mut(root_session_id) {
            scope.messages.mark_admitted(id, disposition);
        }
    }

    fn mark_message_terminal(&mut self, root_session_id: &str, id: MessageId) {
        if let Some(scope) = self.scopes.get_mut(root_session_id) {
            scope.messages.mark_terminal(id);
        }
    }

    fn summaries(&self, session_id: &str, ids: &[AgentId]) -> std::io::Result<Vec<AgentSummary>> {
        let root_session_id = self.root_session_id(session_id);
        for &id in ids {
            self.authorize(session_id, id)?;
        }
        self.summaries_in_scope(root_session_id, ids)
    }

    fn wait_snapshot(
        &self,
        session_id: &str,
        ids: &[AgentId],
    ) -> std::io::Result<Vec<(AgentSummary, u64)>> {
        let summaries = self.summaries(session_id, ids)?;
        let sessions = &self
            .scopes
            .get(self.root_session_id(session_id))
            .ok_or_else(|| std::io::Error::other("subagent scope disappeared"))?
            .sessions;
        Ok(summaries
            .into_iter()
            .map(|summary| {
                let revision = sessions.get(&summary.agent_id).map_or(0, |session| {
                    session
                        .settled_revision
                        .unwrap_or(session.next_instruction_revision)
                });
                (summary, revision)
            })
            .collect())
    }

    fn summaries_in_scope(
        &self,
        root_session_id: &str,
        ids: &[AgentId],
    ) -> std::io::Result<Vec<AgentSummary>> {
        let scope = self
            .scopes
            .get(root_session_id)
            .ok_or_else(|| std::io::Error::other("subagent scope disappeared"))?;
        ids.iter()
            .map(|id| {
                scope
                    .sessions
                    .get(id)
                    .map(ChildSession::summary)
                    .ok_or_else(|| std::io::Error::other(format!("unknown agent_id {id}")))
            })
            .collect()
    }

    fn request_interrupt(
        &mut self,
        session_id: &str,
        id: AgentId,
    ) -> std::io::Result<(String, Vec<AgentId>, Vec<HarnessHandle>)> {
        let root_session_id = self.authorize(session_id, id)?;
        let ids = self.subtree_shutdown_order(&root_session_id, id)?;
        let harnesses = self.harnesses(&root_session_id, &ids, false)?;
        Ok((root_session_id, ids, harnesses))
    }

    fn request_close(&mut self, session_id: &str, id: AgentId) -> std::io::Result<CloseRequest> {
        let root_session_id = self.authorize(session_id, id)?;
        let ids = self.subtree_shutdown_order(&root_session_id, id)?;
        let harnesses = self.harnesses(&root_session_id, &ids, true)?;
        let status_updates = self.closing_updates(&root_session_id, &ids);
        Ok(CloseRequest {
            root_session_id,
            ids,
            harnesses,
            status_updates,
        })
    }

    fn closing_updates(
        &self,
        root_session_id: &str,
        ids: &[AgentId],
    ) -> Vec<(AgentId, AgentStatus)> {
        let Some(scope) = self.scopes.get(root_session_id) else {
            return Vec::new();
        };
        ids.iter()
            .copied()
            .filter(|id| {
                scope
                    .sessions
                    .get(id)
                    .is_some_and(|session| matches!(session.status, AgentStatus::Closing))
            })
            .map(|id| (id, AgentStatus::Closing))
            .collect()
    }

    fn request_close_all(&mut self, session_id: &str) -> std::io::Result<CloseRequest> {
        let root_session_id = self.root_session_id(session_id).to_owned();
        let ids = {
            let scope = self.scope_mut(&root_session_id);
            scope.closing = true;
            scope.topology.all_postorder()
        };
        let harnesses = self.harnesses(&root_session_id, &ids, true)?;
        let status_updates = self.closing_updates(&root_session_id, &ids);
        Ok(CloseRequest {
            root_session_id,
            ids,
            harnesses,
            status_updates,
        })
    }

    fn request_interrupt_all(
        &mut self,
        session_id: &str,
    ) -> (String, Vec<AgentId>, Vec<HarnessHandle>) {
        let root_session_id = self.root_session_id(session_id).to_owned();
        let ids = self
            .scopes
            .get(&root_session_id)
            .map(|scope| scope.topology.ids())
            .unwrap_or_default();
        let harnesses = self
            .harnesses(&root_session_id, &ids, false)
            .unwrap_or_default();
        (root_session_id, ids, harnesses)
    }

    fn harnesses(
        &mut self,
        root_session_id: &str,
        ids: &[AgentId],
        closing: bool,
    ) -> std::io::Result<Vec<HarnessHandle>> {
        let scope = self
            .scopes
            .get_mut(root_session_id)
            .ok_or_else(|| std::io::Error::other("subagent scope disappeared"))?;
        let mut harnesses = Vec::new();
        for id in ids {
            let session = scope
                .sessions
                .get_mut(id)
                .ok_or_else(|| std::io::Error::other(format!("unknown agent_id {id}")))?;
            if closing && !matches!(session.status, AgentStatus::Closed) {
                session.status = AgentStatus::Closing;
            }
            harnesses.extend(session.harness.iter().cloned());
        }
        Ok(harnesses)
    }

    fn finish_close(
        &mut self,
        root_session_id: &str,
        ids: &[AgentId],
    ) -> std::io::Result<ClosedSessions> {
        let scope = self
            .scopes
            .get_mut(root_session_id)
            .ok_or_else(|| std::io::Error::other("subagent scope disappeared"))?;
        let mut harness_tasks = Vec::new();
        let mut event_tasks = Vec::new();
        for id in ids {
            let session = scope
                .sessions
                .get_mut(id)
                .ok_or_else(|| std::io::Error::other(format!("unknown agent_id {id}")))?;
            if session.active {
                return Err(std::io::Error::other(format!(
                    "agent {id} is still running"
                )));
            }
            session.harness = None;
            session.evicted = false;
            harness_tasks.extend(session.harness_task.take());
            event_tasks.extend(session.event_task.take());
            session.status = AgentStatus::Closed;
        }
        let summaries = ids
            .iter()
            .filter_map(|id| scope.sessions.get(id).map(ChildSession::summary))
            .collect();
        Ok(ClosedSessions {
            summaries,
            harness_tasks,
            event_tasks,
        })
    }

    fn all_inactive(&self, root_session_id: &str, ids: &[AgentId]) -> std::io::Result<bool> {
        let scope = self
            .scopes
            .get(root_session_id)
            .ok_or_else(|| std::io::Error::other("subagent scope disappeared"))?;
        Ok(ids.iter().all(|id| {
            scope
                .sessions
                .get(id)
                .is_some_and(|session| !session.active)
        }))
    }

    fn take_resident_eviction(
        &mut self,
        root_session_id: &str,
        limit: usize,
    ) -> Option<ResidentEviction> {
        let scope = self.scopes.get_mut(root_session_id)?;
        let resident = scope
            .sessions
            .values()
            .filter(|session| session.harness.is_some())
            .count();
        if resident <= limit {
            return None;
        }

        let candidate = scope
            .sessions
            .iter()
            .filter(|(_, session)| {
                !session.active
                    && session.status.can_start_turn()
                    && session
                        .harness
                        .as_ref()
                        .is_some_and(|harness| !harness.answering_side_questions())
            })
            .filter(|(id, _)| !scope.messages.has_pending_for(**id))
            .filter(|(id, _)| {
                !scope.sessions.iter().any(|(other_id, other)| {
                    other.harness.is_some() && scope.topology.is_descendant(*other_id, **id)
                })
            })
            .min_by_key(|(id, session)| (session.last_used, **id))
            .map(|(id, _)| *id)?;
        let session = scope.sessions.get_mut(&candidate)?;
        let harness = session.harness.take()?;
        session.evicted = true;
        Some(ResidentEviction {
            id: candidate,
            harness,
        })
    }

    fn subtree_shutdown_order(
        &self,
        root_session_id: &str,
        id: AgentId,
    ) -> std::io::Result<Vec<AgentId>> {
        self.scopes
            .get(root_session_id)
            .ok_or_else(|| std::io::Error::other("subagent scope disappeared"))?
            .topology
            .subtree_postorder(id)
    }

    fn authorize(&self, session_id: &str, id: AgentId) -> std::io::Result<String> {
        let root_session_id = self.root_session_id(session_id);
        self.scopes
            .get(root_session_id)
            .ok_or_else(|| std::io::Error::other(format!("unknown agent_id {id}")))?
            .topology
            .authorize(session_id, id)?;
        Ok(root_session_id.to_owned())
    }

    fn root_session_id<'a>(&'a self, session_id: &'a str) -> &'a str {
        self.root_by_session
            .get(session_id)
            .map_or(session_id, String::as_str)
    }

    fn scope_mut(&mut self, root_session_id: &str) -> &mut AgentScope {
        self.scopes.entry(root_session_id.to_owned()).or_default()
    }
}

pub type AgentWait = (
    Vec<AgentSummary>,
    HashSet<AgentId>,
    bool,
    Option<Arc<dyn ToolReceipt>>,
);

const AGENT_STOP_TIMEOUT: Duration = Duration::from_secs(30);

const IDLE_WAIT_LIMIT: u32 = 2;

fn wait_mark(status: &AgentStatus, revision: u64) -> u64 {
    use std::hash::{Hash as _, Hasher as _};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    revision.hash(&mut hasher);
    serde_json::to_string(status)
        .unwrap_or_default()
        .hash(&mut hasher);
    hasher.finish()
}

impl Registry {
    pub(super) fn new(
        updates: mpsc::UnboundedSender<ScopedAgentUpdate>,
        max_concurrency: usize,
    ) -> Self {
        let (revision, _) = watch::channel(0);
        Self {
            id: SubagentRuntimeId::next(),
            spawn_router: Atom::default(),
            state: Owned::default(),
            updates,
            revision,
            capacity: Capacity::new(max_concurrency),
            session_handles: Atom::default(),
            max_resident: AtomicUsize::new(crate::DEFAULT_MAX_RESIDENT_SUBAGENTS),
            max_resident_explicit: std::sync::atomic::AtomicBool::new(false),
            scope_locks: Atom::default(),
            store: Atom::default(),
            journals: Atom::default(),
            claims: Atom::default(),
            spawn_admission: Atom::default(),
            journal_writer: std::sync::atomic::AtomicBool::new(false),
            journal_writers: Atom::default(),
            checkpoints: Atom::default(),
            pending_checkpoints: Atom::default(),
            progress_captures: Atom::default(),
            completed_tools: Atom::default(),
            pending_resume: Atom::default(),
            wait_reported: Atom::default(),
            idle_waits: Atom::default(),
            this: std::sync::OnceLock::new(),
            status_queues: Atom::default(),
            completion_retries: Atom::default(),
            completion_dispatch: Atom::default(),
            completion_routes: Atom::default(),
            result_readers: Atom::default(),
            root_watches: Atom::default(),
            running_roots: Atom::default(),
            parent_resolver: Atom::default(),
            spawn_policy: Atom::default(),
        }
    }

    pub fn set_parent_resolver(&self, resolver: ParentResolver) {
        self.parent_resolver.set(Some(resolver));
    }

    pub(crate) fn continues_root(&self) -> bool {
        self.parent_resolver.read(Option::is_some)
    }

    fn store_for(&self, root_session_id: &str) -> Option<Arc<dyn SubagentStore>> {
        self.journals
            .read(|journals| journals.get(root_session_id).cloned())
            .or_else(|| Option::clone(&self.store.get()))
    }

    pub fn reserve_journal(
        &self,
        root_session_id: &str,
        journal: nanocode_agent_core::backend::ChildJournal,
    ) -> bool {
        self.claims
            .try_update(|claims| {
                (!claims.restored.contains_key(root_session_id)
                    && !claims.reserved.contains_key(root_session_id))
                .then(|| {
                    claims
                        .reserved
                        .insert(root_session_id.to_owned(), journal.clone());
                })
            })
            .is_some()
    }

    pub fn release_journal(
        &self,
        root_session_id: &str,
        journal: &nanocode_agent_core::backend::ChildJournal,
    ) {
        self.claims.update(|claims| {
            if claims
                .reserved
                .get(root_session_id)
                .is_some_and(|reserved| {
                    std::ptr::addr_eq(
                        Arc::as_ptr(&reserved.store()),
                        Arc::as_ptr(&journal.store()),
                    )
                })
            {
                claims.reserved.remove(root_session_id);
            }
        });
    }

    fn adopt_journal(self: &Arc<Self>, handle: &AgentHandle) {
        let Some(journal) = handle.child_journal() else {
            return;
        };
        let root = handle.session_id().to_owned();
        let (ready, gate) = tokio::sync::watch::channel(RestorationState::Loading);
        let Some(journal) = self.claims.try_update(|claims| {
            if claims.restored.contains_key(&root) {
                return None;
            }
            claims.restored.insert(root.clone(), gate.clone());
            Some(
                claims
                    .reserved
                    .remove(&root)
                    .unwrap_or_else(|| journal.clone()),
            )
        }) else {
            return;
        };
        self.spawn_admission.update(|admission| {
            admission.insert(root.clone(), Sequencer::new());
        });
        let store: Arc<dyn SubagentStore> = Arc::new(durable::JournalStore(journal.store()));
        self.journals.update(|journals| {
            journals.insert(root.clone(), Arc::clone(&store));
        });
        self.ensure_journal_writer();
        let registry = Arc::clone(self);
        drop(platform::spawn(async move {
            let mut completed = Vec::new();
            match registry.restore(&root).await {
                Ok(report) if report.restored > 0 => {
                    tracing::info!(
                        restored = report.restored,
                        interrupted = report.interrupted.len(),
                        completed = report.completed.len(),
                        unrecoverable = report.unrecoverable.len(),
                        %root,
                        "restored durable subagent task tree"
                    );
                    completed = report.completed;
                }
                Ok(_) => {}
                Err(error) => {
                    tracing::warn!(%error, %root, "could not restore durable subagent task tree");
                    let _ = ready.send(RestorationState::Failed(format!(
                        "could not restore subagent journal for {root}: {error}"
                    )));
                    return;
                }
            }

            let _ = ready.send(RestorationState::Resuming);
            registry
                .announce_restored_completions(&root, &completed)
                .await;
            registry.changed();
            let pending = registry.pending_messages(&root).await;
            for (id, result) in registry.resume_interrupted(&root).await {
                if let Err(error) = result {
                    tracing::warn!(%id, %error, "could not resume restored subagent");
                    registry
                        .fail_unresumable(
                            &root,
                            id,
                            format!(
                                "subagent {id} could not resume after a runtime restart: {error}"
                            ),
                        )
                        .await;
                }
            }
            registry.redeliver_messages(&root, pending).await;
            let resolver = Option::clone(&registry.parent_resolver.get());
            if registry.needs_root_watch(&root).await
                && let Some(parent) = resolver.and_then(|resolver| resolver(&root))
            {
                registry.watch_root(&root, &parent);
            }
            let _ = ready.send(RestorationState::Ready);
        }));
    }

    pub(crate) async fn admit_spawn(&self, session_id: &str) -> std::io::Result<Option<Turn>> {
        let root = self
            .state
            .acquire()
            .await
            .root_session_id(session_id)
            .to_owned();
        let admission = self
            .spawn_admission
            .read(|admission| admission.get(&root).cloned());
        let guard = match admission {
            Some(admission) => Some(admission.enter().await),
            None => None,
        };
        self.await_restored(session_id).await?;
        Ok(guard)
    }

    pub(crate) async fn await_restored(&self, session_id: &str) -> std::io::Result<()> {
        let root = self
            .state
            .acquire()
            .await
            .root_session_id(session_id)
            .to_owned();
        let gate = self
            .claims
            .read(|claims| claims.restored.get(&root).cloned());
        if let Some(mut gate) = gate {
            let outcome = gate
                .wait_for(|outcome| {
                    matches!(
                        outcome,
                        RestorationState::Ready | RestorationState::Failed(_)
                    )
                })
                .await
                .map_err(|_| {
                    std::io::Error::other("subagent journal restoration ended without an outcome")
                })?
                .clone();
            if let RestorationState::Failed(error) = outcome {
                return Err(std::io::Error::other(error));
            }
        }
        Ok(())
    }

    pub fn set_store(self: &Arc<Self>, store: Arc<dyn SubagentStore>) {
        if !self.max_resident_explicit.load(Ordering::Relaxed) {
            self.max_resident
                .store(crate::DURABLE_MAX_RESIDENT_SUBAGENTS, Ordering::Relaxed);
        }
        self.store.set(Some(store));
        self.ensure_journal_writer();
    }

    fn ensure_journal_writer(self: &Arc<Self>) {
        if self
            .journal_writer
            .swap(true, std::sync::atomic::Ordering::AcqRel)
        {
            self.changed();
            return;
        }
        let registry = Arc::downgrade(self);
        let mut revision = self.revision.subscribe();
        drop(platform::spawn(async move {
            loop {
                let Some(live) = registry.upgrade() else {
                    return;
                };
                let roots = live
                    .state
                    .acquire()
                    .await
                    .scopes
                    .iter()
                    .filter(|(_, scope)| !scope.journal_frozen)
                    .map(|(root, _)| root.clone())
                    .collect::<Vec<_>>();
                for root in roots {
                    if live.store_for(&root).is_some() {
                        live.send_journal(&root, None);
                    }
                }
                drop(live);
                if revision.changed().await.is_err() {
                    return;
                }
            }
        }));
    }

    fn send_journal(&self, root_session_id: &str, mut request: Option<JournalRequest>) {
        let Some(registry) = self.this.get().cloned() else {
            return;
        };
        let (sender, requests) = loop {
            let current = self
                .journal_writers
                .read(|writers| writers.get(root_session_id).cloned())
                .filter(|sender| !sender.is_closed());
            if let Some(sender) = current {
                match request.map(|request| sender.send(request)) {
                    None | Some(Ok(())) => return,
                    Some(Err(mpsc::error::SendError(returned))) => request = Some(returned),
                }
            }
            let (sender, requests) = mpsc::unbounded_channel();
            let installed =
                self.journal_writers
                    .try_update(|writers| match writers.get(root_session_id) {
                        Some(existing) if !existing.is_closed() => None,
                        _ => {
                            writers.insert(root_session_id.to_owned(), sender.clone());
                            Some(())
                        }
                    });
            if installed.is_some() {
                break (sender, requests);
            }
        };
        if let Some(request) = request {
            let _ = sender.send(request);
        }
        let revision = self.revision.subscribe();
        drop(platform::spawn(journal_writer(
            registry,
            root_session_id.to_owned(),
            requests,
            revision,
        )));
    }

    async fn request_journal(&self, root_session_id: &str, scope: bool) -> std::io::Result<()> {
        if self.store_for(root_session_id).is_none() {
            return Err(std::io::Error::other("no subagent store is installed"));
        }
        let (done, saved) = oneshot::channel();
        self.send_journal(
            root_session_id,
            Some(JournalRequest::Persist { scope, done }),
        );
        saved
            .await
            .map_err(|_| std::io::Error::other("subagent journal write was interrupted"))?
    }

    async fn final_journal(
        &self,
        root_session_id: &str,
        write: JournalWrite,
    ) -> std::io::Result<()> {
        let (done, saved) = oneshot::channel();
        self.send_journal(root_session_id, Some(JournalRequest::Final { write, done }));
        saved
            .await
            .map_err(|_| std::io::Error::other("subagent journal write was interrupted"))?
    }

    pub(super) fn scope_locks(&self, root_session_id: &str) -> Arc<ScopeLocks> {
        if let Some(locks) = self
            .scope_locks
            .read(|locks| locks.get(root_session_id).cloned())
        {
            return locks;
        }
        let created = Arc::new(ScopeLocks::default());
        self.scope_locks.update(|locks| {
            Arc::clone(
                locks
                    .entry(root_session_id.to_owned())
                    .or_insert_with(|| Arc::clone(&created)),
            )
        })
    }

    async fn session_locks(&self, session_id: &str) -> Arc<ScopeLocks> {
        let root = self
            .state
            .acquire()
            .await
            .root_session_id(session_id)
            .to_owned();
        self.scope_locks(&root)
    }

    async fn write_journal(
        &self,
        root_session_id: &str,
        requests: Vec<JournalRequest>,
        saved: &mut Option<String>,
        recorded: &mut HashMap<String, Arc<str>>,
        cache: &mut durable::JournalCache,
    ) -> (bool, bool) {
        let (restoring, background) = {
            let gate = self.claims.read(|claims| {
                claims
                    .restored
                    .get(root_session_id)
                    .map(|gate| gate.borrow().clone())
            });
            (
                matches!(
                    gate,
                    Some(RestorationState::Loading | RestorationState::Failed(_))
                ),
                matches!(gate, None | Some(RestorationState::Ready)),
            )
        };
        let mut waiters = Vec::new();
        let mut finals = Vec::new();
        for request in requests {
            match request {
                JournalRequest::Persist { scope: false, done } if restoring => {
                    let _ = done.send(Err(std::io::Error::other(
                        "subagent task tree is still restoring",
                    )));
                }
                JournalRequest::Persist { scope, done } => waiters.push((scope, done)),
                JournalRequest::Final { write, done } => finals.push((write, done)),
            }
        }
        let Some(store) = self.store_for(root_session_id) else {
            for done in waiters
                .into_iter()
                .map(|(_, done)| done)
                .chain(finals.into_iter().map(|(_, done)| done))
            {
                let _ = done.send(Err(std::io::Error::other("no subagent store is installed")));
            }
            return (true, false);
        };
        let frozen = || {
            Err(std::io::Error::other(
                "subagent journal is frozen for shutdown",
            ))
        };
        let (snapshot, finished) = {
            let state = self.state.acquire().await;
            let checkpoints = self.checkpoints.get();
            let _collect = tracing::info_span!(
                "subagent.journal.collect",
                session.root_id = root_session_id
            )
            .entered();
            match state.scopes.get(root_session_id) {
                None => {
                    for (scope, done) in waiters.drain(..) {
                        let _ = done.send(if scope { Ok(()) } else { frozen() });
                    }
                    (None, true)
                }
                Some(scope) => {
                    if scope.journal_frozen {
                        let (scoped, unscoped): (Vec<_>, Vec<_>) =
                            waiters.drain(..).partition(|(scope, _)| *scope);
                        waiters = scoped;
                        for (_, done) in unscoped {
                            let _ = done.send(frozen());
                        }
                    }
                    let write = !waiters.is_empty() || (background && !scope.journal_frozen);
                    (
                        write.then(|| scope.journal_snapshot(root_session_id, &checkpoints, cache)),
                        scope.journal_frozen,
                    )
                }
            }
        };
        let encoded = snapshot.map(|snapshot| {
            snapshot.and_then(|snapshot| {
                let _encode = tracing::info_span!(
                    "subagent.journal.encode",
                    session.root_id = root_session_id
                )
                .entered();
                snapshot.encode(cache)
            })
        });
        let result = match encoded {
            None => Ok(()),
            Some(Err(error)) => Err(error),
            Some(Ok(encoded)) => {
                let records = &encoded.records;
                if waiters.is_empty() && saved.as_deref() == Some(encoded.payload.as_str()) {
                    self.acknowledge(root_session_id, records);
                    cache.committed(encoded);
                    Ok(())
                } else {
                    let payload = encoded.payload.clone();
                    let contents = records.iter().map(|(_, json)| Arc::clone(json)).collect();
                    let result = store
                        .save(root_session_id, payload.clone(), contents)
                        .instrument(tracing::info_span!(
                            "subagent.journal.save",
                            session.root_id = root_session_id,
                            payload.bytes = payload.len(),
                            record.count = records.len(),
                            record.bytes =
                                records.iter().map(|(_, json)| json.len()).sum::<usize>(),
                            request.count = waiters.len()
                        ))
                        .await;
                    match result {
                        Ok(()) => {
                            self.acknowledge(root_session_id, &encoded.records);
                            cache.committed(encoded);
                            if store.records_sessions() {
                                self.record_children(root_session_id, store.as_ref(), recorded)
                                    .await;
                            }
                            *saved = Some(payload);
                            Ok(())
                        }
                        Err(error) => {
                            *saved = None;
                            Err(error)
                        }
                    }
                }
            }
        };
        if waiters.is_empty()
            && let Err(error) = &result
        {
            tracing::warn!(%error, %root_session_id, "could not save subagent journal");
        }
        for (_, done) in waiters {
            let _ = done.send(result.as_ref().copied().map_err(copy_error));
        }
        let failed = result.is_err();
        let closed = !finals.is_empty();
        for (write, done) in finals {
            *saved = None;
            let _ = done.send(
                self.save_write(root_session_id, store.as_ref(), write)
                    .await,
            );
        }
        (finished || closed, failed)
    }

    async fn record_children(
        &self,
        root_session_id: &str,
        store: &dyn SubagentStore,
        recorded: &mut HashMap<String, Arc<str>>,
    ) {
        for (key, checkpoint) in self.child_checkpoints(root_session_id).await {
            let session_id = checkpoint.session_id().to_owned();
            if recorded.get(&session_id) == Some(&key) {
                continue;
            }
            match store.record_session(root_session_id, checkpoint).await {
                Ok(()) => {
                    self.release_recorded(root_session_id, &key);
                    recorded.insert(session_id, key);
                }
                Err(error) => tracing::warn!(
                    %error, %root_session_id, %session_id,
                    "could not record subagent session"
                ),
            }
        }
    }

    async fn child_checkpoints(&self, root_session_id: &str) -> Vec<(Arc<str>, SessionCheckpoint)> {
        let state = self.state.acquire().await;
        let checkpoints = self.checkpoints.get();
        let Some(scope) = state.scopes.get(root_session_id) else {
            return Vec::new();
        };
        let mut ids = scope.sessions.keys().copied().collect::<Vec<_>>();
        ids.sort_unstable();
        ids.into_iter()
            .filter_map(|id| {
                let checkpoint = match checkpoints.get(&(root_session_id.to_owned(), id)) {
                    Some(checkpoint) => checkpoint.clone(),
                    None => durable::JournalCheckpoint::encode(
                        scope.sessions.get(&id)?.stored_runtime.clone()?,
                        true,
                    )
                    .ok()?,
                };
                Some((checkpoint.key, Arc::unwrap_or_clone(checkpoint.checkpoint?)))
            })
            .collect()
    }

    fn records_sessions(&self, root_session_id: &str) -> bool {
        self.store_for(root_session_id)
            .is_some_and(|store| store.records_sessions())
    }

    fn record_checkpoint(
        &self,
        root_session_id: &str,
        id: AgentId,
        snapshot: SessionCheckpoint,
    ) -> bool {
        let encoded =
            durable::JournalCheckpoint::encode(snapshot, self.records_sessions(root_session_id));
        self.insert_encoded(root_session_id, id, encoded)
    }

    async fn record_checkpoint_off_thread(
        &self,
        root_session_id: &str,
        id: AgentId,
        snapshot: SessionCheckpoint,
    ) -> bool {
        let retain = self.records_sessions(root_session_id);
        let encoded =
            platform::blocking(move || durable::JournalCheckpoint::encode(snapshot, retain)).await;
        self.insert_encoded(root_session_id, id, encoded)
    }

    fn insert_encoded(
        &self,
        root_session_id: &str,
        id: AgentId,
        encoded: std::io::Result<durable::JournalCheckpoint>,
    ) -> bool {
        match encoded {
            Ok(checkpoint) => {
                self.insert_checkpoint(root_session_id, id, checkpoint);
                true
            }
            Err(error) => {
                tracing::warn!(%error, "could not encode subagent checkpoint");
                false
            }
        }
    }

    async fn journal_snapshot(
        &self,
        root_session_id: &str,
        id: AgentId,
    ) -> std::io::Result<SessionCheckpoint> {
        let (key, pending) = self
            .checkpoints
            .read(|checkpoints| {
                checkpoints
                    .get(&(root_session_id.to_owned(), id))
                    .map(|checkpoint| (Arc::clone(&checkpoint.key), checkpoint.pending.clone()))
            })
            .ok_or_else(|| {
                std::io::Error::other(format!("subagent {id} has no journaled checkpoint"))
            })?;
        match pending {
            Some(json) => durable::decode_checkpoint(&json, &key),
            None => {
                let store = self
                    .store_for(root_session_id)
                    .ok_or_else(|| std::io::Error::other("no subagent store is installed"))?;
                store
                    .load_record(root_session_id, &key)
                    .await
                    .and_then(|json| durable::decode_checkpoint(&json, &key))
                    .map_err(|error| {
                        std::io::Error::new(
                            error.kind(),
                            format!("subagent {id}'s checkpoint {key} is unreadable: {error}"),
                        )
                    })
            }
        }
    }

    fn insert_checkpoint(
        &self,
        root_session_id: &str,
        id: AgentId,
        checkpoint: durable::JournalCheckpoint,
    ) {
        self.checkpoints.update(|checkpoints| {
            checkpoints.insert((root_session_id.to_owned(), id), checkpoint.clone());
        });
    }

    fn release_recorded(&self, root_session_id: &str, key: &Arc<str>) {
        let retained = |root: &String, checkpoint: &durable::JournalCheckpoint| {
            root == root_session_id
                && checkpoint.key == *key
                && checkpoint.pending.is_none()
                && checkpoint.checkpoint.is_some()
        };
        if !self.checkpoints.read(|checkpoints| {
            checkpoints
                .iter()
                .any(|((root, _), checkpoint)| retained(root, checkpoint))
        }) {
            return;
        }
        self.checkpoints.update(|checkpoints| {
            for ((root, _), checkpoint) in checkpoints.iter_mut() {
                if retained(root, checkpoint) {
                    checkpoint.checkpoint = None;
                }
            }
        });
    }

    fn acknowledge(&self, root_session_id: &str, records: &[(Arc<str>, Arc<str>)]) {
        if records.is_empty() {
            return;
        }
        let acknowledged = |root: &String, checkpoint: &durable::JournalCheckpoint| {
            root == root_session_id
                && checkpoint.pending.is_some()
                && records.iter().any(|(key, _)| *key == checkpoint.key)
        };
        if !self.checkpoints.read(|checkpoints| {
            checkpoints
                .iter()
                .any(|((root, _), checkpoint)| acknowledged(root, checkpoint))
        }) {
            return;
        }
        self.checkpoints.update(|checkpoints| {
            for ((root, _), checkpoint) in checkpoints.iter_mut() {
                if acknowledged(root, checkpoint) {
                    checkpoint.pending = None;
                }
            }
        });
    }

    fn begin_pending_checkpoint(&self, root_session_id: &str) -> bool {
        if self.store_for(root_session_id).is_none() {
            return false;
        }
        self.pending_checkpoints.update(|pending| {
            *pending.entry(root_session_id.to_owned()).or_default() += 1;
        });
        true
    }

    fn finish_pending_checkpoint(&self, root_session_id: &str) {
        self.pending_checkpoints.update(|pending| {
            if let Some(count) = pending.get_mut(root_session_id) {
                *count = count.saturating_sub(1);
                if *count == 0 {
                    pending.remove(root_session_id);
                }
            }
        });
        self.changed();
    }

    pub(super) fn capture_progress(self: &Arc<Self>, root_session_id: &str, id: AgentId) {
        if self.store_for(root_session_id).is_none() {
            return;
        }
        let key = (root_session_id.to_owned(), id);
        let started = self.progress_captures.update(|captures| {
            if let Some(again) = captures.get_mut(&key) {
                *again = true;
                return false;
            }
            captures.insert(key.clone(), false);
            true
        });
        if !started {
            return;
        }
        let registry = Arc::clone(self);
        drop(platform::spawn(async move {
            loop {
                let progressed = registry
                    .completed_tools
                    .read(|completed| completed.contains(&key))
                    && registry
                        .completed_tools
                        .update(|completed| completed.remove(&key));
                let harness = registry.running_harness(&key.0, key.1).await;

                let commits = registry.committable_calls(&key.0, key.1).await;
                if let Some(harness) = harness
                    && let Ok(snapshot) = harness.snapshot().await
                    && registry.running_harness(&key.0, key.1).await.is_some()
                {
                    if registry
                        .record_checkpoint_off_thread(&key.0, key.1, snapshot)
                        .await
                        && let Some(commits) = commits
                    {
                        registry.retire_committed(&key.0, key.1, commits).await;
                    }
                    if progressed {
                        registry.reset_resume_attempts(&key.0, key.1).await;
                    }
                    registry.changed();
                } else if progressed {
                    registry.completed_tools.update(|completed| {
                        completed.insert(key.clone());
                    });
                }
                let finished =
                    registry
                        .progress_captures
                        .update(|captures| match captures.get_mut(&key) {
                            Some(again) if *again => {
                                *again = false;
                                false
                            }
                            _ => {
                                captures.remove(&key);
                                true
                            }
                        });
                if finished {
                    break;
                }
            }
        }));
    }

    async fn commit_boundary(&self, root_session_id: &str, id: AgentId) {
        let mut state = self.state.acquire().await;
        let Some(session) = state
            .scopes
            .get_mut(root_session_id)
            .and_then(|scope| scope.sessions.get_mut(&id))
        else {
            return;
        };
        for call in &mut session.in_flight_calls {
            if call.live && call.settled {
                call.committable = true;
            }
        }
        let progress = &mut session.in_flight_progress;
        progress.committable = progress.committable.saturating_add(progress.settled);
        progress.settled = 0;
    }

    async fn committable_calls(
        &self,
        root_session_id: &str,
        id: AgentId,
    ) -> Option<CommittedCalls> {
        let state = self.state.acquire().await;
        let session = state
            .scopes
            .get(root_session_id)
            .and_then(|scope| scope.sessions.get(&id))?;
        let revision = session.active_instruction_revision?;
        let call_ids = session
            .in_flight_calls
            .iter()
            .filter(|call| call.live && call.committable)
            .map(|call| call.call_id.clone())
            .collect::<Vec<_>>();
        let omitted = session.in_flight_progress.committable;
        (!call_ids.is_empty() || omitted > 0).then_some(CommittedCalls {
            revision,
            call_ids,
            omitted,
        })
    }

    async fn retire_committed(&self, root_session_id: &str, id: AgentId, commits: CommittedCalls) {
        {
            let mut state = self.state.acquire().await;
            let Some(session) = state
                .scopes
                .get_mut(root_session_id)
                .and_then(|scope| scope.sessions.get_mut(&id))
            else {
                return;
            };

            if session.active_instruction_revision != Some(commits.revision) {
                return;
            }
            session
                .in_flight_calls
                .retain(|call| !(call.live && commits.call_ids.contains(&call.call_id)));
            let progress = &mut session.in_flight_progress;
            let omitted = commits.omitted.min(progress.committable);
            progress.committable -= omitted;
            session.in_flight_omitted = session.in_flight_omitted.saturating_sub(omitted);
        }
        self.changed();
    }

    async fn reset_resume_attempts(&self, root_session_id: &str, id: AgentId) {
        let mut state = self.state.acquire().await;
        if let Some(session) = state
            .scopes
            .get_mut(root_session_id)
            .and_then(|scope| scope.sessions.get_mut(&id))
            && session.active
        {
            session.resume_attempts = 0;
        }
    }

    async fn track_in_flight(
        &self,
        root_session_id: &str,
        id: AgentId,
        kind: &AgentEventKind,
        payload: &Option<Arc<serde_json::value::RawValue>>,
    ) {
        #[derive(serde::Deserialize)]
        struct Call {
            call_id: String,
            #[serde(default)]
            tool: String,
            #[serde(default)]
            arguments: Option<Box<serde_json::value::RawValue>>,
            #[serde(default)]
            parent_call_id: Option<String>,

            #[serde(default)]
            cell: Option<Cell>,

            #[serde(default)]
            metadata: Option<Metadata>,
        }
        #[derive(serde::Deserialize)]
        struct Cell {
            origin_call_id: String,
            running: bool,
        }
        #[derive(serde::Deserialize)]
        struct Metadata {
            #[serde(default, rename = "_nanocode_code")]
            code: Option<Code>,
        }
        #[derive(serde::Deserialize)]
        struct Code {
            #[serde(default)]
            origin_call_id: Option<Value>,
            #[serde(default)]
            running: Option<Value>,
        }
        impl Call {
            fn cell(&self) -> Option<(&str, bool)> {
                if let Some(cell) = &self.cell {
                    return Some((cell.origin_call_id.as_str(), cell.running));
                }
                let code = self.metadata.as_ref()?.code.as_ref()?;
                let origin = code.origin_call_id.as_ref()?.as_str()?;
                Some((
                    origin,
                    code.running.as_ref().and_then(Value::as_bool) != Some(false),
                ))
            }

            fn arguments(&self) -> Option<Value> {
                let raw = self.arguments.as_ref()?.get();
                match serde_json::from_str::<String>(raw) {
                    Ok(text) => Some(Value::String(text)),
                    Err(_) => serde_json::from_str(raw).ok(),
                }
            }
        }
        let Some(call) = payload
            .as_ref()
            .and_then(|payload| serde_json::from_str::<Call>(payload.get()).ok())
        else {
            return;
        };
        {
            let mut state = self.state.acquire().await;
            let Some(session) = state
                .scopes
                .get_mut(root_session_id)
                .and_then(|scope| scope.sessions.get_mut(&id))
            else {
                return;
            };
            match kind {
                AgentEventKind::ToolCall => {
                    durable::retain_call(
                        &mut session.in_flight_calls,
                        durable::InFlightCall::new(
                            &call.call_id,
                            &call.tool,
                            call.arguments().as_ref(),
                            parent_call(&call.call_id, call.parent_call_id.as_deref()),
                        ),
                    );
                }

                AgentEventKind::ToolResult => {
                    let cell = call.cell();
                    let top_level =
                        parent_call(&call.call_id, call.parent_call_id.as_deref()).is_none();
                    if let Some(existing) = session
                        .in_flight_calls
                        .iter_mut()
                        .find(|existing| existing.call_id == call.call_id)
                    {
                        existing.result_recorded = true;
                        if existing.live && existing.parent_call_id.is_none() {
                            if matches!(cell, Some((origin, true)) if origin == call.call_id) {
                                existing.yielded = true;
                            } else {
                                existing.settled = true;
                            }
                        }
                    }

                    if top_level && let Some((origin, false)) = cell {
                        for listed in &mut session.in_flight_calls {
                            if !listed.live {
                                continue;
                            }
                            if listed.parent_call_id.as_deref() == Some(origin)
                                && listed.result_recorded
                            {
                                listed.settled = true;
                            } else if listed.call_id == origin && listed.yielded {
                                listed.yielded = false;
                                listed.settled = true;
                            }
                        }
                        if let Some(count) = session.in_flight_progress.nested.remove(origin) {
                            session.in_flight_progress.settled =
                                session.in_flight_progress.settled.saturating_add(count);
                        }
                    }
                }
                _ => return,
            }
        }
        self.changed();
    }

    async fn running_harness(&self, root_session_id: &str, id: AgentId) -> Option<HarnessHandle> {
        self.state
            .acquire()
            .await
            .scopes
            .get(root_session_id)
            .and_then(|scope| scope.sessions.get(&id))
            .filter(|session| session.active || matches!(session.status, AgentStatus::Running))
            .and_then(|session| session.harness.clone())
    }

    pub async fn restore(
        self: &Arc<Self>,
        root_session_id: &str,
    ) -> std::io::Result<RestoreReport> {
        self.restore_inner(root_session_id)
            .instrument(tracing::info_span!(
                "subagent.restore",
                session.root_id = root_session_id
            ))
            .await
    }

    async fn restore_inner(
        self: &Arc<Self>,
        root_session_id: &str,
    ) -> std::io::Result<RestoreReport> {
        let store = self
            .store_for(root_session_id)
            .ok_or_else(|| std::io::Error::other("no subagent store is installed"))?;
        let Some(payload) = store.load(root_session_id).await? else {
            return Ok(RestoreReport::default());
        };
        let journal: durable::PersistedScope = serde_json::from_str(&payload)
            .map_err(|error| std::io::Error::other(format!("invalid subagent journal: {error}")))?;

        drop(payload);
        if !(durable::MIN_JOURNAL_VERSION..=durable::JOURNAL_VERSION).contains(&journal.version) {
            return Err(std::io::Error::other(format!(
                "unsupported subagent journal version {}",
                journal.version
            )));
        }
        let messages = journal
            .messages
            .hydrate(store.as_ref(), root_session_id)
            .await?;
        let mut agents = journal.agents;
        agents.extend(
            durable::load_agents(store.as_ref(), root_session_id, journal.agent_records).await?,
        );
        agents.sort_by_key(|agent| agent.descriptor.id);
        let spawning_turns = agents
            .iter()
            .filter_map(durable::PersistedAgent::unmarked_spawning_turn)
            .map(str::to_owned)
            .collect::<HashSet<_>>();
        if !spawning_turns.is_empty() {
            let cancelled = store
                .cancelled_turns(root_session_id, spawning_turns.into_iter().collect())
                .await
                .map_err(|error| {
                    std::io::Error::new(
                        error.kind(),
                        format!("could not read the root's cancelled turns: {error}"),
                    )
                })?;
            for agent in &mut agents {
                if agent
                    .unmarked_spawning_turn()
                    .is_some_and(|turn| cancelled.iter().any(|cancelled| cancelled == turn))
                {
                    agent.cancelled_revision = Some(agent.next_instruction_revision);
                }
            }
        }

        let lazy = journal.version >= durable::LAZY_CHECKPOINT_VERSION;
        let mut stored = HashMap::new();
        for agent in &mut agents {
            if lazy {
                if let Some(key) = &agent.checkpoint_ref {
                    stored.insert(
                        agent.descriptor.id,
                        durable::JournalCheckpoint::stored(key.clone(), None),
                    );
                }
            } else if let Some(checkpoint) = agent.hydrate(store.as_ref(), root_session_id).await? {
                stored.insert(agent.descriptor.id, checkpoint);
            }
        }
        let mut report = RestoreReport::default();
        let mut state = self.state.acquire().await;
        if state
            .scopes
            .get(root_session_id)
            .is_some_and(|scope| !scope.sessions.is_empty())
        {
            return Err(std::io::Error::other(
                "subagent scope already has live children and cannot be restored",
            ));
        }
        let lineages = agents
            .iter()
            .map(|agent| durable::journaled_lineage(root_session_id, &agents, agent.descriptor.id))
            .collect::<Vec<_>>();
        for (agent, lineage) in agents.into_iter().zip(lineages) {
            let id = agent.descriptor.id;
            let checkpoint = agent.snapshot(&lineage)?;
            let (session, resume, lost) = durable::restored_session(agent, checkpoint)?;
            if session.pending_completion.is_some() && session.status.wakes_parent() && !resume {
                report.completed.push(id);
            }
            match (stored.remove(&id), &session.stored_runtime) {
                (Some(checkpoint), _) => self.insert_checkpoint(root_session_id, id, checkpoint),

                (None, Some(snapshot)) => {
                    self.insert_checkpoint(
                        root_session_id,
                        id,
                        durable::JournalCheckpoint::encode(
                            snapshot.clone(),
                            store.records_sessions(),
                        )?,
                    );
                }
                (None, None) => {}
            }
            state.insert_restored(root_session_id, session)?;
            report.restored += 1;
            if resume {
                report.interrupted.push(id);
            }
            if lost {
                report.unrecoverable.push(id);
            }
        }
        state
            .scopes
            .entry(root_session_id.to_owned())
            .or_default()
            .messages = messages;
        drop(state);

        if !report.interrupted.is_empty() || !report.completed.is_empty() {
            self.save_scope(root_session_id).await.map_err(|error| {
                std::io::Error::new(
                    error.kind(),
                    format!("could not record subagent resume attempts: {error}"),
                )
            })?;
        }
        self.pending_resume.update(|pending| {
            pending.insert(root_session_id.to_owned(), report.interrupted.clone());
        });
        self.changed();
        Ok(report)
    }

    async fn save_scope(&self, root_session_id: &str) -> std::io::Result<()> {
        self.request_journal(root_session_id, true)
            .instrument(tracing::info_span!(
                "subagent.journal.snapshot",
                session.root_id = root_session_id
            ))
            .await
    }

    async fn save_write(
        &self,
        root_session_id: &str,
        store: &dyn SubagentStore,
        write: JournalWrite,
    ) -> std::io::Result<()> {
        let JournalWrite { payload, records } = write;
        let contents = records.iter().map(|(_, json)| Arc::clone(json)).collect();
        store
            .save(root_session_id, payload, contents)
            .instrument(tracing::info_span!(
                "subagent.journal.save",
                session.root_id = root_session_id
            ))
            .await?;
        self.acknowledge(root_session_id, &records);
        Ok(())
    }

    pub async fn resume_interrupted(
        self: &Arc<Self>,
        root_session_id: &str,
    ) -> Vec<(AgentId, std::io::Result<MessageReceipt>)> {
        self.resume_interrupted_inner(root_session_id)
            .instrument(tracing::info_span!(
                "subagent.resume",
                session.root_id = root_session_id
            ))
            .await
    }

    async fn resume_interrupted_inner(
        self: &Arc<Self>,
        root_session_id: &str,
    ) -> Vec<(AgentId, std::io::Result<MessageReceipt>)> {
        let ids = self
            .pending_resume
            .read(|pending| pending.get(root_session_id).cloned())
            .unwrap_or_default();
        let mut results = Vec::with_capacity(ids.len());
        let settle = |id: AgentId| {
            self.pending_resume.update(|pending| {
                if let Some(ids) = pending.get_mut(root_session_id) {
                    ids.retain(|pending| *pending != id);
                    if ids.is_empty() {
                        pending.remove(root_session_id);
                    }
                }
            });
        };
        for id in ids {
            let (task, evidence) = self
                .state
                .acquire()
                .await
                .scopes
                .get(root_session_id)
                .and_then(|scope| scope.sessions.get(&id))
                .map(|session| {
                    (
                        Some(session.descriptor.task.clone()),
                        durable::in_flight_evidence(
                            &session.in_flight_calls,
                            session.in_flight_omitted,
                        ),
                    )
                })
                .unwrap_or_default();
            let mut message = durable::RESUME_MESSAGE.to_owned();
            if let Some(evidence) = evidence {
                message = format!("{message}\n\n{evidence}");
            }
            if let Some(task) = task {
                message = format!("{message}\n\nDelegated task:\n{task}");
            }
            let result = self
                .send_message_restored(
                    root_session_id,
                    id,
                    MessagePriority::Deferred,
                    MessagePurpose::Coordinate,
                    None,
                    message,
                )
                .await;
            settle(id);

            self.changed();
            results.push((id, result));
        }
        results
    }

    async fn fail_unresumable(&self, root_session_id: &str, id: AgentId, mut error: String) {
        let settled = {
            let mut state = self.state.acquire().await;
            let Some(session) = state
                .scopes
                .get_mut(root_session_id)
                .and_then(|scope| scope.sessions.get_mut(&id))
                .filter(|session| {
                    !session.active
                        && session.harness.is_none()
                        && matches!(session.status, AgentStatus::Interrupted)
                })
            else {
                return;
            };
            if let Some(evidence) =
                durable::in_flight_evidence(&session.in_flight_calls, session.in_flight_omitted)
            {
                error.push_str("\n\n");
                error.push_str(&evidence);
            }
            session.status = AgentStatus::Failed { error };
            session.resume_attempts = 0;
            session.in_flight_calls.clear();
            session.in_flight_omitted = 0;
            session.in_flight_progress = durable::OmittedProgress::default();
            let settled = session.next_instruction_revision;
            session.settled_revision = Some(settled);
            let completion = session.descriptor.parent.is_none().then_some(settled);
            if let Some(revision) = completion {
                session.settle_completion(revision);
            }
            (session.status.clone(), completion)
        };
        let (status, completion) = settled;
        self.enqueue_status(
            root_session_id,
            id,
            OrderedJob::Settle {
                status,
                completion,
                harness: None,
            },
        );
        self.changed();
    }

    pub(super) fn reserve_turn(&self) -> std::io::Result<TurnCapacity> {
        self.capacity.reserve()
    }

    pub(super) fn batch_startup(self: &Arc<Self>) -> BatchStartup {
        BatchStartup {
            registry: Arc::clone(self),
            cleanup: None,
        }
    }

    pub(super) fn reserve_turns(&self, count: usize) -> std::io::Result<Vec<TurnCapacity>> {
        self.capacity.reserve_many(count)
    }

    pub fn set_spawn_router(&self, router: Arc<dyn crate::SpawnRouter>) {
        self.spawn_router.set(Some(router));
    }

    pub(super) fn spawn_router(&self) -> Option<Arc<dyn crate::SpawnRouter>> {
        Option::clone(&self.spawn_router.get())
    }

    pub fn set_max_concurrency(&self, limit: usize) {
        self.capacity.set_limit(limit);
    }

    pub fn set_max_resident(&self, limit: usize) {
        self.max_resident_explicit.store(true, Ordering::Relaxed);
        self.max_resident.store(limit, Ordering::Relaxed);
    }

    pub async fn is_root_session(&self, session_id: &str) -> bool {
        !self
            .state
            .acquire()
            .await
            .root_by_session
            .contains_key(session_id)
    }

    pub async fn child_session_id(&self, caller: &str, id: AgentId) -> std::io::Result<String> {
        self.await_restored(caller).await?;
        let state = self.state.acquire().await;
        let root = state.authorize(caller, id)?;
        state
            .scopes
            .get(&root)
            .and_then(|scope| scope.sessions.get(&id))
            .map(|session| session.descriptor.session_id.clone())
            .ok_or_else(|| std::io::Error::other(format!("unknown agent_id {id}")))
    }

    #[doc(hidden)]
    pub async fn host_context(&self, root_session_id: &str, id: AgentId) -> Option<Arc<str>> {
        self.state
            .acquire()
            .await
            .scopes
            .get(root_session_id)
            .and_then(|scope| scope.sessions.get(&id))
            .and_then(|session| session.host_context.as_ref().map(Arc::clone))
    }

    pub(super) async fn descriptor_for_session(&self, session_id: &str) -> Option<AgentDescriptor> {
        let state = self.state.acquire().await;
        let root_session_id = state.root_by_session.get(session_id)?;
        state
            .scopes
            .get(root_session_id)?
            .sessions
            .values()
            .find(|session| session.descriptor.session_id == session_id)
            .map(|session| session.descriptor.clone())
    }

    #[doc(hidden)]
    pub async fn host_context_for_session(&self, session_id: &str) -> Option<Arc<str>> {
        let state = self.state.acquire().await;
        let root_session_id = state.root_by_session.get(session_id)?;
        state
            .scopes
            .get(root_session_id)?
            .sessions
            .values()
            .find(|session| session.descriptor.session_id == session_id)
            .and_then(|session| session.host_context.as_ref().map(Arc::clone))
    }

    pub(super) async fn reserve(&self, session_id: &str) -> std::io::Result<AgentReservation> {
        self.state.acquire().await.reserve_for(session_id)
    }

    pub(super) async fn reserve_many(
        &self,
        session_id: &str,
        count: usize,
    ) -> std::io::Result<Vec<AgentReservation>> {
        let mut state = self.state.acquire().await;
        (0..count).map(|_| state.reserve_for(session_id)).collect()
    }

    pub(super) async fn submit_result_durably(
        self: &Arc<Self>,
        session_id: &str,
        instruction_revision: Option<u64>,
        output: Value,
    ) -> std::io::Result<(SubmissionOutcome, bool)> {
        self.submit_result_durably_inner(session_id, instruction_revision, output)
            .instrument(tracing::info_span!(
                "subagent.submit",
                session.id = session_id,
                instruction.revision = instruction_revision
            ))
            .await
    }

    async fn submit_result_durably_inner(
        self: &Arc<Self>,
        session_id: &str,
        instruction_revision: Option<u64>,
        output: Value,
    ) -> std::io::Result<(SubmissionOutcome, bool)> {
        self.await_restored(session_id).await?;
        let (outcome, root) = {
            let mut state = self.state.acquire().await;
            let outcome = state.submit_result(session_id, instruction_revision, output)?;
            (outcome, state.root_session_id(session_id).to_owned())
        };
        let durable = match outcome {
            SubmissionOutcome::Accepted { .. } => match self.persist_durably(&root).await {
                Ok(()) => true,
                Err(error) => {
                    tracing::warn!(%error, %root, "accepted subagent result is not journaled yet");
                    self.changed();
                    false
                }
            },
            SubmissionOutcome::Superseded => false,
        };
        Ok((outcome, durable))
    }

    async fn persist_durably(&self, root_session_id: &str) -> std::io::Result<()> {
        self.request_journal(root_session_id, false)
            .instrument(tracing::info_span!(
                "subagent.journal.persist",
                session.root_id = root_session_id
            ))
            .await
    }

    fn send_status(&self, root_session_id: &str, update: AgentUpdate) {
        let id = match &update {
            AgentUpdate::Status { id, .. } | AgentUpdate::Completion { id, .. } => *id,
            _ => return self.send(root_session_id, update),
        };
        self.enqueue_status(root_session_id, id, OrderedJob::Update(update));
    }

    fn enqueue_status(&self, root_session_id: &str, id: AgentId, job: OrderedJob) {
        let Some(registry) = self.this.get().cloned() else {
            match job {
                OrderedJob::Update(update) => self.send(root_session_id, update),
                OrderedJob::Settle {
                    status, completion, ..
                } => self.send(root_session_id, settled_update(id, status, completion)),
            }
            return;
        };
        let key = (root_session_id.to_owned(), id);
        let mut job = job;
        let (sender, mut receiver) = loop {
            if let Some(sender) = self
                .status_queues
                .read(|queues| queues.get(&key).cloned())
                .filter(|sender| !sender.is_closed())
            {
                match sender.send(job) {
                    Ok(()) => return,
                    Err(mpsc::error::SendError(returned)) => job = returned,
                }
            }
            let (sender, receiver) = mpsc::unbounded_channel();
            let installed = self
                .status_queues
                .try_update(|queues| match queues.get(&key) {
                    Some(existing) if !existing.is_closed() => None,
                    _ => {
                        queues.insert(key.clone(), sender.clone());
                        Some(())
                    }
                });
            if installed.is_some() {
                break (sender, receiver);
            }
        };
        let _ = sender.send(job);
        drop(platform::spawn(async move {
            while let Some(job) = receiver.recv().await {
                let Some(registry) = registry.upgrade() else {
                    return;
                };
                registry.run_ordered(&key.0, key.1, job).await;
            }
        }));
    }

    async fn run_ordered(self: &Arc<Self>, root_session_id: &str, id: AgentId, job: OrderedJob) {
        match job {
            OrderedJob::Update(update) => {
                self.send_unless_closed(root_session_id, id, update).await
            }
            OrderedJob::Settle {
                status,
                completion,
                harness,
            } => {
                if let Some(harness) = harness {
                    if let Ok(snapshot) = harness.snapshot().await {
                        self.record_checkpoint(root_session_id, id, snapshot);
                    }
                    self.finish_pending_checkpoint(root_session_id);
                }

                if let Err(error) = self.persist_durably(root_session_id).await {
                    tracing::warn!(%error, %root_session_id, "settled subagent turn not journaled yet; the writer retries");
                }
                self.send_unless_closed(
                    root_session_id,
                    id,
                    settled_update(id, status, completion),
                )
                .await;
                self.changed();
            }
        }
    }

    async fn send_unless_closed(&self, root_session_id: &str, id: AgentId, update: AgentUpdate) {
        let state = self.state.acquire().await;
        let closed = state
            .scopes
            .get(root_session_id)
            .and_then(|scope| scope.sessions.get(&id))
            .is_some_and(|session| matches!(session.status, AgentStatus::Closed));
        let stale = closed
            && matches!(&update, AgentUpdate::Status { status, .. } if !matches!(status, AgentStatus::Closed));
        if !stale {
            self.send(root_session_id, update);
        }
    }

    pub async fn acknowledge_completion(&self, root_session_id: &str, id: AgentId, revision: u64) {
        let acknowledged = {
            let mut state = self.state.acquire().await;
            match state
                .scopes
                .get_mut(root_session_id)
                .and_then(|scope| scope.sessions.get_mut(&id))
            {
                Some(session) if session.pending_completion == Some(revision) => {
                    session.clear_completion();
                    true
                }
                _ => false,
            }
        };
        if acknowledged {
            self.completion_retries.update(|retries| {
                retries.remove(&(root_session_id.to_owned(), id));
            });
            self.changed();
        }
    }

    pub fn retry_completion(self: &Arc<Self>, root_session_id: &str, id: AgentId, revision: u64) {
        let key = (root_session_id.to_owned(), id);
        let Some(delay) = self.completion_retries.try_update(|retries| {
            let entry = retries.entry(key.clone()).or_insert((0, false));
            if entry.1 {
                return None;
            }
            entry.1 = true;
            entry.0 = entry.0.saturating_add(1);
            Some(completion_retry_delay(entry.0))
        }) else {
            return;
        };
        tracing::warn!(%id, revision, ?delay, "subagent completion was not delivered; retrying");

        let registry = Arc::downgrade(self);
        drop(platform::spawn(async move {
            platform::sleep(delay).await;
            let Some(registry) = registry.upgrade() else {
                return;
            };
            registry.completion_retries.update(|retries| {
                if let Some(entry) = retries.get_mut(&key) {
                    entry.1 = false;
                }
            });
            let status = {
                let state = registry.state.acquire().await;
                state
                    .scopes
                    .get(&key.0)
                    .and_then(|scope| scope.sessions.get(&key.1))
                    .filter(|session| {
                        session.pending_completion == Some(revision)
                            && session.status.wakes_parent()
                    })
                    .map(|session| session.status.clone())
            };
            if let Some(status) = status {
                registry.send_status(
                    &key.0,
                    AgentUpdate::Completion {
                        id: key.1,
                        status,
                        revision,
                    },
                );
            }
        }));
    }

    async fn announce_restored_completions(&self, root_session_id: &str, ids: &[AgentId]) {
        for &id in ids {
            let announced = {
                let mut state = self.state.acquire().await;
                let Some(session) = state
                    .scopes
                    .get_mut(root_session_id)
                    .and_then(|scope| scope.sessions.get_mut(&id))
                else {
                    continue;
                };
                let Some(revision) = session.pending_completion else {
                    continue;
                };
                if !session.status.wakes_parent() {
                    continue;
                }
                let descriptor = std::mem::take(&mut session.announce).then(|| {
                    let mut descriptor = session.descriptor.clone();
                    descriptor.task = session.binding_task.clone();
                    descriptor
                });
                (descriptor, session.status.clone(), revision)
            };
            let (descriptor, status, revision) = announced;
            if let Some(descriptor) = descriptor {
                self.send(root_session_id, AgentUpdate::Added(descriptor));
            }
            self.send_status(
                root_session_id,
                AgentUpdate::Completion {
                    id,
                    status,
                    revision,
                },
            );
        }
        self.changed();
    }

    pub(super) async fn begin_turn_steer(
        &self,
        root_session_id: &str,
        id: AgentId,
    ) -> Option<TurnSteer> {
        self.state
            .acquire()
            .await
            .begin_turn_steer(root_session_id, id)
    }

    pub(super) async fn finish_turn_steer(
        &self,
        root_session_id: &str,
        steer: TurnSteer,
        committed: bool,
    ) {
        self.state
            .acquire()
            .await
            .finish_turn_steer(root_session_id, steer, committed);
    }

    pub(super) async fn insert(
        self: &Arc<Self>,
        root_session_id: String,
        descriptor: AgentDescriptor,
        host_context: Option<Arc<str>>,
        agent: Agent,
        event_task: Task<()>,
        contract: OutputContract,
    ) -> std::io::Result<()> {
        let OutputContract { validator, schema } = contract;
        let id = descriptor.id;

        let checkpoint = if self.store_for(&root_session_id).is_some() {
            match agent.checkpoint().await {
                Ok(snapshot) => Some(snapshot),
                Err(error) => {
                    event_task.abort();
                    let _ = agent.shutdown().await;
                    return Err(std::io::Error::other(error));
                }
            }
        } else {
            None
        };
        let mut state = self.state.acquire().await;
        state.validate_insert(&root_session_id, &descriptor)?;
        let (harness, harness_task) = harness::spawn(
            root_session_id.clone(),
            descriptor.id,
            descriptor.context(),
            agent,
            self.capacity.clone(),
            Arc::downgrade(self),
            schema.clone(),
            None,
        );
        if let Some(snapshot) = checkpoint {
            self.record_checkpoint(&root_session_id, descriptor.id, snapshot);
        }
        state.insert(
            root_session_id.clone(),
            descriptor.id,
            descriptor.session_id.clone(),
            ChildSession {
                binding_task: descriptor.task.clone(),
                descriptor,
                host_context,
                event_task: Some(event_task),
                harness: Some(harness),
                harness_task: Some(harness_task),
                status: AgentStatus::Pending,
                active: false,
                output_validator: validator,
                output_schema: serde_json::from_str(&schema).expect("compiled schema is JSON"),
                stored_runtime: None,
                journaled_runtime: false,
                next_instruction_revision: 0,
                active_instruction_revision: None,
                steering: false,
                submitted_output: None,
                last_output: None,
                last_used: 0,
                evicted: false,
                resume_attempts: 0,
                in_flight_calls: Vec::new(),
                in_flight_omitted: 0,
                pending_completion: None,
                completion_attempt: 0,
                completion_parked: false,
                cancelled_revision: None,
                in_spawning_turn: false,
                settled_revision: None,
                usage: AgentUsage::default(),
                in_flight_progress: durable::OmittedProgress::default(),
                announce: false,
                journal_generation: 0,
            },
        )?;
        drop(state);
        self.changed();
        let mut cancellation = self.batch_startup();
        cancellation.track(&root_session_id, id);
        let saved = if self.store_for(&root_session_id).is_some() {
            self.save_scope(&root_session_id).await
        } else {
            Ok(())
        };
        cancellation.cleanup = None;
        if let Err(error) = saved {
            if let Some(session) = self
                .state
                .acquire()
                .await
                .scopes
                .get_mut(&root_session_id)
                .and_then(|scope| scope.sessions.get_mut(&id))
                && let Some(events) = session.event_task.take()
            {
                events.abort();
            }
            return match self.close_batch(&root_session_id, vec![id]).await {
                Ok(()) => Err(error),
                Err(cleanup) => Err(std::io::Error::other(format!(
                    "{error}; subagent startup cleanup failed: {cleanup}"
                ))),
            };
        }
        Ok(())
    }

    pub(super) async fn launch_initial_turn(
        self: &Arc<Self>,
        root_session_id: &str,
        id: AgentId,
        prompt: String,
        capacity: TurnCapacity,
    ) -> std::io::Result<()> {
        let harness = self
            .state
            .acquire()
            .await
            .harness_in_scope(root_session_id, id)?;
        harness.start(prompt, capacity).await
    }

    pub(super) async fn pending_turn(
        &self,
        root: &str,
        session_id: &str,
    ) -> std::io::Result<Option<(String, serde_json::Value)>> {
        match self.store_for(root) {
            Some(store) => store.pending_turn(session_id).await,
            None => Ok(None),
        }
    }

    pub(super) async fn harness_turn_started(
        &self,
        root_session_id: &str,
        id: AgentId,
        recovering: bool,
        context: Option<&AgentContext>,
    ) -> Option<u64> {
        let mut descriptor = None;
        let revision = {
            let mut state = self.state.acquire().await;
            let last_used = state.next_access();
            let session = state
                .scopes
                .get_mut(root_session_id)
                .and_then(|scope| scope.sessions.get_mut(&id))?;
            if !session.status.can_start_turn() || session.active {
                None
            } else {
                let revision = if recovering {
                    session.next_instruction_revision
                } else {
                    session.next_instruction_revision.checked_add(1)?
                };

                if !matches!(session.status, AgentStatus::Interrupted) {
                    session.in_flight_calls.clear();
                    session.in_flight_omitted = 0;
                    session.in_flight_progress = durable::OmittedProgress::default();
                }
                session.next_instruction_revision = revision;
                session.active_instruction_revision = Some(revision);
                session.active = true;
                session.steering = false;
                session.submitted_output = None;
                session.last_used = last_used;
                session.status = AgentStatus::Running;
                if let Some(context) = context
                    && session.descriptor.context().as_ref() != Some(context)
                {
                    session.descriptor.model = Some(context.model.clone());
                    session.descriptor.thinking = Some(context.thinking);
                    session.descriptor.reasoning_mode = context.reasoning_mode;
                    let mut updated = session.descriptor.clone();
                    updated.task = session.binding_task.clone();
                    descriptor = Some(updated);
                }
                Some(revision)
            }
        };
        if revision.is_some() {
            if let Some(descriptor) = descriptor {
                self.send(root_session_id, AgentUpdate::Added(descriptor));
            }
            self.send_status(
                root_session_id,
                AgentUpdate::Status {
                    id,
                    status: AgentStatus::Running,
                },
            );
            self.changed();
        }
        revision
    }

    pub(super) async fn harness_turn_start_failed(
        &self,
        root_session_id: &str,
        id: AgentId,
        error: String,
    ) {
        let status = {
            let mut state = self.state.acquire().await;
            let Some(session) = state
                .scopes
                .get_mut(root_session_id)
                .and_then(|scope| scope.sessions.get_mut(&id))
            else {
                return;
            };
            session.active = false;
            session.active_instruction_revision = None;
            session.steering = false;
            session.submitted_output = None;
            if !matches!(session.status, AgentStatus::Closing | AgentStatus::Closed) {
                session.status = AgentStatus::Failed { error };
            }
            session.status.clone()
        };
        self.send_status(root_session_id, AgentUpdate::Status { id, status });
        self.changed();
    }

    pub(super) async fn harness_turn_finished(
        self: &Arc<Self>,
        root_session_id: &str,
        id: AgentId,
        result: AgentResult<TurnResult>,
    ) {
        let status = {
            let mut state = self.state.acquire().await;
            let Some(session) = state
                .scopes
                .get_mut(root_session_id)
                .and_then(|scope| scope.sessions.get_mut(&id))
            else {
                return;
            };
            if !session.active {
                return;
            }
            session.active = false;
            let revision = session.active_instruction_revision.take();
            session.steering = false;
            if matches!(result, Err(NanocodeError::TurnStopped))
                && session.submitted_output.is_none()
                && !matches!(session.status, AgentStatus::Closing | AgentStatus::Closed)
            {
                let status = session.status.clone();
                drop(state);
                self.enqueue_status(
                    root_session_id,
                    id,
                    OrderedJob::Settle {
                        status,
                        completion: None,
                        harness: None,
                    },
                );
                self.changed();
                return;
            }

            session.resume_attempts = 0;
            session.in_flight_calls.clear();
            session.in_flight_omitted = 0;
            session.in_flight_progress = durable::OmittedProgress::default();
            if let Ok(TurnResult {
                usage: Some(usage), ..
            }) = &result
            {
                session.usage.record(usage);
            }
            let submitted_output = session.submitted_output.take();

            if let Some(output) = &submitted_output {
                session.last_output = Some(output.clone());
            }
            if matches!(session.status, AgentStatus::Closing | AgentStatus::Closed) {
                session.status.clone()
            } else {
                match result {
                    Ok(_) => complete_session(session, submitted_output),
                    Err(NanocodeError::TurnCancelled | NanocodeError::TurnStopped) => {
                        AgentStatus::Interrupted
                    }
                    Err(error) => AgentStatus::Failed {
                        error: error.to_string(),
                    },
                }
            }
            .clone_into(&mut session.status);

            let harness = session
                .harness
                .clone()
                .filter(|_| self.begin_pending_checkpoint(root_session_id));
            let settled = revision.unwrap_or(session.next_instruction_revision);
            session.settled_revision = Some(settled);

            let completion = (session.status.wakes_parent() && session.descriptor.parent.is_none())
                .then_some(settled);
            if let Some(revision) = completion {
                session.settle_completion(revision);
            }
            (session.status.clone(), completion, harness)
        };

        let (status, completion, harness) = status;
        self.enqueue_status(
            root_session_id,
            id,
            OrderedJob::Settle {
                status,
                completion,
                harness,
            },
        );
        self.changed();
        let registry = Arc::clone(self);
        let root_session_id = root_session_id.to_owned();
        drop(platform::spawn(async move {
            registry.enforce_resident_limit(&root_session_id).await;
        }));
    }

    async fn enforce_resident_limit(&self, root_session_id: &str) {
        let locks = self.scope_locks(root_session_id);
        let _residency_guard = locks.residency.enter().await;
        loop {
            let _message_guard = locks.message.enter().await;
            let eviction = self
                .state
                .acquire()
                .await
                .take_resident_eviction(root_session_id, self.max_resident.load(Ordering::Relaxed));
            let Some(ResidentEviction { id, harness }) = eviction else {
                return;
            };
            match harness.snapshot().await {
                Ok(snapshot) => {
                    let stored = if self.store_for(root_session_id).is_some()
                        && self.record_checkpoint(root_session_id, id, snapshot.clone())
                    {
                        None
                    } else {
                        Some(snapshot)
                    };
                    if let Some(session) = self
                        .state
                        .acquire()
                        .await
                        .scopes
                        .get_mut(root_session_id)
                        .and_then(|scope| scope.sessions.get_mut(&id))
                    {
                        session.journaled_runtime = stored.is_none();
                        session.stored_runtime = stored;
                    }
                }
                Err(_) => {
                    if let Some(session) = self
                        .state
                        .acquire()
                        .await
                        .scopes
                        .get_mut(root_session_id)
                        .and_then(|scope| scope.sessions.get_mut(&id))
                    {
                        session.harness = Some(harness);
                        session.evicted = false;
                    }
                    return;
                }
            }
            self.changed();
            if harness.close().await.is_err() {
                self.harness_closed(root_session_id, id).await;
            }

            let tasks = {
                let mut state = self.state.acquire().await;
                let session = state
                    .scopes
                    .get_mut(root_session_id)
                    .and_then(|scope| scope.sessions.get_mut(&id));
                session.map(|session| (session.harness_task.take(), session.event_task.take()))
            };
            if let Some((harness_task, event_task)) = tasks {
                if let Some(task) = harness_task {
                    let _ = task.await;
                }
                if let Some(task) = event_task {
                    let _ = task.await;
                }
            }
        }
    }

    pub(super) async fn harness_closed(&self, root_session_id: &str, id: AgentId) {
        let (status_update, detach) = {
            let mut state = self.state.acquire().await;
            let Some(scope) = state.scopes.get_mut(root_session_id) else {
                return;
            };

            let detach = scope.closing;
            let Some(session) = scope.sessions.get_mut(&id) else {
                return;
            };
            let status = if matches!(session.status, AgentStatus::Closed) {
                None
            } else {
                session.harness = None;
                session.active = false;
                session.active_instruction_revision = None;
                session.steering = false;
                session.submitted_output = None;
                if session.evicted && !matches!(session.status, AgentStatus::Closing) {
                    None
                } else {
                    session.evicted = false;
                    session.status = AgentStatus::Closed;
                    Some(AgentStatus::Closed)
                }
            };
            (status, detach)
        };
        if let Some(status) = status_update {
            let _ = send_update(
                &self.updates,
                root_session_id,
                AgentUpdate::Status { id, status },
                detach,
            );
        }
        self.changed();
    }

    async fn runtime_closed(&self, root_session_id: &str, id: AgentId) {
        let harness = {
            let state = self.state.acquire().await;
            state
                .scopes
                .get(root_session_id)
                .and_then(|scope| scope.sessions.get(&id))
                .filter(|session| {
                    !matches!(session.status, AgentStatus::Closing | AgentStatus::Closed)
                })
                .and_then(|session| session.harness.clone())
        };
        let Some(harness) = harness else {
            self.harness_closed(root_session_id, id).await;
            return;
        };
        drop(harness.close().await);
    }

    pub(super) fn send(&self, root_session_id: &str, update: AgentUpdate) {
        if let AgentUpdate::Completion { id, revision, .. } = &update {
            self.schedule_parent_continuation(root_session_id, *id, *revision);
        }
        let _ = send_update(&self.updates, root_session_id, update, false);
    }

    fn schedule_parent_continuation(&self, root_session_id: &str, id: AgentId, revision: u64) {
        let resolver = Option::clone(&self.parent_resolver.get());
        let (Some(resolver), Some(registry)) = (resolver, self.this.get().and_then(Weak::upgrade))
        else {
            return;
        };
        let root_session_id = root_session_id.to_owned();
        drop(platform::spawn(async move {
            let Some(parent) = resolver(&root_session_id) else {
                registry.retry_completion(&root_session_id, id, revision);
                return;
            };
            if let Err(error) = registry
                .continue_parent(&parent, &root_session_id, id, revision)
                .await
            {
                tracing::warn!(%error, %id, revision, "subagent completion did not continue its parent");
            }
        }));
    }

    pub async fn continue_parent(
        self: &Arc<Self>,
        parent: &Agent,
        root_session_id: &str,
        id: AgentId,
        revision: u64,
    ) -> std::io::Result<bool> {
        if parent.session_id() != root_session_id {
            return Err(std::io::Error::other(
                "subagent completion parent is not the root session",
            ));
        }
        let dispatch = self.completion_dispatch.update(|dispatch| {
            dispatch
                .entry((root_session_id.to_owned(), id))
                .or_default()
                .clone()
        });
        let _dispatch = dispatch.enter().await;
        let key = (root_session_id.to_owned(), id);
        if self
            .completion_routes
            .read(|routes| routes.get(&key) == Some(&revision))
        {
            return Ok(false);
        }
        let Some((_, _, _, parked)) = self.peek_completion(root_session_id, id, revision).await
        else {
            return Ok(false);
        };
        if parked
            || self
                .park_completion(root_session_id, id, revision, false)
                .await
        {
            self.watch_root(root_session_id, parent);
            if !parked && self.store_for(root_session_id).is_some() {
                self.persist_durably(root_session_id).await?;
            }
            return Ok(false);
        }
        if self.store_for(root_session_id).is_some()
            && let Err(error) = self.persist_durably(root_session_id).await
        {
            self.retry_completion(root_session_id, id, revision);
            return Err(error);
        }
        let Some((status, session_id, attempt, _)) =
            self.peek_completion(root_session_id, id, revision).await
        else {
            return Ok(false);
        };
        let request_id = match attempt {
            0 => format!(
                "{}{session_id}:{revision}",
                crate::COMPLETION_REQUEST_PREFIX
            ),
            attempt => format!(
                "{}{session_id}:{revision}:{attempt}",
                crate::COMPLETION_REQUEST_PREFIX
            ),
        };
        let prompt = completion_continuation_prompt(id, &status);
        let events = parent.subscribe();
        match parent
            .route_prompt_request(
                nanocode_agent_core::PromptRequest::new(prompt.clone())
                    .request_id(request_id.clone()),
            )
            .await
        {
            Ok(route) => {
                self.wait_reported.update(|reported| {
                    reported.insert(
                        (root_session_id.to_owned(), id),
                        wait_mark(&status, revision),
                    );
                });
                self.completion_routes.update(|routes| {
                    routes.insert(key.clone(), revision);
                });
                let registry = Arc::downgrade(self);
                let parent = parent.downgrade();
                drop(platform::spawn(async move {
                    let outcome = match route {
                        PromptRoute::Started(turn) => {
                            drop(events);
                            NoticeOutcome::of_turn(turn.result().await)
                        }
                        PromptRoute::Steered => {
                            NoticeOutcome::of_steer(events, &request_id, prompt, &parent).await
                        }
                    };
                    let Some(registry) = registry.upgrade() else {
                        return;
                    };
                    registry
                        .settle_notice(&key.0, id, revision, outcome, &parent)
                        .await;
                }));
                Ok(true)
            }
            Err(error) => {
                if !matches!(
                    error,
                    NanocodeError::AgentStopped | NanocodeError::TurnStopped
                ) {
                    self.retry_completion(root_session_id, id, revision);
                }
                Err(std::io::Error::other(error))
            }
        }
    }

    async fn peek_completion(
        &self,
        root_session_id: &str,
        id: AgentId,
        revision: u64,
    ) -> Option<(AgentStatus, String, u32, bool)> {
        let state = self.state.acquire().await;
        state
            .scopes
            .get(root_session_id)
            .and_then(|scope| scope.sessions.get(&id))
            .filter(|session| {
                session.pending_completion == Some(revision) && session.status.wakes_parent()
            })
            .map(|session| {
                (
                    session.status.clone(),
                    session.descriptor.session_id.clone(),
                    session.completion_attempt,
                    session.completion_parked,
                )
            })
    }

    async fn settle_notice(
        self: &Arc<Self>,
        root_session_id: &str,
        id: AgentId,
        revision: u64,
        outcome: NoticeOutcome,
        parent: &WeakAgent,
    ) {
        self.completion_routes.update(|routes| {
            let key = (root_session_id.to_owned(), id);
            if routes.get(&key) == Some(&revision) {
                routes.remove(&key);
            }
        });
        match outcome {
            NoticeOutcome::Read => {
                self.acknowledge_completion(root_session_id, id, revision)
                    .await;
                if self.store_for(root_session_id).is_some()
                    && let Err(error) = self.persist_durably(root_session_id).await
                {
                    tracing::warn!(%error, %id, revision, "subagent completion acknowledgement was not journaled");
                    self.restore_completion(root_session_id, id, revision).await;
                    self.retry_completion(root_session_id, id, revision);
                }
            }
            NoticeOutcome::Unread => {
                self.next_completion_attempt(root_session_id, id, revision)
                    .await;
                self.retry_completion(root_session_id, id, revision);
            }
            NoticeOutcome::Undelivered => {
                self.schedule_parent_continuation(root_session_id, id, revision);
            }
            NoticeOutcome::Pending => self.retry_completion(root_session_id, id, revision),
            NoticeOutcome::Cancelled => {
                if !self
                    .park_completion(root_session_id, id, revision, true)
                    .await
                {
                    return;
                }
                if let Some(parent) = parent.upgrade() {
                    self.watch_root(root_session_id, &parent);
                }
                if self.store_for(root_session_id).is_some()
                    && let Err(error) = self.persist_durably(root_session_id).await
                {
                    tracing::warn!(%error, %id, revision, "parked subagent completion was not journaled");
                }
            }
            NoticeOutcome::Stopped => {}
        }
    }

    async fn next_completion_attempt(&self, root_session_id: &str, id: AgentId, revision: u64) {
        let mut state = self.state.acquire().await;
        if let Some(scope) = state.scopes.get_mut(root_session_id)
            && scope
                .sessions
                .get(&id)
                .is_some_and(|session| session.pending_completion == Some(revision))
            && let Some(session) = scope.sessions.get_mut(&id)
        {
            session.completion_attempt = session.completion_attempt.saturating_add(1);
        }
    }

    async fn park_completion(
        &self,
        root_session_id: &str,
        id: AgentId,
        revision: u64,
        held: bool,
    ) -> bool {
        let parked = {
            let mut state = self.state.acquire().await;
            let Some(scope) = state.scopes.get_mut(root_session_id) else {
                return false;
            };
            let lineage = held
                || scope.sessions.values().any(|session| {
                    session.completion_parked && session.pending_completion.is_some()
                });
            let pending = scope
                .sessions
                .get(&id)
                .is_some_and(|session| session.pending_completion == Some(revision));
            if lineage
                && pending
                && let Some(session) = scope.sessions.get_mut(&id)
            {
                session.completion_parked = true;
                true
            } else {
                false
            }
        };
        if parked {
            self.completion_retries.update(|retries| {
                retries.remove(&(root_session_id.to_owned(), id));
            });
            self.changed();
        }
        parked
    }

    fn claim_root_watch(&self, root_session_id: &str) -> bool {
        self.root_watches
            .try_update(|watches| {
                if watches.contains(root_session_id) {
                    return None;
                }
                watches.insert(root_session_id.to_owned());
                Some(())
            })
            .is_some()
    }

    fn watch_root(self: &Arc<Self>, root_session_id: &str, parent: &Agent) {
        if !self.claim_root_watch(root_session_id) {
            return;
        }
        let root = root_session_id.to_owned();
        let mut events = parent.subscribe();
        let parent = parent.downgrade();
        let registry = Arc::downgrade(self);
        drop(platform::spawn(async move {
            let mut requests = HashMap::<String, String>::new();
            while let Some(event) = events.recv().await {
                let accepted = event.kind == AgentEventKind::InputAccepted;
                let started = event.kind == AgentEventKind::RunStarted;
                if !accepted
                    && !started
                    && !matches!(
                        event.kind,
                        AgentEventKind::RunCompleted | AgentEventKind::RunFailed
                    )
                {
                    continue;
                }
                let Ok(run) = event.decode_payload::<RootRunEvent>() else {
                    continue;
                };
                let Some(registry) = registry.upgrade() else {
                    return;
                };
                if started {
                    registry.running_roots.update(|running| {
                        running.insert(root.clone());
                    });
                } else if accepted {
                    if run.kind.as_deref() == Some("prompt")
                        && let (Some(turn), Some(request)) = (&run.turn_id, &run.request_id)
                    {
                        requests.insert(turn.clone(), request.clone());
                    }
                    if run.starts_user_turn() {
                        registry.unpark_completions(&root).await;
                    }
                } else if let Some(turn) = run.turn_id {
                    registry.running_roots.update(|running| {
                        running.remove(&root);
                    });
                    let request = requests.remove(&turn);
                    let cancelled = run.status == Some(RunStatus::Cancelled)
                        && parent.upgrade().is_some_and(|parent| !parent.is_stopped());
                    registry
                        .root_turn_ended(&root, &turn, request.as_deref(), cancelled)
                        .await;
                }
                if registry.needs_root_watch(&root).await {
                    continue;
                }
                registry.root_watches.update(|watches| {
                    watches.remove(&root);
                });
                if !registry.needs_root_watch(&root).await || !registry.claim_root_watch(&root) {
                    registry.running_roots.update(|running| {
                        running.remove(&root);
                    });
                    return;
                }
            }
            if let Some(registry) = registry.upgrade() {
                registry.root_watches.update(|watches| {
                    watches.remove(&root);
                });
                registry.running_roots.update(|running| {
                    running.remove(&root);
                });
            }
        }));
    }

    async fn needs_root_watch(&self, root_session_id: &str) -> bool {
        self.state
            .acquire()
            .await
            .scopes
            .get(root_session_id)
            .is_some_and(|scope| {
                scope.sessions.values().any(|session| {
                    (session.completion_parked && session.pending_completion.is_some())
                        || (session.descriptor.parent.is_none() && session.status.is_active())
                })
            })
    }

    pub(super) async fn spawned_in_turn(self: &Arc<Self>, session_id: &str, id: AgentId) {
        let root = {
            let mut state = self.state.acquire().await;
            let root = state.root_session_id(session_id).to_owned();
            if root != session_id {
                return;
            }
            let Some(session) = state
                .scopes
                .get_mut(&root)
                .and_then(|scope| scope.sessions.get_mut(&id))
            else {
                return;
            };
            session.in_spawning_turn = true;
            root
        };
        self.running_roots.update(|running| {
            running.insert(root.clone());
        });
        let resolver = Option::clone(&self.parent_resolver.get());
        if let Some(parent) = resolver.and_then(|resolver| resolver(&root)) {
            self.watch_root(&root, &parent);
        }
    }

    async fn root_turn_ended(
        &self,
        root_session_id: &str,
        turn_id: &str,
        request_id: Option<&str>,
        cancelled: bool,
    ) {
        let marked = {
            let mut state = self.state.acquire().await;
            let Some(scope) = state.scopes.get_mut(root_session_id) else {
                return;
            };
            let spawned = scope
                .sessions
                .iter()
                .filter(|(_, session)| {
                    session.descriptor.parent.is_none()
                        && (session.in_spawning_turn
                            || session.host_context.as_deref().is_some_and(|context| {
                                context == turn_id || Some(context) == request_id
                            }))
                })
                .map(|(id, _)| *id)
                .collect::<Vec<_>>();
            let mut marked = false;
            for id in spawned {
                let Some(session) = scope.sessions.get_mut(&id) else {
                    continue;
                };
                session.in_spawning_turn = false;
                if cancelled && session.status.is_active() && session.cancelled_revision.is_none() {
                    session.cancelled_revision = session
                        .active_instruction_revision
                        .or_else(|| session.next_instruction_revision.checked_add(1));
                    marked = true;
                }
            }
            marked
        };
        if marked
            && self.store_for(root_session_id).is_some()
            && let Err(error) = self.persist_durably(root_session_id).await
        {
            tracing::warn!(%error, root_session_id, "cancelled subagent lineage was not journaled");
        }
    }

    async fn unpark_completions(&self, root_session_id: &str) {
        let unparked = {
            let mut state = self.state.acquire().await;
            let Some(scope) = state.scopes.get_mut(root_session_id) else {
                return;
            };
            let ids = scope
                .sessions
                .iter()
                .filter(|(_, session)| session.completion_parked)
                .map(|(id, _)| *id)
                .collect::<Vec<_>>();
            ids.into_iter()
                .filter_map(|id| {
                    let session = scope.sessions.get_mut(&id)?;
                    session.completion_parked = false;
                    session.completion_attempt = session.completion_attempt.saturating_add(1);
                    session.pending_completion.map(|revision| (id, revision))
                })
                .collect::<Vec<_>>()
        };
        if unparked.is_empty() {
            return;
        }
        self.changed();
        for (id, revision) in unparked {
            self.schedule_parent_continuation(root_session_id, id, revision);
        }
    }

    async fn restore_completion(&self, root_session_id: &str, id: AgentId, revision: u64) {
        let mut state = self.state.acquire().await;
        if let Some(session) = state
            .scopes
            .get_mut(root_session_id)
            .and_then(|scope| scope.sessions.get_mut(&id))
            && session.pending_completion.is_none()
            && session.settled_revision == Some(revision)
            && session.status.wakes_parent()
        {
            session.pending_completion = Some(revision);
        }
        drop(state);
        self.changed();
    }

    async fn observe_completions(
        self: &Arc<Self>,
        root_session_id: &str,
        ids: &[AgentId],
        call_id: &str,
    ) -> Option<Arc<dyn ToolReceipt>> {
        let resolver = Option::clone(&self.parent_resolver.get());
        let Some(parent) = resolver.and_then(|resolver| resolver(root_session_id)) else {
            self.acknowledge_observed(root_session_id, ids).await;
            return None;
        };
        let pending = {
            let state = self.state.acquire().await;
            let scope = state.scopes.get(root_session_id)?;
            ids.iter()
                .filter_map(|id| {
                    let session = scope.sessions.get(id)?;
                    let revision = session.pending_completion?;
                    session.status.wakes_parent().then_some((*id, revision))
                })
                .collect::<Vec<_>>()
        };
        let (claimed, routed) = self.completion_routes.update(|routes| {
            let mut claimed = Vec::new();
            let mut routed = Vec::new();
            for &(id, revision) in &pending {
                let key = (root_session_id.to_owned(), id);
                match routes.get(&key) {
                    None => {
                        routes.insert(key, revision);
                        claimed.push((id, revision));
                    }
                    Some(&held) if held == revision => routed.push(key),
                    Some(_) => {}
                }
            }
            (claimed, routed)
        });
        let receipt = Arc::new(ResultReceipt {
            carrier: Atom::new(Some(Arc::from(call_id))),
        });
        let joined = self.result_readers.read(|readers| {
            routed
                .iter()
                .filter_map(|key| readers.get(key))
                .map(|readers| readers.update(|readers| readers.push(Arc::clone(&receipt))))
                .count()
        });
        if claimed.is_empty() {
            return (joined > 0).then_some(receipt as Arc<dyn ToolReceipt>);
        }
        let watched = claimed
            .into_iter()
            .map(|claim| (claim, parent.subscribe()))
            .collect::<Vec<_>>();
        let running = self
            .running_roots
            .read(|running| running.contains(root_session_id));
        for ((id, revision), events) in watched {
            let key = (root_session_id.to_owned(), id);
            let readers: Readers = Arc::new(Atom::new(vec![Arc::clone(&receipt)]));
            self.result_readers.update(|all| {
                all.insert(key.clone(), Arc::clone(&readers));
            });
            let registry = Arc::downgrade(self);
            let parent = parent.downgrade();
            drop(platform::spawn(async move {
                let outcome = if running {
                    NoticeOutcome::of_tool_output(events, &readers).await
                } else {
                    NoticeOutcome::Undelivered
                };
                let Some(registry) = registry.upgrade() else {
                    return;
                };
                registry.result_readers.update(|all| {
                    if all
                        .get(&key)
                        .is_some_and(|current| Arc::ptr_eq(current, &readers))
                    {
                        all.remove(&key);
                    }
                });
                registry
                    .settle_notice(&key.0, id, revision, outcome, &parent)
                    .await;
            }));
        }
        Some(receipt)
    }

    pub(crate) async fn observe_listed(
        self: &Arc<Self>,
        session_id: &str,
        listed: &[AgentDirectoryEntry],
        call_id: &str,
    ) -> Option<Arc<dyn ToolReceipt>> {
        let root = self
            .state
            .acquire()
            .await
            .root_session_id(session_id)
            .to_owned();
        if root != session_id {
            return None;
        }
        let ids = listed
            .iter()
            .map(|entry| entry.agent_id)
            .collect::<Vec<_>>();
        self.observe_completions(&root, &ids, call_id).await
    }

    async fn acknowledge_observed(&self, root_session_id: &str, ids: &[AgentId]) {
        let observed = {
            let mut state = self.state.acquire().await;
            let Some(scope) = state.scopes.get_mut(root_session_id) else {
                return;
            };
            ids.iter()
                .copied()
                .filter(|id| {
                    scope.sessions.get_mut(id).is_some_and(|session| {
                        let observed =
                            session.status.wakes_parent() && session.pending_completion.is_some();
                        if observed {
                            session.clear_completion();
                        }
                        observed
                    })
                })
                .collect::<Vec<_>>()
        };
        if observed.is_empty() {
            return;
        }
        self.completion_retries.update(|retries| {
            for id in &observed {
                retries.remove(&(root_session_id.to_owned(), *id));
            }
        });
        self.changed();
    }

    pub async fn directory(
        &self,
        session_id: &str,
        include_completed: bool,
        include_self: bool,
    ) -> std::io::Result<Vec<AgentDirectoryEntry>> {
        self.await_restored(session_id).await?;
        let state = self.state.acquire().await;
        let mut entries = state.directory(session_id, include_completed, include_self);
        let root = state.root_session_id(session_id).to_owned();
        drop(state);

        if let Some(resuming) = self
            .pending_resume
            .read(|pending| pending.get(&root).cloned())
        {
            for entry in &mut entries {
                entry.resuming = matches!(entry.status, AgentStatus::Interrupted)
                    && resuming.contains(&entry.agent_id);
            }
        }
        Ok(entries)
    }

    pub(crate) fn register_handle(self: &Arc<Self>, handle: AgentHandle) {
        self.adopt_journal(&handle);
        self.session_handles.update(|handles| {
            handles.insert(handle.session_id().to_owned(), handle.clone());
        });
    }

    async fn rehydrate(
        self: &Arc<Self>,
        session_id: &str,
        to: AgentId,
        purpose: MessagePurpose,
    ) -> std::io::Result<()> {
        let (root, mut missing) = {
            let state = self.state.acquire().await;
            let root = state.root_session_id(session_id).to_owned();
            let scope = state
                .scopes
                .get(&root)
                .ok_or_else(|| std::io::Error::other(format!("unknown agent_id {to}")))?;
            if scope.topology.agent_for_session(session_id) == Some(to) {
                return Err(std::io::Error::other("agents cannot message themselves"));
            }
            if purpose == MessagePurpose::Delegate {
                scope.topology.authorize(session_id, to)?;
            }
            let mut id = to;
            let mut missing = Vec::new();
            loop {
                let session = scope
                    .sessions
                    .get(&id)
                    .ok_or_else(|| std::io::Error::other(format!("unknown agent_id {id}")))?;
                if matches!(
                    session.status,
                    AgentStatus::Pending | AgentStatus::Closing | AgentStatus::Closed
                ) || session.harness.is_some()
                {
                    break;
                }
                let snapshot = match (&session.stored_runtime, session.journaled_runtime) {
                    (Some(snapshot), _) => Some(snapshot.clone()),
                    (None, true) => None,
                    (None, false) => break,
                };
                let parent_session = session
                    .descriptor
                    .parent
                    .and_then(|parent| scope.sessions.get(&parent))
                    .map_or_else(
                        || root.clone(),
                        |parent| parent.descriptor.session_id.clone(),
                    );
                missing.push((
                    id,
                    parent_session,
                    snapshot,
                    session.host_context.clone(),
                    session.output_schema.clone(),
                    session.descriptor.task.clone(),
                ));
                match session.descriptor.parent {
                    Some(parent) => id = parent,
                    None => break,
                }
            }
            (root, missing)
        };
        while let Some((id, parent_session, snapshot, host_context, schema, task)) = missing.pop() {
            let snapshot = match snapshot {
                Some(snapshot) => snapshot,
                None => self.journal_snapshot(&root, id).await?,
            };
            let parent = self
                .session_handles
                .read(|handles| handles.get(&parent_session).cloned())
                .ok_or_else(|| {
                    std::io::Error::other("subagent parent runtime is unavailable for rehydration")
                })?;
            let contract = OutputContract::compile(&schema)?;
            let needs_assignment = !snapshot.has_conversation();
            let context = super::model::AgentContext {
                model: snapshot.model(),
                thinking: snapshot.thinking(),
                reasoning_mode: snapshot.reasoning_mode(),
            };
            let (agent, events) = parent
                .restore_runtime(snapshot, host_context)
                .await
                .map_err(std::io::Error::other)?;
            let (start, ready) = oneshot::channel();
            let event_task = forward_events(
                root.clone(),
                id,
                events,
                ready,
                Arc::downgrade(self),
                self.updates.clone(),
            );
            let (harness, task) = harness::spawn(
                root.clone(),
                id,
                Some(context.clone()),
                agent,
                self.capacity.clone(),
                Arc::downgrade(self),
                contract.schema,
                needs_assignment.then(|| super::model::agent_prompt(id, &task)),
            );
            let mut state = self.state.acquire().await;
            let session = state
                .scopes
                .get_mut(&root)
                .expect("locked scope")
                .sessions
                .get_mut(&id)
                .expect("retained child");
            session.descriptor.model = Some(context.model);
            session.descriptor.thinking = Some(context.thinking);
            session.descriptor.reasoning_mode = context.reasoning_mode;
            session.harness = Some(harness);
            session.harness_task = Some(task);
            session.event_task = Some(event_task);
            session.evicted = false;

            let announce = std::mem::take(&mut session.announce).then(|| {
                let mut descriptor = session.descriptor.clone();
                descriptor.task = session.binding_task.clone();
                descriptor
            });
            drop(state);
            if let Some(descriptor) = announce {
                self.send(&root, AgentUpdate::Added(descriptor));
            }
            let _ = start.send(());
        }
        Ok(())
    }

    pub async fn send_message(
        self: &Arc<Self>,
        session_id: &str,
        to: AgentId,
        priority: MessagePriority,
        purpose: MessagePurpose,
        in_reply_to: Option<MessageId>,
        body: String,
    ) -> std::io::Result<MessageReceipt> {
        self.await_restored(session_id).await?;
        self.send_message_restored(session_id, to, priority, purpose, in_reply_to, body)
            .await
    }

    async fn send_message_restored(
        self: &Arc<Self>,
        session_id: &str,
        to: AgentId,
        priority: MessagePriority,
        purpose: MessagePurpose,
        in_reply_to: Option<MessageId>,
        body: String,
    ) -> std::io::Result<MessageReceipt> {
        self.deliver_message(session_id, to, priority, purpose, in_reply_to, body, false)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn deliver_message(
        self: &Arc<Self>,
        session_id: &str,
        to: AgentId,
        priority: MessagePriority,
        purpose: MessagePurpose,
        in_reply_to: Option<MessageId>,
        body: String,
        user: bool,
    ) -> std::io::Result<MessageReceipt> {
        let locks = self.session_locks(session_id).await;
        let _residency_guard = locks.residency.enter().await;
        let _message_guard = locks.message.enter().await;
        self.rehydrate(session_id, to, purpose).await?;
        let prepared = self.state.acquire().await.prepare_message(
            session_id,
            to,
            priority,
            purpose,
            in_reply_to,
            body,
            user,
        )?;
        let delivery = prepared
            .harness
            .enqueue_delivery(prepared.message.clone())?;
        self.state
            .acquire()
            .await
            .commit_message(&prepared.root_session_id, prepared.message.clone())?;
        if self.store_for(&prepared.root_session_id).is_some()
            && let Err(error) = self.persist_durably(&prepared.root_session_id).await
        {
            self.state
                .acquire()
                .await
                .rollback_message(&prepared.root_session_id, prepared.message.id);
            return Err(error);
        }
        let disposition = match delivery.release().await {
            Ok(disposition) => disposition,
            Err(error) => {
                self.state
                    .acquire()
                    .await
                    .rollback_message(&prepared.root_session_id, prepared.message.id);
                if self.store_for(&prepared.root_session_id).is_some()
                    && let Err(persist) = self.persist_durably(&prepared.root_session_id).await
                {
                    return Err(std::io::Error::new(
                        persist.kind(),
                        format!("{error}; the refused message stays journaled: {persist}"),
                    ));
                }
                return Err(error);
            }
        };
        if self.store_for(&prepared.root_session_id).is_some() {
            self.persist_durably(&prepared.root_session_id).await?;
        }
        Ok(MessageReceipt {
            message_id: prepared.message.id,
            thread_id: prepared.message.thread_id,
            from: prepared.message.from,
            to_agent_id: prepared.message.to,
            disposition,
        })
    }

    async fn pending_messages(&self, root_session_id: &str) -> Vec<AgentMessage> {
        self.state
            .acquire()
            .await
            .scopes
            .get(root_session_id)
            .map(|scope| scope.messages.pending_messages())
            .unwrap_or_default()
    }

    async fn redeliver_messages(
        self: &Arc<Self>,
        root_session_id: &str,
        messages: Vec<AgentMessage>,
    ) {
        if messages.is_empty() {
            return;
        }
        for message in messages {
            let id = message.id;
            if let Err(error) = self.redeliver_message(root_session_id, message).await {
                self.message_failed(
                    root_session_id,
                    id,
                    format!("message {id} could not be delivered after a runtime restart: {error}"),
                )
                .await;
            }
        }
        if self.store_for(root_session_id).is_some()
            && let Err(error) = self.persist_durably(root_session_id).await
        {
            tracing::warn!(%error, root_session_id, "redelivered subagent messages are not journaled yet");
        }
    }

    async fn redeliver_message(
        self: &Arc<Self>,
        root_session_id: &str,
        message: AgentMessage,
    ) -> std::io::Result<MessageDisposition> {
        let locks = self.session_locks(root_session_id).await;
        let _residency_guard = locks.residency.enter().await;
        let _message_guard = locks.message.enter().await;
        self.rehydrate(root_session_id, message.to, MessagePurpose::Coordinate)
            .await?;
        let harness = self
            .state
            .acquire()
            .await
            .harness_in_scope(root_session_id, message.to)?;
        harness.redeliver(message).await
    }

    pub(super) async fn message_admitted(
        &self,
        root_session_id: &str,
        id: MessageId,
        disposition: MessageDisposition,
    ) {
        let thread = {
            let mut state = self.state.acquire().await;
            let thread = state.thread_for_message(root_session_id, id);
            state.mark_message_admitted(root_session_id, id, disposition);
            thread
        };
        let Some(thread) = thread else {
            return;
        };
        self.send(
            root_session_id,
            AgentUpdate::Message(AgentMessageUpdate {
                message_id: id,
                thread,
                delivery: MessageDeliveryState::Admitted { disposition },
            }),
        );
        self.changed();
    }

    pub(super) async fn message_rejected(&self, root_session_id: &str, id: MessageId) {
        self.state
            .acquire()
            .await
            .rollback_message(root_session_id, id);
    }

    pub(super) async fn message_delivered(
        &self,
        root_session_id: &str,
        id: MessageId,
        disposition: MessageDisposition,
    ) {
        self.publish_message_state(
            root_session_id,
            id,
            MessageDeliveryState::Delivered { disposition },
        )
        .await;
    }

    pub(super) async fn begin_message_delivery(
        &self,
        root_session_id: &str,
        id: MessageId,
    ) -> Option<DelegationChange> {
        let (change, descriptor) = self
            .state
            .acquire()
            .await
            .begin_delegation(root_session_id, id)?;
        self.send(root_session_id, AgentUpdate::Added(descriptor));
        self.changed();
        Some(change)
    }

    pub(super) async fn rollback_message_delivery(
        &self,
        root_session_id: &str,
        change: DelegationChange,
    ) {
        let descriptor = self
            .state
            .acquire()
            .await
            .rollback_delegation(root_session_id, change);
        if let Some(descriptor) = descriptor {
            self.send(root_session_id, AgentUpdate::Added(descriptor));
            self.changed();
        }
    }

    pub(super) async fn message_failed(&self, root_session_id: &str, id: MessageId, error: String) {
        self.publish_message_state(root_session_id, id, MessageDeliveryState::Failed { error })
            .await;
    }

    async fn publish_message_state(
        &self,
        root_session_id: &str,
        message_id: MessageId,
        delivery: MessageDeliveryState,
    ) {
        let thread = self
            .state
            .acquire()
            .await
            .thread_for_message(root_session_id, message_id);
        let Some(thread) = thread else {
            return;
        };
        self.send(
            root_session_id,
            AgentUpdate::Message(AgentMessageUpdate {
                message_id,
                thread,
                delivery,
            }),
        );
        self.changed();
        self.state
            .acquire()
            .await
            .mark_message_terminal(root_session_id, message_id);
    }

    pub async fn wait(
        self: &Arc<Self>,
        session_id: &str,
        call_id: &str,
        ids: &[AgentId],
        duration: Duration,
    ) -> std::io::Result<AgentWait> {
        self.wait_inner(session_id, call_id, ids, duration)
            .instrument(tracing::info_span!(
                "subagent.wait",
                session.id = session_id,
                agent.count = ids.len()
            ))
            .await
    }

    async fn wait_inner(
        self: &Arc<Self>,
        session_id: &str,
        call_id: &str,
        ids: &[AgentId],
        duration: Duration,
    ) -> std::io::Result<AgentWait> {
        self.await_restored(session_id).await?;
        if ids.is_empty() {
            return Err(std::io::Error::other("agent_ids must not be empty"));
        }
        let mut revision = self.revision.subscribe();

        let deadline = Instant::now().checked_add(duration);
        loop {
            let (snapshot, root) = {
                let state = self.state.acquire().await;
                let root = state.root_session_id(session_id).to_owned();
                (state.wait_snapshot(session_id, ids)?, root)
            };

            let resuming = self
                .pending_resume
                .read(|pending| pending.get(&root).cloned())
                .unwrap_or_default();
            let terminal = snapshot
                .iter()
                .filter(|(summary, _)| {
                    summary.status.is_wait_terminal()
                        && !(matches!(summary.status, AgentStatus::Interrupted)
                            && resuming.contains(&summary.agent_id))
                })
                .map(|(summary, revision)| {
                    (summary.agent_id, wait_mark(&summary.status, *revision))
                })
                .collect::<Vec<_>>();
            let all_terminal = terminal.len() == snapshot.len();
            let summaries = snapshot.into_iter().map(|(summary, _)| summary).collect();

            let reported = |reported: &HashMap<(String, AgentId), u64>| {
                terminal
                    .iter()
                    .all(|&(id, mark)| reported.get(&(session_id.to_owned(), id)) == Some(&mark))
            };
            let (fresh, delivered) = if self.wait_reported.read(reported) {
                (false, terminal.iter().map(|&(id, _)| id).collect())
            } else {
                self.wait_reported.update(|reported| {
                    let mut fresh = false;
                    let mut delivered = HashSet::new();
                    for &(id, mark) in &terminal {
                        if reported.insert((session_id.to_owned(), id), mark) == Some(mark) {
                            delivered.insert(id);
                        } else {
                            fresh = true;
                        }
                    }
                    (fresh, delivered)
                })
            };
            if fresh {
                self.forget_idle_wait(session_id);
                let receipt = if root == session_id {
                    let ids = terminal.iter().map(|(id, _)| *id).collect::<Vec<_>>();
                    self.observe_completions(&root, &ids, call_id).await
                } else {
                    None
                };
                return Ok((summaries, delivered, false, receipt));
            }
            if all_terminal {
                let mut key = ids.to_vec();
                key.sort_unstable();
                key.dedup();
                let exhausted = self.idle_waits.update(|idle| {
                    let entry = idle
                        .entry(session_id.to_owned())
                        .or_insert_with(|| (key.clone(), 0));
                    if entry.0 != key {
                        *entry = (key.clone(), 0);
                    }
                    entry.1 += 1;
                    (entry.1 > IDLE_WAIT_LIMIT)
                        .then(|| entry.0.iter().map(ToString::to_string).collect::<Vec<_>>())
                });
                if let Some(ids) = exhausted {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        format!(
                            "agent_ids [{}] are all terminal and every result was already returned by an earlier wait_agent call; nothing is left to wait for. Stop waiting: read each agents[i].status.state from that result, or call list_agents({{include_completed:true}}).",
                            ids.join(", ")
                        ),
                    ));
                }
                return Ok((summaries, delivered, false, None));
            }
            self.forget_idle_wait(session_id);
            let timed_out = match deadline {
                Some(deadline) => timeout_at(deadline, revision.changed()).await.is_err(),
                None => {
                    let _ = revision.changed().await;
                    false
                }
            };
            if timed_out {
                let summaries = self.state.acquire().await.summaries(session_id, ids)?;
                return Ok((summaries, delivered, true, None));
            }
        }
    }

    fn forget_idle_wait(&self, session_id: &str) {
        if self.idle_waits.read(|idle| idle.contains_key(session_id)) {
            self.idle_waits.update(|idle| {
                idle.remove(session_id);
            });
        }
    }

    pub async fn interrupt(
        &self,
        session_id: &str,
        id: AgentId,
    ) -> std::io::Result<Vec<AgentSummary>> {
        self.await_restored(session_id).await?;
        let locks = self.session_locks(session_id).await;
        let _message_guard = locks.message.enter().await;
        let (root_session_id, ids, harnesses) = {
            let mut state = self.state.acquire().await;
            state.request_interrupt(session_id, id)?
        };
        self.changed();
        let deadline = Instant::now() + AGENT_STOP_TIMEOUT;
        self.interrupt_harnesses(&root_session_id, &ids, harnesses, deadline)
            .await?;
        self.state
            .acquire()
            .await
            .summaries_in_scope(&root_session_id, &ids)
    }

    pub async fn close(&self, session_id: &str, id: AgentId) -> std::io::Result<Vec<AgentSummary>> {
        self.await_restored(session_id).await?;
        let locks = self.session_locks(session_id).await;
        let _message_guard = locks.message.enter().await;
        let CloseRequest {
            root_session_id,
            ids,
            harnesses,
            status_updates,
        } = {
            let mut state = self.state.acquire().await;
            state.request_close(session_id, id)?
        };
        for (id, status) in status_updates {
            self.send_status(&root_session_id, AgentUpdate::Status { id, status });
        }
        self.changed();
        let summaries = self
            .stop_and_close(root_session_id.clone(), ids, harnesses, false)
            .await?;
        let frozen = self
            .state
            .acquire()
            .await
            .scopes
            .get(&root_session_id)
            .is_some_and(|scope| scope.journal_frozen);
        if self.store_for(&root_session_id).is_some() && !frozen {
            self.persist_durably(&root_session_id)
                .await
                .map_err(|error| {
                    std::io::Error::new(
                        error.kind(),
                        format!("agent {id} closed, but its closure is not durable yet: {error}"),
                    )
                })?;
        }
        Ok(summaries)
    }

    async fn close_batch(&self, root_session_id: &str, ids: Vec<AgentId>) -> std::io::Result<()> {
        let mut errors = Vec::new();
        for id in ids.into_iter().rev() {
            if let Err(error) = self.close(root_session_id, id).await {
                errors.push(format!("subagent {id}: {error}"));
            }
        }
        if self.store_for(root_session_id).is_some()
            && let Err(error) = self.persist_durably(root_session_id).await
        {
            errors.push(format!("persisting startup cleanup: {error}"));
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(std::io::Error::other(errors.join("; ")))
        }
    }

    async fn close_all(&self, session_id: &str) -> std::io::Result<Vec<AgentSummary>> {
        self.await_restored(session_id).await?;
        let locks = self.session_locks(session_id).await;
        let _message_guard = locks.message.enter().await;
        let mut revision = self.revision.subscribe();
        let capture_deadline = Instant::now() + AGENT_STOP_TIMEOUT;
        let (request, final_journal) = loop {
            let mut state = self.state.acquire().await;
            let root = state.root_session_id(session_id).to_owned();
            let captures_pending = self
                .pending_checkpoints
                .read(|pending| pending.contains_key(&root));
            if captures_pending {
                drop(state);

                match timeout_at(capture_deadline, revision.changed()).await {
                    Ok(Ok(())) => continue,
                    Ok(Err(_)) => {
                        return Err(std::io::Error::other("subagent runtime is closed"));
                    }
                    Err(_) => {
                        tracing::warn!(%root, "subagent checkpoint capture timed out during shutdown");
                        self.pending_checkpoints.update(|pending| {
                            pending.remove(&root);
                        });
                        continue;
                    }
                }
            }

            let frozen = state
                .scopes
                .get(&root)
                .is_some_and(|scope| scope.journal_frozen);
            let final_journal = match self.store_for(&root) {
                Some(_) if !frozen => {
                    let checkpoints = self.checkpoints.get();
                    let scope = state.scope_mut(&root);
                    let payload = scope.journal_payload(&root, &checkpoints)?;
                    scope.journal_frozen = true;
                    Some(payload)
                }
                _ => None,
            };

            break (state.request_close_all(session_id)?, final_journal);
        };
        let CloseRequest {
            root_session_id,
            ids,
            harnesses,
            status_updates,
        } = request;
        let journal_result = match final_journal {
            Some(write) => self.final_journal(&root_session_id, write).await,
            None => Ok(()),
        };

        for (id, status) in status_updates {
            self.send_status(&root_session_id, AgentUpdate::Status { id, status });
        }
        self.changed();
        let root = root_session_id.clone();
        let result = self
            .stop_and_close(root_session_id, ids, harnesses, true)
            .await;
        if result.is_ok() {
            self.session_handles.update(|handles| {
                handles.remove(&root);
            });
        }
        let summaries = result?;
        journal_result?;
        Ok(summaries)
    }

    async fn stop_and_close(
        &self,
        root_session_id: String,
        ids: Vec<AgentId>,
        harnesses: Vec<HarnessHandle>,
        detach: bool,
    ) -> std::io::Result<Vec<AgentSummary>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let deadline = Instant::now() + AGENT_STOP_TIMEOUT;
        let closing_result = self.close_harnesses(harnesses, deadline).await;
        self.wait_until_inactive(&root_session_id, &ids, deadline)
            .await?;

        drop(closing_result);
        let ClosedSessions {
            summaries,
            harness_tasks,
            event_tasks,
        } = self
            .state
            .acquire()
            .await
            .finish_close(&root_session_id, &ids)?;
        for summary in &summaries {
            let _ = send_update(
                &self.updates,
                &root_session_id,
                AgentUpdate::Status {
                    id: summary.agent_id,
                    status: AgentStatus::Closed,
                },
                detach,
            );
        }
        let closed_sessions = {
            let state = self.state.acquire().await;
            state
                .scopes
                .get(&root_session_id)
                .map(|scope| {
                    ids.iter()
                        .filter_map(|id| {
                            scope
                                .sessions
                                .get(id)
                                .map(|session| session.descriptor.session_id.clone())
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        };
        self.session_handles.update(|handles| {
            for session in &closed_sessions {
                handles.remove(session);
            }
        });
        self.changed();
        self.wait_for_tasks(harness_tasks, deadline, "subagent harnesses")
            .await?;
        self.wait_for_tasks(event_tasks, deadline, "subagent event streams")
            .await?;
        Ok(summaries)
    }

    async fn cancel_all(&self, session_id: &str) {
        if self.await_restored(session_id).await.is_err() {
            return;
        }
        let locks = self.session_locks(session_id).await;
        let _message_guard = locks.message.enter().await;
        let (root_session_id, ids, harnesses) = {
            let mut state = self.state.acquire().await;
            state.request_interrupt_all(session_id)
        };
        self.changed();
        let deadline = Instant::now() + AGENT_STOP_TIMEOUT;
        drop(
            self.interrupt_harnesses(&root_session_id, &ids, harnesses, deadline)
                .await,
        );
    }

    async fn interrupt_harnesses(
        &self,
        root_session_id: &str,
        ids: &[AgentId],
        harnesses: Vec<HarnessHandle>,
        deadline: Instant,
    ) -> std::io::Result<()> {
        let interruption = async move {
            let results = join_all(
                harnesses
                    .into_iter()
                    .map(|harness| async move { harness.interrupt().await }),
            )
            .await;
            first_error(results)
        };
        let interruption_result = timeout_at(deadline, interruption).await.map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "timed out interrupting subagent harnesses",
            )
        })?;
        self.wait_until_inactive(root_session_id, ids, deadline)
            .await?;
        drop(interruption_result);
        Ok(())
    }

    async fn close_harnesses(
        &self,
        harnesses: Vec<HarnessHandle>,
        deadline: Instant,
    ) -> std::io::Result<()> {
        let closing = async move {
            let results = join_all(
                harnesses
                    .into_iter()
                    .map(|harness| async move { harness.close().await }),
            )
            .await;
            first_error(results)
        };
        timeout_at(deadline, closing).await.map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "timed out closing subagent harnesses",
            )
        })?
    }

    async fn wait_for_tasks(
        &self,
        mut tasks: Vec<Task<()>>,
        deadline: Instant,
        description: &str,
    ) -> std::io::Result<()> {
        if tasks.is_empty() {
            return Ok(());
        }
        let completion = join_all(tasks.iter_mut());
        match timeout_at(deadline, completion).await {
            Ok(results) => results
                .into_iter()
                .find_map(Result::err)
                .map_or(Ok(()), |error| {
                    Err(std::io::Error::other(format!(
                        "{description} failed during shutdown: {error}"
                    )))
                }),
            Err(_) => {
                for task in tasks {
                    task.abort();
                }
                Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    format!("timed out waiting for {description} to close"),
                ))
            }
        }
    }

    async fn wait_until_inactive(
        &self,
        root_session_id: &str,
        ids: &[AgentId],
        deadline: Instant,
    ) -> std::io::Result<()> {
        let mut revision = self.revision.subscribe();
        loop {
            if self
                .state
                .acquire()
                .await
                .all_inactive(root_session_id, ids)?
            {
                return Ok(());
            }
            timeout_at(deadline, revision.changed())
                .await
                .map_err(|_| {
                    std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "timed out waiting for subagent turns to stop",
                    )
                })?
                .map_err(|_| std::io::Error::other("subagent runtime is closed"))?;
        }
    }

    fn changed(&self) {
        self.revision.send_modify(|revision| {
            *revision = revision.wrapping_add(1);
        });
    }
}

fn complete_session(session: &mut ChildSession, output: Option<Value>) -> AgentStatus {
    let Some(output) = output else {
        return AgentStatus::Failed {
            error: CompletionError::new(
                CompletionErrorCode::MissingResult,
                false,
                "subagent turn ended without a valid submit_result call",
                "Inspect child evidence before assigning recovery. A plain final answer is not an accepted result; do not replay task side effects.",
            ).to_string(),
        };
    };
    session.last_output = Some(output.clone());
    AgentStatus::Completed { output }
}

fn validate_submitted_output(
    validator: &Validator,
    schema: &Value,
    output: Value,
) -> Result<(Value, bool), CompletionError> {
    if validator.is_valid(&output) {
        return Ok((output, false));
    }
    if let Value::String(text) = &output
        && let Ok(decoded) = serde_json::from_str::<Value>(text)
        && matches!(decoded, Value::Object(_) | Value::Array(_))
        && validator.is_valid(&decoded)
    {
        return Ok((decoded, true));
    }
    let mut errors = validator
        .iter_errors(&output)
        .take(4)
        .map(|error| {
            format!(
                "instance {}: schema {}",
                error.instance_path(),
                error.schema_path()
            )
        })
        .collect::<Vec<_>>();
    errors.push(format!("required schema: {schema}"));
    Err(CompletionError::new(
        CompletionErrorCode::SchemaValidation,
        true,
        "submitted output does not match the required schema",
        "Correct output using the required schema and retry submit_result within this turn; do not repeat task side effects.",
    ).with_details(errors))
}

fn first_error(results: Vec<std::io::Result<()>>) -> std::io::Result<()> {
    results.into_iter().find(Result::is_err).unwrap_or(Ok(()))
}

pub(crate) fn bounded_summary(value: &str) -> String {
    const MAX_BYTES: usize = 160;
    if value.len() <= MAX_BYTES {
        return value.to_owned();
    }
    let end = value
        .char_indices()
        .map(|(index, _)| index)
        .take_while(|index| *index <= MAX_BYTES)
        .last()
        .unwrap_or_default();
    value[..end].to_owned()
}

impl ChildSession {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn restored(
        descriptor: AgentDescriptor,
        host_context: Option<Arc<str>>,
        status: AgentStatus,
        contract: OutputContract,
        output_schema: Value,
        stored_runtime: Option<SessionCheckpoint>,
        next_instruction_revision: u64,
        last_output: Option<Value>,
    ) -> Self {
        Self {
            binding_task: descriptor.task.clone(),
            descriptor,
            host_context,
            event_task: None,
            harness: None,
            harness_task: None,
            status,
            active: false,
            output_validator: contract.validator,
            output_schema,
            stored_runtime,
            journaled_runtime: false,
            next_instruction_revision,
            active_instruction_revision: None,
            steering: false,
            submitted_output: None,
            last_output,
            last_used: 0,
            evicted: true,
            resume_attempts: 0,
            in_flight_calls: Vec::new(),
            in_flight_omitted: 0,
            pending_completion: None,
            completion_attempt: 0,
            completion_parked: false,
            cancelled_revision: None,
            in_spawning_turn: false,
            settled_revision: None,
            usage: AgentUsage::default(),
            in_flight_progress: durable::OmittedProgress::default(),
            announce: true,
            journal_generation: 0,
        }
    }

    pub(super) fn settle_completion(&mut self, revision: u64) {
        self.pending_completion = Some(revision);
        self.completion_attempt = 0;
        self.completion_parked = self
            .cancelled_revision
            .is_some_and(|cancelled| revision <= cancelled);
    }

    const fn clear_completion(&mut self) {
        self.pending_completion = None;
        self.completion_attempt = 0;
        self.completion_parked = false;
    }

    pub(super) fn summary(&self) -> AgentSummary {
        let last_output = if matches!(self.status, AgentStatus::Completed { .. }) {
            None
        } else {
            self.last_output.clone()
        };
        AgentSummary {
            agent_id: self.descriptor.id,
            role: self.descriptor.role.clone(),
            task: self.descriptor.task.clone(),
            parent_agent_id: self.descriptor.parent,
            status: self.status.clone(),
            model: self.descriptor.model.clone(),
            thinking: self.descriptor.thinking,
            reasoning_mode: self.descriptor.reasoning_mode,
            last_output,
            completion_revision: self.completion_revision(),
            usage: self.usage.clone(),
        }
    }

    pub(super) fn completion_revision(&self) -> Option<u64> {
        matches!(self.status, AgentStatus::Completed { .. })
            .then_some(self.settled_revision)
            .flatten()
    }
}

#[derive(Clone)]
pub struct SubagentControl {
    pub(super) registry: Arc<Registry>,
}

impl SubagentControl {
    pub async fn completion_origin(&self, request_id: &str) -> Option<Arc<str>> {
        let (session_id, _) = request_id
            .strip_prefix(crate::COMPLETION_REQUEST_PREFIX)?
            .split_once(':')?;
        self.registry.host_context_for_session(session_id).await
    }

    pub fn set_max_concurrency(&self, limit: usize) {
        self.registry.set_max_concurrency(limit);
    }

    pub fn set_max_resident(&self, limit: usize) {
        self.registry.set_max_resident(limit);
    }

    pub async fn cancel_all(&self, root_session_id: &str) {
        self.registry.cancel_all(root_session_id).await;
    }

    pub async fn close_all(&self, root_session_id: &str) -> std::io::Result<()> {
        self.registry.close_all(root_session_id).await.map(drop)
    }

    pub fn runtime_id(&self) -> SubagentRuntimeId {
        self.registry.id
    }
}

pub(super) fn forward_events(
    root_session_id: String,
    id: AgentId,
    mut events: AgentEvents,
    start: oneshot::Receiver<()>,
    registry: Weak<Registry>,
    updates: mpsc::UnboundedSender<ScopedAgentUpdate>,
) -> Task<()> {
    platform::spawn(async move {
        if start.await.is_err() {
            return;
        }
        while let Some(event) = events.recv().await {
            let boundary = matches!(
                event.kind,
                AgentEventKind::ModelCallStarted | AgentEventKind::ModelCallCompleted
            );
            let progress = boundary || event.kind == AgentEventKind::ToolCall;
            let completed_tool = event.kind == AgentEventKind::ToolResult;
            let kind = event.kind;
            let payload = matches!(kind, AgentEventKind::ToolCall | AgentEventKind::ToolResult)
                .then(|| event.payload.clone());
            let _ = send_update(
                &updates,
                &root_session_id,
                AgentUpdate::Event { id, event },
                false,
            );
            if let Some(registry) = registry.upgrade() {
                registry
                    .track_in_flight(&root_session_id, id, &kind, &payload)
                    .await;
                if boundary {
                    registry.commit_boundary(&root_session_id, id).await;
                }
                if completed_tool {
                    registry.completed_tools.update(|completed| {
                        completed.insert((root_session_id.clone(), id));
                    });
                }
                if progress {
                    registry.capture_progress(&root_session_id, id);
                }
            }
        }
        if let Some(registry) = registry.upgrade() {
            registry.runtime_closed(&root_session_id, id).await;
        }
    })
}

fn parent_call<'a>(call_id: &'a str, parent_call_id: Option<&'a str>) -> Option<&'a str> {
    parent_call_id
        .filter(|parent| !parent.is_empty())
        .or_else(|| call_id.split_once("/code-").map(|(parent, _)| parent))
}

fn send_update(
    updates: &mpsc::UnboundedSender<ScopedAgentUpdate>,
    root_session_id: &str,
    update: AgentUpdate,
    detach: bool,
) -> bool {
    updates
        .send(ScopedAgentUpdate {
            detach,
            root_session_id: root_session_id.to_owned(),
            update,
        })
        .is_ok()
}

pub fn channel(
    max_concurrency: usize,
) -> (
    Arc<Registry>,
    SubagentControl,
    mpsc::UnboundedReceiver<ScopedAgentUpdate>,
) {
    let (updates, receiver) = mpsc::unbounded_channel();
    let registry = Arc::new(Registry::new(updates, max_concurrency));
    let _ = registry.this.set(Arc::downgrade(&registry));
    let control = SubagentControl {
        registry: Arc::clone(&registry),
    };
    (registry, control, receiver)
}
