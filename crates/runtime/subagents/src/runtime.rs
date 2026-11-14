use super::diagnostics::{CompletionError, CompletionErrorCode};
use super::durable::{self, RestoreReport, SubagentStore};
use super::{
    capacity::{Capacity, TurnCapacity, Waiter},
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
use nanocode_agent_core::usage::{UsdAmount, usd_number};
use nanocode_agent_core::{
    Agent, AgentEvents, AgentHandle, AgentUsage, ExecutionPolicyDisposition, ModelSelection,
    NanocodeError, PromptRoute, ReasoningMode, Result as AgentResult, SessionCheckpoint,
    SteerReceiptStatus, Thinking, TurnResult, WeakAgent,
    events::{AgentEventKind, RunStatus},
};
use nanocode_durability_core::StoreError;
use nanocode_platform::Instant;
use nanocode_schema::Validator;
use nanocode_sync::{Atom, Sequencer, Turn};
use nanocode_tool_contracts::ToolReceipt;
use nanocode_trace::Instrument;
use serde::Serialize;
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque},
    sync::{
        Arc, Weak,
        atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::sync::{mpsc, oneshot, watch};

mod activity;
mod completion;
mod focus;

pub use activity::{Restoration, SubagentActivity};
pub use completion::{RunningSubagents, SubagentCompletion};

type AgentMap<V> = nanocode_sync::Map<(String, AgentId), V>;

type MissingRuntime = (
    AgentId,
    Option<AgentId>,
    String,
    Option<SessionCheckpoint>,
    Option<Arc<str>>,
    Arc<Value>,
    String,
    Option<UsdAmount>,
    bool,
);

pub(super) enum Residency {
    Detached,
    Resident(HarnessHandle),
    Evicted,
}

#[derive(Clone)]
pub(super) enum StoredRuntime {
    None,
    Checkpoint(Box<SessionCheckpoint>),
    Journaled,
    Unstarted(Arc<durable::Unstarted>),
}

impl StoredRuntime {
    pub(super) const fn recoverable(&self) -> bool {
        !matches!(self, Self::None)
    }

    pub(super) const fn checkpoint(&self) -> Option<&SessionCheckpoint> {
        match self {
            Self::Checkpoint(checkpoint) => Some(checkpoint),
            Self::None | Self::Journaled | Self::Unstarted(_) => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Completion {
    #[default]
    None,
    Pending {
        revision: u64,
        attempt: u32,
    },
    Parked {
        revision: u64,
        attempt: u32,
    },
}

impl Completion {
    pub(super) const fn journaled(revision: Option<u64>, attempt: u32, parked: bool) -> Self {
        match (revision, parked) {
            (None, _) => Self::None,
            (Some(revision), false) => Self::Pending { revision, attempt },
            (Some(revision), true) => Self::Parked { revision, attempt },
        }
    }

    pub(super) const fn revision(self) -> Option<u64> {
        match self {
            Self::None => None,
            Self::Pending { revision, .. } | Self::Parked { revision, .. } => Some(revision),
        }
    }

    pub(super) const fn attempt(self) -> u32 {
        match self {
            Self::None => 0,
            Self::Pending { attempt, .. } | Self::Parked { attempt, .. } => attempt,
        }
    }

    pub(super) const fn parked(self) -> bool {
        matches!(self, Self::Parked { .. })
    }

    pub(super) const fn awaits(self, revision: u64) -> bool {
        matches!(self.revision(), Some(pending) if pending == revision)
    }

    const fn next_attempt(&mut self) {
        match self {
            Self::None => {}
            Self::Pending { attempt, .. } | Self::Parked { attempt, .. } => {
                *attempt = attempt.saturating_add(1);
            }
        }
    }

    const fn park(&mut self) {
        if let Self::Pending { revision, attempt } = *self {
            *self = Self::Parked { revision, attempt };
        }
    }

    const fn unpark(&mut self) -> Option<u64> {
        match *self {
            Self::Parked { revision, attempt } => {
                *self = Self::Pending {
                    revision,
                    attempt: attempt.saturating_add(1),
                };
                Some(revision)
            }
            _ => None,
        }
    }
}

pub(super) struct UnstartedChild {
    pub(super) lock: Sequencer,
    pub(super) descriptor: AgentDescriptor,
    pub(super) parent_session: String,
    pub(super) host_context: Option<Arc<str>>,
    pub(super) output_schema: Arc<Value>,
    pub(super) unstarted: Arc<durable::Unstarted>,
}

pub(super) struct ChildSession {
    pub(super) descriptor: AgentDescriptor,

    pub(super) binding_task: String,
    pub(super) host_context: Option<Arc<str>>,
    pub(super) event_task: Option<Task<()>>,
    pub(super) residency: Residency,
    pub(super) harness_task: Option<Task<()>>,
    pub(super) status: AgentStatus,
    pub(super) turn: Option<ActiveTurn>,
    pub(super) output_validator: Arc<Validator>,
    pub(super) output_schema: Arc<Value>,
    pub(super) stored_runtime: StoredRuntime,
    pub(super) next_instruction_revision: u64,
    pub(super) last_output: Option<Value>,
    pub(super) follow_up_output: Option<Value>,
    pub(super) delegated_turn: bool,
    pub(super) last_used: u64,

    pub(super) resume_attempts: u32,

    pub(super) in_flight_calls: Vec<durable::InFlightCall>,

    pub(super) in_flight_omitted: u32,

    pub(super) completion: Completion,

    pub(super) cancelled_revision: Option<u64>,

    pub(super) in_spawning_turn: bool,

    pub(super) settled_revision: Option<u64>,

    pub(super) usage: AgentUsage,

    pub(super) in_flight_progress: durable::OmittedProgress,

    pub(super) hibernated: Option<Hibernated>,

    pub(super) hibernations: u64,

    residency_lock: Sequencer,

    marks: Arc<AgentMarks>,

    changes: Option<watch::Sender<u64>>,

    announce: bool,

    journal_generation: u64,

    pub(super) conversation: bool,

    pub(super) waited: Vec<durable::Waited>,

    pub(super) sent: Vec<durable::Sent>,

    pub(super) spawned: Option<durable::Spawned>,
}

pub(super) struct ActiveTurn {
    pub(super) revision: u64,
    pub(super) stage: TurnStage,
    pub(super) interrupted: bool,
}

pub(super) enum TurnStage {
    Working,
    Steering,
    Submitted(Value),
}

static JOURNAL_GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

fn next_generation() -> u64 {
    JOURNAL_GENERATION.fetch_add(1, Ordering::Relaxed)
}

#[derive(Default)]
pub(super) struct Sessions {
    sessions: HashMap<AgentId, Box<ChildSession>>,
    changed: HashSet<AgentId>,
    stale: HashSet<AgentId>,
    resident: HashSet<AgentId>,
    idle: BTreeSet<(u64, AgentId)>,
    idle_at: HashMap<AgentId, u64>,
    parked: HashSet<AgentId>,
    active_direct: HashSet<AgentId>,
    spawning: HashSet<AgentId>,
    unsettled: HashSet<AgentId>,
    by_context: HashMap<Arc<str>, HashSet<AgentId>>,
    by_spawn: HashMap<durable::Spawned, AgentId>,
}

impl Sessions {
    pub(super) fn get(&self, id: &AgentId) -> Option<&ChildSession> {
        self.sessions.get(id).map(Box::as_ref)
    }

    pub(super) fn get_mut(&mut self, id: &AgentId) -> Option<&mut ChildSession> {
        let session = self.sessions.get_mut(id)?;
        self.stale.insert(*id);
        session.journal_generation = next_generation();
        if let Some(changes) = &session.changes {
            if changes.receiver_count() > 0 {
                changes.send_modify(|version| *version = version.wrapping_add(1));
            } else {
                session.changes = None;
            }
        }
        self.changed.insert(*id);
        Some(session)
    }

    fn subscribe(&mut self, id: &AgentId) -> Option<watch::Receiver<u64>> {
        let session = self.sessions.get_mut(id)?;
        Some(
            session
                .changes
                .get_or_insert_with(|| watch::Sender::new(0))
                .subscribe(),
        )
    }

    fn insert(&mut self, id: AgentId, mut session: ChildSession) {
        session.journal_generation = next_generation();
        self.changed.insert(id);
        self.stale.insert(id);
        if session.descriptor.parent.is_none()
            && let Some(context) = &session.host_context
        {
            self.by_context
                .entry(Arc::clone(context))
                .or_default()
                .insert(id);
        }
        if let Some(spawned) = &session.spawned {
            self.by_spawn.insert(spawned.clone(), id);
        }
        self.sessions.insert(id, Box::new(session));
    }

    fn spawned_by(&self, spawned: &durable::Spawned) -> Option<AgentId> {
        self.by_spawn.get(spawned).copied()
    }

    fn remove(&mut self, id: &AgentId) -> Option<Box<ChildSession>> {
        let session = self.sessions.remove(id)?;
        self.changed.remove(id);
        self.stale.remove(id);
        self.resident.remove(id);
        if let Some(at) = self.idle_at.remove(id) {
            self.idle.remove(&(at, *id));
        }
        self.parked.remove(id);
        self.active_direct.remove(id);
        self.spawning.remove(id);
        self.unsettled.remove(id);
        if let Some(context) = &session.host_context
            && let Some(children) = self.by_context.get_mut(context)
        {
            children.remove(id);
            if children.is_empty() {
                self.by_context.remove(context);
            }
        }
        Some(session)
    }

    fn insert_stored(&mut self, id: AgentId, session: ChildSession) -> u64 {
        self.insert(id, session);
        self.changed.remove(&id);
        self.sessions
            .get(&id)
            .map_or(0, |session| session.journal_generation)
    }

    fn take_changed(&mut self) -> HashSet<AgentId> {
        std::mem::take(&mut self.changed)
    }

    fn settled(&self, leaf: u64) -> bool {
        durable::leaf_ids(leaf).all(|id| self.get(&id).is_some_and(ChildSession::settled))
    }

    fn refresh(&mut self) {
        for id in std::mem::take(&mut self.stale) {
            let Some(session) = self.sessions.get(&id) else {
                continue;
            };
            if session.completion.parked() {
                self.parked.insert(id);
            } else {
                self.parked.remove(&id);
            }
            if session.descriptor.parent.is_none() && session.status.is_active() {
                self.active_direct.insert(id);
            } else {
                self.active_direct.remove(&id);
            }
            if session.descriptor.parent.is_none() && session.in_spawning_turn {
                self.spawning.insert(id);
            } else {
                self.spawning.remove(&id);
            }
            if session.unsettled() {
                self.unsettled.insert(id);
            } else {
                self.unsettled.remove(&id);
            }
            let resident = session.harness().is_some();
            if resident {
                self.resident.insert(id);
            } else {
                self.resident.remove(&id);
            }
            let idle = resident
                && !session.active()
                && !matches!(session.status, AgentStatus::Pending)
                && session.status.can_start_turn();
            let at = idle.then_some(session.last_used);
            let previous = match at {
                Some(at) => self.idle_at.insert(id, at),
                None => self.idle_at.remove(&id),
            };
            if previous != at {
                if let Some(previous) = previous {
                    self.idle.remove(&(previous, id));
                }
                if let Some(at) = at {
                    self.idle.insert((at, id));
                }
            }
        }
    }

    fn any_parked(&mut self) -> bool {
        self.refresh();
        !self.parked.is_empty()
    }

    fn parked_ids(&mut self) -> Vec<AgentId> {
        self.refresh();
        let mut ids = self.parked.iter().copied().collect::<Vec<_>>();
        ids.sort_unstable();
        ids
    }

    fn spawned_in(&mut self, contexts: &[&str]) -> Vec<AgentId> {
        self.refresh();
        let mut ids = self.spawning.iter().copied().collect::<HashSet<_>>();
        for context in contexts {
            ids.extend(self.by_context.get(*context).into_iter().flatten().copied());
        }
        ids.into_iter().collect()
    }

    fn holds_root_watch(&mut self) -> bool {
        self.refresh();
        !self.parked.is_empty() || !self.active_direct.is_empty()
    }

    fn quiescent(&mut self) -> bool {
        self.refresh();
        self.unsettled.is_empty()
    }

    pub(super) fn keys(&self) -> impl Iterator<Item = &AgentId> {
        self.sessions.keys()
    }

    pub(super) fn iter(&self) -> impl Iterator<Item = (AgentId, &ChildSession)> {
        self.sessions
            .iter()
            .map(|(id, session)| (*id, session.as_ref()))
    }

    pub(super) fn values(&self) -> impl Iterator<Item = &ChildSession> {
        self.sessions.values().map(Box::as_ref)
    }

    fn is_empty(&self) -> bool {
        self.sessions.is_empty()
    }
}

impl std::ops::Index<&AgentId> for Sessions {
    type Output = ChildSession;

    fn index(&self, id: &AgentId) -> &ChildSession {
        &self.sessions[id]
    }
}

pub(super) struct OutputContract {
    validator: Arc<Validator>,
    schema: String,
    value: Arc<Value>,
}

type CompiledContract = (std::sync::Weak<Validator>, std::sync::Weak<Value>);

impl OutputContract {
    pub(super) fn into_schema(self) -> String {
        self.schema
    }

    pub(super) fn compile(schema: &Value) -> std::io::Result<Self> {
        static COMPILED: std::sync::OnceLock<Atom<HashMap<String, CompiledContract>>> =
            std::sync::OnceLock::new();
        let rendered = serde_json::to_string(schema)
            .map_err(|error| std::io::Error::other(format!("could not render schema: {error}")))?;
        let compiled = COMPILED.get_or_init(Atom::default);
        if let Some((validator, value)) = compiled.read(|compiled| {
            let (validator, value) = compiled.get(&rendered)?;
            Some((validator.upgrade()?, value.upgrade()?))
        }) {
            return Ok(Self {
                validator,
                schema: rendered,
                value,
            });
        }
        let validator =
            Arc::new(nanocode_schema::validator_for(schema).map_err(|error| {
                std::io::Error::other(format!("invalid output_schema: {error}"))
            })?);
        let value = Arc::new(schema.clone());
        compiled.update(|compiled| {
            compiled.retain(|_, (validator, value)| {
                validator.strong_count() > 0 && value.strong_count() > 0
            });
            compiled.insert(
                rendered.clone(),
                (Arc::downgrade(&validator), Arc::downgrade(&value)),
            );
        });
        Ok(Self {
            validator,
            schema: rendered,
            value,
        })
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

pub(super) fn follow_up_reminder() -> String {
    format!(
        "{COMPLETION_INSTRUCTIONS_LEAD} again only if this changes your result; otherwise your \
         accepted result stands, so finish with a brief final assistant message."
    )
}

pub(super) struct StartedTurn {
    pub(super) revision: u64,
    pub(super) keeps_result: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum SubmissionOutcome {
    Accepted { decoded_json_text: bool },
    Superseded,
}

#[derive(Clone)]
struct RestoredWork {
    messages: Vec<AgentMessage>,
    unstarted: Vec<AgentId>,
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
    session_handles: Atom<nanocode_sync::Map<String, AgentHandle>>,
    roots: Atom<nanocode_sync::Map<String, String>>,
    spawn_router: Atom<Option<Arc<dyn crate::SpawnRouter>>>,
    id: SubagentRuntimeId,
    state: nanocode_sync::Mailbox<RegistryState>,
    pub(super) updates: mpsc::UnboundedSender<ScopedAgentUpdate>,
    revision: watch::Sender<u64>,
    capacity: Capacity,
    queue_spawns: AtomicBool,
    unstarted: mpsc::UnboundedSender<(Arc<str>, AgentId)>,
    unstarted_queue: nanocode_sync::Slot<mpsc::UnboundedReceiver<(Arc<str>, AgentId)>>,
    max_resident: AtomicUsize,

    max_resident_explicit: std::sync::atomic::AtomicBool,

    hibernation: AtomicBool,
    scope_locks: Atom<HashMap<String, Arc<ScopeLocks>>>,
    store: Atom<Option<Arc<dyn SubagentStore>>>,

    journals: Atom<HashMap<String, Arc<dyn SubagentStore>>>,

    claims: Atom<JournalClaims>,

    spawn_admission: Atom<HashMap<String, Sequencer>>,
    journal_writer: std::sync::atomic::AtomicBool,

    journal_writers: Atom<HashMap<String, mpsc::UnboundedSender<JournalRequest>>>,

    steer_watchers: Atom<HashMap<String, mpsc::UnboundedSender<SteerWatch>>>,
    checkpoints: Atom<AgentMap<durable::JournalCheckpoint>>,

    checkpoint_changes: Atom<nanocode_sync::Map<String, nanocode_sync::OrdSet<AgentId>>>,

    pending_resume: Atom<nanocode_sync::Map<String, nanocode_sync::OrdSet<AgentId>>>,

    manual_resume: std::sync::atomic::AtomicBool,

    journal_version: std::sync::atomic::AtomicU32,

    deferred_resume: Atom<nanocode_sync::Map<String, nanocode_sync::OrdSet<AgentId>>>,

    deferred_work: Atom<nanocode_sync::Map<String, RestoredWork>>,

    wait_reported: Atom<AgentMap<u64>>,

    idle_waits: Atom<HashMap<String, (Vec<AgentId>, u32)>>,

    this: std::sync::OnceLock<Weak<Self>>,

    completion_retries: Atom<AgentMap<(u32, bool)>>,
    completion_dispatch: Atom<AgentMap<Sequencer>>,
    completion_batches: Atom<nanocode_sync::Map<String, mpsc::UnboundedSender<(AgentId, u64)>>>,
    completion_routes: Atom<AgentMap<u64>>,
    result_readers: Atom<AgentMap<Readers>>,
    root_watches: Atom<HashSet<String>>,
    running_roots: Atom<HashSet<String>>,

    parent_resolver: Atom<Option<ParentResolver>>,

    session_released: Atom<Option<SessionReleased>>,
    pending_turns: Atom<Option<PendingTurnResolver>>,

    pub(super) spawn_policy: Atom<crate::SpawnPolicy>,
}

#[derive(Clone, Default)]
struct JournalClaims {
    restored: HashMap<String, RestorationOutcome>,
    reserved: HashMap<String, nanocode_agent_core::backend::ChildJournal>,
}

pub type ParentResolver = Arc<dyn Fn(&str) -> Option<Agent> + Send + Sync>;

pub type SessionReleased = Arc<dyn Fn(&str) + Send + Sync>;

pub type PendingTurnResolver = Arc<
    dyn Fn(
            &str,
        ) -> Option<
            durable::SubagentStoreFuture<
                'static,
                std::io::Result<Option<(String, serde_json::Value)>>,
            >,
        > + Send
        + Sync,
>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CompletionStatus {
    Completed,
    Failed,
    Interrupted,
}

impl CompletionStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Interrupted => "interrupted",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CompletionPrompt<'a> {
    pub agent_id: AgentId,
    pub status: CompletionStatus,
    pub detail: &'a str,
}

pub fn parse_completion_prompt<'a>(
    request_id: &str,
    text: &'a str,
) -> Option<CompletionPrompt<'a>> {
    if !request_id.starts_with(crate::COMPLETION_REQUEST_PREFIX) {
        return None;
    }
    let marker = "<subagent_completion agent_id=\"";
    let trimmed = text.trim_end();
    let start = trimmed.rfind(marker)?;
    let (id, status) = trimmed[start + marker.len()..]
        .strip_suffix("\" />")?
        .split_once("\" status=\"")?;
    if id.is_empty() || !id.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let status = match status {
        "completed" => CompletionStatus::Completed,
        "failed" => CompletionStatus::Failed,
        "interrupted" => CompletionStatus::Interrupted,
        _ => return None,
    };
    Some(CompletionPrompt {
        agent_id: id.parse().ok()?,
        status,
        detail: text[..start].trim(),
    })
}

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

fn completion_request_id(session_id: &str, revision: u64, attempt: u32) -> String {
    match attempt {
        0 => format!(
            "{}{session_id}:{revision}",
            crate::COMPLETION_REQUEST_PREFIX
        ),
        attempt => format!(
            "{}{session_id}:{revision}:{attempt}",
            crate::COMPLETION_REQUEST_PREFIX
        ),
    }
}

fn completion_retry_delay(attempt: u32) -> Duration {
    Duration::from_millis(1000_u64 << attempt.clamp(1, 9)).min(Duration::from_secs(300))
}

struct SteerWatch {
    request_id: String,
    prompt: String,
    outcome: oneshot::Sender<NoticeOutcome>,
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

    async fn of_route(
        route: PromptRoute,
        steered: oneshot::Receiver<Self>,
        parent: &WeakAgent,
        request_id: String,
        prompt: String,
    ) -> Self {
        match route {
            PromptRoute::Started(turn) => {
                drop(steered);
                Self::of_turn(turn.result().await)
            }
            PromptRoute::Steered => {
                let mut steered = steered;
                loop {
                    let Some(agent) = parent.upgrade() else {
                        return steered.await.unwrap_or(Self::Stopped);
                    };
                    if agent.turn_controls().is_empty() {
                        drop(steered);
                        return Self::of_receipt(agent.steer_receipt(request_id, prompt).await);
                    }
                    let settled = agent.requests_settled();
                    drop(agent);
                    nanocode_sync::select! {
                        outcome = &mut steered => return outcome.unwrap_or(Self::Stopped),
                        () = settled => {}
                    }
                }
            }
        }
    }

    fn of_receipt(receipt: AgentResult<SteerReceiptStatus>) -> Self {
        match receipt {
            Ok(SteerReceiptStatus::Accepted) => Self::Read,
            _ => Self::Unread,
        }
    }

    async fn of_steers(
        mut events: AgentEvents,
        parent: WeakAgent,
        mut steers: mpsc::UnboundedReceiver<SteerWatch>,
    ) {
        let mut watched = BTreeMap::<String, (SteerWatch, Option<String>)>::new();
        loop {
            let event = nanocode_sync::select! {
                steer = steers.recv() => match steer {
                    Some(steer) => {
                        watched.insert(steer.request_id.clone(), (steer, None));
                        continue;
                    }
                    None => return,
                },
                event = events.recv() => match event {
                    Some(event) => event,
                    None => return,
                },
            };
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
                if let Some((_, carrier)) =
                    run.message_id.as_deref().and_then(|id| watched.get_mut(id))
                {
                    *carrier = run.turn_id;
                }
                continue;
            }
            let ended = watched
                .iter()
                .filter(|(_, (_, carrier))| carrier.is_none() || *carrier == run.turn_id)
                .map(|(id, _)| id.clone())
                .collect::<Vec<_>>();
            for id in ended {
                let Some((steer, carrier)) = watched.remove(&id) else {
                    continue;
                };
                if steer.outcome.is_closed() {
                    continue;
                }
                let outcome = match run.status {
                    Some(RunStatus::Completed) if carrier.is_some() => Self::Read,
                    Some(RunStatus::Completed) => {
                        let parent = parent.clone();
                        drop(platform::spawn(async move {
                            let outcome = match parent.upgrade() {
                                Some(parent) => Self::of_receipt(
                                    parent.steer_receipt(steer.request_id, steer.prompt).await,
                                ),
                                None => Self::Unread,
                            };
                            let _ = steer.outcome.send(outcome);
                        }));
                        continue;
                    }
                    Some(RunStatus::Cancelled) => Self::Cancelled,
                    _ => Self::Unread,
                };
                let _ = steer.outcome.send(outcome);
            }
        }
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

type Held = Vec<(AgentId, Option<u64>, Arc<AgentMarks>)>;

type InterruptRequest = (String, Vec<AgentId>, Held);

#[derive(Default)]
struct Told {
    stopped: HashSet<AgentId>,
    woken: HashSet<AgentId>,
}

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
    life: ScopeLife,

    paused: usize,

    cold: nanocode_sync::OrdMap<u64, durable::ColdLeaf>,

    pins: HashMap<u64, usize>,

    recheck: BTreeSet<u64>,
}

#[derive(Clone, Copy, Default)]
enum ScopeLife {
    #[default]
    Open,
    Closing,
    Frozen,
    Released,
}

impl ScopeLife {
    const fn closing(self) -> bool {
        !matches!(self, Self::Open)
    }

    const fn frozen(self) -> bool {
        matches!(self, Self::Frozen | Self::Released)
    }

    const fn released(self) -> bool {
        matches!(self, Self::Released)
    }
}

struct CommittedCalls {
    revision: u64,
    call_ids: Vec<String>,
    omitted: u32,
}

#[derive(Default)]
pub(super) struct AgentMarks {
    statuses: StatusLane,
    captured: AtomicBool,
    completed_tool: AtomicBool,
    capturing: Rerun,
}

impl AgentMarks {
    fn unobserved(&self) -> bool {
        self.statuses.idle() && self.capturing.idle()
    }
}

#[derive(Default)]
struct StatusLane {
    jobs: nanocode_sync::Inbox<OrderedJob>,
    running: AtomicBool,
}

impl StatusLane {
    fn idle(&self) -> bool {
        !self.running.load(Ordering::Acquire) && self.jobs.is_empty()
    }
}

#[derive(Default)]
struct Rerun(AtomicU8);

impl Rerun {
    const IDLE: u8 = 0;
    const RUNNING: u8 = 1;
    const AGAIN: u8 = 2;

    fn start(&self) -> bool {
        self.0
            .try_update(Ordering::AcqRel, Ordering::Acquire, |state| {
                Some(if state == Self::IDLE {
                    Self::RUNNING
                } else {
                    Self::AGAIN
                })
            })
            == Ok(Self::IDLE)
    }

    fn finish(&self) -> bool {
        self.0
            .try_update(Ordering::AcqRel, Ordering::Acquire, |state| {
                Some(if state == Self::AGAIN {
                    Self::RUNNING
                } else {
                    Self::IDLE
                })
            })
            != Ok(Self::AGAIN)
    }

    fn idle(&self) -> bool {
        self.0.load(Ordering::Acquire) == Self::IDLE
    }
}

#[derive(Clone)]
enum OrderedJob {
    Status(AgentStatus),

    Completion {
        status: AgentStatus,
        revision: u64,
    },

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
    acknowledged: Vec<(AgentId, Arc<str>)>,
}

enum JournalRequest {
    Stored(Box<durable::JournalCache>),
    Warmed(Vec<(AgentId, durable::CachedAgent)>),
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
    pub(super) closing: Sequencer,
    warming: Sequencer,
    pub(super) pending_checkpoints: AtomicUsize,
    evicting: Rerun,
}

pub(super) struct Warm {
    state: nanocode_sync::Mailbox<RegistryState>,
    registry: Weak<Registry>,
    root: String,
    leaves: Vec<u64>,
}

impl Drop for Warm {
    fn drop(&mut self) {
        let root = std::mem::take(&mut self.root);
        let leaves = std::mem::take(&mut self.leaves);
        let registry = std::mem::take(&mut self.registry);
        self.state.send(move |state| {
            if let Some(scope) = state.scopes.get_mut(&root)
                && scope.unpin(&leaves)
                && let Some(registry) = registry.upgrade()
            {
                registry.changed();
            }
        });
    }
}

pub(super) struct MessagePause {
    state: nanocode_sync::Mailbox<RegistryState>,
    root: String,
    registry: Option<Weak<Registry>>,
}

impl Drop for MessagePause {
    fn drop(&mut self) {
        let root = std::mem::take(&mut self.root);
        let registry = self.registry.take();
        self.state.send(move |state| {
            if let Some(scope) = state.scopes.get_mut(&root) {
                scope.paused = scope.paused.saturating_sub(1);
            }
            if let Some(registry) = registry.and_then(|registry| registry.upgrade()) {
                registry.changed();
            }
        });
    }
}

fn copy_error(error: &std::io::Error) -> std::io::Error {
    if is_fenced(error) {
        return std::io::Error::other(StoreError::Fenced);
    }
    std::io::Error::new(error.kind(), error.to_string())
}

fn is_fenced(error: &std::io::Error) -> bool {
    error
        .get_ref()
        .and_then(|error| error.downcast_ref::<StoreError>())
        .is_some_and(|error| matches!(error, StoreError::Fenced))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum JournalSave {
    Idle,
    Saved,
    Failed,
    Fenced,
}

type Filled = (
    nanocode_agent_core::backend::JournalCommit,
    Vec<(AgentId, Arc<str>)>,
);

fn acknowledge_deferred(
    registry: Weak<Registry>,
    root_session_id: &str,
    waiters: Vec<oneshot::Sender<std::io::Result<()>>>,
    filled: Option<Filled>,
) {
    if waiters.is_empty() && filled.as_ref().is_none_or(|(_, saving)| saving.is_empty()) {
        return;
    }
    let root_session_id = root_session_id.to_owned();
    drop(platform::spawn(async move {
        let interrupted = || std::io::Error::other("subagent journal write was interrupted");
        let result = match filled {
            Some((commit, saving)) => {
                let result = commit.await.unwrap_or_else(|_| Err(interrupted()));
                if let Some(registry) = registry.upgrade() {
                    registry.settle_saving(&root_session_id, &saving, result.is_ok());
                }
                result
            }
            None => Err(interrupted()),
        };
        for done in waiters {
            let _ = done.send(match &result {
                Ok(()) => Ok(()),
                Err(error) => Err(copy_error(error)),
            });
        }
    }));
}

async fn refill(
    open: nanocode_agent_core::backend::DeferredJournal,
    payload: String,
) -> Option<Filled> {
    let (fill, commit) = open.await.ok()?;
    fill.send((payload, Vec::new())).ok()?;
    Some((commit, Vec::new()))
}

async fn journal_writer(
    registry: Weak<Registry>,
    root_session_id: String,
    mut requests: mpsc::UnboundedReceiver<JournalRequest>,
    mut revision: watch::Receiver<u64>,
) {
    let mut saved = None;
    let mut recorded = HashMap::new();
    let mut cache = durable::JournalCache::default().written_as(
        registry
            .upgrade()
            .map_or(durable::JOURNAL_VERSION, |registry| {
                registry.journal_version.load(Ordering::Relaxed)
            }),
    );
    let mut pending = Vec::new();
    let mut failures = 0_u32;
    let mut fenced = false;
    let mut slot: Option<nanocode_agent_core::backend::DeferredJournal> = None;
    let mut deferrable = true;
    let mut deferred = Vec::new();
    loop {
        for _ in 0..requests.len() {
            if let Ok(request) = requests.try_recv() {
                pending.push(request);
            }
        }
        let Some(live) = registry.upgrade() else {
            return;
        };
        let (finished, save) = live
            .write_journal(
                &root_session_id,
                std::mem::take(&mut pending),
                &mut saved,
                &mut recorded,
                &mut cache,
                fenced,
                &mut slot,
                &mut deferrable,
                &mut deferred,
            )
            .await;
        match save {
            JournalSave::Fenced if !fenced => {
                fenced = true;
                nanocode_trace::info!(
                    %root_session_id,
                    "another process owns the subagent journal now; this process stops saving it"
                );
            }
            JournalSave::Saved => fenced = false,
            JournalSave::Idle | JournalSave::Failed | JournalSave::Fenced => {}
        }
        failures = if save == JournalSave::Failed {
            failures.saturating_add(1)
        } else {
            0
        };
        if finished {
            while let Ok(request) = requests.try_recv() {
                pending.push(request);
            }
            if pending.is_empty() {
                if let Some(open) = slot.take() {
                    let filled = match saved.clone() {
                        Some(payload) => refill(open, payload).await,
                        None => None,
                    };
                    acknowledge_deferred(
                        registry.clone(),
                        &root_session_id,
                        std::mem::take(&mut deferred),
                        filled,
                    );
                }
                requests.close();
                live.journal_writers.update(|writers| {
                    if writers
                        .get(&root_session_id)
                        .is_some_and(mpsc::UnboundedSender::is_closed)
                    {
                        writers.remove(&root_session_id);
                    }
                });
                while let Some(request) = requests.recv().await {
                    live.send_journal(&root_session_id, Some(request));
                }
                return;
            }
            continue;
        }
        drop(live);
        if cache.settles() {
            continue;
        }
        let retry = async {
            match failures {
                0 => std::future::pending().await,
                attempt => platform::sleep(completion_retry_delay(attempt)).await,
            }
        };
        let mut retry = std::pin::pin!(retry);
        loop {
            let taken = async {
                match slot.as_mut() {
                    Some(open) => open.await.ok(),
                    None => std::future::pending().await,
                }
            };
            nanocode_sync::select! {
                fill = taken => {
                    slot = None;
                    let filled = match (fill, registry.upgrade()) {
                        (Some((fill, commit)), Some(live)) => live
                            .fill_with(&root_session_id, fill, &mut saved, &mut recorded, &mut cache)
                            .await
                            .map(|saving| (commit, saving)),
                        _ => None,
                    };
                    acknowledge_deferred(
                        registry.clone(),
                        &root_session_id,
                        std::mem::take(&mut deferred),
                        filled,
                    );
                }
                changed = revision.changed() => {
                    if changed.is_err() {
                        return;
                    }
                    break;
                }
                request = requests.recv() => match request {
                    Some(request) => {
                        pending.push(request);
                        break;
                    }
                    None => return,
                },
                () = &mut retry => break,
            }
        }
    }
}

impl AgentScope {
    fn warming(&self, ids: &[AgentId]) -> BTreeSet<u64> {
        let mut leaves = BTreeSet::new();
        let mut pending = Vec::new();
        for id in ids {
            let leaf = durable::leaf_of(*id);
            if self.topology.contains(*id) && leaves.insert(leaf) && self.cold.contains_key(&leaf) {
                pending.push(leaf);
            }
        }
        while let Some(leaf) = pending.pop() {
            for id in durable::leaf_ids(leaf) {
                let mut parent = self.topology.parent(id);
                while let Some(above) = parent {
                    let leaf = durable::leaf_of(above);
                    if !self.cold.contains_key(&leaf) || !leaves.insert(leaf) {
                        break;
                    }
                    pending.push(leaf);
                    parent = self.topology.parent(above);
                }
            }
        }
        leaves
    }

    fn unpin(&mut self, leaves: &[u64]) -> bool {
        let mut released = false;
        for leaf in leaves {
            let Some(count) = self.pins.get_mut(leaf) else {
                continue;
            };
            *count -= 1;
            if *count == 0 {
                self.pins.remove(leaf);
                if self.sessions.settled(*leaf) {
                    self.recheck.insert(*leaf);
                    released = true;
                }
            }
        }
        released
    }

    fn coolable(
        &self,
        root_session_id: &str,
        id: AgentId,
        cache: &durable::JournalCache,
        checkpoints: &AgentMap<durable::JournalCheckpoint>,
    ) -> bool {
        let Some(session) = self.sessions.get(&id) else {
            return false;
        };
        let key = (root_session_id.to_owned(), id);
        let checkpoint = checkpoints.get(&key);
        let leaf = durable::leaf_of(id);
        session.settled()
            && !self.messages.has_pending_for(id)
            && self.topology.children(id).all(|child| {
                let below = durable::leaf_of(child);
                below == leaf || self.cold.contains_key(&below)
            })
            && checkpoint.is_none_or(|checkpoint| {
                matches!(checkpoint.payload, durable::Payload::Stored)
                    && checkpoint.checkpoint.is_none()
            })
            && cache.written(
                id,
                session.journal_generation,
                checkpoint.map(|checkpoint| &checkpoint.key),
            )
            && session.marks.unobserved()
    }

    fn cool(
        &mut self,
        root_session_id: &str,
        registry: &Registry,
        cache: &mut durable::JournalCache,
    ) {
        let mut candidates = cache.take_settled();
        candidates.append(&mut self.recheck);
        if candidates.is_empty() || !cache.stubs() || self.life.frozen() {
            return;
        }
        let checkpoints = registry.checkpoints.get();
        let mut cooled = Vec::new();
        while let Some(leaf) = candidates.pop_first() {
            if self.cold.contains_key(&leaf) || self.pins.contains_key(&leaf) {
                continue;
            }
            let Some(stored) = cache.cold(leaf) else {
                continue;
            };
            if !durable::leaf_ids(leaf)
                .all(|id| self.coolable(root_session_id, id, cache, &checkpoints))
            {
                continue;
            }
            for id in durable::leaf_ids(leaf) {
                let Some(session) = self.sessions.remove(&id) else {
                    continue;
                };
                if let Some(parent) = session.descriptor.parent {
                    let above = durable::leaf_of(parent);
                    if above != leaf {
                        candidates.insert(above);
                    }
                }
                let waiters = session
                    .waited
                    .iter()
                    .map(|waited| waited.session.clone())
                    .collect::<Vec<_>>();
                cooled.push((id, waiters));
            }
            cache.cool(leaf);
            self.cold.insert(leaf, stored);
        }
        registry.forget(root_session_id, &cooled);
    }

    fn holds_descendants(&self, id: AgentId) -> Option<bool> {
        Some(
            self.topology.has_children(id)
                && self
                    .topology
                    .subtree_postorder(id)
                    .ok()?
                    .into_iter()
                    .any(|descendant| {
                        descendant != id
                            && (self.sessions.resident.contains(&descendant)
                                || self.sessions.get(&descendant).is_some_and(|session| {
                                    matches!(session.status, AgentStatus::Pending)
                                }))
                    }),
        )
    }

    fn journal_snapshot(
        &mut self,
        root_session_id: &str,
        checkpoints: &AgentMap<durable::JournalCheckpoint>,
        cache: &durable::JournalCache,
    ) -> std::io::Result<durable::JournalSnapshot> {
        let (next_message_id, messages, logged, pending) = self.messages.snapshot(cache);
        let fold = cache.folds(messages.kept.is_some(), self.sessions.quiescent());
        let groups = match cache.tracked(fold, || {
            let mut ids = self.sessions.keys().copied().collect::<Vec<_>>();
            ids.sort_unstable();
            ids
        }) {
            durable::Tracked::Leaves(leaves) => leaves
                .into_iter()
                .map(|leaf| (leaf, durable::leaf_agents(leaf).collect::<Vec<_>>()))
                .collect::<Vec<_>>(),
            durable::Tracked::Agents(ids) => vec![(0, ids)],
        };
        let mut records = Vec::new();
        let mut acknowledged = Vec::new();
        let mut agents = Vec::with_capacity(groups.len());
        let root = root_session_id.to_owned();
        for (leaf, ids) in groups {
            let mut entries = Vec::new();
            for id in ids {
                let Some(session) = self.sessions.get(&id) else {
                    continue;
                };
                let checkpoint = checkpoints.get(&(root.clone(), id));
                let pending = checkpoint.is_some_and(|checkpoint| checkpoint.unsaved().is_some());
                let checkpoint_key = checkpoint.map(|checkpoint| &checkpoint.key);
                let cached = cache.agent(id, session.journal_generation, checkpoint_key);
                if !fold && !pending && cached.is_some() {
                    continue;
                }
                if let Some(checkpoint) = checkpoint
                    && pending
                {
                    acknowledged.push((id, Arc::clone(&checkpoint.key)));
                }
                let checkpoint_ref = match cached {
                    Some(cached) => {
                        if let Some(checkpoint) = checkpoint
                            && let Some(json) = checkpoint.unsaved()
                        {
                            records.push((Arc::clone(&checkpoint.key), Arc::clone(json)));
                        }
                        cached.checkpoint_ref.clone()
                    }
                    None => durable::checkpoint_ref(session, checkpoint, &mut records)?,
                };
                entries.push((
                    id,
                    durable::AgentEntry {
                        generation: session.journal_generation,
                        checkpoint: checkpoint_key.cloned(),
                        unchanged: cached.is_some(),
                        agent: durable::persist_agent(session, checkpoint_ref, cache.deltas()),
                    },
                ));
            }
            agents.push((leaf, entries));
        }
        Ok(durable::JournalSnapshot {
            fold,
            cold: self.cold.clone(),
            agents,
            messages,
            logged,
            next_message_id,
            pending,
            records,
            acknowledged,
        })
    }

    fn journal_payload(
        &mut self,
        root_session_id: &str,
        checkpoints: &AgentMap<durable::JournalCheckpoint>,
        version: u32,
    ) -> std::io::Result<JournalWrite> {
        let cache = durable::JournalCache::default().written_as(version);
        let encoded = self
            .journal_snapshot(root_session_id, checkpoints, &cache)?
            .encode(&cache)?;
        Ok(JournalWrite {
            payload: encoded.payload,
            records: encoded.records,
            acknowledged: encoded.acknowledged,
        })
    }
}

pub(super) struct AgentReservation {
    pub(super) root_session_id: String,
    pub(super) id: AgentId,
    pub(super) parent: Option<AgentId>,
    warm: Option<Arc<Warm>>,
}

pub(super) struct CloseRequest {
    pub(super) root_session_id: String,
    pub(super) ids: Vec<AgentId>,
    pub(super) harnesses: Vec<HarnessHandle>,
    pub(super) status_updates: Vec<(AgentId, AgentStatus, Arc<AgentMarks>)>,
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
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "usd_number::serialize"
    )]
    pub budget_usd: Option<UsdAmount>,
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
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "usd_number::serialize"
    )]
    pub budget_usd: Option<UsdAmount>,

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

pub(super) struct Hibernated {
    pub(super) call: Arc<dyn nanocode_agent_core::backend::ParkedCall>,
    pub(super) capacity: Option<TurnCapacity>,
    pub(super) urgent: VecDeque<AgentMessage>,
    pub(super) deferred: VecDeque<AgentMessage>,
}

struct ResidentEviction {
    id: AgentId,
    harness: HarnessHandle,
    marks: Arc<AgentMarks>,
    _lock: Turn,
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
        stored: bool,
    ) -> std::io::Result<u64> {
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
        let sessions = &mut self.scope_mut(root_session_id).sessions;
        if stored {
            return Ok(sessions.insert_stored(id, session));
        }
        sessions.insert(id, session);
        Ok(0)
    }

    fn insert_cold(
        &mut self,
        root_session_id: &str,
        stub: &durable::AgentStub,
    ) -> std::io::Result<()> {
        let scope = self.scope_mut(root_session_id);
        scope
            .topology
            .insert(stub.id, stub.session.clone(), stub.parent)?;
        if let Some(spawned) = &stub.spawned {
            scope.sessions.by_spawn.insert(spawned.clone(), stub.id);
        }
        self.root_by_session
            .insert(stub.session.clone(), root_session_id.to_owned());
        Ok(())
    }

    fn session_or_cold(
        &self,
        session_id: &str,
    ) -> Result<Option<&ChildSession>, (String, AgentId)> {
        let Some(root) = self.root_by_session.get(session_id) else {
            return Ok(None);
        };
        let Some(scope) = self.scopes.get(root) else {
            return Ok(None);
        };
        let Some(id) = scope.topology.agent_for_session(session_id) else {
            return Ok(None);
        };
        if scope.cold.contains_key(&durable::leaf_of(id)) {
            return Err((root.clone(), id));
        }
        Ok(scope.sessions.get(&id))
    }

    fn submit_result(
        &mut self,
        session_id: &str,
        instruction_revision: Option<u64>,
        output: Value,
        resumed: bool,
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
        let Some(turn) = session.turn.as_mut() else {
            return Err(CompletionError::new(
                CompletionErrorCode::InactiveTurn,
                false,
                "submit_result is only available during an active subagent turn",
                "Start a new assigned turn before submitting.",
            )
            .into());
        };
        let Some(instruction_revision) = instruction_revision else {
            return Err(CompletionError::new(
                CompletionErrorCode::MissingInstructionRevision,
                false,
                "submit_result requires runtime-owned instruction context",
                "The execution host must preserve the originating model response context.",
            )
            .into());
        };

        if matches!(turn.stage, TurnStage::Steering) || turn.revision != instruction_revision {
            return Ok(SubmissionOutcome::Superseded);
        }
        if let TurnStage::Submitted(accepted) = &turn.stage {
            if resumed
                && let Ok((output, decoded_json_text)) = validate_submitted_output(
                    &session.output_validator,
                    &session.output_schema,
                    output,
                )
                && output == *accepted
            {
                return Ok(SubmissionOutcome::Accepted { decoded_json_text });
            }
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
        turn.stage = TurnStage::Submitted(output);
        Ok(SubmissionOutcome::Accepted { decoded_json_text })
    }

    fn begin_turn_steer(&mut self, root_session_id: &str, id: AgentId) -> Option<TurnSteer> {
        let session = self
            .scopes
            .get_mut(root_session_id)?
            .sessions
            .get_mut(&id)?;
        let turn = session.turn.as_mut()?;
        if !matches!(turn.stage, TurnStage::Working) {
            return None;
        }
        let previous_revision = turn.revision;
        let revision = session.next_instruction_revision.checked_add(1)?;
        session.next_instruction_revision = revision;
        turn.revision = revision;
        turn.stage = TurnStage::Steering;
        Some(TurnSteer {
            id,
            previous_revision,
            revision,
        })
    }

    fn finish_turn_steer(&mut self, root_session_id: &str, steer: TurnSteer, committed: bool) {
        let Some(turn) = self
            .scopes
            .get_mut(root_session_id)
            .and_then(|scope| scope.sessions.get_mut(&steer.id))
            .and_then(|session| session.turn.as_mut())
            .filter(|turn| turn.revision == steer.revision)
        else {
            return;
        };
        if !committed {
            turn.revision = steer.previous_revision;
        }
        if matches!(turn.stage, TurnStage::Steering) {
            turn.stage = TurnStage::Working;
        }
    }

    fn reserve_for(&mut self, session_id: &str) -> std::io::Result<AgentReservation> {
        let root_session_id = self.root_session_id(session_id).to_owned();
        if self
            .scopes
            .get(&root_session_id)
            .is_some_and(|scope| scope.life.closing())
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
            .is_some_and(|scope| scope.life.closing())
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
            warm: None,
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
            .is_some_and(|scope| scope.life.closing())
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
            .and_then(|session| session.harness().cloned())
            .ok_or_else(|| std::io::Error::other(format!("agent {id} is closed")))
    }

    fn listed(&self, session_id: &str, include_self: bool, after: Option<AgentId>) -> Vec<AgentId> {
        let Some(scope) = self.scopes.get(self.root_session_id(session_id)) else {
            return Vec::new();
        };
        let caller = scope.topology.agent_for_session(session_id);
        let mut ids = scope
            .topology
            .ids()
            .into_iter()
            .filter(|id| include_self || caller != Some(*id))
            .filter(|id| after.is_none_or(|after| *id > after))
            .collect::<Vec<_>>();
        ids.sort_unstable();
        ids
    }

    fn directory(
        &self,
        session_id: &str,
        include_completed: bool,
        include_self: bool,
        only: Option<Vec<AgentId>>,
    ) -> Vec<AgentDirectoryEntry> {
        let root_session_id = self.root_session_id(session_id);
        let Some(scope) = self.scopes.get(root_session_id) else {
            return Vec::new();
        };
        let caller = scope.topology.agent_for_session(session_id);
        let ids = only.unwrap_or_else(|| {
            let mut ids = scope.sessions.keys().copied().collect::<Vec<_>>();
            ids.sort_unstable();
            ids
        });
        ids.into_iter()
            .filter(|id| include_self || caller != Some(*id))
            .filter_map(|id| {
                let session = scope.sessions.get(&id)?;

                let recoverable_interrupted = matches!(session.status, AgentStatus::Interrupted)
                    && (session.harness().is_some() || session.stored_runtime.recoverable());
                if !include_completed
                    && !matches!(session.status, AgentStatus::Pending | AgentStatus::Running)
                    && !recoverable_interrupted
                {
                    return None;
                }
                let can_message = caller != Some(id)
                    && (session.harness().is_some() || session.stored_runtime.recoverable())
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
                    budget_usd: session.descriptor.budget_usd,
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
        let harness = target.harness().cloned().ok_or_else(|| {
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
            let to = scope.messages.message(id).map(|message| message.to);
            scope.messages.rollback(id);
            let recorded = to.and_then(|to| {
                scope
                    .sessions
                    .get(&to)
                    .is_some_and(|session| session.sent.iter().any(|sent| sent.message == id))
                    .then_some(to)
            });
            if let Some(session) = recorded.and_then(|to| scope.sessions.get_mut(&to)) {
                session.sent.retain(|sent| sent.message != id);
            }
        }
    }

    fn record_sent(&mut self, root_session_id: &str, session_id: &str, call: &str, id: MessageId) {
        let Some(scope) = self.scopes.get_mut(root_session_id) else {
            return;
        };
        let Some(to) = scope.messages.message(id).map(|message| message.to) else {
            return;
        };
        let Some(target) = scope.sessions.get_mut(&to) else {
            return;
        };
        let cell = durable::cell_of(call);
        target.sent.retain(|sent| {
            sent.session != session_id
                || (durable::cell_of(&sent.call) == cell && sent.call != call)
        });
        target.sent.push(durable::Sent {
            session: session_id.to_owned(),
            call: call.to_owned(),
            message: id,
            disposition: None,
        });
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
        target.delegated_turn = true;
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
        target.delegated_turn = false;
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
            let Some(to) = scope.messages.message(id).map(|message| message.to) else {
                return;
            };
            let recorded = scope.sessions.get(&to).is_some_and(|session| {
                session
                    .sent
                    .iter()
                    .any(|sent| sent.message == id && sent.disposition.is_none())
            });
            if recorded && let Some(session) = scope.sessions.get_mut(&to) {
                for sent in &mut session.sent {
                    if sent.message == id {
                        sent.disposition = Some(disposition);
                    }
                }
            }
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
    ) -> std::io::Result<InterruptRequest> {
        let root_session_id = self.authorize(session_id, id)?;
        let ids = self.subtree_shutdown_order(&root_session_id, id)?;
        let held = self.hold_unstarted(&root_session_id, &ids);
        Ok((root_session_id, ids, held))
    }

    fn request_close(&mut self, session_id: &str, id: AgentId) -> std::io::Result<CloseRequest> {
        let root_session_id = self.authorize(session_id, id)?;
        let ids = self.subtree_shutdown_order(&root_session_id, id)?;
        let harnesses = self.harnesses(&root_session_id, &ids)?;
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
    ) -> Vec<(AgentId, AgentStatus, Arc<AgentMarks>)> {
        let Some(scope) = self.scopes.get(root_session_id) else {
            return Vec::new();
        };
        ids.iter()
            .filter_map(|id| {
                let session = scope
                    .sessions
                    .get(id)
                    .filter(|session| matches!(session.status, AgentStatus::Closing))?;
                Some((*id, AgentStatus::Closing, Arc::clone(&session.marks)))
            })
            .collect()
    }

    fn request_close_all(&mut self, session_id: &str) -> std::io::Result<CloseRequest> {
        let root_session_id = self.root_session_id(session_id).to_owned();
        let ids = {
            let scope = self.scope_mut(&root_session_id);
            if !scope.life.closing() {
                scope.life = ScopeLife::Closing;
            }
            let mut ids = scope.topology.all_postorder();
            ids.retain(|id| scope.sessions.get(id).is_some());
            ids
        };
        let harnesses = self.harnesses(&root_session_id, &ids)?;
        let status_updates = self.closing_updates(&root_session_id, &ids);
        Ok(CloseRequest {
            root_session_id,
            ids,
            harnesses,
            status_updates,
        })
    }

    fn request_interrupt_all(&mut self, session_id: &str) -> InterruptRequest {
        let root_session_id = self.root_session_id(session_id).to_owned();
        let mut ids = self
            .scopes
            .get(&root_session_id)
            .map(|scope| scope.sessions.keys().copied().collect::<Vec<_>>())
            .unwrap_or_default();
        ids.sort_unstable();
        let held = self.hold_unstarted(&root_session_id, &ids);
        (root_session_id, ids, held)
    }

    fn hold_unstarted(&mut self, root_session_id: &str, ids: &[AgentId]) -> Held {
        let Some(scope) = self.scopes.get_mut(root_session_id) else {
            return Vec::new();
        };
        ids.iter()
            .filter_map(|id| {
                let session = scope.sessions.get_mut(id)?;
                if session.active()
                    || session.harness().is_some()
                    || !matches!(session.status, AgentStatus::Pending)
                    || !matches!(session.stored_runtime, StoredRuntime::Unstarted(_))
                {
                    return None;
                }
                session.status = AgentStatus::Interrupted;
                let settled = session.next_instruction_revision.saturating_add(1);
                session.next_instruction_revision = settled;
                session.settled_revision = Some(settled);
                let completion = session.descriptor.parent.is_none().then_some(settled);
                if let Some(revision) = completion {
                    session.settle_completion(revision);
                }
                Some((*id, completion, Arc::clone(&session.marks)))
            })
            .collect()
    }

    fn release_unstarted(
        &mut self,
        session_id: &str,
        id: AgentId,
    ) -> Option<(String, Arc<AgentMarks>)> {
        let root = self.root_session_id(session_id).to_owned();
        let session = self.scopes.get_mut(&root)?.sessions.get_mut(&id)?;
        if session.active()
            || session.harness().is_some()
            || !matches!(session.status, AgentStatus::Interrupted)
            || !matches!(session.stored_runtime, StoredRuntime::Unstarted(_))
        {
            return None;
        }
        session.status = AgentStatus::Pending;
        let marks = Arc::clone(&session.marks);
        Some((root, marks))
    }

    fn harnesses(
        &mut self,
        root_session_id: &str,
        ids: &[AgentId],
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
            if !matches!(session.status, AgentStatus::Closed) {
                session.status = AgentStatus::Closing;
            }
            harnesses.extend(session.harness().into_iter().cloned());
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
            if session.active() {
                return Err(std::io::Error::other(format!(
                    "agent {id} is still running"
                )));
            }
            session.residency = Residency::Detached;
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

    fn interrupt_active(
        &mut self,
        root_session_id: &str,
        ids: &[AgentId],
        mut told: Told,
    ) -> std::io::Result<(bool, Vec<HarnessHandle>, Vec<AgentId>, Told)> {
        let scope = self
            .scopes
            .get_mut(root_session_id)
            .ok_or_else(|| std::io::Error::other("subagent scope disappeared"))?;
        let mut inactive = true;
        let mut harnesses = Vec::new();
        let mut asleep = Vec::new();
        for id in ids {
            let Some(session) = scope.sessions.get_mut(id) else {
                continue;
            };
            let Some(turn) = session.turn.as_mut() else {
                continue;
            };
            inactive = false;
            turn.interrupted = true;
            if let Some(harness) = session.harness() {
                if told.stopped.insert(*id) {
                    harnesses.push(harness.clone());
                }
            } else if session.hibernated.is_some() && told.woken.insert(*id) {
                asleep.push(*id);
            }
        }
        Ok((inactive, harnesses, asleep, told))
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
                .is_some_and(|session| !session.active())
        }))
    }

    fn take_resident_evictions(
        &mut self,
        root_session_id: &str,
        limit: usize,
    ) -> Vec<ResidentEviction> {
        let Some(scope) = self.scopes.get_mut(root_session_id) else {
            return Vec::new();
        };
        scope.sessions.refresh();
        let excess = scope.sessions.resident.len().saturating_sub(limit);
        let sessions = &scope.sessions;
        let candidates = sessions
            .idle
            .iter()
            .filter_map(|&(_, id)| {
                let session = sessions.get(&id)?;
                if scope.messages.has_pending_for(id)
                    || !session
                        .harness()
                        .is_some_and(|harness| !harness.answering_side_questions())
                {
                    return None;
                }
                if scope.holds_descendants(id)? {
                    return None;
                }
                Some((id, session.residency_lock.try_enter()?))
            })
            .take(excess)
            .collect::<Vec<_>>();
        let mut evictions = Vec::with_capacity(candidates.len());
        for (id, lock) in candidates {
            let Some(session) = scope.sessions.get_mut(&id) else {
                continue;
            };
            match std::mem::replace(&mut session.residency, Residency::Evicted) {
                Residency::Resident(harness) => evictions.push(ResidentEviction {
                    id,
                    harness,
                    marks: Arc::clone(&session.marks),
                    _lock: lock,
                }),
                Residency::Detached | Residency::Evicted => {
                    session.residency = Residency::Detached;
                }
            }
        }
        evictions
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
    fn shared(&self) -> Option<Arc<Self>> {
        self.this.get().and_then(Weak::upgrade)
    }

    pub(super) fn new(
        updates: mpsc::UnboundedSender<ScopedAgentUpdate>,
        max_concurrency: usize,
    ) -> Self {
        let (revision, _) = watch::channel(0);
        let (unstarted, unstarted_queue) = mpsc::unbounded_channel();
        Self {
            id: SubagentRuntimeId::next(),
            spawn_router: Atom::default(),
            state: nanocode_sync::Mailbox::default(),
            updates,
            revision,
            capacity: Capacity::new(max_concurrency),
            queue_spawns: AtomicBool::new(false),
            unstarted,
            unstarted_queue: nanocode_sync::Slot::new(Some(unstarted_queue)),
            session_handles: Atom::default(),
            roots: Atom::default(),
            max_resident: AtomicUsize::new(crate::DEFAULT_MAX_RESIDENT_SUBAGENTS),
            max_resident_explicit: std::sync::atomic::AtomicBool::new(false),
            hibernation: AtomicBool::new(false),
            scope_locks: Atom::default(),
            store: Atom::default(),
            journals: Atom::default(),
            claims: Atom::default(),
            spawn_admission: Atom::default(),
            journal_writer: std::sync::atomic::AtomicBool::new(false),
            journal_writers: Atom::default(),
            steer_watchers: Atom::default(),
            checkpoints: Atom::default(),
            checkpoint_changes: Atom::default(),
            pending_resume: Atom::default(),
            manual_resume: std::sync::atomic::AtomicBool::new(false),
            journal_version: std::sync::atomic::AtomicU32::new(durable::JOURNAL_VERSION),
            deferred_resume: Atom::default(),
            deferred_work: Atom::default(),
            wait_reported: Atom::default(),
            idle_waits: Atom::default(),
            this: std::sync::OnceLock::new(),
            completion_retries: Atom::default(),
            completion_dispatch: Atom::default(),
            completion_batches: Atom::default(),
            completion_routes: Atom::default(),
            result_readers: Atom::default(),
            root_watches: Atom::default(),
            running_roots: Atom::default(),
            parent_resolver: Atom::default(),
            session_released: Atom::default(),
            pending_turns: Atom::default(),
            spawn_policy: Atom::default(),
        }
    }

    pub fn set_parent_resolver(&self, resolver: ParentResolver) {
        self.parent_resolver.set(Some(resolver));
    }

    pub fn set_session_released(&self, released: SessionReleased) {
        self.session_released.set(Some(released));
    }

    pub fn set_pending_turn_resolver(&self, resolver: PendingTurnResolver) {
        self.pending_turns.set(Some(resolver));
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
            let mut unstarted = Vec::new();
            match registry.restore(&root).await {
                Ok(report) if report.restored > 0 => {
                    nanocode_trace::info!(
                        restored = report.restored,
                        interrupted = report.interrupted.len(),
                        completed = report.completed.len(),
                        unrecoverable = report.unrecoverable.len(),
                        %root,
                        "restored durable subagent task tree"
                    );
                    completed = report.completed;
                    unstarted = report.unstarted;
                }
                Ok(_) => {}
                Err(error) => {
                    nanocode_trace::warn!(%error, %root, "could not restore durable subagent task tree");
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
                    nanocode_trace::warn!(%id, %error, "could not resume restored subagent");
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
            let work = RestoredWork {
                messages: pending,
                unstarted,
            };
            if registry
                .manual_resume
                .load(std::sync::atomic::Ordering::SeqCst)
            {
                registry.deferred_work.update(|deferred| {
                    deferred.insert(root.clone(), work.clone());
                });
            } else {
                registry.start_restored_work(&root, work).await;
            }
            let resolver = Option::clone(&registry.parent_resolver.get());
            if registry.needs_root_watch(&root).await
                && let Some(parent) = resolver.and_then(|resolver| resolver(&root))
            {
                registry.watch_root(&root, &parent);
            }
            let _ = ready.send(RestorationState::Ready);
            let resolver = Option::clone(&registry.parent_resolver.get());
            if let Some(parent) = resolver.and_then(|resolver| resolver(&root))
                && parent
                    .interrupted_requests()
                    .iter()
                    .any(|id| id.starts_with(crate::COMPLETION_REQUEST_PREFIX))
                && let Err(error) = parent.resume_interrupted_turns().await
            {
                nanocode_trace::warn!(%error, %root, "interrupted subagent completion turns did not resume");
            }
        }));
    }

    fn root_of(&self, session_id: &str) -> String {
        self.roots
            .read(|roots| roots.get(session_id).cloned())
            .unwrap_or_else(|| session_id.to_owned())
    }

    pub(crate) async fn admit_spawn(&self, session_id: &str) -> std::io::Result<Option<Turn>> {
        let root = self.root_of(session_id);
        let restored = self.claims.read(|claims| {
            claims
                .restored
                .get(&root)
                .is_none_or(|gate| matches!(*gate.borrow(), RestorationState::Ready))
        });
        if restored {
            return Ok(None);
        }
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
        let root = self.root_of(session_id);
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
        drop(platform::spawn(async move {
            let Some(live) = registry.upgrade() else {
                return;
            };
            let roots = live
                .state
                .call(|state| {
                    state
                        .scopes
                        .iter()
                        .filter(|(_, scope)| !scope.life.frozen())
                        .map(|(root, _)| root.clone())
                        .collect::<Vec<_>>()
                })
                .await;
            for root in roots {
                if live.store_for(&root).is_some() {
                    live.send_journal(&root, None);
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
        self.scope_locks(&self.root_of(session_id))
    }

    pub(super) async fn pause_messages(&self, session_id: &str) -> MessagePause {
        let root = self.root_of(session_id);
        let paused = root.clone();
        self.state
            .call(move |state| {
                if let Some(scope) = state.scopes.get_mut(&paused) {
                    scope.paused += 1;
                }
            })
            .await;
        MessagePause {
            state: self.state.clone(),
            root,
            registry: self.this.get().cloned(),
        }
    }

    pub(super) async fn when_messages_flow<T, F>(
        &self,
        session_id: &str,
        job: F,
    ) -> std::io::Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&mut RegistryState) -> T + Send + 'static,
    {
        let root = self.root_of(session_id);
        let mut changes = self.revision.subscribe();
        let mut job = job;
        loop {
            let scoped = root.clone();
            let outcome = self
                .state
                .call(move |state| {
                    if state
                        .scopes
                        .get(&scoped)
                        .is_some_and(|scope| scope.paused > 0)
                    {
                        return Err(job);
                    }
                    Ok(job(state))
                })
                .await;
            match outcome {
                Ok(value) => return Ok(value),
                Err(returned) => {
                    job = returned;
                    changes
                        .changed()
                        .await
                        .map_err(|_| std::io::Error::other("subagent runtime is closed"))?;
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn write_journal(
        &self,
        root_session_id: &str,
        requests: Vec<JournalRequest>,
        saved: &mut Option<String>,
        recorded: &mut HashMap<String, Arc<str>>,
        cache: &mut durable::JournalCache,
        fenced: bool,
        slot: &mut Option<nanocode_agent_core::backend::DeferredJournal>,
        deferrable: &mut bool,
        awaiting: &mut Vec<oneshot::Sender<std::io::Result<()>>>,
    ) -> (bool, JournalSave) {
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
                matches!(gate, None | Some(RestorationState::Ready)) && !fenced,
            )
        };
        let mut waiters = Vec::new();
        let mut finals = Vec::new();
        for request in requests {
            match request {
                JournalRequest::Stored(stored) => cache.adopt(*stored),
                JournalRequest::Warmed(agents) => cache.warm(agents),
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
            return (true, JournalSave::Idle);
        };
        let frozen = || {
            Err(std::io::Error::other(
                "subagent journal is frozen for shutdown",
            ))
        };
        let Some(registry) = self.shared() else {
            return (true, JournalSave::Idle);
        };
        let root = root_session_id.to_owned();
        let mut owned_cache = std::mem::take(cache);
        let defer = *deferrable;
        let (snapshot, finished, returned_waiters, returned_cache, wanted) = self
            .state
            .call(move |state| {
                let root_session_id = root.as_str();
                let cache = &mut owned_cache;
                let _collect = nanocode_trace::info_span!(
                    "subagent.journal.collect",
                    session.root_id = root_session_id
                )
                .entered();
                let mut wanted = false;
                let (snapshot, finished) = match state.scopes.get_mut(root_session_id) {
                    None => {
                        for (scope, done) in waiters.drain(..) {
                            let _ = done.send(if scope { Ok(()) } else { frozen() });
                        }
                        (None, true)
                    }
                    Some(scope) => {
                        if scope.life.frozen() {
                            let (scoped, unscoped): (Vec<_>, Vec<_>) =
                                waiters.drain(..).partition(|(scope, _)| *scope);
                            waiters = scoped;
                            for (_, done) in unscoped {
                                let _ = done.send(frozen());
                            }
                        }
                        if scope.life.released() {
                            for (_, done) in waiters.drain(..) {
                                let _ = done.send(frozen());
                            }
                        }
                        let write = !waiters.is_empty() || (background && !scope.life.frozen());
                        wanted = write;
                        scope.cool(root_session_id, &registry, cache);
                        (
                            (write && !defer).then(|| {
                                cache.track(
                                    registry
                                        .checkpoint_changes
                                        .update(|changes| changes.remove(root_session_id))
                                        .into_iter()
                                        .flatten(),
                                );
                                cache.track(scope.sessions.take_changed());
                                let checkpoints = registry.checkpoints.get();
                                scope.journal_snapshot(root_session_id, &checkpoints, cache)
                            }),
                            scope.life.frozen(),
                        )
                    }
                };
                (snapshot, finished, waiters, owned_cache, wanted)
            })
            .await;
        let waiters = returned_waiters;
        *cache = returned_cache;
        if defer && wanted {
            let deferred = match slot {
                Some(_) => Ok(true),
                None => match store.save_deferred(root_session_id).await {
                    Ok(Some(open)) => {
                        *slot = Some(open);
                        Ok(true)
                    }
                    Ok(None) => Ok(false),
                    Err(error) => Err(error),
                },
            };
            match deferred {
                Ok(true) => {
                    awaiting.extend(waiters.into_iter().map(|(_, done)| done));
                    let closed = !finals.is_empty();
                    if closed && let Some(open) = slot.take() {
                        let filled = self
                            .fill_journal(root_session_id, open, saved, recorded, cache)
                            .await;
                        acknowledge_deferred(
                            self.this.get().cloned().unwrap_or_default(),
                            root_session_id,
                            std::mem::take(awaiting),
                            filled,
                        );
                    }
                    for (write, done) in finals {
                        *saved = None;
                        let _ = done.send(
                            self.save_write(root_session_id, store.as_ref(), write)
                                .await,
                        );
                    }
                    return (finished || closed, JournalSave::Saved);
                }
                Ok(false) => {
                    *deferrable = false;
                    let mut requeued = waiters
                        .into_iter()
                        .map(|(scope, done)| JournalRequest::Persist { scope, done })
                        .collect::<Vec<_>>();
                    requeued.extend(
                        finals
                            .into_iter()
                            .map(|(write, done)| JournalRequest::Final { write, done }),
                    );
                    return Box::pin(self.write_journal(
                        root_session_id,
                        requeued,
                        saved,
                        recorded,
                        cache,
                        fenced,
                        slot,
                        deferrable,
                        awaiting,
                    ))
                    .await;
                }
                Err(error) => {
                    let save = if is_fenced(&error) {
                        JournalSave::Fenced
                    } else {
                        nanocode_trace::warn!(%error, %root_session_id, "could not save subagent journal");
                        JournalSave::Failed
                    };
                    for (_, done) in waiters {
                        let _ = done.send(Err(copy_error(&error)));
                    }
                    let closed = !finals.is_empty();
                    for (write, done) in finals {
                        *saved = None;
                        let _ = done.send(
                            self.save_write(root_session_id, store.as_ref(), write)
                                .await,
                        );
                    }
                    return (finished || closed, save);
                }
            }
        }
        let encoded = snapshot.map(|snapshot| {
            snapshot.and_then(|snapshot| {
                let _encode = nanocode_trace::info_span!(
                    "subagent.journal.encode",
                    session.root_id = root_session_id
                )
                .entered();
                snapshot.encode(cache)
            })
        });
        let attempted = encoded.is_some();
        let result = match encoded {
            None => Ok(()),
            Some(Err(error)) => Err(error),
            Some(Ok(encoded)) => {
                let records = &encoded.records;
                if waiters.is_empty() && saved.as_deref() == Some(encoded.payload.as_str()) {
                    self.acknowledge(root_session_id, &encoded.acknowledged);
                    cache.committed(encoded);
                    Ok(())
                } else {
                    let payload = encoded.payload.clone();
                    let result = store
                        .save(root_session_id, payload.clone(), records.clone())
                        .instrument(nanocode_trace::info_span!(
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
                            self.acknowledge(root_session_id, &encoded.acknowledged);
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
            && !is_fenced(error)
        {
            nanocode_trace::warn!(%error, %root_session_id, "could not save subagent journal");
        }
        for (_, done) in waiters {
            let _ = done.send(result.as_ref().copied().map_err(copy_error));
        }
        let save = match &result {
            Ok(()) if attempted => JournalSave::Saved,
            Ok(()) => JournalSave::Idle,
            Err(error) if is_fenced(error) => JournalSave::Fenced,
            Err(_) => JournalSave::Failed,
        };
        let closed = !finals.is_empty();
        if closed && let Some(open) = slot.take() {
            let filled = self
                .fill_journal(root_session_id, open, saved, recorded, cache)
                .await;
            acknowledge_deferred(
                self.this.get().cloned().unwrap_or_default(),
                root_session_id,
                Vec::new(),
                filled,
            );
        }
        for (write, done) in finals {
            *saved = None;
            let _ = done.send(
                self.save_write(root_session_id, store.as_ref(), write)
                    .await,
            );
        }
        (finished || closed, save)
    }

    async fn fill_journal(
        &self,
        root_session_id: &str,
        open: nanocode_agent_core::backend::DeferredJournal,
        saved: &mut Option<String>,
        recorded: &mut HashMap<String, Arc<str>>,
        cache: &mut durable::JournalCache,
    ) -> Option<Filled> {
        let (fill, commit) = open.await.ok()?;
        self.fill_with(root_session_id, fill, saved, recorded, cache)
            .await
            .map(|saving| (commit, saving))
    }

    async fn fill_with(
        &self,
        root_session_id: &str,
        fill: nanocode_agent_core::backend::JournalFill,
        saved: &mut Option<String>,
        recorded: &mut HashMap<String, Arc<str>>,
        cache: &mut durable::JournalCache,
    ) -> Option<Vec<(AgentId, Arc<str>)>> {
        let (Some(store), Some(registry)) = (self.store_for(root_session_id), self.shared()) else {
            return None;
        };
        let root = root_session_id.to_owned();
        let mut owned_cache = std::mem::take(cache);
        let (snapshot, returned_cache) = self
            .state
            .call(move |state| {
                let root_session_id = root.as_str();
                let cache = &mut owned_cache;
                let snapshot = state.scopes.get_mut(root_session_id).map(|scope| {
                    scope.cool(root_session_id, &registry, cache);
                    cache.track(
                        registry
                            .checkpoint_changes
                            .update(|changes| changes.remove(root_session_id))
                            .into_iter()
                            .flatten(),
                    );
                    cache.track(scope.sessions.take_changed());
                    let checkpoints = registry.checkpoints.get();
                    scope.journal_snapshot(root_session_id, &checkpoints, cache)
                });
                (snapshot, owned_cache)
            })
            .await;
        *cache = returned_cache;
        let encoded = snapshot.map(|snapshot| {
            snapshot.and_then(|snapshot| {
                let _encode = nanocode_trace::info_span!(
                    "subagent.journal.encode",
                    session.root_id = root_session_id
                )
                .entered();
                snapshot.encode(cache)
            })
        });
        let Some(Ok(encoded)) = encoded else {
            if let Some(Err(error)) = encoded {
                nanocode_trace::warn!(%error, %root_session_id, "could not encode subagent journal");
            }
            return None;
        };
        let payload = encoded.payload.clone();
        if fill
            .send((payload.clone(), encoded.records.clone()))
            .is_err()
        {
            return None;
        }
        let saving = encoded.acknowledged.clone();
        self.settle(root_session_id, &saving, |payload| match payload {
            durable::Payload::Unsaved(json) => durable::Payload::Saving(Arc::clone(json)),
            other => other.clone(),
        });
        cache.committed(encoded);
        if store.records_sessions() {
            self.record_children(root_session_id, store.as_ref(), recorded)
                .await;
        }
        *saved = Some(payload);
        Some(saving)
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
                Err(error) => nanocode_trace::warn!(
                    %error, %root_session_id, %session_id,
                    "could not record subagent session"
                ),
            }
        }
    }

    async fn child_checkpoints(&self, root_session_id: &str) -> Vec<(Arc<str>, SessionCheckpoint)> {
        let checkpoints = self.checkpoints.get();
        let root = root_session_id.to_owned();
        self.state
            .call(move |state| {
                let root_session_id = root.as_str();
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
                                scope
                                    .sessions
                                    .get(&id)?
                                    .stored_runtime
                                    .checkpoint()?
                                    .clone(),
                                true,
                            )
                            .ok()?,
                        };
                        Some((checkpoint.key, Arc::unwrap_or_clone(checkpoint.checkpoint?)))
                    })
                    .collect()
            })
            .await
    }

    fn records_sessions(&self, root_session_id: &str) -> bool {
        self.store_for(root_session_id)
            .is_some_and(|store| store.records_sessions())
    }

    fn journal_holds_conversation(&self, root_session_id: &str, marks: &AgentMarks) -> bool {
        marks.captured.load(Ordering::Acquire) && !self.records_sessions(root_session_id)
    }

    fn recorded_conversation(&self, root_session_id: &str, marks: &AgentMarks, conversation: bool) {
        if conversation && !self.records_sessions(root_session_id) {
            marks.captured.store(true, Ordering::Release);
        }
    }

    fn record_checkpoint(
        &self,
        root_session_id: &str,
        id: AgentId,
        marks: &AgentMarks,
        snapshot: SessionCheckpoint,
    ) -> bool {
        if self.journal_holds_conversation(root_session_id, marks) {
            return true;
        }
        let conversation = snapshot.has_conversation();
        let encoded =
            durable::JournalCheckpoint::encode(snapshot, self.records_sessions(root_session_id));
        let recorded = self.insert_encoded(root_session_id, id, encoded);
        if recorded {
            self.recorded_conversation(root_session_id, marks, conversation);
        }
        recorded
    }

    async fn record_checkpoint_off_thread(
        &self,
        root_session_id: &str,
        id: AgentId,
        marks: &AgentMarks,
        snapshot: SessionCheckpoint,
    ) -> bool {
        if self.journal_holds_conversation(root_session_id, marks) {
            return true;
        }
        let conversation = snapshot.has_conversation();
        let retain = self.records_sessions(root_session_id);
        let encoded = durable::JournalCheckpoint::encode(snapshot, retain);
        let recorded = self.insert_encoded(root_session_id, id, encoded);
        if recorded {
            self.recorded_conversation(root_session_id, marks, conversation);
        }
        recorded
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
                nanocode_trace::warn!(%error, "could not encode subagent checkpoint");
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
                    .map(|checkpoint| (Arc::clone(&checkpoint.key), checkpoint.held().cloned()))
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
        self.checkpoint_changes.update(|changes| {
            changes
                .entry(root_session_id.to_owned())
                .or_default()
                .insert(id);
        });
    }

    fn release_recorded(&self, root_session_id: &str, key: &Arc<str>) {
        let retained = |root: &String, checkpoint: &durable::JournalCheckpoint| {
            root == root_session_id
                && checkpoint.key == *key
                && matches!(checkpoint.payload, durable::Payload::Stored)
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

    fn acknowledge(&self, root_session_id: &str, acknowledged: &[(AgentId, Arc<str>)]) {
        self.settle(root_session_id, acknowledged, |_| durable::Payload::Stored);
    }

    fn settle_saving(&self, root_session_id: &str, saving: &[(AgentId, Arc<str>)], stored: bool) {
        self.settle(root_session_id, saving, |payload| match payload {
            durable::Payload::Saving(_) if stored => durable::Payload::Stored,
            durable::Payload::Saving(json) => durable::Payload::Unsaved(Arc::clone(json)),
            other => other.clone(),
        });
    }

    fn settle(
        &self,
        root_session_id: &str,
        settled: &[(AgentId, Arc<str>)],
        next: impl Fn(&durable::Payload) -> durable::Payload,
    ) {
        if settled.is_empty() {
            return;
        }
        self.checkpoints.update(|checkpoints| {
            for (id, key) in settled {
                if let Some(checkpoint) = checkpoints.get_mut(&(root_session_id.to_owned(), *id))
                    && checkpoint.key == *key
                {
                    checkpoint.payload = next(&checkpoint.payload);
                }
            }
        });
    }

    fn begin_pending_checkpoint(&self, root_session_id: &str) -> bool {
        if self.store_for(root_session_id).is_none() {
            return false;
        }
        self.scope_locks(root_session_id)
            .pending_checkpoints
            .fetch_add(1, Ordering::AcqRel);
        true
    }

    fn finish_pending_checkpoint(&self, root_session_id: &str) {
        let _ = self
            .scope_locks(root_session_id)
            .pending_checkpoints
            .try_update(Ordering::AcqRel, Ordering::Acquire, |pending| {
                pending.checked_sub(1)
            });
        self.changed();
    }

    fn capture_progress(
        self: &Arc<Self>,
        root_session_id: &str,
        id: AgentId,
        marks: &Arc<AgentMarks>,
    ) {
        if self.store_for(root_session_id).is_none() {
            return;
        }
        let key = (root_session_id.to_owned(), id);
        if self.journal_holds_conversation(root_session_id, marks) {
            return;
        }
        let marks = Arc::clone(marks);
        if !self.records_sessions(root_session_id) {
            let registry = Arc::clone(self);
            drop(platform::spawn(async move {
                let progressed = marks.completed_tool.swap(false, Ordering::AcqRel);
                let (harness, commits) = registry.capture_target(&key.0, key.1).await;
                if harness.is_none() {
                    return;
                }
                let root = key.0.clone();
                let id = key.1;
                registry
                    .state
                    .call(move |state| {
                        if let Some(session) = state
                            .scopes
                            .get_mut(&root)
                            .and_then(|scope| scope.sessions.get_mut(&id))
                        {
                            session.conversation = true;
                        }
                    })
                    .await;
                registry.recorded_conversation(&key.0, &marks, true);
                registry
                    .settle_capture(&key.0, key.1, commits, progressed)
                    .await;
                registry.changed();
            }));
            return;
        }
        if !marks.capturing.start() {
            return;
        }
        let registry = Arc::clone(self);
        drop(platform::spawn(async move {
            loop {
                let progressed = marks.completed_tool.swap(false, Ordering::AcqRel);
                let (harness, commits) = registry.capture_target(&key.0, key.1).await;
                if let Some(harness) = harness
                    && let Ok(snapshot) = harness.snapshot().await
                    && registry.running_harness(&key.0, key.1).await.is_some()
                {
                    let recorded = registry
                        .record_checkpoint_off_thread(&key.0, key.1, &marks, snapshot)
                        .await;
                    registry
                        .settle_capture(&key.0, key.1, commits.filter(|_| recorded), progressed)
                        .await;
                    registry.changed();
                } else if progressed {
                    marks.completed_tool.store(true, Ordering::Release);
                }
                if marks.capturing.finish() {
                    break;
                }
            }
        }));
    }

    async fn capture_target(
        &self,
        root_session_id: &str,
        id: AgentId,
    ) -> (Option<HarnessHandle>, Option<CommittedCalls>) {
        let root = root_session_id.to_owned();
        self.state
            .call(move |state| {
                let Some(session) = state
                    .scopes
                    .get(&root)
                    .and_then(|scope| scope.sessions.get(&id))
                else {
                    return (None, None);
                };
                let harness = (session.active() || matches!(session.status, AgentStatus::Running))
                    .then(|| session.harness().cloned())
                    .flatten();
                let commits = session.turn_revision().and_then(|revision| {
                    let call_ids = session
                        .in_flight_calls
                        .iter()
                        .filter(|call| call.live(durable::CallProgress::Committable))
                        .map(|call| call.call_id.clone())
                        .collect::<Vec<_>>();
                    let omitted = session.in_flight_progress.committable;
                    (!call_ids.is_empty() || omitted > 0).then_some(CommittedCalls {
                        revision,
                        call_ids,
                        omitted,
                    })
                });
                (harness, commits)
            })
            .await
    }

    async fn settle_capture(
        &self,
        root_session_id: &str,
        id: AgentId,
        commits: Option<CommittedCalls>,
        progressed: bool,
    ) {
        let root = root_session_id.to_owned();
        self.state.send(move |state| {
            let Some(session) = state
                .scopes
                .get_mut(&root)
                .and_then(|scope| scope.sessions.get_mut(&id))
            else {
                return;
            };
            if let Some(commits) = commits
                && session.turn_revision() == Some(commits.revision)
            {
                session.in_flight_calls.retain(|call| {
                    !(matches!(call.state, durable::CallState::Live { .. })
                        && commits.call_ids.contains(&call.call_id))
                });
                let progress = &mut session.in_flight_progress;
                let omitted = commits.omitted.min(progress.committable);
                progress.committable -= omitted;
                session.in_flight_omitted = session.in_flight_omitted.saturating_sub(omitted);
            }
            if progressed && session.active() {
                session.resume_attempts = 0;
            }
        });
    }

    async fn track_events(&self, root_session_id: &str, id: AgentId, events: Vec<TrackedEvent>) {
        let root = root_session_id.to_owned();
        let registry = self.shared();
        self.state.send(move |state| {
            let Some(session) = state
                .scopes
                .get_mut(&root)
                .and_then(|scope| scope.sessions.get_mut(&id))
            else {
                return;
            };
            let mut tracked = false;
            for event in events {
                match event {
                    TrackedEvent::Boundary => session.mark_boundary(),
                    TrackedEvent::Call(kind, call) => {
                        session.track_call(&kind, &call);
                        tracked = true;
                    }
                }
            }
            if tracked && let Some(registry) = registry {
                registry.changed();
            }
        });
    }

    async fn running_harness(&self, root_session_id: &str, id: AgentId) -> Option<HarnessHandle> {
        let root = root_session_id.to_owned();
        self.state
            .call(move |state| {
                state
                    .scopes
                    .get(&root)
                    .and_then(|scope| scope.sessions.get(&id))
                    .filter(|session| {
                        session.active() || matches!(session.status, AgentStatus::Running)
                    })
                    .and_then(|session| session.harness().cloned())
            })
            .await
    }
}

enum TrackedEvent {
    Boundary,
    Call(AgentEventKind, InFlightEvent),
}

#[derive(serde::Deserialize)]
struct InFlightEvent {
    call_id: String,
    #[serde(default)]
    tool: String,
    #[serde(default)]
    arguments: Option<Box<serde_json::value::RawValue>>,
    #[serde(default)]
    parent_call_id: Option<String>,

    #[serde(default)]
    cell: Option<InFlightCell>,

    #[serde(default)]
    metadata: Option<InFlightMetadata>,
}

#[derive(serde::Deserialize)]
struct InFlightCell {
    origin_call_id: String,
    running: bool,
}

#[derive(serde::Deserialize)]
struct InFlightMetadata {
    #[serde(default, rename = "_nanocode_code")]
    code: Option<InFlightCode>,
}

#[derive(serde::Deserialize)]
struct InFlightCode {
    #[serde(default)]
    origin_call_id: Option<Value>,
    #[serde(default)]
    running: Option<Value>,
}

impl InFlightEvent {
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

impl ChildSession {
    fn mark_boundary(&mut self) {
        for call in &mut self.in_flight_calls {
            if let durable::CallState::Live { progress, .. } = &mut call.state
                && *progress == durable::CallProgress::Settled
            {
                *progress = durable::CallProgress::Committable;
            }
        }
        let progress = &mut self.in_flight_progress;
        progress.committable = progress.committable.saturating_add(progress.settled);
        progress.settled = 0;
    }

    fn track_call(&mut self, kind: &AgentEventKind, call: &InFlightEvent) {
        match kind {
            AgentEventKind::ToolCall => {
                durable::retain_call(
                    &mut self.in_flight_calls,
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
                if let Some(existing) = self
                    .in_flight_calls
                    .iter_mut()
                    .find(|existing| existing.call_id == call.call_id)
                {
                    let yielded = matches!(cell, Some((origin, true)) if origin == call.call_id);
                    match &mut existing.state {
                        durable::CallState::Restored {
                            result_recorded, ..
                        } => *result_recorded = true,
                        durable::CallState::Live {
                            parent_call_id: None,
                            progress,
                        } => {
                            *progress = if yielded {
                                durable::CallProgress::Yielded
                            } else {
                                durable::CallProgress::Settled
                            };
                        }
                        durable::CallState::Live { progress, .. } => {
                            if *progress == durable::CallProgress::Started {
                                *progress = durable::CallProgress::Recorded;
                            }
                        }
                    }
                }

                if top_level && let Some((origin, false)) = cell {
                    for listed in &mut self.in_flight_calls {
                        let durable::CallState::Live {
                            parent_call_id,
                            progress,
                        } = &mut listed.state
                        else {
                            continue;
                        };
                        let settles = match *progress {
                            durable::CallProgress::Recorded => {
                                parent_call_id.as_deref() == Some(origin)
                            }
                            durable::CallProgress::Yielded => listed.call_id == origin,
                            _ => false,
                        };
                        if settles {
                            *progress = durable::CallProgress::Settled;
                        }
                    }
                    if let Some(count) = self.in_flight_progress.nested.remove(origin) {
                        self.in_flight_progress.settled =
                            self.in_flight_progress.settled.saturating_add(count);
                    }
                }
            }
            _ => {}
        }
    }
}

impl Registry {
    fn forget(&self, root_session_id: &str, cooled: &[(AgentId, Vec<String>)]) {
        if cooled.is_empty() {
            return;
        }
        let key = |id: &AgentId| (root_session_id.to_owned(), *id);
        self.checkpoints.update(|checkpoints| {
            for (id, _) in cooled {
                checkpoints.remove(&key(id));
            }
        });
        self.wait_reported.update(|reported| {
            for (id, waiters) in cooled {
                reported.remove(&key(id));
                for waiter in waiters {
                    reported.remove(&(waiter.clone(), *id));
                }
            }
        });
    }

    pub(super) async fn warm(
        &self,
        root_session_id: &str,
        ids: Vec<AgentId>,
    ) -> std::io::Result<Option<Warm>> {
        let Some(store) = self.store_for(root_session_id) else {
            return Ok(None);
        };
        let root = root_session_id.to_owned();
        let (leaves, cold) = self
            .state
            .call(move |state| {
                let Some(scope) = state.scopes.get_mut(&root) else {
                    return (Vec::new(), false);
                };
                let leaves = scope.warming(&ids);
                for leaf in &leaves {
                    *scope.pins.entry(*leaf).or_default() += 1;
                }
                let cold = leaves.iter().any(|leaf| scope.cold.contains_key(leaf));
                (leaves.into_iter().collect::<Vec<_>>(), cold)
            })
            .await;
        if leaves.is_empty() {
            return Ok(None);
        }
        let warm = Warm {
            state: self.state.clone(),
            registry: self.this.get().cloned().unwrap_or_default(),
            root: root_session_id.to_owned(),
            leaves,
        };
        if cold {
            self.thaw(root_session_id, store.as_ref(), &warm.leaves)
                .instrument(nanocode_trace::info_span!(
                    "subagent.warm",
                    session.root_id = root_session_id,
                    leaf.count = warm.leaves.len()
                ))
                .await?;
        }
        Ok(Some(warm))
    }

    async fn thaw(
        &self,
        root_session_id: &str,
        store: &dyn SubagentStore,
        leaves: &[u64],
    ) -> std::io::Result<()> {
        let locks = self.scope_locks(root_session_id);
        let _warming = locks.warming.enter().await;
        let root = root_session_id.to_owned();
        let wanted = leaves.to_vec();
        let cold = self
            .state
            .call(move |state| {
                let Some(scope) = state.scopes.get(&root) else {
                    return Vec::new();
                };
                wanted
                    .iter()
                    .filter_map(|leaf| Some((*leaf, Arc::clone(&scope.cold.get(leaf)?.key))))
                    .collect::<Vec<_>>()
            })
            .await;
        if cold.is_empty() {
            return Ok(());
        }
        let thawed = cold.iter().map(|(leaf, _)| *leaf).collect::<Vec<_>>();
        let agents = durable::load_cold(store, root_session_id, cold).await?;
        let mut sessions = Vec::with_capacity(agents.len());
        let mut checkpoints = Vec::new();
        let mut waited = Vec::new();
        for (agent, cached) in agents {
            let id = agent.descriptor.id;
            if let Some(key) = &agent.checkpoint_ref {
                checkpoints.push((id, durable::JournalCheckpoint::stored(key.clone(), None)));
            }
            let (session, _, _) = durable::restored_session(agent, None)?;
            if session.conversation && !store.records_sessions() {
                session.marks.captured.store(true, Ordering::Release);
            }
            waited.extend(
                session
                    .waited
                    .iter()
                    .map(|waited| ((waited.session.clone(), id), waited.mark)),
            );
            sessions.push((id, session, cached));
        }
        let root = root_session_id.to_owned();
        let warmed = self
            .state
            .call(move |state| {
                let last_used = state.next_access();
                let Some(scope) = state.scopes.get_mut(&root) else {
                    return Vec::new();
                };
                for leaf in &thawed {
                    scope.cold.remove(leaf);
                }
                sessions
                    .into_iter()
                    .map(|(id, mut session, cached)| {
                        session.last_used = last_used;
                        (id, cached.at(scope.sessions.insert_stored(id, session)))
                    })
                    .collect::<Vec<_>>()
            })
            .await;
        if !checkpoints.is_empty() {
            self.checkpoints.update(|stored| {
                for (id, checkpoint) in &checkpoints {
                    stored.insert((root_session_id.to_owned(), *id), checkpoint.clone());
                }
            });
        }
        if !waited.is_empty() {
            self.wait_reported
                .update(|reported| reported.extend(waited.iter().cloned()));
        }
        self.send_journal(root_session_id, Some(JournalRequest::Warmed(warmed)));
        Ok(())
    }

    async fn warm_subtree(&self, session_id: &str, id: AgentId) -> std::io::Result<Option<Warm>> {
        let session = session_id.to_owned();
        let (root, ids) = self
            .state
            .call(move |state| {
                let root = state.root_session_id(&session).to_owned();
                let ids = state
                    .scopes
                    .get(&root)
                    .and_then(|scope| scope.topology.subtree_postorder(id).ok())
                    .unwrap_or_default();
                (root, ids)
            })
            .await;
        self.warm(&root, ids).await
    }

    async fn warm_all(&self, session_id: &str) -> std::io::Result<Option<Warm>> {
        let session = session_id.to_owned();
        let (root, ids) = self
            .state
            .call(move |state| {
                let root = state.root_session_id(&session).to_owned();
                let ids = state
                    .scopes
                    .get(&root)
                    .filter(|scope| !scope.cold.is_empty())
                    .map(|scope| scope.topology.ids())
                    .unwrap_or_default();
                (root, ids)
            })
            .await;
        self.warm(&root, ids).await
    }

    async fn of_session<T, F>(&self, session_id: &str, read: F) -> Option<T>
    where
        T: Send + 'static,
        F: Fn(&ChildSession) -> T + Clone + Send + 'static,
    {
        let mut warm = None;
        loop {
            let session = session_id.to_owned();
            let read = read.clone();
            let found = self
                .state
                .call(move |state| {
                    state
                        .session_or_cold(&session)
                        .map(|session| session.map(read))
                })
                .await;
            match found {
                Ok(found) => return found,
                Err(_) if warm.is_some() => return None,
                Err((root, id)) => warm = Some(self.warm(&root, vec![id]).await.ok()??),
            }
        }
    }

    pub async fn restore(
        self: &Arc<Self>,
        root_session_id: &str,
    ) -> std::io::Result<RestoreReport> {
        self.restore_inner(root_session_id)
            .instrument(nanocode_trace::info_span!(
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
        let folds_all = !journal.agents.is_empty()
            || !journal.agent_records.is_empty()
            || journal.agent_pages.is_some()
            || (journal.version <= durable::FIELDS_JOURNAL_VERSION
                && self.journal_version.load(Ordering::Relaxed) > durable::FIELDS_JOURNAL_VERSION);
        let mut written = durable::JournalCache::default().written_as(journal.version);
        let deltas = durable::load_deltas(
            store.as_ref(),
            root_session_id,
            journal.deltas,
            &mut written,
        )
        .await?;
        let messages = journal
            .messages
            .hydrate(
                store.as_ref(),
                root_session_id,
                deltas.messages,
                &mut written,
            )
            .await?;
        let mut records = journal.agent_records;
        if let Some(pages) = journal.agent_pages {
            records.extend(pages.agent_records(store.as_ref(), root_session_id).await?);
        }
        let mut agents = journal.agents;
        agents.extend(durable::load_agents(store.as_ref(), root_session_id, records).await?);
        let leaves = durable::load_leaves(
            store.as_ref(),
            root_session_id,
            journal.agent_leaves,
            deltas.agents,
            &mut written,
        )
        .await?;
        agents.extend(leaves.agents);
        let cold = leaves.cold;
        let mut written = written.stored(folds_all);
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
        let registry = self.shared();
        let root = root_session_id.to_owned();
        let records_sessions = store.records_sessions();
        let (report, restored_sessions, waited, written) = self
            .state
            .call(move |state| -> std::io::Result<_> {
                let root_session_id = root.as_str();
                let mut report = RestoreReport::default();
                let mut waited = Vec::new();
                let mut checkpoints = Vec::new();
                if state
                    .scopes
                    .get(root_session_id)
                    .is_some_and(|scope| !scope.sessions.is_empty() || !scope.cold.is_empty())
                {
                    return Err(std::io::Error::other(
                        "subagent scope already has live children and cannot be restored",
                    ));
                }
                let lineages = durable::journaled_lineages(root_session_id, &agents);
                let mut restored_sessions = Vec::with_capacity(agents.len());
                let mut stubs = cold.iter().flat_map(|(_, _, stubs)| stubs).peekable();
                for (agent, lineage) in agents.into_iter().zip(lineages) {
                    let id = agent.descriptor.id;
                    while let Some(stub) = stubs.next_if(|stub| stub.id < id) {
                        state.insert_cold(root_session_id, stub)?;
                        restored_sessions.push(stub.session.clone());
                        report.restored += 1;
                    }
                    let checkpoint = agent.snapshot(&lineage)?;
                    let (session, resume, lost) = durable::restored_session(agent, checkpoint)?;
                    if session.conversation
                        && registry
                            .as_ref()
                            .is_some_and(|registry| !registry.records_sessions(root_session_id))
                    {
                        session.marks.captured.store(true, Ordering::Release);
                    }
                    if session.completion.revision().is_some()
                        && session.status.wakes_parent()
                        && !resume
                    {
                        report.completed.push(id);
                    }
                    match (stored.remove(&id), session.stored_runtime.checkpoint()) {
                        (Some(checkpoint), _) => checkpoints.push((id, checkpoint)),

                        (None, Some(snapshot)) => {
                            let checkpoint = durable::JournalCheckpoint::encode(
                                snapshot.clone(),
                                records_sessions,
                            )?;
                            if let Some(registry) = &registry {
                                registry.insert_checkpoint(root_session_id, id, checkpoint);
                            }
                        }
                        (None, None) => {}
                    }
                    restored_sessions.push(session.descriptor.session_id.clone());
                    waited.extend(
                        session
                            .waited
                            .iter()
                            .map(|waited| ((waited.session.clone(), id), waited.mark)),
                    );
                    if matches!(session.stored_runtime, StoredRuntime::Unstarted(_)) {
                        report.unstarted.push(id);
                    }
                    let generation = state.insert_restored(root_session_id, session, !resume)?;
                    if let Some(written) = &mut written
                        && !resume
                    {
                        written.restored(id, generation);
                    }
                    report.restored += 1;
                    if resume {
                        report.interrupted.push(id);
                    }
                    if lost {
                        report.unrecoverable.push(id);
                    }
                }
                for stub in stubs {
                    state.insert_cold(root_session_id, stub)?;
                    restored_sessions.push(stub.session.clone());
                    report.restored += 1;
                }
                let scope = state.scope_mut(root_session_id);
                scope.messages = messages;
                for (leaf, stored, _) in &cold {
                    scope.cold.insert(*leaf, stored.clone());
                }
                if let Some(registry) = &registry {
                    registry.checkpoints.update(|stored| {
                        for (id, checkpoint) in &checkpoints {
                            stored.insert((root_session_id.to_owned(), *id), checkpoint.clone());
                        }
                    });
                    if written.is_none() {
                        registry.checkpoint_changes.update(|changes| {
                            changes
                                .entry(root_session_id.to_owned())
                                .or_default()
                                .extend(checkpoints.iter().map(|(id, _)| *id));
                        });
                    }
                }
                Ok((report, restored_sessions, waited, written))
            })
            .await?;
        if let Some(written) = written {
            self.send_journal(
                root_session_id,
                Some(JournalRequest::Stored(Box::new(written))),
            );
        }
        self.roots.update(|roots| {
            for session_id in &restored_sessions {
                roots.insert(session_id.clone(), root_session_id.to_owned());
            }
        });
        if !waited.is_empty() {
            self.wait_reported
                .update(|reported| reported.extend(waited.iter().cloned()));
        }

        if !report.interrupted.is_empty() || !report.completed.is_empty() {
            self.save_scope(root_session_id).await.map_err(|error| {
                std::io::Error::new(
                    error.kind(),
                    format!("could not record subagent resume attempts: {error}"),
                )
            })?;
        }
        if self.manual_resume.load(std::sync::atomic::Ordering::SeqCst) {
            self.deferred_resume.update(|deferred| {
                deferred.insert(
                    root_session_id.to_owned(),
                    report.interrupted.iter().copied().collect(),
                );
            });
        } else {
            self.pending_resume.update(|pending| {
                pending.insert(
                    root_session_id.to_owned(),
                    report.interrupted.iter().copied().collect(),
                );
            });
        }
        self.changed();
        Ok(report)
    }

    async fn save_scope(&self, root_session_id: &str) -> std::io::Result<()> {
        self.request_journal(root_session_id, true)
            .instrument(nanocode_trace::info_span!(
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
        let JournalWrite {
            payload,
            records,
            acknowledged,
        } = write;
        store
            .save(root_session_id, payload, records)
            .instrument(nanocode_trace::info_span!(
                "subagent.journal.save",
                session.root_id = root_session_id
            ))
            .await?;
        self.acknowledge(root_session_id, &acknowledged);
        Ok(())
    }

    pub async fn resume_interrupted(
        self: &Arc<Self>,
        root_session_id: &str,
    ) -> Vec<(AgentId, std::io::Result<MessageReceipt>)> {
        self.resume_interrupted_inner(root_session_id)
            .instrument(nanocode_trace::info_span!(
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
        let resumes = ids
            .into_iter()
            .map(|id| {
                let registry = Arc::clone(self);
                let root = root_session_id.to_owned();
                let resume = async move { registry.resume_one(&root, id).await };
                (id, platform::spawn(resume.in_current_span()))
            })
            .collect::<Vec<_>>();
        let mut resumed = Vec::with_capacity(resumes.len());
        for (id, resume) in resumes {
            let result = match resume.await {
                Ok(result) => result,
                Err(error) => Err(std::io::Error::other(error.to_string())),
            };
            resumed.push((id, result));
        }
        resumed
    }

    async fn resume_one(
        self: &Arc<Self>,
        root_session_id: &str,
        id: AgentId,
    ) -> std::io::Result<MessageReceipt> {
        let root = root_session_id.to_owned();
        let message = self
            .state
            .call(move |state| {
                state
                    .scopes
                    .get(&root)
                    .and_then(|scope| scope.sessions.get(&id))
                    .map(|session| {
                        durable::resume_message(
                            &session.in_flight_calls,
                            session.in_flight_omitted,
                            Some(&session.descriptor.task),
                        )
                    })
                    .unwrap_or_else(|| durable::RESUME_MESSAGE.to_owned())
            })
            .await;
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
        self.pending_resume.update(|pending| {
            if let Some(ids) = pending.get_mut(root_session_id) {
                ids.remove(&id);
                if ids.is_empty() {
                    pending.remove(root_session_id);
                }
            }
        });
        self.changed();
        result
    }

    async fn fail_unresumable(&self, root_session_id: &str, id: AgentId, mut error: String) {
        let root = root_session_id.to_owned();
        let settled = self.state.call(move |state| {
            let session = state
                .scopes
                .get_mut(&root)
                .and_then(|scope| scope.sessions.get_mut(&id))
                .filter(|session| {
                    !session.active()
                        && session.harness().is_none()
                        && matches!(session.status, AgentStatus::Interrupted)
                })?;
            if let Some(evidence) =
                durable::in_flight_evidence(&session.in_flight_calls, session.in_flight_omitted)
            {
                error.push_str("\n\n");
                error.push_str(&evidence);
            }
            session.status = AgentStatus::Failed { error };
            session.resume_attempts = 0;
            session.in_flight_calls = Vec::new();
            session.in_flight_omitted = 0;
            session.in_flight_progress = durable::OmittedProgress::default();
            let settled = session.next_instruction_revision;
            session.settled_revision = Some(settled);
            let completion = session.descriptor.parent.is_none().then_some(settled);
            if let Some(revision) = completion {
                session.settle_completion(revision);
            }
            Some((
                session.status.clone(),
                completion,
                Arc::clone(&session.marks),
            ))
        });
        let Some((status, completion, lane)) = settled.await else {
            return;
        };
        self.enqueue_status(
            root_session_id,
            id,
            lane,
            OrderedJob::Settle {
                status,
                completion,
                harness: None,
            },
        );
        self.changed();
    }

    pub(super) async fn reserve_turn(&self) -> std::io::Result<TurnCapacity> {
        if self.queue_spawns.load(Ordering::Relaxed) {
            return Ok(self.capacity.acquire(Waiter::Spawn).await);
        }
        self.capacity.reserve()
    }

    pub(super) fn batch_startup(self: &Arc<Self>) -> BatchStartup {
        BatchStartup {
            registry: Arc::clone(self),
            cleanup: None,
        }
    }

    pub(super) async fn reserve_turns(&self, count: usize) -> std::io::Result<Vec<TurnCapacity>> {
        if self.queue_spawns.load(Ordering::Relaxed) {
            let mut turns = Vec::with_capacity(count);
            for _ in 0..count {
                turns.push(self.capacity.acquire(Waiter::Spawn).await);
            }
            return Ok(turns);
        }
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

    pub fn set_queued_spawns(&self, queued: bool) {
        self.queue_spawns.store(queued, Ordering::Relaxed);
    }

    pub fn set_max_resident(&self, limit: usize) {
        self.max_resident_explicit.store(true, Ordering::Relaxed);
        self.max_resident.store(limit, Ordering::Relaxed);
    }

    pub fn set_hibernation(&self, hibernation: bool) {
        self.hibernation.store(hibernation, Ordering::Relaxed);
    }

    pub(super) fn hibernates(&self) -> bool {
        self.hibernation.load(Ordering::Relaxed)
    }

    pub(super) async fn begin_hibernation(
        &self,
        root_session_id: &str,
        id: AgentId,
    ) -> Option<Turn> {
        let root = root_session_id.to_owned();
        self.state
            .call(move |state| {
                let scope = state.scopes.get_mut(&root)?;
                if scope.life.frozen() {
                    return None;
                }
                scope.sessions.refresh();
                let session = scope.sessions.get(&id)?;
                if session.turn.as_ref().is_none_or(|turn| turn.interrupted)
                    || !session
                        .harness()
                        .is_some_and(|harness| !harness.answering_side_questions())
                {
                    return None;
                }
                if scope.holds_descendants(id)? {
                    return None;
                }
                session.residency_lock.try_enter()
            })
            .await
    }

    pub(super) fn hibernation_runtime(
        &self,
        root_session_id: &str,
        id: AgentId,
    ) -> Option<StoredRuntime> {
        let journaled = self.store_for(root_session_id).is_some()
            && self
                .checkpoints
                .read(|checkpoints| checkpoints.contains_key(&(root_session_id.to_owned(), id)));
        journaled.then_some(StoredRuntime::Journaled)
    }

    pub(super) async fn hibernated(
        self: &Arc<Self>,
        root_session_id: &str,
        id: AgentId,
        stored: StoredRuntime,
        hibernated: Hibernated,
        lock: Turn,
    ) {
        let finished = hibernated.call.finished();
        let root = root_session_id.to_owned();
        let asleep = self
            .state
            .call(move |state| {
                let session = state
                    .scopes
                    .get_mut(&root)
                    .and_then(|scope| scope.sessions.get_mut(&id))?;
                session.residency = Residency::Evicted;
                session.stored_runtime = stored;
                session.hibernated = Some(hibernated);
                session.hibernations += 1;
                drop(session.harness_task.take());
                drop(session.event_task.take());
                Some((
                    session.descriptor.session_id.clone(),
                    session.turn.as_ref().is_some_and(|turn| turn.interrupted),
                ))
            })
            .await;
        let interrupted = asleep.as_ref().is_some_and(|(_, interrupted)| *interrupted);
        if let Some((session_id, _)) = asleep {
            self.session_handles.update(|handles| {
                handles.remove(&session_id);
            });
            if let Some(released) = Option::clone(&self.session_released.get()) {
                released(&session_id);
            }
        }
        drop(lock);
        self.changed();
        let registry = Arc::downgrade(self);
        let root = root_session_id.to_owned();
        let capacity = self.capacity.clone();
        drop(platform::spawn(async move {
            if !interrupted {
                finished.await;
                let granted = capacity.acquire(Waiter::Resident).await;
                let Some(registry) = registry.upgrade() else {
                    return;
                };
                let scoped = root.clone();
                registry
                    .state
                    .call(move |state| {
                        if let Some(hibernated) = state
                            .scopes
                            .get_mut(&scoped)
                            .and_then(|scope| scope.sessions.get_mut(&id))
                            .and_then(|session| session.hibernated.as_mut())
                        {
                            hibernated.capacity = Some(granted);
                        }
                    })
                    .await;
            }
            if let Some(registry) = registry.upgrade() {
                Box::pin(registry.wake(&root, id)).await;
            }
        }));
    }

    async fn wake(self: &Arc<Self>, root_session_id: &str, id: AgentId) {
        let root = root_session_id.to_owned();
        let asleep = self
            .state
            .call(move |state| {
                state
                    .scopes
                    .get(&root)
                    .and_then(|scope| scope.sessions.get(&id))
                    .is_some_and(|session| session.hibernated.is_some())
            })
            .await;
        if !asleep {
            return;
        }
        if let Err(error) = self
            .rehydrate(root_session_id, id, MessagePurpose::Coordinate)
            .instrument(nanocode_trace::info_span!(
                "subagent.wake",
                session.root_id = root_session_id
            ))
            .await
        {
            nanocode_trace::warn!(%id, %error, "could not wake a hibernated subagent");
            let failure = format!("agent {id} could not wake: {error}");
            self.settle_hibernated(
                root_session_id,
                &[id],
                || NanocodeError::InvalidRequest(failure.clone()),
                &failure,
            )
            .await;
        }
    }

    async fn settle_hibernated(
        self: &Arc<Self>,
        root_session_id: &str,
        ids: &[AgentId],
        error: impl Fn() -> NanocodeError,
        undelivered: &str,
    ) {
        let root = root_session_id.to_owned();
        let asleep = ids.to_vec();
        let stopped = self
            .state
            .call(move |state| {
                let Some(scope) = state.scopes.get_mut(&root) else {
                    return Vec::new();
                };
                let mut stopped = Vec::new();
                for id in asleep {
                    let asleep = scope
                        .sessions
                        .get(&id)
                        .is_some_and(|session| session.hibernated.is_some());
                    if asleep
                        && let Some(hibernated) = scope
                            .sessions
                            .get_mut(&id)
                            .and_then(|session| session.hibernated.take())
                    {
                        stopped.push((id, hibernated));
                    }
                }
                stopped
            })
            .await;
        for (id, hibernated) in stopped {
            let Hibernated {
                call,
                capacity,
                urgent,
                deferred,
            } = hibernated;
            drop((call, capacity));
            for message in urgent.into_iter().chain(deferred) {
                self.message_failed(root_session_id, message.id, undelivered.to_owned())
                    .await;
            }
            self.harness_turn_finished(root_session_id, id, Err(error()))
                .await;
        }
    }

    pub async fn is_root_session(&self, session_id: &str) -> bool {
        let session_id = session_id.to_owned();
        !self
            .state
            .call(move |state| state.root_by_session.contains_key(&session_id))
            .await
    }

    pub async fn child_session_id(&self, caller: &str, id: AgentId) -> std::io::Result<String> {
        self.await_restored(caller).await?;
        let _warm = self.warm(&self.root_of(caller), vec![id]).await?;
        let caller = caller.to_owned();
        self.state
            .call(move |state| {
                let root = state.authorize(&caller, id)?;
                state
                    .scopes
                    .get(&root)
                    .and_then(|scope| scope.sessions.get(&id))
                    .map(|session| session.descriptor.session_id.clone())
                    .ok_or_else(|| std::io::Error::other(format!("unknown agent_id {id}")))
            })
            .await
    }

    #[doc(hidden)]
    pub async fn host_context(&self, root_session_id: &str, id: AgentId) -> Option<Arc<str>> {
        let _warm = self.warm(root_session_id, vec![id]).await.ok()?;
        let root = root_session_id.to_owned();
        self.state
            .call(move |state| {
                state
                    .scopes
                    .get(&root)
                    .and_then(|scope| scope.sessions.get(&id))
                    .and_then(|session| session.host_context.as_ref().map(Arc::clone))
            })
            .await
    }

    pub(super) async fn descriptor_for_session(&self, session_id: &str) -> Option<AgentDescriptor> {
        self.of_session(session_id, |session| session.descriptor.clone())
            .await
    }

    #[doc(hidden)]
    pub async fn host_context_for_session(&self, session_id: &str) -> Option<Arc<str>> {
        self.of_session(session_id, |session| session.host_context.clone())
            .await
            .flatten()
    }

    async fn lineage(&self, session_id: &str) -> Vec<AgentId> {
        let session_id = session_id.to_owned();
        self.state
            .call(move |state| {
                let Some(scope) = state
                    .root_by_session
                    .get(&session_id)
                    .and_then(|root| state.scopes.get(root))
                else {
                    return Vec::new();
                };
                let mut lineage = Vec::new();
                let mut next = scope.topology.agent_for_session(&session_id);
                while let Some(id) = next {
                    lineage.push(id);
                    next = scope.topology.parent(id);
                }
                lineage
            })
            .await
    }

    pub(super) async fn budgets(&self, session_id: &str) -> Vec<(AgentId, UsdAmount)> {
        let session_id = session_id.to_owned();
        self.state
            .call(move |state| {
                let Some(scope) = state
                    .root_by_session
                    .get(&session_id)
                    .and_then(|root| state.scopes.get(root))
                else {
                    return Vec::new();
                };
                let mut budgets = Vec::new();
                let mut next = scope.topology.agent_for_session(&session_id);
                while let Some(session) = next.and_then(|id| scope.sessions.get(&id)) {
                    if let Some(budget) = session.descriptor.budget_usd {
                        budgets.push((session.descriptor.id, budget));
                    }
                    next = session.descriptor.parent;
                }
                budgets
            })
            .await
    }

    pub(super) async fn set_budget(
        &self,
        root_session_id: &str,
        id: AgentId,
        budget: Option<UsdAmount>,
    ) -> std::io::Result<AgentSummary> {
        let _warm = self.warm(root_session_id, vec![id]).await?;
        let root = root_session_id.to_owned();
        let (descriptor, summary) = self
            .state
            .call(move |state| {
                let session = state
                    .scopes
                    .get_mut(&root)
                    .and_then(|scope| scope.sessions.get_mut(&id))?;
                session.descriptor.budget_usd = budget;
                let mut descriptor = session.descriptor.clone();
                descriptor.task = session.binding_task.clone();
                Some((descriptor, session.summary()))
            })
            .await
            .ok_or_else(|| std::io::Error::other(format!("unknown agent_id {id}")))?;
        self.persist_durably(root_session_id).await?;
        self.send(root_session_id, AgentUpdate::Added(descriptor));
        self.changed();
        Ok(summary)
    }

    pub(super) async fn reserve(&self, session_id: &str) -> std::io::Result<AgentReservation> {
        self.reserve_many(session_id, 1)
            .await?
            .pop()
            .ok_or_else(|| std::io::Error::other("no subagent was reserved"))
    }

    pub(super) async fn reserve_many(
        &self,
        session_id: &str,
        count: usize,
    ) -> std::io::Result<Vec<AgentReservation>> {
        let mut warm = None;
        loop {
            let session = session_id.to_owned();
            let reserved = self
                .state
                .call(move |state| {
                    state.session_or_cold(&session).map(drop)?;
                    Ok((0..count)
                        .map(|_| state.reserve_for(&session))
                        .collect::<std::io::Result<Vec<_>>>())
                })
                .await;
            match reserved {
                Ok(reserved) => {
                    let warm = warm.map(Arc::new);
                    let mut reserved = reserved?;
                    for reservation in &mut reserved {
                        reservation.warm.clone_from(&warm);
                    }
                    return Ok(reserved);
                }
                Err((root, id)) if warm.is_none() => {
                    warm = self.warm(&root, vec![id]).await?;
                    if warm.is_none() {
                        return Err(std::io::Error::other("subagent parent disappeared"));
                    }
                }
                Err(_) => return Err(std::io::Error::other("subagent parent disappeared")),
            }
        }
    }

    pub(super) async fn submit_result_durably(
        self: &Arc<Self>,
        session_id: &str,
        instruction_revision: Option<u64>,
        output: Value,
        resumed: bool,
    ) -> std::io::Result<(SubmissionOutcome, bool)> {
        self.submit_result_durably_inner(session_id, instruction_revision, output, resumed)
            .instrument(nanocode_trace::info_span!(
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
        resumed: bool,
    ) -> std::io::Result<(SubmissionOutcome, bool)> {
        self.await_restored(session_id).await?;
        let session = session_id.to_owned();
        let (outcome, root) = self
            .state
            .call(move |state| -> std::io::Result<_> {
                let outcome =
                    state.submit_result(&session, instruction_revision, output, resumed)?;
                Ok((outcome, state.root_session_id(&session).to_owned()))
            })
            .await?;
        let durable = match outcome {
            SubmissionOutcome::Accepted { .. } => match self.persist_durably(&root).await {
                Ok(()) => true,
                Err(error) if is_fenced(&error) => {
                    nanocode_trace::debug!(%error, %root, "accepted subagent result is not journaled: another process owns the journal");
                    self.changed();
                    false
                }
                Err(error) => {
                    nanocode_trace::warn!(%error, %root, "accepted subagent result is not journaled yet");
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
            .instrument(nanocode_trace::info_span!(
                "subagent.journal.persist",
                session.root_id = root_session_id
            ))
            .await
    }

    fn enqueue_status(
        &self,
        root_session_id: &str,
        id: AgentId,
        marks: Arc<AgentMarks>,
        job: OrderedJob,
    ) {
        let Some(registry) = self.this.get().cloned() else {
            self.send(
                root_session_id,
                match job {
                    OrderedJob::Status(status) => AgentUpdate::Status { id, status },
                    OrderedJob::Completion { status, revision } => AgentUpdate::Completion {
                        id,
                        status,
                        revision,
                    },
                    OrderedJob::Settle {
                        status, completion, ..
                    } => settled_update(id, status, completion),
                },
            );
            return;
        };
        marks.statuses.jobs.push(job);
        if marks.statuses.running.swap(true, Ordering::AcqRel) {
            return;
        }
        let root = root_session_id.to_owned();
        drop(platform::spawn(async move {
            let lane = &marks.statuses;
            loop {
                for job in lane.jobs.drain() {
                    let Some(registry) = registry.upgrade() else {
                        return;
                    };
                    registry.run_ordered(&root, id, &marks, job).await;
                }
                lane.running.store(false, Ordering::Release);
                if lane.jobs.is_empty() || lane.running.swap(true, Ordering::AcqRel) {
                    return;
                }
            }
        }));
    }

    async fn run_ordered(
        self: &Arc<Self>,
        root_session_id: &str,
        id: AgentId,
        marks: &AgentMarks,
        job: OrderedJob,
    ) {
        match job {
            OrderedJob::Status(status) => {
                self.send_unless_closed(root_session_id, id, AgentUpdate::Status { id, status })
                    .await
            }
            OrderedJob::Completion { status, revision } => {
                self.send_unless_closed(
                    root_session_id,
                    id,
                    AgentUpdate::Completion {
                        id,
                        status,
                        revision,
                    },
                )
                .await
            }
            OrderedJob::Settle {
                status,
                completion,
                harness,
            } => {
                if let Some(harness) = harness {
                    if !self.journal_holds_conversation(root_session_id, marks)
                        && let Ok(snapshot) = harness.snapshot().await
                    {
                        self.record_checkpoint(root_session_id, id, marks, snapshot);
                    }
                    self.finish_pending_checkpoint(root_session_id);
                }

                match self.persist_durably(root_session_id).await {
                    Ok(()) => {}
                    Err(error) if is_fenced(&error) => nanocode_trace::debug!(
                        %error, %root_session_id,
                        "settled subagent turn not journaled: another process owns the journal"
                    ),
                    Err(error) => nanocode_trace::warn!(
                        %error, %root_session_id,
                        "settled subagent turn not journaled yet; the writer retries"
                    ),
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
        let Some(registry) = self.shared() else {
            return;
        };
        let root = root_session_id.to_owned();
        self.state
            .call(move |state| {
                let closed = state
                    .scopes
                    .get(&root)
                    .and_then(|scope| scope.sessions.get(&id))
                    .is_some_and(|session| matches!(session.status, AgentStatus::Closed));
                let stale = closed
                    && matches!(&update, AgentUpdate::Status { status, .. } if !matches!(status, AgentStatus::Closed));
                if !stale {
                    registry.send(&root, update);
                }
            })
            .await;
    }

    pub async fn acknowledge_completion(&self, root_session_id: &str, id: AgentId, revision: u64) {
        let root = root_session_id.to_owned();
        let acknowledged = self
            .state
            .call(move |state| {
                match state
                    .scopes
                    .get_mut(&root)
                    .and_then(|scope| scope.sessions.get_mut(&id))
                {
                    Some(session) if session.completion.awaits(revision) => {
                        session.clear_completion();
                        let mark = wait_mark(&session.status, revision);
                        session.waited.retain(|waited| waited.session != root);
                        session.waited.push(durable::Waited {
                            session: root.clone(),
                            call: String::new(),
                            mark,
                        });
                        true
                    }
                    _ => false,
                }
            })
            .await;
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
        nanocode_trace::warn!(%id, revision, ?delay, "subagent completion was not delivered; retrying");

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
            let lookup = key.clone();
            let status = registry
                .state
                .call(move |state| {
                    state
                        .scopes
                        .get(&lookup.0)
                        .and_then(|scope| scope.sessions.get(&lookup.1))
                        .filter(|session| {
                            session.completion.awaits(revision) && session.status.wakes_parent()
                        })
                        .map(|session| (session.status.clone(), Arc::clone(&session.marks)))
                })
                .await;
            if let Some((status, lane)) = status {
                registry.enqueue_status(
                    &key.0,
                    key.1,
                    lane,
                    OrderedJob::Completion { status, revision },
                );
            }
        }));
    }

    async fn announce_restored_completions(&self, root_session_id: &str, ids: &[AgentId]) {
        for &id in ids {
            let root = root_session_id.to_owned();
            let announced = self
                .state
                .call(move |state| {
                    let session = state
                        .scopes
                        .get_mut(&root)
                        .and_then(|scope| scope.sessions.get_mut(&id))?;
                    let revision = session.completion.revision()?;
                    if !session.status.wakes_parent() {
                        return None;
                    }
                    let descriptor = std::mem::take(&mut session.announce).then(|| {
                        let mut descriptor = session.descriptor.clone();
                        descriptor.task = session.binding_task.clone();
                        descriptor
                    });
                    Some((
                        descriptor,
                        session.status.clone(),
                        revision,
                        Arc::clone(&session.marks),
                    ))
                })
                .await;
            let Some((descriptor, status, revision, lane)) = announced else {
                continue;
            };
            if let Some(descriptor) = descriptor {
                self.send(root_session_id, AgentUpdate::Added(descriptor));
            }
            self.enqueue_status(
                root_session_id,
                id,
                lane,
                OrderedJob::Completion { status, revision },
            );
        }
        self.changed();
    }

    pub(super) async fn begin_turn_steer(
        &self,
        root_session_id: &str,
        id: AgentId,
    ) -> Option<TurnSteer> {
        let root = root_session_id.to_owned();
        self.state
            .call(move |state| state.begin_turn_steer(&root, id))
            .await
    }

    pub(super) async fn turn_interrupted(&self, root_session_id: &str, id: AgentId) -> bool {
        let root = root_session_id.to_owned();
        self.state
            .call(move |state| {
                state
                    .scopes
                    .get(&root)
                    .and_then(|scope| scope.sessions.get(&id))
                    .and_then(|session| session.turn.as_ref())
                    .is_some_and(|turn| turn.interrupted)
            })
            .await
    }

    pub(super) async fn finish_turn_steer(
        &self,
        root_session_id: &str,
        steer: TurnSteer,
        committed: bool,
    ) {
        let root = root_session_id.to_owned();
        self.state
            .send(move |state| state.finish_turn_steer(&root, steer, committed));
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn insert(
        self: &Arc<Self>,
        root_session_id: String,
        descriptor: AgentDescriptor,
        host_context: Option<Arc<str>>,
        agent: Agent,
        event_task: Task<()>,
        contract: OutputContract,
        spawned: Option<durable::Spawned>,
    ) -> std::io::Result<Arc<AgentMarks>> {
        let marks = Arc::<AgentMarks>::default();
        let created = Arc::clone(&marks);
        let OutputContract {
            validator,
            schema,
            value,
        } = contract;
        let id = descriptor.id;
        let child_session_id = descriptor.session_id.clone();

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
        let registry = Arc::clone(self);
        let root = root_session_id.clone();
        self.state
            .call(move |state| -> std::io::Result<()> {
                let root_session_id = root;
                state.validate_insert(&root_session_id, &descriptor)?;
                let (harness, harness_task) = harness::spawn(
                    root_session_id.clone(),
                    descriptor.id,
                    descriptor.context(),
                    agent,
                    registry.capacity.clone(),
                    Arc::downgrade(&registry),
                    schema.clone(),
                    None,
                );
                if let Some(snapshot) = checkpoint {
                    registry.record_checkpoint(&root_session_id, descriptor.id, &marks, snapshot);
                }
                state.insert(
                    root_session_id,
                    descriptor.id,
                    descriptor.session_id.clone(),
                    ChildSession {
                        binding_task: descriptor.task.clone(),
                        descriptor,
                        host_context,
                        event_task: Some(event_task),
                        residency: Residency::Resident(harness),
                        harness_task: Some(harness_task),
                        status: AgentStatus::Pending,
                        turn: None,
                        output_validator: validator,
                        output_schema: value,
                        stored_runtime: StoredRuntime::None,
                        next_instruction_revision: 0,
                        last_output: None,
                        follow_up_output: None,
                        delegated_turn: false,
                        last_used: 0,
                        resume_attempts: 0,
                        in_flight_calls: Vec::new(),
                        in_flight_omitted: 0,
                        completion: Completion::None,
                        cancelled_revision: None,
                        in_spawning_turn: false,
                        settled_revision: None,
                        usage: AgentUsage::default(),
                        in_flight_progress: durable::OmittedProgress::default(),
                        hibernated: None,
                        hibernations: 0,
                        residency_lock: Sequencer::new(),
                        marks,
                        changes: None,
                        announce: false,
                        journal_generation: 0,
                        conversation: false,
                        waited: Vec::new(),
                        sent: Vec::new(),
                        spawned,
                    },
                )
            })
            .await?;
        self.roots.update(|roots| {
            roots.insert(child_session_id.clone(), root_session_id.clone());
        });
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
            let root = root_session_id.clone();
            self.state
                .call(move |state| {
                    if let Some(session) = state
                        .scopes
                        .get_mut(&root)
                        .and_then(|scope| scope.sessions.get_mut(&id))
                        && let Some(events) = session.event_task.take()
                    {
                        events.abort();
                    }
                })
                .await;
            return match self.close_batch(&root_session_id, vec![id]).await {
                Ok(()) => Err(error),
                Err(cleanup) => Err(std::io::Error::other(format!(
                    "{error}; subagent startup cleanup failed: {cleanup}"
                ))),
            };
        }
        Ok(created)
    }

    pub(super) fn queues_spawns(&self) -> bool {
        self.queue_spawns.load(Ordering::Relaxed)
    }

    pub(super) async fn ensure_resident(
        self: &Arc<Self>,
        root_session_id: &str,
        id: AgentId,
    ) -> std::io::Result<()> {
        self.rehydrate(root_session_id, id, MessagePurpose::Coordinate)
            .await
            .map(drop)
    }

    pub(super) fn session_handle(&self, session_id: &str) -> Option<AgentHandle> {
        self.session_handles
            .read(|handles| handles.get(session_id).cloned())
    }

    pub(super) async fn insert_unstarted(
        self: &Arc<Self>,
        root_session_id: String,
        descriptor: AgentDescriptor,
        host_context: Option<Arc<str>>,
        contract: OutputContract,
        spawned: Option<durable::Spawned>,
        unstarted: durable::Unstarted,
    ) -> std::io::Result<()> {
        let id = descriptor.id;
        let child_session_id = descriptor.session_id.clone();
        let root = root_session_id.clone();
        self.state
            .call(move |state| -> std::io::Result<()> {
                state.validate_insert(&root, &descriptor)?;
                let mut session = ChildSession::restored(
                    descriptor.clone(),
                    host_context,
                    AgentStatus::Pending,
                    contract,
                    StoredRuntime::Unstarted(Arc::new(unstarted)),
                    0,
                    None,
                );
                session.announce = false;
                session.spawned = spawned;
                let (id, session_id) = (descriptor.id, descriptor.session_id);
                state.insert(root, id, session_id, session)
            })
            .await?;
        self.roots.update(|roots| {
            roots.insert(child_session_id.clone(), root_session_id.clone());
        });
        self.changed();
        if self.store_for(&root_session_id).is_some()
            && let Err(error) = self.save_scope(&root_session_id).await
        {
            return match self.close_batch(&root_session_id, vec![id]).await {
                Ok(()) => Err(error),
                Err(cleanup) => Err(std::io::Error::other(format!(
                    "{error}; subagent startup cleanup failed: {cleanup}"
                ))),
            };
        }
        self.enqueue_unstarted(&root_session_id, id);
        Ok(())
    }

    fn enqueue_unstarted(self: &Arc<Self>, root_session_id: &str, id: AgentId) {
        let _ = self.unstarted.send((Arc::from(root_session_id), id));
        let Some(mut queue) = self.unstarted_queue.take() else {
            return;
        };
        let capacity = self.capacity.clone();
        let registry = Arc::downgrade(self);
        drop(platform::spawn(async move {
            while let Some((root, id)) = queue.recv().await {
                let turn = capacity.acquire(Waiter::Spawn).await;
                let Some(registry) = registry.upgrade() else {
                    return;
                };
                drop(platform::spawn(async move {
                    super::tools::start_unstarted(&registry, &root, id, turn).await;
                }));
            }
        }));
    }

    pub(super) async fn unstarted(
        &self,
        root_session_id: &str,
        id: AgentId,
    ) -> Option<UnstartedChild> {
        let root = root_session_id.to_owned();
        let (child, reissued) = self
            .state
            .call(move |state| {
                let scope = state.scopes.get_mut(&root)?;
                let session = scope.sessions.get(&id)?;
                let StoredRuntime::Unstarted(unstarted) = &session.stored_runtime else {
                    return None;
                };
                if !matches!(session.status, AgentStatus::Pending) || session.harness().is_some() {
                    return None;
                }
                let unstarted = Arc::clone(unstarted);
                let parent_session = session
                    .descriptor
                    .parent
                    .and_then(|parent| scope.sessions.get(&parent))
                    .map_or_else(
                        || root.clone(),
                        |parent| parent.descriptor.session_id.clone(),
                    );
                let reissued = if unstarted.restored {
                    let previous = session.descriptor.session_id.clone();
                    let issued = nanocode_platform::uuid_v7().to_string();
                    scope.topology.rebind(id, &previous, issued.clone());
                    let session = scope.sessions.get_mut(&id)?;
                    session.descriptor.session_id.clone_from(&issued);
                    session.stored_runtime =
                        StoredRuntime::Unstarted(Arc::new(unstarted.reissued()));
                    state.root_by_session.remove(&previous);
                    state.root_by_session.insert(issued.clone(), root.clone());
                    Some((previous, issued))
                } else {
                    None
                };
                let session = state.scopes.get(&root)?.sessions.get(&id)?;
                Some((
                    UnstartedChild {
                        lock: session.residency_lock.clone(),
                        descriptor: session.descriptor.clone(),
                        parent_session,
                        host_context: session.host_context.clone(),
                        output_schema: session.output_schema.clone(),
                        unstarted,
                    },
                    reissued,
                ))
            })
            .await?;
        if let Some((previous, issued)) = reissued {
            self.roots.update(|roots| {
                roots.remove(&previous);
                roots.insert(issued.clone(), root_session_id.to_owned());
            });
        }
        Some(child)
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn attach_started(
        self: &Arc<Self>,
        root_session_id: &str,
        id: AgentId,
        agent: Agent,
        events: nanocode_agent_core::AgentEvents,
        model: Option<nanocode_agent_core::ModelSelection>,
        thinking: Option<nanocode_agent_core::Thinking>,
        reasoning_mode: nanocode_agent_core::ReasoningMode,
        schema: String,
    ) -> std::io::Result<()> {
        let checkpoint = if self.store_for(root_session_id).is_some() {
            match agent.checkpoint().await {
                Ok(snapshot) => Some(snapshot),
                Err(error) => {
                    let _ = agent.shutdown().await;
                    return Err(std::io::Error::other(error));
                }
            }
        } else {
            None
        };
        let registry = Arc::clone(self);
        let root = root_session_id.to_owned();
        let updates = self.updates.clone();
        let (start, ready) = oneshot::channel();
        let attached = self
            .state
            .call(move |state| {
                let Some(session) = state
                    .scopes
                    .get_mut(&root)
                    .and_then(|scope| scope.sessions.get_mut(&id))
                    .filter(|session| {
                        matches!(session.status, AgentStatus::Pending)
                            && matches!(session.stored_runtime, StoredRuntime::Unstarted(_))
                    })
                else {
                    return Err(agent);
                };
                session.descriptor.model = model;
                session.descriptor.thinking = thinking;
                session.descriptor.reasoning_mode = reasoning_mode;
                let event_task = forward_events(
                    root.clone(),
                    id,
                    session.descriptor.parent,
                    events,
                    ready,
                    Arc::downgrade(&registry),
                    updates,
                );
                let (harness, harness_task) = harness::spawn(
                    root.clone(),
                    id,
                    session.descriptor.context(),
                    agent,
                    registry.capacity.clone(),
                    Arc::downgrade(&registry),
                    schema,
                    None,
                );
                if let Some(snapshot) = checkpoint {
                    registry.record_checkpoint(&root, id, &session.marks, snapshot);
                }
                session.residency = Residency::Resident(harness);
                session.harness_task = Some(harness_task);
                session.event_task = Some(event_task);
                session.stored_runtime = StoredRuntime::None;
                Ok(Arc::clone(&session.marks))
            })
            .await;
        match attached {
            Ok(marks) => {
                let _ = start.send((0, marks));
                self.changed();
                Ok(())
            }
            Err(agent) => {
                let _ = agent.shutdown().await;
                Err(std::io::Error::other(format!(
                    "subagent {id} was stopped before it started"
                )))
            }
        }
    }

    pub(super) async fn fail_unstarted(&self, root_session_id: &str, id: AgentId, error: String) {
        let root = root_session_id.to_owned();
        let settled = self.state.call(move |state| {
            let session = state
                .scopes
                .get_mut(&root)
                .and_then(|scope| scope.sessions.get_mut(&id))
                .filter(|session| {
                    !session.active() && matches!(session.status, AgentStatus::Pending)
                })?;
            let harness = match std::mem::replace(&mut session.residency, Residency::Evicted) {
                Residency::Resident(harness) => Some(harness),
                Residency::Detached | Residency::Evicted => None,
            };
            session.status = AgentStatus::Failed { error };
            session.stored_runtime = StoredRuntime::None;
            let settled = session.next_instruction_revision.saturating_add(1);
            session.next_instruction_revision = settled;
            session.settled_revision = Some(settled);
            let completion = session.descriptor.parent.is_none().then_some(settled);
            if let Some(revision) = completion {
                session.settle_completion(revision);
            }
            Some((
                session.status.clone(),
                completion,
                harness,
                Arc::clone(&session.marks),
            ))
        });
        let Some((status, completion, harness, lane)) = settled.await else {
            return;
        };
        self.enqueue_status(
            root_session_id,
            id,
            lane,
            OrderedJob::Settle {
                status,
                completion,
                harness,
            },
        );
        self.changed();
    }

    pub(super) async fn launch_initial_turn(
        self: &Arc<Self>,
        root_session_id: &str,
        id: AgentId,
        prompt: String,
        capacity: TurnCapacity,
    ) -> std::io::Result<()> {
        let root = root_session_id.to_owned();
        let harness = self
            .state
            .call(move |state| state.harness_in_scope(&root, id))
            .await?;
        harness.start(prompt, capacity).await
    }

    pub(super) async fn pending_turn(
        &self,
        root: &str,
        id: AgentId,
        session_id: &str,
    ) -> std::io::Result<Option<(String, serde_json::Value)>> {
        if let Some(resolver) = Option::clone(&self.pending_turns.get()) {
            let scope = root.to_owned();
            let resuming = self
                .state
                .call(move |state| {
                    state
                        .scopes
                        .get(&scope)
                        .and_then(|scope| scope.sessions.get(&id))
                        .is_some_and(|session| session.resume_attempts > 0)
                })
                .await;
            if let Some(pending) = resuming.then(|| resolver(session_id)).flatten() {
                return pending.await;
            }
        }
        match self.store_for(root) {
            Some(store) => store.pending_turn(session_id).await,
            None => Ok(None),
        }
    }

    pub(super) async fn harness_turn_started(
        &self,
        root_session_id: &str,
        id: AgentId,
        recovered: Option<u64>,
        context: Option<&AgentContext>,
    ) -> Option<StartedTurn> {
        let recovering = recovered.is_some();
        let root = root_session_id.to_owned();
        let context = context.cloned();
        let (started, descriptor) = self
            .state
            .call(move |state| {
                let mut descriptor = None;
                let last_used = state.next_access();
                let Some(session) = state
                    .scopes
                    .get_mut(&root)
                    .and_then(|scope| scope.sessions.get_mut(&id))
                else {
                    return (None, None);
                };
                let started = if !session.status.can_start_turn() || session.active() {
                    None
                } else {
                    let revision = if let Some(recovered) = recovered {
                        session.next_instruction_revision.max(recovered)
                    } else {
                        let Some(next) = session.next_instruction_revision.checked_add(1) else {
                            return (None, None);
                        };
                        next
                    };

                    let delegated = std::mem::take(&mut session.delegated_turn);
                    let completed =
                        !delegated && matches!(session.status, AgentStatus::Completed { .. });
                    if completed || !recovering {
                        session.follow_up_output =
                            completed.then(|| session.last_output.clone()).flatten();
                    }
                    if !matches!(session.status, AgentStatus::Interrupted) {
                        session.in_flight_calls = Vec::new();
                        session.in_flight_omitted = 0;
                        session.in_flight_progress = durable::OmittedProgress::default();
                    }
                    session.next_instruction_revision = revision;
                    session.turn = Some(ActiveTurn {
                        revision,
                        stage: TurnStage::Working,
                        interrupted: false,
                    });
                    session.last_used = last_used;
                    session.status = AgentStatus::Running;
                    if let Some(context) = &context
                        && session.descriptor.context().as_ref() != Some(context)
                    {
                        session.descriptor.model = Some(context.model.clone());
                        session.descriptor.thinking = Some(context.thinking);
                        session.descriptor.reasoning_mode = context.reasoning_mode;
                        let mut updated = session.descriptor.clone();
                        updated.task = session.binding_task.clone();
                        descriptor = Some(updated);
                    }
                    Some((
                        StartedTurn {
                            revision,
                            keeps_result: session.follow_up_output.is_some(),
                        },
                        Arc::clone(&session.marks),
                    ))
                };
                (started, descriptor)
            })
            .await;
        let (started, lane) = started?;
        if let Some(descriptor) = descriptor {
            self.send(root_session_id, AgentUpdate::Added(descriptor));
        }
        self.enqueue_status(
            root_session_id,
            id,
            lane,
            OrderedJob::Status(AgentStatus::Running),
        );
        self.changed();
        Some(started)
    }

    pub(super) async fn harness_turn_start_failed(
        &self,
        root_session_id: &str,
        id: AgentId,
        error: String,
    ) {
        let root = root_session_id.to_owned();
        let status = self
            .state
            .call(move |state| {
                let session = state
                    .scopes
                    .get_mut(&root)
                    .and_then(|scope| scope.sessions.get_mut(&id))?;
                session.turn = None;
                if !matches!(session.status, AgentStatus::Closing | AgentStatus::Closed) {
                    session.status = AgentStatus::Failed { error };
                }
                Some((session.status.clone(), Arc::clone(&session.marks)))
            })
            .await;
        let Some((status, lane)) = status else {
            return;
        };
        self.enqueue_status(root_session_id, id, lane, OrderedJob::Status(status));
        self.changed();
    }

    pub(super) async fn harness_turn_finished(
        self: &Arc<Self>,
        root_session_id: &str,
        id: AgentId,
        result: AgentResult<TurnResult>,
    ) {
        let registry = Arc::clone(self);
        let root = root_session_id.to_owned();
        let finished = self
            .state
            .call(move |state| {
                let root_session_id = root.as_str();
                let session = state
                    .scopes
                    .get_mut(root_session_id)
                    .and_then(|scope| scope.sessions.get_mut(&id))?;
                let turn = session.turn.take()?;
                let settled = turn.revision;
                let submitted_output = match turn.stage {
                    TurnStage::Submitted(output) => Some(output),
                    TurnStage::Working | TurnStage::Steering => None,
                };
                if matches!(result, Err(NanocodeError::TurnStopped))
                    && submitted_output.is_none()
                    && !matches!(session.status, AgentStatus::Closing | AgentStatus::Closed)
                {
                    return Some((
                        session.status.clone(),
                        None,
                        None,
                        false,
                        Arc::clone(&session.marks),
                    ));
                }

                session.resume_attempts = 0;
                session.in_flight_calls = Vec::new();
                session.in_flight_omitted = 0;
                session.in_flight_progress = durable::OmittedProgress::default();
                if let Ok(TurnResult {
                    usage: Some(usage), ..
                }) = &result
                {
                    session.usage.record(usage);
                }
                let follow_up_output = session.follow_up_output.take();

                if let Some(output) = &submitted_output {
                    session.last_output = Some(output.clone());
                }
                if matches!(session.status, AgentStatus::Closing | AgentStatus::Closed) {
                    session.status.clone()
                } else {
                    match result {
                        Ok(_) => complete_session(session, submitted_output.or(follow_up_output)),
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
                    .harness()
                    .cloned()
                    .filter(|_| registry.begin_pending_checkpoint(root_session_id));
                let delivered = session.settled_revision == Some(settled)
                    && session.completion == Completion::None;
                session.settled_revision = Some(settled);

                let completion = (session.status.wakes_parent()
                    && session.descriptor.parent.is_none()
                    && !delivered)
                    .then_some(settled);
                if let Some(revision) = completion {
                    session.settle_completion(revision);
                }
                Some((
                    session.status.clone(),
                    completion,
                    harness,
                    true,
                    Arc::clone(&session.marks),
                ))
            })
            .await;
        let Some((status, completion, harness, settled, lane)) = finished else {
            return;
        };
        self.enqueue_status(
            root_session_id,
            id,
            lane,
            OrderedJob::Settle {
                status,
                completion,
                harness,
            },
        );
        self.changed();
        if !settled {
            return;
        }
        let registry = Arc::clone(self);
        let root_session_id = root_session_id.to_owned();
        let locks = registry.scope_locks(&root_session_id);
        if !locks.evicting.start() {
            return;
        }
        drop(platform::spawn(async move {
            loop {
                registry.enforce_resident_limit(&root_session_id).await;
                if locks.evicting.finish() {
                    break;
                }
            }
        }));
    }

    async fn enforce_resident_limit(self: &Arc<Self>, root_session_id: &str) {
        loop {
            let root = root_session_id.to_owned();
            let limit = self.max_resident.load(Ordering::Relaxed);
            let Ok(evictions) = self
                .when_messages_flow(root_session_id, move |state| {
                    state.take_resident_evictions(&root, limit)
                })
                .await
            else {
                return;
            };
            if evictions.is_empty() {
                return;
            }
            let evicting = evictions
                .into_iter()
                .map(|eviction| {
                    let registry = Arc::clone(self);
                    let root = root_session_id.to_owned();
                    platform::spawn(async move { registry.evict(&root, eviction).await })
                })
                .collect::<Vec<_>>();
            let mut kept = false;
            for evicted in evicting {
                kept |= !matches!(evicted.await, Ok(true));
            }
            if kept {
                return;
            }
        }
    }

    async fn evict(&self, root_session_id: &str, eviction: ResidentEviction) -> bool {
        let ResidentEviction {
            id,
            harness,
            marks,
            _lock,
        } = eviction;
        let snapshot = if self.store_for(root_session_id).is_some()
            && self.journal_holds_conversation(root_session_id, &marks)
        {
            Ok(None)
        } else {
            harness.snapshot().await.map(Some)
        };
        match snapshot {
            Ok(snapshot) => {
                let stored = match snapshot {
                    None => None,
                    Some(snapshot)
                        if self.store_for(root_session_id).is_some()
                            && self.record_checkpoint(
                                root_session_id,
                                id,
                                &marks,
                                snapshot.clone(),
                            ) =>
                    {
                        None
                    }
                    Some(snapshot) => Some(snapshot),
                };
                let root = root_session_id.to_owned();
                self.state
                    .call(move |state| {
                        if let Some(session) = state
                            .scopes
                            .get_mut(&root)
                            .and_then(|scope| scope.sessions.get_mut(&id))
                        {
                            session.stored_runtime = stored
                                .map_or(StoredRuntime::Journaled, |stored| {
                                    StoredRuntime::Checkpoint(Box::new(stored))
                                });
                        }
                    })
                    .await;
            }
            Err(_) => {
                let root = root_session_id.to_owned();
                self.state
                    .call(move |state| {
                        if let Some(session) = state
                            .scopes
                            .get_mut(&root)
                            .and_then(|scope| scope.sessions.get_mut(&id))
                        {
                            session.residency = Residency::Resident(harness);
                        }
                    })
                    .await;
                return false;
            }
        }
        self.changed();
        if harness.close().await.is_err() {
            self.harness_closed(root_session_id, id).await;
        }

        let root = root_session_id.to_owned();
        let tasks = self
            .state
            .call(move |state| {
                let session = state
                    .scopes
                    .get_mut(&root)
                    .and_then(|scope| scope.sessions.get_mut(&id));
                session.map(|session| (session.harness_task.take(), session.event_task.take()))
            })
            .await;
        if let Some((harness_task, event_task)) = tasks {
            if let Some(task) = harness_task {
                let _ = task.await;
            }
            if let Some(task) = event_task {
                let _ = task.await;
            }
        }
        true
    }

    pub(super) async fn harness_closed(&self, root_session_id: &str, id: AgentId) {
        let root = root_session_id.to_owned();
        let closed = self
            .state
            .call(move |state| {
                let scope = state.scopes.get_mut(&root)?;

                let detach = scope.life.closing();
                let announce = !scope.life.released();
                if scope
                    .sessions
                    .get(&id)
                    .is_some_and(|session| session.hibernated.is_some())
                {
                    return None;
                }
                let session = scope.sessions.get_mut(&id)?;
                let status = if matches!(session.status, AgentStatus::Closed) {
                    None
                } else {
                    let evicted = matches!(session.residency, Residency::Evicted)
                        && !matches!(session.status, AgentStatus::Closing);
                    session.residency = if evicted {
                        Residency::Evicted
                    } else {
                        Residency::Detached
                    };
                    session.turn = None;
                    if evicted {
                        None
                    } else {
                        session.status = AgentStatus::Closed;
                        Some(AgentStatus::Closed)
                    }
                };
                Some((
                    status.filter(|_| announce),
                    detach,
                    session.descriptor.session_id.clone(),
                ))
            })
            .await;
        let Some((status_update, detach, session_id)) = closed else {
            return;
        };
        self.session_handles.update(|handles| {
            handles.remove(&session_id);
        });
        if let Some(released) = Option::clone(&self.session_released.get()) {
            released(&session_id);
        }
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

    async fn runtime_closed(&self, root_session_id: &str, id: AgentId, hibernations: u64) {
        let root = root_session_id.to_owned();
        let harness = self
            .state
            .call(move |state| {
                let session = state
                    .scopes
                    .get(&root)
                    .and_then(|scope| scope.sessions.get(&id));
                if session.is_some_and(|session| session.hibernations != hibernations) {
                    return None;
                }
                Some(
                    session
                        .filter(|session| {
                            !matches!(session.status, AgentStatus::Closing | AgentStatus::Closed)
                        })
                        .and_then(|session| session.harness().cloned()),
                )
            })
            .await;
        let Some(harness) = harness else {
            return;
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
        let mut completion = (id, revision);
        let completions = loop {
            if let Some(sender) = self
                .completion_batches
                .read(|batches| batches.get(root_session_id).cloned())
                .filter(|sender| !sender.is_closed())
            {
                match sender.send(completion) {
                    Ok(()) => return,
                    Err(mpsc::error::SendError(returned)) => completion = returned,
                }
            }
            let (sender, completions) = mpsc::unbounded_channel();
            let installed =
                self.completion_batches
                    .try_update(|batches| match batches.get(root_session_id) {
                        Some(existing) if !existing.is_closed() => None,
                        _ => {
                            batches.insert(root_session_id.to_owned(), sender.clone());
                            Some(())
                        }
                    });
            if installed.is_some() {
                let _ = sender.send(completion);
                break completions;
            }
        };
        drop(platform::spawn(continue_root(
            registry,
            resolver,
            root_session_id.to_owned(),
            completions,
        )));
    }

    async fn continue_parent_batch(
        self: &Arc<Self>,
        parent: &Agent,
        root_session_id: &str,
        completions: Vec<(AgentId, u64)>,
    ) -> std::io::Result<()> {
        let ids = completions.iter().map(|(id, _)| *id).collect::<Vec<_>>();
        let result = self
            .dispatch_parent_batch(parent, root_session_id, completions)
            .await;
        self.release_dispatch(root_session_id, &ids);
        result
    }

    fn release_dispatch(&self, root_session_id: &str, ids: &[AgentId]) {
        self.completion_dispatch.update(|dispatch| {
            for &id in ids {
                let key = (root_session_id.to_owned(), id);
                if dispatch.get(&key).is_some_and(Sequencer::is_unshared) {
                    dispatch.remove(&key);
                }
            }
        });
    }

    fn claim_route(&self, root_session_id: &str, id: AgentId, revision: u64) -> bool {
        let key = (root_session_id.to_owned(), id);
        self.completion_routes
            .try_update(|routes| {
                (routes.get(&key) != Some(&revision)).then(|| {
                    routes.insert(key.clone(), revision);
                })
            })
            .is_some()
    }

    fn release_route(&self, root_session_id: &str, id: AgentId, revision: u64) {
        let key = (root_session_id.to_owned(), id);
        self.completion_routes.update(|routes| {
            if routes.get(&key) == Some(&revision) {
                routes.remove(&key);
            }
        });
    }

    async fn dispatch_parent_batch(
        self: &Arc<Self>,
        parent: &Agent,
        root_session_id: &str,
        mut completions: Vec<(AgentId, u64)>,
    ) -> std::io::Result<()> {
        completions.sort_unstable_by(|left, right| left.0.cmp(&right.0).then(right.1.cmp(&left.1)));
        completions.dedup_by_key(|(id, _)| *id);
        if let [(id, revision)] = completions[..] {
            return self
                .continue_parent(parent, root_session_id, id, revision)
                .await
                .map(drop);
        }
        let mut dispatches = Vec::with_capacity(completions.len());
        for &(id, _) in &completions {
            let dispatch = self.completion_dispatch.update(|dispatch| {
                dispatch
                    .entry((root_session_id.to_owned(), id))
                    .or_default()
                    .clone()
            });
            dispatches.push(dispatch.enter().await);
        }
        let mut ready = Vec::new();
        let mut parked_any = false;
        for (id, revision) in completions {
            let key = (root_session_id.to_owned(), id);
            if self
                .completion_routes
                .read(|routes| routes.get(&key) == Some(&revision))
            {
                continue;
            }
            let Some((_, _, _, parked)) = self.peek_completion(root_session_id, id, revision).await
            else {
                continue;
            };
            if parked
                || self
                    .park_completion(root_session_id, id, revision, false)
                    .await
            {
                self.watch_root(root_session_id, parent);
                parked_any |= !parked;
                continue;
            }
            ready.push((id, revision));
        }
        if self.store_for(root_session_id).is_some()
            && (parked_any || !ready.is_empty())
            && let Err(error) = self.persist_durably(root_session_id).await
        {
            for &(id, revision) in &ready {
                self.retry_completion(root_session_id, id, revision);
            }
            return Err(error);
        }
        let mut groups = Vec::<(
            Option<Arc<str>>,
            Vec<(AgentId, u64, AgentStatus, String, u32)>,
        )>::new();
        for (id, revision) in ready {
            let Some((status, session_id, attempt, _)) =
                self.peek_completion(root_session_id, id, revision).await
            else {
                continue;
            };
            let started = parent
                .unfinished_request(&completion_request_id(&session_id, revision, attempt))
                .await;
            if !matches!(started, Ok(false)) {
                groups.push((None, vec![(id, revision, status, session_id, attempt)]));
                continue;
            }
            let host_context = self.host_context(root_session_id, id).await;
            match groups
                .iter_mut()
                .find(|(context, _)| *context == host_context)
            {
                Some((_, group)) => group.push((id, revision, status, session_id, attempt)),
                None => groups.push((
                    host_context,
                    vec![(id, revision, status, session_id, attempt)],
                )),
            }
        }
        for (_, group) in groups {
            self.route_completions(parent, root_session_id, group).await;
        }
        drop(dispatches);
        Ok(())
    }

    fn watch_steer(
        &self,
        root_session_id: &str,
        parent: &Agent,
        request_id: &str,
        prompt: &str,
    ) -> oneshot::Receiver<NoticeOutcome> {
        let (outcome, steered) = oneshot::channel();
        let mut steer = SteerWatch {
            request_id: request_id.to_owned(),
            prompt: prompt.to_owned(),
            outcome,
        };
        loop {
            let watcher = self
                .steer_watchers
                .read(|watchers| watchers.get(root_session_id).cloned())
                .filter(|watcher| !watcher.is_closed());
            let watcher = match watcher {
                Some(watcher) => watcher,
                None => {
                    let (watcher, steers) = mpsc::unbounded_channel();
                    let installed = self.steer_watchers.try_update(|watchers| {
                        match watchers.get(root_session_id) {
                            Some(existing) if !existing.is_closed() => None,
                            _ => {
                                watchers.insert(root_session_id.to_owned(), watcher.clone());
                                Some(())
                            }
                        }
                    });
                    if installed.is_none() {
                        continue;
                    }
                    drop(platform::spawn(NoticeOutcome::of_steers(
                        parent.subscribe(),
                        parent.downgrade(),
                        steers,
                    )));
                    watcher
                }
            };
            match watcher.send(steer) {
                Ok(()) => return steered,
                Err(mpsc::error::SendError(returned)) => steer = returned,
            }
        }
    }

    async fn route_completions(
        self: &Arc<Self>,
        parent: &Agent,
        root_session_id: &str,
        group: Vec<(AgentId, u64, AgentStatus, String, u32)>,
    ) {
        let group = group
            .into_iter()
            .filter(|(id, revision, _, _, _)| self.claim_route(root_session_id, *id, *revision))
            .collect::<Vec<_>>();
        let Some((_, first_revision, _, first_session, _)) = group.first() else {
            return;
        };
        let request_id = if let [(_, revision, _, session_id, attempt)] = &group[..] {
            completion_request_id(session_id, *revision, *attempt)
        } else {
            let members = group
                .iter()
                .map(|(_, revision, _, session_id, attempt)| {
                    format!("{session_id}:{revision}:{attempt}")
                })
                .collect::<Vec<_>>()
                .join("\n");
            format!(
                "{}{first_session}:{first_revision}:batch-{}",
                crate::COMPLETION_REQUEST_PREFIX,
                durable::checkpoint_key(&members)
            )
        };
        let prompt = group
            .iter()
            .map(|(id, _, status, _, _)| completion_continuation_prompt(*id, status))
            .collect::<Vec<_>>()
            .join("\n\n");
        let steered = self.watch_steer(root_session_id, parent, &request_id, &prompt);
        match parent
            .route_prompt_request(
                nanocode_agent_core::PromptRequest::new(prompt.clone())
                    .request_id(request_id.clone()),
            )
            .await
        {
            Ok(route) => {
                for (id, revision, status, _, _) in &group {
                    let key = (root_session_id.to_owned(), *id);
                    self.wait_reported.update(|reported| {
                        reported.insert(key.clone(), wait_mark(status, *revision));
                    });
                    self.completion_routes.update(|routes| {
                        routes.insert(key.clone(), *revision);
                    });
                }
                let registry = Arc::downgrade(self);
                let parent = parent.downgrade();
                let root_session_id = root_session_id.to_owned();
                drop(platform::spawn(async move {
                    let outcome =
                        NoticeOutcome::of_route(route, steered, &parent, request_id, prompt).await;
                    let Some(registry) = registry.upgrade() else {
                        return;
                    };
                    for (id, revision, _, _, _) in group {
                        registry
                            .settle_notice(&root_session_id, id, revision, outcome, &parent)
                            .await;
                    }
                }));
            }
            Err(error) => {
                for (id, revision, _, _, _) in &group {
                    self.release_route(root_session_id, *id, *revision);
                }
                if !matches!(
                    error,
                    NanocodeError::AgentStopped | NanocodeError::TurnStopped
                ) {
                    for (id, revision, _, _, _) in group {
                        if matches!(error, NanocodeError::InvalidRequest(_)) {
                            self.next_completion_attempt(root_session_id, id, revision)
                                .await;
                        }
                        self.retry_completion(root_session_id, id, revision);
                    }
                }
                nanocode_trace::warn!(%error, "subagent completions did not continue their parent");
            }
        }
    }

    pub async fn continue_parent(
        self: &Arc<Self>,
        parent: &Agent,
        root_session_id: &str,
        id: AgentId,
        revision: u64,
    ) -> std::io::Result<bool> {
        let result = self
            .dispatch_parent(parent, root_session_id, id, revision)
            .await;
        self.release_dispatch(root_session_id, &[id]);
        result
    }

    async fn dispatch_parent(
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
        let request_id = completion_request_id(&session_id, revision, attempt);
        if !self.claim_route(root_session_id, id, revision) {
            return Ok(false);
        }
        let prompt = completion_continuation_prompt(id, &status);
        let steered = self.watch_steer(root_session_id, parent, &request_id, &prompt);
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
                    let outcome =
                        NoticeOutcome::of_route(route, steered, &parent, request_id, prompt).await;
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
                self.release_route(root_session_id, id, revision);
                if matches!(error, NanocodeError::InvalidRequest(_)) {
                    self.next_completion_attempt(root_session_id, id, revision)
                        .await;
                }
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
        let root = root_session_id.to_owned();
        self.state
            .call(move |state| {
                state
                    .scopes
                    .get(&root)
                    .and_then(|scope| scope.sessions.get(&id))
                    .filter(|session| {
                        session.completion.awaits(revision) && session.status.wakes_parent()
                    })
                    .map(|session| {
                        (
                            session.status.clone(),
                            session.descriptor.session_id.clone(),
                            session.completion.attempt(),
                            session.completion.parked(),
                        )
                    })
            })
            .await
    }

    async fn settle_notice(
        self: &Arc<Self>,
        root_session_id: &str,
        id: AgentId,
        revision: u64,
        outcome: NoticeOutcome,
        parent: &WeakAgent,
    ) {
        let key = (root_session_id.to_owned(), id);
        if self
            .completion_routes
            .read(|routes| routes.get(&key) == Some(&revision))
        {
            self.completion_routes.update(|routes| {
                if routes.get(&key) == Some(&revision) {
                    routes.remove(&key);
                }
            });
        }
        match outcome {
            NoticeOutcome::Read => {
                self.acknowledge_completion(root_session_id, id, revision)
                    .await;
                if self.store_for(root_session_id).is_some()
                    && let Err(error) = self.persist_durably(root_session_id).await
                {
                    nanocode_trace::warn!(%error, %id, revision, "subagent completion acknowledgement was not journaled");
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
                    nanocode_trace::warn!(%error, %id, revision, "parked subagent completion was not journaled");
                }
            }
            NoticeOutcome::Stopped => {}
        }
    }

    async fn next_completion_attempt(&self, root_session_id: &str, id: AgentId, revision: u64) {
        let root = root_session_id.to_owned();
        self.state
            .call(move |state| {
                if let Some(scope) = state.scopes.get_mut(&root)
                    && scope
                        .sessions
                        .get(&id)
                        .is_some_and(|session| session.completion.awaits(revision))
                    && let Some(session) = scope.sessions.get_mut(&id)
                {
                    session.completion.next_attempt();
                }
            })
            .await;
    }

    async fn park_completion(
        &self,
        root_session_id: &str,
        id: AgentId,
        revision: u64,
        held: bool,
    ) -> bool {
        let root = root_session_id.to_owned();
        let parked = self
            .state
            .call(move |state| {
                let Some(scope) = state.scopes.get_mut(&root) else {
                    return false;
                };
                let lineage = held || scope.sessions.any_parked();
                let pending = scope
                    .sessions
                    .get(&id)
                    .is_some_and(|session| session.completion.awaits(revision));
                if lineage
                    && pending
                    && let Some(session) = scope.sessions.get_mut(&id)
                {
                    session.completion.park();
                    true
                } else {
                    false
                }
            })
            .await;
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
        let root = root_session_id.to_owned();
        self.state
            .call(move |state| {
                state
                    .scopes
                    .get_mut(&root)
                    .is_some_and(|scope| scope.sessions.holds_root_watch())
            })
            .await
    }

    pub(super) async fn respawned(
        &self,
        session_id: &str,
        call_id: &str,
    ) -> std::io::Result<Option<AgentDescriptor>> {
        self.await_restored(session_id).await?;
        let spawned = durable::Spawned {
            session: session_id.to_owned(),
            call: call_id.to_owned(),
        };
        let child = self
            .state
            .call(move |state| {
                let root = state.root_session_id(&spawned.session).to_owned();
                let id = state.scopes.get(&root)?.sessions.spawned_by(&spawned)?;
                Some((root, id))
            })
            .await;
        let Some((root, id)) = child else {
            return Ok(None);
        };
        let _warm = self.warm(&root, vec![id]).await?;
        Ok(self
            .state
            .call(move |state| {
                let child = state.scopes.get(&root)?.sessions.get(&id)?;
                Some(child.descriptor.clone())
            })
            .await)
    }

    pub(super) async fn spawned_in_turn(self: &Arc<Self>, session_id: &str, id: AgentId) {
        let session = session_id.to_owned();
        let root = self
            .state
            .call(move |state| {
                let root = state.root_session_id(&session).to_owned();
                if root != session {
                    return None;
                }
                let spawned = state
                    .scopes
                    .get_mut(&root)
                    .and_then(|scope| scope.sessions.get_mut(&id))?;
                spawned.in_spawning_turn = true;
                Some(root)
            })
            .await;
        let Some(root) = root else {
            return;
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
        let root = root_session_id.to_owned();
        let turn_id = turn_id.to_owned();
        let request_id = request_id.map(str::to_owned);
        let marked = self
            .state
            .call(move |state| {
                let turn_id = turn_id.as_str();
                let request_id = request_id.as_deref();
                let Some(scope) = state.scopes.get_mut(&root) else {
                    return false;
                };
                let contexts = std::iter::once(turn_id)
                    .chain(request_id)
                    .collect::<Vec<_>>();
                let spawned = scope.sessions.spawned_in(&contexts);
                let mut marked = false;
                for id in spawned {
                    let Some(session) = scope.sessions.get_mut(&id) else {
                        continue;
                    };
                    session.in_spawning_turn = false;
                    if cancelled
                        && session.status.is_active()
                        && session.cancelled_revision.is_none()
                    {
                        session.cancelled_revision = session
                            .turn_revision()
                            .or_else(|| session.next_instruction_revision.checked_add(1));
                        marked = true;
                    }
                }
                marked
            })
            .await;
        if marked
            && self.store_for(root_session_id).is_some()
            && let Err(error) = self.persist_durably(root_session_id).await
        {
            nanocode_trace::warn!(%error, root_session_id, "cancelled subagent lineage was not journaled");
        }
    }

    async fn unpark_completions(&self, root_session_id: &str) {
        let root = root_session_id.to_owned();
        let unparked = self
            .state
            .call(move |state| {
                let Some(scope) = state.scopes.get_mut(&root) else {
                    return Vec::new();
                };
                let ids = scope.sessions.parked_ids();
                ids.into_iter()
                    .filter_map(|id| {
                        let revision = scope.sessions.get_mut(&id)?.completion.unpark()?;
                        Some((id, revision))
                    })
                    .collect::<Vec<_>>()
            })
            .await;
        if unparked.is_empty() {
            return;
        }
        self.changed();
        for (id, revision) in unparked {
            self.schedule_parent_continuation(root_session_id, id, revision);
        }
    }

    async fn restore_completion(&self, root_session_id: &str, id: AgentId, revision: u64) {
        let root = root_session_id.to_owned();
        self.state
            .call(move |state| {
                if let Some(session) = state
                    .scopes
                    .get_mut(&root)
                    .and_then(|scope| scope.sessions.get_mut(&id))
                    && session.completion == Completion::None
                    && session.settled_revision == Some(revision)
                    && session.status.wakes_parent()
                {
                    session.completion = Completion::Pending {
                        revision,
                        attempt: 0,
                    };
                }
            })
            .await;
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
        let root = root_session_id.to_owned();
        let wanted = ids.to_vec();
        let pending = self
            .state
            .call(move |state| {
                let scope = state.scopes.get(&root)?;
                Some(
                    wanted
                        .iter()
                        .filter_map(|id| {
                            let session = scope.sessions.get(id)?;
                            let revision = session.completion.revision()?;
                            session.status.wakes_parent().then_some((*id, revision))
                        })
                        .collect::<Vec<_>>(),
                )
            })
            .await?;
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
        let session = session_id.to_owned();
        let root = self
            .state
            .call(move |state| state.root_session_id(&session).to_owned())
            .await;
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
        let root = root_session_id.to_owned();
        let wanted = ids.to_vec();
        let observed = self
            .state
            .call(move |state| {
                let Some(scope) = state.scopes.get_mut(&root) else {
                    return Vec::new();
                };
                wanted
                    .into_iter()
                    .filter(|id| {
                        scope.sessions.get_mut(id).is_some_and(|session| {
                            let observed = session.status.wakes_parent()
                                && session.completion.revision().is_some();
                            if observed {
                                session.clear_completion();
                            }
                            observed
                        })
                    })
                    .collect::<Vec<_>>()
            })
            .await;
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
        let _warm = if include_completed {
            self.warm_all(session_id).await?
        } else {
            None
        };
        self.listed(session_id, include_completed, include_self, None)
            .await
    }

    pub(super) async fn directory_page(
        &self,
        session_id: &str,
        include_completed: bool,
        include_self: bool,
        after: Option<AgentId>,
        limit: Option<std::num::NonZeroUsize>,
    ) -> std::io::Result<(Vec<AgentDirectoryEntry>, Option<AgentId>)> {
        self.await_restored(session_id).await?;
        let mut warm = None;
        let mut only = None;
        if include_completed {
            let session = session_id.to_owned();
            let mut ids = self
                .state
                .call(move |state| state.listed(&session, include_self, after))
                .await;
            if let Some(limit) = limit {
                ids.truncate(limit.get().saturating_add(1));
            }
            warm = self.warm(&self.root_of(session_id), ids.clone()).await?;
            only = Some(ids);
        }
        let mut agents = self
            .listed(session_id, include_completed, include_self, only)
            .await?;
        drop(warm);
        if let Some(after) = after {
            agents.retain(|agent| agent.agent_id > after);
        }
        let next = limit
            .filter(|limit| agents.len() > limit.get())
            .map(|limit| {
                agents.truncate(limit.get());
                agents[limit.get() - 1].agent_id
            });
        Ok((agents, next))
    }

    pub(super) async fn directory_of(
        &self,
        root_session_id: &str,
        ids: &[AgentId],
    ) -> std::io::Result<Vec<AgentDirectoryEntry>> {
        self.await_restored(root_session_id).await?;
        let _warm = self.warm(root_session_id, ids.to_vec()).await?;
        self.listed(root_session_id, true, false, Some(ids.to_vec()))
            .await
    }

    pub(super) async fn listed(
        &self,
        session_id: &str,
        include_completed: bool,
        include_self: bool,
        only: Option<Vec<AgentId>>,
    ) -> std::io::Result<Vec<AgentDirectoryEntry>> {
        let session = session_id.to_owned();
        let (mut entries, root) = self
            .state
            .call(move |state| {
                (
                    state.directory(&session, include_completed, include_self, only),
                    state.root_session_id(&session).to_owned(),
                )
            })
            .await;

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
    ) -> std::io::Result<Turn> {
        let _warm = self.warm(&self.root_of(session_id), vec![to]).await?;
        let session = session_id.to_owned();
        if let Some((root, lane)) = self
            .state
            .call(move |state| state.release_unstarted(&session, to))
            .await
        {
            self.enqueue_status(&root, to, lane, OrderedJob::Status(AgentStatus::Pending));
            let turn = self.capacity.acquire(Waiter::Spawn).await;
            Box::pin(super::tools::start_unstarted(self, &root, to, turn)).await;
        }
        let session = session_id.to_owned();
        let (root, target) = self
            .state
            .call(move |state| -> std::io::Result<_> {
                let session_id = session.as_str();
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
                let target = scope
                    .sessions
                    .get(&to)
                    .ok_or_else(|| std::io::Error::other(format!("unknown agent_id {to}")))?
                    .residency_lock
                    .clone();
                Ok((root, target))
            })
            .await?;
        let guard = target.enter().await;
        let hibernates = self.hibernates();
        let mut ancestors = HashMap::new();
        let mut missing = loop {
            let held = ancestors.keys().copied().collect::<HashSet<_>>();
            let scoped = root.clone();
            let (missing, locks) =
                self.state
                    .call(move |state| -> std::io::Result<_> {
                        let root = scoped;
                        let scope = state.scopes.get(&root).ok_or_else(|| {
                            std::io::Error::other(format!("unknown agent_id {to}"))
                        })?;
                        let missing = Self::missing_runtimes(scope, &root, to)?;
                        let parents = std::iter::successors(
                            scope
                                .sessions
                                .get(&to)
                                .and_then(|session| session.descriptor.parent),
                            |id| {
                                scope
                                    .sessions
                                    .get(id)
                                    .and_then(|session| session.descriptor.parent)
                            },
                        )
                        .filter(|_| hibernates);
                        let locks = missing
                            .iter()
                            .map(|(id, ..)| *id)
                            .filter(|id| *id != to)
                            .chain(parents)
                            .filter(|id| !held.contains(id))
                            .collect::<BTreeSet<_>>()
                            .into_iter()
                            .filter_map(|id| {
                                Some((id, scope.sessions.get(&id)?.residency_lock.clone()))
                            })
                            .collect::<Vec<_>>();
                        Ok((missing, locks))
                    })
                    .await?;
            if locks.is_empty() {
                break missing;
            }
            for (id, lock) in locks {
                ancestors.insert(id, lock.enter().await);
            }
        };
        self.restore_missing(&root, &mut missing).await?;
        drop(ancestors);
        Ok(guard)
    }

    fn missing_runtimes(
        scope: &AgentScope,
        root: &str,
        to: AgentId,
    ) -> std::io::Result<Vec<MissingRuntime>> {
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
            ) || session.harness().is_some()
            {
                break;
            }
            let snapshot = match &session.stored_runtime {
                StoredRuntime::Checkpoint(snapshot) => Some(SessionCheckpoint::clone(snapshot)),
                StoredRuntime::Journaled => None,
                StoredRuntime::None | StoredRuntime::Unstarted(_) => break,
            };
            let parent_session = session
                .descriptor
                .parent
                .and_then(|parent| scope.sessions.get(&parent))
                .map_or_else(
                    || root.to_owned(),
                    |parent| parent.descriptor.session_id.clone(),
                );
            missing.push((
                id,
                session.descriptor.parent,
                parent_session,
                snapshot,
                session.host_context.clone(),
                session.output_schema.clone(),
                session.descriptor.task.clone(),
                session.descriptor.budget_usd,
                session.conversation,
            ));
            match session.descriptor.parent {
                Some(parent) => id = parent,
                None => break,
            }
        }
        Ok(missing)
    }

    async fn restore_missing(
        self: &Arc<Self>,
        root: &str,
        missing: &mut Vec<MissingRuntime>,
    ) -> std::io::Result<()> {
        let root = root.to_owned();
        while let Some((
            id,
            parent_id,
            parent_session,
            snapshot,
            host_context,
            schema,
            task,
            budget,
            conversation,
        )) = missing.pop()
        {
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
            let needs_assignment = !snapshot.has_conversation() && !conversation;
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
                parent_id,
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
                needs_assignment.then(|| super::model::agent_prompt(id, &task, budget)),
            );
            let resident = harness.clone();
            let scoped = root.clone();
            let installed = self
                .state
                .call(move |state| {
                    let session = state
                        .scopes
                        .get_mut(&scoped)
                        .expect("locked scope")
                        .sessions
                        .get_mut(&id)
                        .expect("retained child");
                    if matches!(session.status, AgentStatus::Closing | AgentStatus::Closed) {
                        return None;
                    }
                    session.descriptor.model = Some(context.model);
                    session.descriptor.thinking = Some(context.thinking);
                    session.descriptor.reasoning_mode = context.reasoning_mode;
                    session.residency = Residency::Resident(resident);
                    session.harness_task = Some(task);
                    session.event_task = Some(event_task);

                    Some((
                        std::mem::take(&mut session.announce).then(|| {
                            let mut descriptor = session.descriptor.clone();
                            descriptor.task = session.binding_task.clone();
                            descriptor
                        }),
                        session.hibernated.take(),
                        session.hibernations,
                        session.turn.as_ref().is_some_and(|turn| turn.interrupted),
                        Arc::clone(&session.marks),
                    ))
                })
                .await;
            let Some((announce, hibernated, hibernations, interrupted, marks)) = installed else {
                drop(start);
                if let Err(error) = harness.close().await {
                    nanocode_trace::debug!(%id, %error, "a restored subagent that began closing did not shut down");
                }
                return Ok(());
            };
            if let Some(descriptor) = announce {
                self.send(&root, AgentUpdate::Added(descriptor));
            }
            let _ = start.send((hibernations, marks));
            if let Some(mut hibernated) = hibernated {
                let capacity = hibernated
                    .capacity
                    .take()
                    .unwrap_or_else(|| self.capacity.overdraw());
                harness.resume(hibernated, capacity, interrupted).await?;
            }
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn send_message(
        self: &Arc<Self>,
        session_id: &str,
        call_id: Option<&str>,
        to: AgentId,
        priority: MessagePriority,
        purpose: MessagePurpose,
        in_reply_to: Option<MessageId>,
        body: String,
    ) -> std::io::Result<MessageReceipt> {
        self.await_restored(session_id).await?;
        self.deliver_message(
            session_id,
            call_id,
            to,
            priority,
            purpose,
            in_reply_to,
            body,
            false,
        )
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
        self.deliver_message(
            session_id,
            None,
            to,
            priority,
            purpose,
            in_reply_to,
            body,
            false,
        )
        .await
    }

    pub async fn resent(
        self: &Arc<Self>,
        session_id: &str,
        call_id: &str,
    ) -> std::io::Result<Option<MessageReceipt>> {
        self.await_restored(session_id).await?;
        let _warm = self.warm_all(session_id).await?;
        self.sent(session_id, call_id).await
    }

    pub(super) async fn resent_to(
        &self,
        session_id: &str,
        call_id: &str,
        to: AgentId,
    ) -> std::io::Result<Option<MessageReceipt>> {
        self.await_restored(session_id).await?;
        let _warm = self.warm(&self.root_of(session_id), vec![to]).await?;
        self.sent(session_id, call_id).await
    }

    async fn sent(
        &self,
        session_id: &str,
        call_id: &str,
    ) -> std::io::Result<Option<MessageReceipt>> {
        let session = session_id.to_owned();
        let call = call_id.to_owned();
        Ok(self
            .state
            .call(move |state| {
                let root = state.root_session_id(&session).to_owned();
                let scope = state.scopes.get(&root)?;
                let sent = scope.sessions.values().find_map(|child| {
                    child
                        .sent
                        .iter()
                        .find(|sent| sent.session == session && sent.call == call)
                        .cloned()
                })?;
                let message = scope.messages.message(sent.message)?;
                Some(MessageReceipt {
                    message_id: message.id,
                    thread_id: message.thread_id,
                    from: message.from,
                    to_agent_id: message.to,
                    disposition: sent.disposition.unwrap_or(MessageDisposition::Queued),
                })
            })
            .await)
    }

    #[allow(clippy::too_many_arguments)]
    async fn deliver_message(
        self: &Arc<Self>,
        session_id: &str,
        call_id: Option<&str>,
        to: AgentId,
        priority: MessagePriority,
        purpose: MessagePurpose,
        in_reply_to: Option<MessageId>,
        body: String,
        user: bool,
    ) -> std::io::Result<MessageReceipt> {
        let target = self.rehydrate(session_id, to, purpose).await?;
        let session = session_id.to_owned();
        let call = call_id.map(str::to_owned);
        let (prepared, delivery) = self
            .when_messages_flow(session_id, move |state| -> std::io::Result<_> {
                let prepared = state.prepare_message(
                    &session,
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
                state.commit_message(&prepared.root_session_id, prepared.message.clone())?;
                if let Some(call) = &call {
                    state.record_sent(
                        &prepared.root_session_id,
                        &session,
                        call,
                        prepared.message.id,
                    );
                }
                Ok((prepared, delivery))
            })
            .await??;
        drop(target);
        let registry = Arc::clone(self);
        let settled = platform::spawn(
            async move { registry.settle_delivery(prepared, delivery).await }.in_current_span(),
        );
        match settled.await {
            Ok(receipt) => receipt,
            Err(error) => Err(std::io::Error::other(error.to_string())),
        }
    }

    async fn settle_delivery(
        &self,
        prepared: PreparedMessage,
        delivery: super::harness::EnqueuedDelivery,
    ) -> std::io::Result<MessageReceipt> {
        if self.store_for(&prepared.root_session_id).is_some()
            && let Err(error) = self.persist_durably(&prepared.root_session_id).await
        {
            self.rollback_message(&prepared.root_session_id, prepared.message.id)
                .await;
            return Err(error);
        }
        let disposition = match delivery.release().await {
            Ok(disposition) => disposition,
            Err(error) => {
                self.rollback_message(&prepared.root_session_id, prepared.message.id)
                    .await;
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

    async fn rollback_message(&self, root_session_id: &str, id: MessageId) {
        let root = root_session_id.to_owned();
        self.state
            .call(move |state| state.rollback_message(&root, id))
            .await;
    }

    async fn start_restored_work(self: &Arc<Self>, root_session_id: &str, work: RestoredWork) {
        self.redeliver_messages(root_session_id, work.messages)
            .await;
        for id in work.unstarted {
            self.enqueue_unstarted(root_session_id, id);
        }
    }

    fn take_deferred_work(&self, root_session_id: &str) -> Option<RestoredWork> {
        let mut work = None;
        self.deferred_work.update(|deferred| {
            work = deferred.remove(root_session_id);
        });
        work
    }

    async fn pending_messages(&self, root_session_id: &str) -> Vec<AgentMessage> {
        let root = root_session_id.to_owned();
        self.state
            .call(move |state| {
                state
                    .scopes
                    .get(&root)
                    .map(|scope| scope.messages.pending_messages())
                    .unwrap_or_default()
            })
            .await
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
            if is_fenced(&error) {
                nanocode_trace::debug!(%error, root_session_id, "redelivered subagent messages are not journaled: another process owns the journal");
            } else {
                nanocode_trace::warn!(%error, root_session_id, "redelivered subagent messages are not journaled yet");
            }
        }
    }

    async fn redeliver_message(
        self: &Arc<Self>,
        root_session_id: &str,
        message: AgentMessage,
    ) -> std::io::Result<MessageDisposition> {
        let _target = self
            .rehydrate(root_session_id, message.to, MessagePurpose::Coordinate)
            .await?;
        let root = root_session_id.to_owned();
        let to = message.to;
        let harness = self
            .when_messages_flow(root_session_id, move |state| {
                state.harness_in_scope(&root, to)
            })
            .await??;
        harness.redeliver(message).await
    }

    pub(super) async fn message_admitted(
        &self,
        root_session_id: &str,
        id: MessageId,
        disposition: MessageDisposition,
    ) {
        let root = root_session_id.to_owned();
        let thread = self
            .state
            .call(move |state| {
                let thread = state.thread_for_message(&root, id);
                state.mark_message_admitted(&root, id, disposition);
                thread
            })
            .await;
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
        self.rollback_message(root_session_id, id).await;
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
        let root = root_session_id.to_owned();
        let (change, descriptor) = self
            .state
            .call(move |state| state.begin_delegation(&root, id))
            .await?;
        self.send(root_session_id, AgentUpdate::Added(descriptor));
        self.changed();
        Some(change)
    }

    pub(super) async fn rollback_message_delivery(
        &self,
        root_session_id: &str,
        change: DelegationChange,
    ) {
        let root = root_session_id.to_owned();
        let descriptor = self
            .state
            .call(move |state| state.rollback_delegation(&root, change))
            .await;
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
        let Some(registry) = self.shared() else {
            return;
        };
        let root = root_session_id.to_owned();
        self.state
            .call(move |state| {
                let Some(thread) = state.thread_for_message(&root, message_id) else {
                    return;
                };
                registry.send(
                    &root,
                    AgentUpdate::Message(AgentMessageUpdate {
                        message_id,
                        thread,
                        delivery,
                    }),
                );
                registry.changed();
                state.mark_message_terminal(&root, message_id);
            })
            .await;
    }

    pub async fn wait(
        self: &Arc<Self>,
        session_id: &str,
        call_id: &str,
        ids: &[AgentId],
        duration: Duration,
    ) -> std::io::Result<AgentWait> {
        self.wait_inner(session_id, call_id, ids, duration)
            .instrument(nanocode_trace::info_span!(
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
        let _warm = self.warm(&self.root_of(session_id), ids.to_vec()).await?;
        let session = session_id.to_owned();
        let wanted = ids.to_vec();
        let mut changes = self
            .state
            .call(move |state| {
                let root = state.root_session_id(&session).to_owned();
                state
                    .scopes
                    .get_mut(&root)
                    .map(|scope| {
                        wanted
                            .iter()
                            .filter_map(|id| scope.sessions.subscribe(id))
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default()
            })
            .await;

        let deadline = nanocode_platform::now().checked_add(duration);
        loop {
            let session = session_id.to_owned();
            let wanted = ids.to_vec();
            let (snapshot, root) = self
                .state
                .call(move |state| -> std::io::Result<_> {
                    let root = state.root_session_id(&session).to_owned();
                    Ok((state.wait_snapshot(&session, &wanted)?, root))
                })
                .await?;

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

            let reported = |reported: &AgentMap<u64>| {
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
                self.record_waited(session_id, call_id, &terminal, &delivered)
                    .await;
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
            let changed = async {
                if changes.is_empty() {
                    return std::future::pending().await;
                }
                drop(
                    futures_util::future::select_all(
                        changes
                            .iter_mut()
                            .map(|changes| Box::pin(changes.changed())),
                    )
                    .await,
                );
            };
            let timed_out = match deadline {
                Some(deadline) => timeout_at(deadline, changed).await.is_err(),
                None => {
                    changed.await;
                    false
                }
            };
            if timed_out {
                let session = session_id.to_owned();
                let wanted = ids.to_vec();
                let summaries = self
                    .state
                    .call(move |state| state.summaries(&session, &wanted))
                    .await?;
                return Ok((summaries, delivered, true, None));
            }
        }
    }

    async fn record_waited(
        &self,
        session_id: &str,
        call_id: &str,
        terminal: &[(AgentId, u64)],
        delivered: &HashSet<AgentId>,
    ) {
        let reported = terminal
            .iter()
            .filter(|(id, _)| !delivered.contains(id))
            .copied()
            .collect::<Vec<_>>();
        let session = session_id.to_owned();
        let call = call_id.to_owned();
        let root = self
            .state
            .call(move |state| {
                let root = state.root_session_id(&session).to_owned();
                let Some(scope) = state.scopes.get_mut(&root) else {
                    return root;
                };
                for (id, mark) in reported {
                    let Some(child) = scope.sessions.get_mut(&id) else {
                        continue;
                    };
                    child.waited.retain(|waited| waited.session != session);
                    child.waited.push(durable::Waited {
                        session: session.clone(),
                        call: call.clone(),
                        mark,
                    });
                }
                root
            })
            .await;
        self.changed();
        if self.store_for(&root).is_some()
            && let Err(error) = self.persist_durably(&root).await
        {
            nanocode_trace::warn!(%error, session_id, call_id, "the results this wait delivers are not journaled; a restart waits afresh");
        }
    }

    pub async fn rewait(
        self: &Arc<Self>,
        session_id: &str,
        call_id: &str,
        ids: &[AgentId],
    ) -> std::io::Result<Option<AgentWait>> {
        self.await_restored(session_id).await?;
        let _warm = self.warm(&self.root_of(session_id), ids.to_vec()).await?;
        let session = session_id.to_owned();
        let call = call_id.to_owned();
        let wanted = ids.to_vec();
        let (snapshot, root, answered) = self
            .state
            .call(move |state| -> std::io::Result<_> {
                let root = state.root_session_id(&session).to_owned();
                let snapshot = state.wait_snapshot(&session, &wanted)?;
                let answered = state.scopes.get(&root).map_or_else(Vec::new, |scope| {
                    snapshot
                        .iter()
                        .filter(|(summary, revision)| {
                            let mark = wait_mark(&summary.status, *revision);
                            summary.status.is_wait_terminal()
                                && scope.sessions.get(&summary.agent_id).is_some_and(|child| {
                                    child.waited.iter().any(|waited| {
                                        waited.session == session
                                            && waited.call == call
                                            && waited.mark == mark
                                    })
                                })
                        })
                        .map(|(summary, _)| summary.agent_id)
                        .collect()
                });
                Ok((snapshot, root, answered))
            })
            .await?;
        if answered.is_empty() {
            return Ok(None);
        }
        let delivered = self.wait_reported.read(|reported| {
            snapshot
                .iter()
                .filter(|(summary, revision)| {
                    !answered.contains(&summary.agent_id)
                        && summary.status.is_wait_terminal()
                        && reported.get(&(session_id.to_owned(), summary.agent_id))
                            == Some(&wait_mark(&summary.status, *revision))
                })
                .map(|(summary, _)| summary.agent_id)
                .collect::<HashSet<_>>()
        });
        let summaries = snapshot.into_iter().map(|(summary, _)| summary).collect();
        let receipt = if root == session_id {
            self.observe_completions(&root, &answered, call_id).await
        } else {
            None
        };
        Ok(Some((summaries, delivered, false, receipt)))
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
        let _warm = self.warm_subtree(session_id, id).await?;
        let locks = self.session_locks(session_id).await;
        let _closing = locks.closing.enter().await;
        let _paused = self.pause_messages(session_id).await;
        let session = session_id.to_owned();
        let (root_session_id, ids, held) = self
            .state
            .call(move |state| state.request_interrupt(&session, id))
            .await?;
        self.settle_held(&root_session_id, held);
        self.changed();
        let deadline = nanocode_platform::now() + AGENT_STOP_TIMEOUT;
        self.interrupt_harnesses(&root_session_id, &ids, deadline)
            .await?;
        self.state
            .call(move |state| state.summaries_in_scope(&root_session_id, &ids))
            .await
    }

    pub async fn close(&self, session_id: &str, id: AgentId) -> std::io::Result<Vec<AgentSummary>> {
        self.await_restored(session_id).await?;
        let _warm = self.warm_subtree(session_id, id).await?;
        let locks = self.session_locks(session_id).await;
        let _closing = locks.closing.enter().await;
        let _paused = self.pause_messages(session_id).await;
        let CloseRequest {
            root_session_id,
            ids,
            harnesses,
            status_updates,
        } = {
            let session = session_id.to_owned();
            self.state
                .call(move |state| state.request_close(&session, id))
                .await?
        };
        for (id, status, lane) in status_updates {
            self.enqueue_status(&root_session_id, id, lane, OrderedJob::Status(status));
        }
        self.changed();
        let summaries = self
            .stop_and_close(root_session_id.clone(), ids, harnesses, false)
            .await?;
        let root = root_session_id.clone();
        let frozen = self
            .state
            .call(move |state| {
                state
                    .scopes
                    .get(&root)
                    .is_some_and(|scope| scope.life.frozen())
            })
            .await;
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
        let _closing = locks.closing.enter().await;
        let _paused = self.pause_messages(session_id).await;
        let mut revision = self.revision.subscribe();
        let capture_deadline = nanocode_platform::now() + AGENT_STOP_TIMEOUT;
        let registry = self.shared();
        let (request, final_journal) = loop {
            let session = session_id.to_owned();
            let registry = registry.clone();
            let attempt = self
                .state
                .call(move |state| -> std::io::Result<_> {
                    let root = state.root_session_id(&session).to_owned();
                    let Some(registry) = registry else {
                        return Err(std::io::Error::other("subagent runtime is closed"));
                    };
                    let captures_pending = registry
                        .scope_locks(&root)
                        .pending_checkpoints
                        .load(Ordering::Acquire)
                        > 0;
                    if captures_pending {
                        return Ok(Err(root));
                    }
                    let frozen = state
                        .scopes
                        .get(&root)
                        .is_some_and(|scope| scope.life.frozen());
                    let final_journal = match registry.store_for(&root) {
                        Some(_) if !frozen => {
                            let checkpoints = registry.checkpoints.get();
                            let scope = state.scope_mut(&root);
                            let payload = scope.journal_payload(
                                &root,
                                &checkpoints,
                                registry.journal_version.load(Ordering::Relaxed),
                            )?;
                            scope.life = ScopeLife::Frozen;
                            Some(payload)
                        }
                        _ => None,
                    };
                    Ok(Ok((state.request_close_all(&session)?, final_journal)))
                })
                .await?;
            if let Err(root) = attempt {
                match timeout_at(capture_deadline, revision.changed()).await {
                    Ok(Ok(())) => continue,
                    Ok(Err(_)) => {
                        return Err(std::io::Error::other("subagent runtime is closed"));
                    }
                    Err(_) => {
                        nanocode_trace::warn!(%root, "subagent checkpoint capture timed out during shutdown");
                        self.scope_locks(&root)
                            .pending_checkpoints
                            .store(0, Ordering::Release);
                        continue;
                    }
                }
            }
            if let Ok(ready) = attempt {
                break ready;
            }
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

        for (id, status, lane) in status_updates {
            self.enqueue_status(&root_session_id, id, lane, OrderedJob::Status(status));
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

    async fn release_all(&self) {
        let mut roots = self
            .state
            .call(|state| state.scopes.keys().cloned().collect::<Vec<_>>())
            .await;
        roots.sort_unstable();
        let released =
            nanocode_sync::join_in_order(roots.iter().map(|root| self.release_scope(root))).await;
        for (root, result) in roots.iter().zip(released) {
            if let Err(error) = result {
                nanocode_trace::warn!(%root, %error, "could not stop subagents at shutdown");
            }
        }
        self.session_handles.update(|handles| handles.clear());
    }

    async fn release_scope(&self, root_session_id: &str) -> std::io::Result<()> {
        drop(
            timeout_at(
                nanocode_platform::now() + AGENT_STOP_TIMEOUT,
                self.await_restored(root_session_id),
            )
            .await,
        );
        let locks = self.session_locks(root_session_id).await;
        let _closing = locks.closing.enter().await;
        let _paused = self.pause_messages(root_session_id).await;
        if self.store_for(root_session_id).is_some()
            && let Ok(Err(error)) = timeout_at(
                nanocode_platform::now() + AGENT_STOP_TIMEOUT,
                self.save_scope(root_session_id),
            )
            .await
        {
            nanocode_trace::debug!(%error, %root_session_id, "subagent journal was not saved before its scope stopped");
        }
        let root = root_session_id.to_owned();
        let request = self
            .state
            .call(move |state| {
                state.scope_mut(&root).life = ScopeLife::Released;
                state.request_close_all(&root)
            })
            .await?;
        self.changed();
        self.stop_and_close(
            request.root_session_id,
            request.ids,
            request.harnesses,
            true,
        )
        .await
        .map(drop)
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
        let deadline = nanocode_platform::now() + AGENT_STOP_TIMEOUT;
        let closing_result = self.close_harnesses(harnesses, deadline).await;
        if let Some(registry) = self.shared() {
            registry
                .settle_hibernated(
                    &root_session_id,
                    &ids,
                    || NanocodeError::TurnStopped,
                    "message cancelled because the agent closed",
                )
                .await;
        }
        self.wait_until_inactive(&root_session_id, &ids, deadline)
            .await?;

        drop(closing_result);
        let (
            ClosedSessions {
                summaries,
                harness_tasks,
                event_tasks,
            },
            released,
        ) = {
            let root = root_session_id.clone();
            let closing = ids.clone();
            self.state
                .call(move |state| {
                    let closed = state.finish_close(&root, &closing)?;
                    let released = state
                        .scopes
                        .get(&root)
                        .is_some_and(|scope| scope.life.released());
                    Ok::<_, std::io::Error>((closed, released))
                })
                .await?
        };
        for summary in summaries.iter().filter(|_| !released) {
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
            let root = root_session_id.clone();
            let closing = ids.clone();
            self.state
                .call(move |state| {
                    state
                        .scopes
                        .get(&root)
                        .map(|scope| {
                            closing
                                .iter()
                                .filter_map(|id| {
                                    scope
                                        .sessions
                                        .get(id)
                                        .map(|session| session.descriptor.session_id.clone())
                                })
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default()
                })
                .await
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
        let _closing = locks.closing.enter().await;
        let _paused = self.pause_messages(session_id).await;
        let session = session_id.to_owned();
        let (root_session_id, ids, held) = self
            .state
            .call(move |state| state.request_interrupt_all(&session))
            .await;
        self.settle_held(&root_session_id, held);
        self.changed();
        let deadline = nanocode_platform::now() + AGENT_STOP_TIMEOUT;
        drop(
            self.interrupt_harnesses(&root_session_id, &ids, deadline)
                .await,
        );
    }

    fn settle_held(&self, root_session_id: &str, held: Held) {
        for (id, completion, lane) in held {
            self.enqueue_status(
                root_session_id,
                id,
                lane,
                OrderedJob::Settle {
                    status: AgentStatus::Interrupted,
                    completion,
                    harness: None,
                },
            );
        }
    }

    async fn interrupt_harnesses(
        &self,
        root_session_id: &str,
        ids: &[AgentId],
        deadline: Instant,
    ) -> std::io::Result<()> {
        let mut revision = self.revision.subscribe();
        let mut told = Told::default();
        loop {
            let root = root_session_id.to_owned();
            let waiting = ids.to_vec();
            let (inactive, harnesses, asleep, kept) = self
                .state
                .call(move |state| state.interrupt_active(&root, &waiting, told))
                .await?;
            told = kept;
            if inactive {
                return Ok(());
            }
            let interruption = async {
                nanocode_sync::join_in_order(
                    harnesses
                        .into_iter()
                        .map(|harness| async move { harness.interrupt().await }),
                )
                .await;
                if let Some(registry) = self.shared() {
                    nanocode_sync::join_in_order(
                        asleep.iter().map(|id| registry.wake(root_session_id, *id)),
                    )
                    .await;
                }
            };
            timeout_at(deadline, interruption).await.map_err(|_| {
                std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "timed out interrupting subagent harnesses",
                )
            })?;
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

    async fn close_harnesses(
        &self,
        harnesses: Vec<HarnessHandle>,
        deadline: Instant,
    ) -> std::io::Result<()> {
        let closing = async move {
            let results = nanocode_sync::join_in_order(
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
        let completion = nanocode_sync::join_in_order(tasks.iter_mut());
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
            let root = root_session_id.to_owned();
            let waiting = ids.to_vec();
            if self
                .state
                .call(move |state| state.all_inactive(&root, &waiting))
                .await?
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
        stored_runtime: StoredRuntime,
        next_instruction_revision: u64,
        last_output: Option<Value>,
    ) -> Self {
        Self {
            binding_task: descriptor.task.clone(),
            descriptor,
            host_context,
            event_task: None,
            residency: Residency::Evicted,
            harness_task: None,
            status,
            turn: None,
            output_validator: contract.validator,
            output_schema: contract.value,
            stored_runtime,
            next_instruction_revision,
            last_output,
            follow_up_output: None,
            delegated_turn: false,
            last_used: 0,
            resume_attempts: 0,
            in_flight_calls: Vec::new(),
            in_flight_omitted: 0,
            completion: Completion::None,
            cancelled_revision: None,
            in_spawning_turn: false,
            settled_revision: None,
            usage: AgentUsage::default(),
            in_flight_progress: durable::OmittedProgress::default(),
            hibernated: None,
            hibernations: 0,
            residency_lock: Sequencer::new(),
            marks: Arc::default(),
            changes: None,
            announce: true,
            journal_generation: 0,
            conversation: false,
            waited: Vec::new(),
            sent: Vec::new(),
            spawned: None,
        }
    }

    pub(super) const fn active(&self) -> bool {
        self.turn.is_some()
    }

    fn settled(&self) -> bool {
        matches!(
            self.status,
            AgentStatus::Completed { .. } | AgentStatus::Failed { .. } | AgentStatus::Closed
        ) && !self.unsettled()
            && self.hibernated.is_none()
            && self.harness().is_none()
            && self.harness_task.is_none()
            && self.event_task.is_none()
            && matches!(
                self.stored_runtime,
                StoredRuntime::None | StoredRuntime::Journaled
            )
            && self.in_flight_calls.is_empty()
            && self.in_flight_omitted == 0
            && self.follow_up_output.is_none()
            && !self.delegated_turn
            && self.residency_lock.is_unshared()
            && self
                .changes
                .as_ref()
                .is_none_or(|changes| changes.receiver_count() == 0)
    }

    const fn unsettled(&self) -> bool {
        self.active()
            || self.status.is_active()
            || self.completion.revision().is_some()
            || (matches!(self.status, AgentStatus::Interrupted) && self.resume_attempts > 0)
    }

    pub(super) fn turn_revision(&self) -> Option<u64> {
        self.turn.as_ref().map(|turn| turn.revision)
    }

    pub(super) const fn submitted_output(&self) -> Option<&Value> {
        match &self.turn {
            Some(ActiveTurn {
                stage: TurnStage::Submitted(output),
                ..
            }) => Some(output),
            _ => None,
        }
    }

    pub(super) const fn harness(&self) -> Option<&HarnessHandle> {
        match &self.residency {
            Residency::Resident(harness) => Some(harness),
            Residency::Detached | Residency::Evicted => None,
        }
    }

    pub(super) fn settle_completion(&mut self, revision: u64) {
        let parked = self
            .cancelled_revision
            .is_some_and(|cancelled| revision <= cancelled);
        self.completion = Completion::journaled(Some(revision), 0, parked);
    }

    const fn clear_completion(&mut self) {
        self.completion = Completion::None;
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
            budget_usd: self.descriptor.budget_usd,
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
    pub async fn shutdown(&self) {
        self.registry.release_all().await;
    }

    pub async fn completion_origin(&self, request_id: &str) -> Option<Arc<str>> {
        let (session_id, _) = request_id
            .strip_prefix(crate::COMPLETION_REQUEST_PREFIX)?
            .split_once(':')?;
        self.registry.host_context_for_session(session_id).await
    }

    pub async fn lineage(&self, session_id: &str) -> Vec<AgentId> {
        self.registry.lineage(session_id).await
    }

    pub fn set_max_concurrency(&self, limit: usize) {
        self.registry.set_max_concurrency(limit);
    }

    pub fn set_queued_spawns(&self, queued: bool) {
        self.registry.set_queued_spawns(queued);
    }

    pub fn set_max_resident(&self, limit: usize) {
        self.registry.set_max_resident(limit);
    }

    pub fn set_hibernation(&self, hibernation: bool) {
        self.registry.set_hibernation(hibernation);
    }

    pub async fn cancel_all(&self, root_session_id: &str) {
        self.registry.cancel_all(root_session_id).await;
    }

    pub async fn close_all(&self, root_session_id: &str) -> std::io::Result<()> {
        self.registry.close_all(root_session_id).await.map(drop)
    }

    pub fn set_manual_resume(&self, manual: bool) {
        self.registry
            .manual_resume
            .store(manual, std::sync::atomic::Ordering::SeqCst);
    }

    pub fn set_journal_version(&self, version: u32) -> std::io::Result<()> {
        if !(durable::TREE_JOURNAL_VERSION..=durable::JOURNAL_VERSION).contains(&version) {
            return Err(std::io::Error::other(format!(
                "subagent journal version {version} is not one this build writes ({} to {})",
                durable::TREE_JOURNAL_VERSION,
                durable::JOURNAL_VERSION
            )));
        }
        self.registry
            .journal_version
            .store(version, Ordering::Relaxed);
        Ok(())
    }

    pub async fn deferred_resume(&self, root_session_id: &str) -> std::io::Result<Vec<AgentId>> {
        self.registry.await_restored(root_session_id).await?;
        Ok(self
            .registry
            .deferred_resume
            .read(|deferred| {
                deferred
                    .get(root_session_id)
                    .map(|ids| ids.iter().copied().collect())
            })
            .unwrap_or_default())
    }

    pub async fn resume_restored(
        &self,
        root_session_id: &str,
    ) -> std::io::Result<Vec<(AgentId, std::io::Result<MessageReceipt>)>> {
        self.registry.await_restored(root_session_id).await?;
        let mut ids = Vec::new();
        self.registry.deferred_resume.update(|deferred| {
            ids = deferred
                .remove(root_session_id)
                .map(|ids| ids.into_iter().collect())
                .unwrap_or_default();
        });
        let work = self.registry.take_deferred_work(root_session_id);
        let results = if ids.is_empty() {
            Vec::new()
        } else {
            self.registry.pending_resume.update(|pending| {
                pending
                    .entry(root_session_id.to_owned())
                    .or_default()
                    .extend(ids.iter().copied());
            });
            self.registry.changed();
            self.registry.resume_interrupted(root_session_id).await
        };
        if let Some(work) = work {
            self.registry
                .start_restored_work(root_session_id, work)
                .await;
        }
        Ok(results)
    }

    pub async fn discard_restored(&self, root_session_id: &str) -> std::io::Result<()> {
        self.registry.await_restored(root_session_id).await?;
        let mut ids = std::collections::BTreeSet::new();
        self.registry.deferred_resume.update(|deferred| {
            ids = deferred
                .remove(root_session_id)
                .map(|ids| ids.into_iter().collect())
                .unwrap_or_default();
        });
        let messages = match self.registry.take_deferred_work(root_session_id) {
            Some(work) => {
                ids.extend(work.unstarted);
                ids.extend(work.messages.iter().map(|message| message.to));
                work.messages
            }
            None => Vec::new(),
        };
        let closed = if ids.is_empty() {
            Ok(())
        } else {
            self.registry
                .close_batch(root_session_id, ids.into_iter().collect())
                .await
        };
        for message in messages {
            self.registry
                .message_failed(
                    root_session_id,
                    message.id,
                    format!(
                        "message {} was discarded with restored agent {}",
                        message.id, message.to
                    ),
                )
                .await;
        }
        closed
    }

    pub fn runtime_id(&self) -> SubagentRuntimeId {
        self.registry.id
    }
}

async fn continue_root(
    registry: Arc<Registry>,
    resolver: ParentResolver,
    root_session_id: String,
    mut completions: mpsc::UnboundedReceiver<(AgentId, u64)>,
) {
    while let Some(first) = completions.recv().await {
        let mut batch = vec![first];
        while let Ok(completion) = completions.try_recv() {
            batch.push(completion);
        }
        match resolver(&root_session_id) {
            Some(parent) => {
                if let Err(error) = registry
                    .continue_parent_batch(&parent, &root_session_id, batch)
                    .await
                {
                    nanocode_trace::warn!(%error, "subagent completions did not continue their parent");
                }
            }
            None => {
                for (id, revision) in batch {
                    registry.retry_completion(&root_session_id, id, revision);
                }
            }
        }
        if completions.is_empty() {
            completions.close();
            while let Some((id, revision)) = completions.recv().await {
                registry.schedule_parent_continuation(&root_session_id, id, revision);
            }
            return;
        }
    }
}

pub(super) fn forward_events(
    root_session_id: String,
    id: AgentId,
    parent: Option<AgentId>,
    mut events: AgentEvents,
    start: oneshot::Receiver<(u64, Arc<AgentMarks>)>,
    registry: Weak<Registry>,
    updates: mpsc::UnboundedSender<ScopedAgentUpdate>,
) -> Task<()> {
    platform::spawn(async move {
        let Ok((hibernations, marks)) = start.await else {
            return;
        };
        while let Some(first) = events.recv().await {
            let mut tracked = Vec::new();
            let mut progress = false;
            let mut completed_tool = false;
            let mut next = Some(first);
            while let Some(event) = next {
                let kind = event.kind;
                let boundary = matches!(
                    kind,
                    AgentEventKind::ModelCallStarted | AgentEventKind::ModelCallCompleted
                );
                progress |= boundary || kind == AgentEventKind::ToolCall;
                completed_tool |= kind == AgentEventKind::ToolResult;
                if boundary {
                    tracked.push(TrackedEvent::Boundary);
                } else if matches!(kind, AgentEventKind::ToolCall | AgentEventKind::ToolResult)
                    && let Ok(call) = serde_json::from_str::<InFlightEvent>(event.payload.get())
                {
                    tracked.push(TrackedEvent::Call(kind, call));
                }
                let _ = send_update(
                    &updates,
                    &root_session_id,
                    AgentUpdate::Event { id, parent, event },
                    false,
                );
                next = events.try_recv();
            }
            if let Some(registry) = registry.upgrade() {
                if !tracked.is_empty() {
                    registry.track_events(&root_session_id, id, tracked).await;
                }
                if completed_tool {
                    marks.completed_tool.store(true, Ordering::Release);
                }
                if progress {
                    registry.capture_progress(&root_session_id, id, &marks);
                }
            }
        }
        if let Some(registry) = registry.upgrade() {
            registry
                .runtime_closed(&root_session_id, id, hibernations)
                .await;
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
