use std::{
    collections::{BTreeMap, HashMap, HashSet},
    ops::Range,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering},
    },
};

use arc_swap::ArcSwapOption;
use nanocode_agent_core::backend::SteerReceiptStatus;
use serde::{Serialize, de::DeserializeOwned};
use tokio::sync::{mpsc, oneshot};
use tracing::Instrument;

use crate::{
    Committed, DurableState, EncodedPayload, Error, OperationStatus, OwnerId, OwnerToken, Result,
    SessionRecord, StateCommit, StateStore, SteerState, StepStatus, StoreError, StoreEvent,
    StoreReplacement, StoredState, SummaryUpdate, Transition,
    catalog::{is_listed, prompt_text, summarize},
    shared_store::SharedStore,
    state::RetainedCheckpoint,
};

const COMMAND_CAPACITY: usize = 64;
const RELEASE_BURST_LIMIT: usize = 32;

#[derive(Clone, Debug)]
pub enum Admission<C = EncodedPayload, O = EncodedPayload> {
    Accepted,

    Pending,

    Completed { checkpoint: C, output: O },

    Failed { checkpoint: C, error: String },

    Cancelled,
}

#[derive(Clone, Debug)]
pub struct AutomaticAdmission<C = EncodedPayload, O = EncodedPayload> {
    operation_id: String,
    admission: Admission<C, O>,
}

impl<C, O> AutomaticAdmission<C, O> {
    #[must_use]
    pub fn operation_id(&self) -> &str {
        &self.operation_id
    }

    #[must_use]
    pub fn into_parts(self) -> (String, Admission<C, O>) {
        (self.operation_id, self.admission)
    }
}

#[derive(Clone, Debug)]
pub enum BeginStep<O = EncodedPayload> {
    Execute,

    Interrupted,

    Replay(O),
}

enum StoredAdmission {
    Accepted,
    Pending,
    Completed {
        checkpoint: EncodedPayload,
        output: EncodedPayload,
    },
    Failed {
        checkpoint: EncodedPayload,
        error: String,
    },
    Cancelled,
}

impl StoredAdmission {
    fn into_encoded(self) -> Admission {
        match self {
            Self::Accepted => Admission::Accepted,
            Self::Pending => Admission::Pending,
            Self::Completed { checkpoint, output } => Admission::Completed { checkpoint, output },
            Self::Failed { checkpoint, error } => Admission::Failed { checkpoint, error },
            Self::Cancelled => Admission::Cancelled,
        }
    }

    fn decode<C, O>(self) -> Result<Admission<C, O>>
    where
        C: DeserializeOwned,
        O: DeserializeOwned,
    {
        match self {
            Self::Accepted => Ok(Admission::Accepted),
            Self::Pending => Ok(Admission::Pending),
            Self::Completed { checkpoint, output } => Ok(Admission::Completed {
                checkpoint: checkpoint.decode()?,
                output: output.decode()?,
            }),
            Self::Failed { checkpoint, error } => Ok(Admission::Failed {
                checkpoint: checkpoint.decode()?,
                error,
            }),
            Self::Cancelled => Ok(Admission::Cancelled),
        }
    }
}

enum StoredBeginStep {
    Execute,
    Interrupted,
    Replay(EncodedPayload),
}

impl From<StoredBeginStep> for BeginStep {
    fn from(step: StoredBeginStep) -> Self {
        match step {
            StoredBeginStep::Execute => Self::Execute,
            StoredBeginStep::Interrupted => Self::Interrupted,
            StoredBeginStep::Replay(output) => Self::Replay(output),
        }
    }
}

pub enum OwnerWrite {
    Advance {
        continuation: EncodedPayload,
    },

    BeginStep {
        step_id: String,
        kind: String,
        input: EncodedPayload,
    },

    CompleteStep {
        step_id: String,
        output: EncodedPayload,
    },
}

pub enum WriteOutcome {
    Advanced,

    Began(BeginStep),

    Completed,
}

#[derive(Clone)]
pub struct StoredSteer {
    pub index: u32,

    pub state: SteerState,
}

#[derive(Clone, Eq, PartialEq)]
enum Caller {
    Direct(OwnerId),
    Agent(u64),
}

struct AgentAcquisition {
    generation: u64,
    checkpoint: Option<EncodedPayload>,
    initial_committed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InitialAcquisition {
    Committed,
    Existing(Option<EncodedPayload>),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum TitleChange {
    #[default]
    Unchanged,
    Set(String),
    Clear,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MetadataExpectation {
    pub title: Option<Option<String>>,
    pub values: BTreeMap<String, Option<serde_json::Value>>,
}

impl MetadataExpectation {
    fn matches(&self, record: &SessionRecord) -> bool {
        self.title
            .as_ref()
            .is_none_or(|title| *title == record.title)
            && self
                .values
                .iter()
                .all(|(key, value)| record.metadata.get(key) == value.as_ref())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MetadataWrite {
    pub expect: Option<MetadataExpectation>,
    pub title: TitleChange,
    pub values: BTreeMap<String, Option<serde_json::Value>>,
}

impl MetadataWrite {
    fn apply(&self, record: &mut SessionRecord) {
        if self
            .expect
            .as_ref()
            .is_some_and(|expect| !expect.matches(record))
        {
            return;
        }
        match &self.title {
            TitleChange::Unchanged => {}
            TitleChange::Set(title) => record.title = Some(title.clone()),
            TitleChange::Clear => record.title = None,
        }
        for (key, value) in &self.values {
            match value {
                Some(value) => {
                    record.metadata.insert(key.clone(), value.clone());
                }
                None => {
                    record.metadata.remove(key);
                }
            }
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SessionWrite {
    pub events: Vec<StoreEvent>,
    pub metadata: Option<MetadataWrite>,
}

#[derive(Clone, Debug)]
pub struct SessionReceipt {
    pub revision: u64,
    pub events: Range<u64>,
    pub record: Option<SessionRecord>,
}

enum Command {
    LoadPayloads {
        caller: Option<Caller>,
        payloads: Vec<EncodedPayload>,
        result: oneshot::Sender<Result<Vec<EncodedPayload>>>,
    },
    LoadPayload {
        caller: Option<Caller>,
        payload: EncodedPayload,
        result: oneshot::Sender<Result<EncodedPayload>>,
    },
    Continuation {
        caller: Caller,
        operation_id: String,
        result: oneshot::Sender<Result<Option<EncodedPayload>>>,
    },
    Advance {
        caller: Caller,
        operation_id: String,
        continuation: EncodedPayload,
        deferred: bool,
        result: oneshot::Sender<Result<()>>,
    },
    RecoverFailure {
        caller: Caller,
        operation_id: String,
        result: oneshot::Sender<Result<Option<OperationStatus>>>,
    },
    Acquire {
        result: oneshot::Sender<Result<()>>,
    },
    CurrentOwner {
        result: oneshot::Sender<Result<bool>>,
    },
    State {
        result: oneshot::Sender<Result<DurableState>>,
    },
    LatestCheckpoint {
        result: oneshot::Sender<Result<Option<EncodedPayload>>>,
    },
    AcquireAgent {
        initial: Option<crate::InitialSettings>,
        checkpoint: Option<EncodedPayload>,
        journal: Option<crate::child_journal::Seed>,
        preloaded: Option<LoadedCheckpoint>,
        result: oneshot::Sender<Result<AgentAcquisition>>,
    },
    Admit {
        caller: Caller,
        operation_id: String,
        input: EncodedPayload,
        acknowledged: oneshot::Receiver<()>,
        release_commands: mpsc::Sender<CommandRequest>,
        result: oneshot::Sender<Result<StoredAdmission>>,
    },
    AdmitAutomatic {
        caller: Caller,
        candidate_operation_id: String,
        input: EncodedPayload,
        acknowledged: oneshot::Receiver<()>,
        release_commands: mpsc::Sender<CommandRequest>,
        result: oneshot::Sender<Result<(String, StoredAdmission)>>,
    },
    Release {
        caller: Caller,
        operation_id: String,
        result: oneshot::Sender<Result<()>>,
    },
    BeginAttempt {
        caller: Caller,
        operation_id: String,
        result: oneshot::Sender<Result<()>>,
    },
    AcceptSteer {
        message_id: Option<String>,
        caller: Caller,
        operation_id: String,
        accepted_after_model_call_index: u32,
        input: EncodedPayload,
        identity: Option<EncodedPayload>,
        result: oneshot::Sender<Result<Option<u32>>>,
    },
    IdentifiedSteerAccepted {
        caller: Caller,
        message_id: String,
        input: EncodedPayload,
        result: oneshot::Sender<Result<bool>>,
    },
    SteerReceipt {
        caller: Caller,
        message_id: String,
        input: EncodedPayload,
        result: oneshot::Sender<Result<SteerReceiptStatus>>,
    },
    RetainedSteers {
        caller: Caller,
        operation_id: String,
        result: oneshot::Sender<Result<Vec<StoredSteer>>>,
    },
    WithdrawSteer {
        caller: Caller,
        operation_id: String,
        steer_index: u32,
        result: oneshot::Sender<Result<()>>,
    },
    BindSteer {
        caller: Caller,
        operation_id: String,
        steer_index: u32,
        model_call_index: u32,
        result: oneshot::Sender<Result<()>>,
    },
    BeginStep {
        caller: Caller,
        operation_id: String,
        step_id: String,
        kind: String,
        input: EncodedPayload,
        result: oneshot::Sender<Result<StoredBeginStep>>,
    },
    CompleteStep {
        caller: Caller,
        operation_id: String,
        step_id: String,
        output: EncodedPayload,
        deferred: bool,
        result: oneshot::Sender<Result<()>>,
    },
    CommitWrites {
        caller: Caller,
        operation_id: String,
        writes: Vec<OwnerWrite>,
        result: oneshot::Sender<Result<Vec<WriteOutcome>>>,
    },
    Complete {
        caller: Caller,
        operation_id: String,
        checkpoint: EncodedPayload,
        output: EncodedPayload,
        result: oneshot::Sender<Result<()>>,
    },
    Fail {
        caller: Caller,
        operation_id: String,
        checkpoint: EncodedPayload,
        error: String,
        result: oneshot::Sender<Result<()>>,
    },
    FailAttempt {
        caller: Caller,
        operation_id: String,
        result: oneshot::Sender<Result<()>>,
    },
    Cancel {
        caller: Caller,
        operation_id: String,
        checkpoint: Option<EncodedPayload>,
        result: oneshot::Sender<Result<()>>,
    },
    PruneReceipts {
        caller: Caller,
        result: oneshot::Sender<Result<()>>,
    },
    CommitCheckpoint {
        caller: Caller,
        checkpoint: EncodedPayload,
        write: SessionWrite,
        result: oneshot::Sender<Result<SessionReceipt>>,
    },
    Stage {
        write: SessionWrite,
        result: oneshot::Sender<Result<()>>,
    },
    Append {
        write: SessionWrite,
        result: oneshot::Sender<Result<SessionReceipt>>,
    },
    DiscardUnused {
        caller: Caller,
        result: oneshot::Sender<Result<()>>,
    },
    Abandonable {
        caller: Caller,
        kinds: Vec<String>,
        result: oneshot::Sender<Result<()>>,
    },
    Describe {
        caller: Caller,
        record: crate::catalog::SessionRecord,
        result: oneshot::Sender<Result<crate::catalog::SessionRecord>>,
    },
}

impl Command {
    fn reject(self, error: Error) {
        match self {
            Self::LoadPayloads { result, .. } => drop(result.send(Err(error))),
            Self::LoadPayload { result, .. } => drop(result.send(Err(error))),
            Self::Continuation { result, .. } => drop(result.send(Err(error))),
            Self::Advance { result, .. }
            | Self::Release { result, .. }
            | Self::BeginAttempt { result, .. }
            | Self::WithdrawSteer { result, .. }
            | Self::BindSteer { result, .. }
            | Self::CompleteStep { result, .. }
            | Self::Complete { result, .. }
            | Self::Fail { result, .. }
            | Self::FailAttempt { result, .. }
            | Self::Cancel { result, .. }
            | Self::PruneReceipts { result, .. }
            | Self::DiscardUnused { result, .. }
            | Self::Abandonable { result, .. }
            | Self::Stage { result, .. }
            | Self::Acquire { result } => drop(result.send(Err(error))),
            Self::CommitCheckpoint { result, .. } | Self::Append { result, .. } => {
                drop(result.send(Err(error)));
            }
            Self::RecoverFailure { result, .. } => drop(result.send(Err(error))),
            Self::State { result } => drop(result.send(Err(error))),
            Self::CurrentOwner { result } => drop(result.send(Err(error))),
            Self::LatestCheckpoint { result } => drop(result.send(Err(error))),
            Self::AcquireAgent { result, .. } => drop(result.send(Err(error))),
            Self::Admit { result, .. } => drop(result.send(Err(error))),
            Self::AdmitAutomatic { result, .. } => drop(result.send(Err(error))),
            Self::AcceptSteer { result, .. } => drop(result.send(Err(error))),
            Self::IdentifiedSteerAccepted { result, .. } => drop(result.send(Err(error))),
            Self::SteerReceipt { result, .. } => drop(result.send(Err(error))),
            Self::RetainedSteers { result, .. } => drop(result.send(Err(error))),
            Self::BeginStep { result, .. } => drop(result.send(Err(error))),
            Self::CommitWrites { result, .. } => drop(result.send(Err(error))),
            Self::Describe { result, .. } => drop(result.send(Err(error))),
        }
    }
}

struct CommandRequest {
    command: Command,
    span: tracing::Span,
}

impl From<Command> for CommandRequest {
    fn from(command: Command) -> Self {
        Self {
            command,
            span: tracing::Span::current(),
        }
    }
}

enum Resolution {
    Committed(Committed),
    Absent,
    Unknown,
}

struct Driver {
    store: Box<dyn StateStore>,
    state_id: Arc<str>,
    state: DurableState,
    terminal_receipt_limit: Option<usize>,
    owner: Option<OwnerToken>,
    pending_record: Option<crate::catalog::SessionRecord>,
    create: bool,
    next_agent_generation: u64,
    active_agent_generation: Option<u64>,
    claimed: HashMap<String, Caller>,
    running: HashSet<String>,
    abandonable: HashSet<String>,
    poisoned: bool,
    staged: Vec<SessionWrite>,
    preview: Option<(String, Option<String>)>,
    batch: Option<u64>,

    committed_records: HashSet<String>,
    next_event: Option<u64>,
    commands: mpsc::Receiver<CommandRequest>,
    releases: mpsc::UnboundedReceiver<ReleaseSignal>,
}

const OWNER_ACTIVE: u8 = 0;
const OWNER_RELEASING: u8 = 1;
const OWNER_RELEASED: u8 = 2;

struct OwnerReleaseState {
    state: AtomicU8,
    completed: tokio::sync::watch::Sender<bool>,
}

impl OwnerReleaseState {
    fn new() -> Self {
        let (completed, _) = tokio::sync::watch::channel(false);
        Self {
            state: AtomicU8::new(OWNER_ACTIVE),
            completed,
        }
    }

    fn finish(&self) {
        self.state.store(OWNER_RELEASED, Ordering::Release);
        self.completed.send_replace(true);
    }
}

struct AgentRelease {
    generation: u64,
    state: Arc<OwnerReleaseState>,
}

enum ReleaseSignal {
    Agent(AgentRelease),
    Direct(OwnerId),
}

impl Driver {
    async fn run(mut self) {
        loop {
            for _ in 0..RELEASE_BURST_LIMIT {
                let Ok(release) = self.releases.try_recv() else {
                    break;
                };
                self.handle_release(release);
            }
            let command = tokio::select! {
                biased;
                command = self.commands.recv() => command,
                Some(release) = self.releases.recv() => {
                    self.handle_release(release);
                    continue;
                }
            };
            let Some(CommandRequest { command, span }) = command else {
                break;
            };

            for _ in 0..RELEASE_BURST_LIMIT {
                let Ok(release) = self.releases.try_recv() else {
                    break;
                };
                self.handle_release(release);
            }
            self.handle_command(command).instrument(span).await;
            if self.poisoned {
                break;
            }
        }
    }

    async fn handle_command(&mut self, command: Command) {
        if self.owner.is_none()
            && !matches!(
                command,
                Command::AcquireAgent { .. } | Command::CurrentOwner { .. }
            )
            && let Err(error) = self.acquire(None, None, None).await
        {
            command.reject(error);
            return;
        }
        match command {
            Command::LoadPayloads {
                caller,
                payloads,
                result,
            } => {
                let outcome = match caller
                    .as_ref()
                    .map_or(Ok(()), |caller| self.authorize(caller))
                {
                    Ok(()) => {
                        EncodedPayload::load_many_noting(
                            &payloads,
                            &mut *self.store,
                            &self.state_id,
                            &mut self.committed_records,
                        )
                        .await
                    }
                    Err(error) => Err(error),
                };
                drop(result.send(outcome));
            }
            Command::LoadPayload {
                caller,
                payload,
                result,
            } => {
                let outcome = match caller
                    .as_ref()
                    .map_or(Ok(()), |caller| self.authorize(caller))
                {
                    Ok(()) => {
                        payload
                            .load_noting(
                                &mut *self.store,
                                &self.state_id,
                                &mut self.committed_records,
                            )
                            .await
                    }
                    Err(error) => Err(error),
                };
                drop(result.send(outcome));
            }
            Command::Continuation {
                caller,
                operation_id,
                result,
            } => {
                let outcome = async {
                    self.authorize(&caller)?;
                    self.require_claimed(&caller, &operation_id)?;
                    self.require_running(&operation_id)?;
                    match self
                        .state
                        .operation(&operation_id)
                        .and_then(|op| op.continuation.as_ref())
                    {
                        Some(value) => {
                            Ok(Some(value.load(&mut *self.store, &self.state_id).await?))
                        }
                        None => Ok(None),
                    }
                }
                .await;
                drop(result.send(outcome));
            }
            Command::Advance {
                caller,
                operation_id,
                continuation,
                deferred,
                result,
            } => {
                let outcome = async {
                    self.authorize(&caller)?;
                    self.require_claimed(&caller, &operation_id)?;
                    self.require_running(&operation_id)?;
                    let transition = Transition::ExecutionAdvanced {
                        operation_id,
                        continuation,
                    };
                    if deferred {
                        let revision = self.state.revision();
                        self.state.apply_within(revision, transition)
                    } else {
                        self.apply(transition).await
                    }
                }
                .await;
                drop(result.send(outcome));
            }
            Command::RecoverFailure {
                caller,
                operation_id,
                result,
            } => {
                let outcome = self.authorize(&caller).and_then(|()| {
                    let operation = self.state.operation(&operation_id);
                    if operation.is_some_and(|operation| !operation.status.is_terminal())
                        && let Some((pending_id, _)) = self.state.first_pending_operation()
                        && pending_id != operation_id
                    {
                        return Err(Error::OperationBlocked {
                            operation_id,
                            pending_id: pending_id.to_owned(),
                        });
                    }
                    Ok(operation.map(|operation| operation.status.clone()))
                });
                drop(result.send(outcome));
            }
            Command::Acquire { result } => drop(result.send(Ok(()))),
            Command::CurrentOwner { result } => {
                let outcome = match &self.owner {
                    None => Ok(true),
                    Some(owner) => self
                        .store
                        .owner(&self.state_id)
                        .await
                        .map(|current| current.as_ref() == Some(owner))
                        .map_err(Error::from),
                };
                drop(result.send(outcome));
            }
            Command::State { result } => drop(result.send(Ok(self.state.clone()))),
            Command::LatestCheckpoint { result } => {
                let outcome = match self.state.latest_checkpoint() {
                    Some(value) => value.load(&mut *self.store, &self.state_id).await.map(Some),
                    None => Ok(None),
                };
                drop(result.send(outcome));
            }
            Command::AcquireAgent {
                initial,
                checkpoint,
                journal,
                preloaded,
                result,
            } => {
                let outcome = self
                    .acquire_agent(initial, checkpoint, journal, preloaded)
                    .await;
                let generation = outcome.as_ref().ok().map(|owner| owner.generation);
                if result.send(outcome).is_err()
                    && let Some(generation) = generation
                    && self.active_agent_generation == Some(generation)
                {
                    self.active_agent_generation = None;
                    self.claimed.clear();
                    self.running.clear();
                }
            }
            Command::Admit {
                caller,
                operation_id,
                input,
                acknowledged,
                release_commands,
                result,
            } => {
                let admitted_id = operation_id.clone();
                let outcome = match self.authorize(&caller) {
                    Ok(()) => self.admit(&caller, operation_id, input).await,
                    Err(error) => Err(error),
                };
                let claimed = matches!(
                    &outcome,
                    Ok(StoredAdmission::Accepted | StoredAdmission::Pending)
                );
                if result.send(outcome).is_err() {
                    self.release_claim_if_owned(&caller, &admitted_id);
                } else if claimed {
                    spawn_claim_ack(release_commands, acknowledged, caller, admitted_id);
                }
            }
            Command::AdmitAutomatic {
                caller,
                candidate_operation_id,
                input,
                acknowledged,
                release_commands,
                result,
            } => {
                let outcome = match self.authorize(&caller) {
                    Ok(()) => {
                        self.admit_automatic(&caller, candidate_operation_id, input)
                            .await
                    }
                    Err(error) => Err(error),
                };
                let claimed_id = match &outcome {
                    Ok((operation_id, StoredAdmission::Accepted | StoredAdmission::Pending)) => {
                        Some(operation_id.clone())
                    }
                    _ => None,
                };
                if let Err(Ok((operation_id, _))) = result.send(outcome) {
                    self.release_claim_if_owned(&caller, &operation_id);
                } else if let Some(operation_id) = claimed_id {
                    spawn_claim_ack(release_commands, acknowledged, caller, operation_id);
                }
            }
            Command::Release {
                caller,
                operation_id,
                result,
            } => {
                let outcome = self
                    .authorize(&caller)
                    .and_then(|()| self.release_claim(&caller, &operation_id));
                drop(result.send(outcome));
            }
            Command::BeginAttempt {
                caller,
                operation_id,
                result,
            } => {
                let outcome = match self.authorize(&caller) {
                    Ok(()) => self.begin_attempt(&caller, operation_id).await,
                    Err(error) => Err(error),
                };
                drop(result.send(outcome));
            }
            Command::AcceptSteer {
                message_id,
                caller,
                operation_id,
                accepted_after_model_call_index,
                input,
                identity,
                result,
            } => {
                let outcome = match self.authorize(&caller) {
                    Ok(()) => {
                        self.accept_steer(
                            &caller,
                            operation_id,
                            accepted_after_model_call_index,
                            input,
                            message_id,
                            identity,
                        )
                        .await
                    }
                    Err(error) => Err(error),
                };
                drop(result.send(outcome));
            }
            Command::IdentifiedSteerAccepted {
                caller,
                message_id,
                input,
                result,
            } => {
                let outcome = self.authorize(&caller).and_then(|()| {
                    let mut accepted = false;
                    for operation in self.state.operations().values() {
                        if let Some(receipt) = operation.steer_receipts.get(&message_id) {
                            if !input.matches_key(&receipt.input_key) {
                                return Err(Error::SteerConflict { message_id });
                            }
                            if receipt.withdrawn {
                                return Err(Error::SteerWithdrawn { message_id });
                            }
                            accepted = true;
                        }
                    }
                    Ok(accepted)
                });
                drop(result.send(outcome));
            }
            Command::SteerReceipt {
                caller,
                message_id,
                input,
                result,
            } => {
                let outcome = self.authorize(&caller).map(|()| {
                    let receipts: Vec<_> = self
                        .state
                        .operations()
                        .values()
                        .filter_map(|operation| operation.steer_receipts.get(&message_id))
                        .collect();
                    if receipts
                        .iter()
                        .any(|receipt| !input.matches_key(&receipt.input_key))
                    {
                        SteerReceiptStatus::Different
                    } else if receipts.iter().any(|receipt| !receipt.withdrawn) {
                        SteerReceiptStatus::Accepted
                    } else if !receipts.is_empty() {
                        SteerReceiptStatus::Withdrawn
                    } else if self.terminal_receipt_limit.is_none()
                        && self.state.receipts_complete()
                    {
                        SteerReceiptStatus::Absent
                    } else {
                        SteerReceiptStatus::Unknown
                    }
                });
                drop(result.send(outcome));
            }
            Command::RetainedSteers {
                caller,
                operation_id,
                result,
            } => {
                let outcome = self
                    .authorize(&caller)
                    .and_then(|()| self.retained_steers(&caller, &operation_id));
                let outcome = match outcome {
                    Ok(mut steers) => {
                        let mut error = None;
                        for steer in &mut steers {
                            match steer
                                .state
                                .input
                                .load(&mut *self.store, &self.state_id)
                                .await
                            {
                                Ok(value) => steer.state.input = value,
                                Err(failure) => {
                                    error = Some(failure);
                                    break;
                                }
                            }
                        }
                        error.map_or(Ok(steers), Err)
                    }
                    Err(error) => Err(error),
                };
                drop(result.send(outcome));
            }
            Command::WithdrawSteer {
                caller,
                operation_id,
                steer_index,
                result,
            } => {
                let outcome = match self.authorize(&caller) {
                    Ok(()) => {
                        self.withdraw_steer(&caller, operation_id, steer_index)
                            .await
                    }
                    Err(error) => Err(error),
                };
                drop(result.send(outcome));
            }
            Command::BindSteer {
                caller,
                operation_id,
                steer_index,
                model_call_index,
                result,
            } => {
                let outcome = match self.authorize(&caller) {
                    Ok(()) => {
                        self.bind_steer(&caller, operation_id, steer_index, model_call_index)
                            .await
                    }
                    Err(error) => Err(error),
                };
                drop(result.send(outcome));
            }
            Command::BeginStep {
                caller,
                operation_id,
                step_id,
                kind,
                input,
                result,
            } => {
                let outcome = match self.authorize(&caller) {
                    Ok(()) => {
                        self.begin_step(&caller, operation_id, step_id, kind, input)
                            .await
                    }
                    Err(error) => Err(error),
                };
                drop(result.send(outcome));
            }
            Command::CompleteStep {
                caller,
                operation_id,
                step_id,
                output,
                deferred,
                result,
            } => {
                let outcome = match self.authorize(&caller) {
                    Ok(()) if deferred => self.defer_step(&caller, operation_id, step_id, output),
                    Ok(()) => {
                        self.complete_step(&caller, operation_id, step_id, output)
                            .await
                    }
                    Err(error) => Err(error),
                };
                drop(result.send(outcome));
            }
            Command::CommitWrites {
                caller,
                operation_id,
                writes,
                result,
            } => {
                let outcome = match self.authorize(&caller) {
                    Ok(()) => self.commit_writes(&caller, operation_id, writes).await,
                    Err(error) => Err(error),
                };
                drop(result.send(outcome));
            }
            Command::Complete {
                caller,
                operation_id,
                checkpoint,
                output,
                result,
            } => {
                let outcome = match self.authorize(&caller) {
                    Ok(()) => {
                        self.complete(&caller, operation_id, checkpoint, output)
                            .await
                    }
                    Err(error) => Err(error),
                };
                drop(result.send(outcome));
            }
            Command::Fail {
                caller,
                operation_id,
                checkpoint,
                error,
                result,
            } => {
                let outcome = match self.authorize(&caller) {
                    Ok(()) => self.fail(&caller, operation_id, checkpoint, error).await,
                    Err(error) => Err(error),
                };
                drop(result.send(outcome));
            }
            Command::FailAttempt {
                caller,
                operation_id,
                result,
            } => {
                let outcome = match self.authorize(&caller) {
                    Ok(()) => {
                        let outcome = match self.require_claimed(&caller, &operation_id) {
                            Ok(()) => self.require_running(&operation_id),
                            Err(error) => Err(error),
                        };
                        if finishing_attempt_releases_claim(&outcome) {
                            self.release_claim_if_owned(&caller, &operation_id);
                        }
                        outcome
                    }
                    Err(error) => Err(error),
                };
                drop(result.send(outcome));
            }
            Command::Cancel {
                caller,
                operation_id,
                checkpoint,
                result,
            } => {
                let outcome = match self.authorize(&caller) {
                    Ok(()) => {
                        let outcome = match self.require_claimed(&caller, &operation_id) {
                            Ok(()) => {
                                let admissible = match checkpoint.as_ref() {
                                    Some(_) => self.require_running(&operation_id),
                                    None if self.running.contains(&operation_id)
                                        || self.state.operation(&operation_id).is_some_and(
                                            |operation| {
                                                operation.cancellation_requires_checkpoint()
                                            },
                                        ) =>
                                    {
                                        Err(Error::CancellationCheckpointRequired {
                                            operation_id: operation_id.clone(),
                                        })
                                    }
                                    None => Ok(()),
                                }
                                .and_then(|()| {
                                    self.state
                                        .operation(&operation_id)
                                        .filter(|operation| !operation.status.is_terminal())
                                        .ok_or_else(|| Error::OperationTerminal {
                                            operation_id: operation_id.clone(),
                                        })
                                        .map(|_| ())
                                });
                                match admissible {
                                    Ok(()) => {
                                        self.apply_terminal(Transition::OperationCancelled {
                                            operation_id: operation_id.clone(),
                                            checkpoint,
                                        })
                                        .await
                                    }
                                    Err(error) => Err(error),
                                }
                            }
                            Err(error) => Err(error),
                        };
                        if finishing_attempt_releases_claim(&outcome) {
                            self.release_claim_if_owned(&caller, &operation_id);
                        }
                        outcome
                    }
                    Err(error) => Err(error),
                };
                drop(result.send(outcome));
            }
            Command::PruneReceipts { caller, result } => {
                let outcome = match self.authorize(&caller) {
                    Ok(()) => self.prune_retained(true).await,
                    Err(error) => Err(error),
                };
                drop(result.send(outcome));
            }
            Command::CommitCheckpoint {
                caller,
                checkpoint,
                write,
                result,
            } => {
                let outcome = self.commit_standalone(&caller, checkpoint, write).await;
                drop(result.send(outcome));
            }
            Command::Stage { write, result } => drop(result.send(self.stage(write))),
            Command::Append { write, result } => drop(result.send(self.append(write).await)),
            Command::DiscardUnused { caller, result } => {
                let outcome = match self.authorize(&caller) {
                    Ok(()) => self.discard_unused().await,
                    Err(error) => Err(error),
                };
                drop(result.send(outcome));
            }
            Command::Abandonable {
                caller,
                kinds,
                result,
            } => {
                let outcome = match self.authorize(&caller) {
                    Ok(()) => {
                        self.abandonable = kinds.into_iter().collect();
                        self.abandon_stale("").await
                    }
                    Err(error) => Err(error),
                };
                drop(result.send(outcome));
            }
            Command::Describe {
                caller,
                record,
                result,
            } => {
                let outcome = match self.authorize(&caller) {
                    Ok(()) => self.describe(record).await,
                    Err(error) => Err(error),
                };
                drop(result.send(outcome));
            }
        }
    }

    fn handle_release(&mut self, release: ReleaseSignal) {
        match release {
            ReleaseSignal::Agent(release) => {
                if self.active_agent_generation == Some(release.generation) {
                    self.active_agent_generation = None;
                    self.claimed.clear();
                    self.running.clear();
                }
                release.state.finish();
            }
            ReleaseSignal::Direct(caller_id) => {
                let released = self
                    .claimed
                    .iter()
                    .filter(|(_, caller)| *caller == &Caller::Direct(caller_id.clone()))
                    .map(|(operation_id, _)| operation_id.clone())
                    .collect::<Vec<_>>();
                self.claimed
                    .retain(|_, caller| caller != &Caller::Direct(caller_id.clone()));
                for operation_id in released {
                    self.running.remove(&operation_id);
                }
            }
        }
    }

    const fn authorize(&self, caller: &Caller) -> Result<()> {
        match (caller, self.active_agent_generation) {
            (Caller::Direct(_), None) => Ok(()),
            (Caller::Agent(generation), Some(active)) if *generation == active => Ok(()),
            (Caller::Direct(_), Some(_)) => Err(Error::ModelOwnerActive),
            (Caller::Agent(_), _) => Err(Error::ModelOwnerFenced),
        }
    }

    async fn acquire(
        &mut self,
        initial: Option<crate::InitialSettings>,
        checkpoint: Option<EncodedPayload>,
        journal: Option<crate::child_journal::Seed>,
    ) -> Result<bool> {
        let record = self.pending_record.clone();
        let create = self.create && record.is_some();
        if create
            && reduce(self.store.peek(&self.state_id).await?)?
                .session()
                .is_some()
        {
            return Err(self.exists());
        }
        let committed = Arc::new(AtomicBool::new(false));
        let staged = Arc::clone(&committed);
        let existed = Arc::new(AtomicBool::new(false));
        let found = Arc::clone(&existed);
        let update: crate::StoreUpdate = Box::new(move |stored| {
            if create && reduce(stored.clone()).is_ok_and(|state| state.session().is_some()) {
                found.store(true, Ordering::Release);
                return Err(StoreError::NotCommitted(
                    "the session already exists".to_owned(),
                ));
            }
            update_session(stored, record, initial, checkpoint, &staged)
        });
        let acquired = match journal.filter(crate::child_journal::Seed::awaited) {
            Some(journal) => self
                .store
                .acquire_with_companion(
                    &self.state_id,
                    OwnerId::new(),
                    update,
                    &journal.state_id,
                    OwnerId::new(),
                )
                .await
                .map(|(acquired, companion)| {
                    journal.resolve(companion);
                    acquired
                }),
            None => {
                self.store
                    .acquire_with(&self.state_id, OwnerId::new(), update)
                    .await
            }
        };
        let acquired = match acquired {
            Ok(acquired) => acquired,
            Err(_) if existed.load(Ordering::Acquire) => return Err(self.exists()),
            Err(error @ StoreError::NotCommitted(_)) => return Err(error.into()),
            Err(error) => {
                self.poisoned = true;
                return Err(error.into());
            }
        };
        let mut state = match reduce(acquired.state) {
            Ok(state) => state,
            Err(error) => {
                self.poisoned = true;
                return Err(error);
            }
        };
        self.committed_records.clear();
        if let Err(error) = state
            .hydrate(
                &mut *self.store,
                &self.state_id,
                &mut self.committed_records,
            )
            .await
        {
            self.poisoned = true;
            return Err(error);
        }
        self.owner = Some(acquired.owner);
        self.pending_record = None;
        self.create = false;
        self.state = state;
        self.next_event = None;
        Ok(committed.load(Ordering::Acquire))
    }

    fn exists(&self) -> Error {
        Error::SessionExists {
            session_id: self.state_id.to_string(),
        }
    }

    async fn acquire_agent(
        &mut self,
        initial: Option<crate::InitialSettings>,
        checkpoint: Option<EncodedPayload>,
        journal: Option<crate::child_journal::Seed>,
        preloaded: Option<LoadedCheckpoint>,
    ) -> Result<AgentAcquisition> {
        let initial_committed = self.acquire(initial, checkpoint, journal).await?;
        let generation = match self.next_agent_generation.checked_add(1) {
            Some(generation) => generation,
            None => {
                self.poisoned = true;
                return Err(Error::InvalidState(
                    "model-owner generation exceeded the u64 range".to_owned(),
                ));
            }
        };
        self.claimed.clear();
        self.running.clear();
        self.next_agent_generation = generation;
        self.active_agent_generation = Some(generation);
        Ok(AgentAcquisition {
            generation,
            checkpoint: match self.state.latest_checkpoint() {
                Some(value) => Some(
                    match preloaded.filter(|(payload, _)| payload.key() == value.key()) {
                        Some((payload, records)) => {
                            self.committed_records.extend(records.iter().cloned());
                            payload
                        }
                        None => {
                            value
                                .load_noting(
                                    &mut *self.store,
                                    &self.state_id,
                                    &mut self.committed_records,
                                )
                                .await?
                        }
                    },
                ),
                None => None,
            },
            initial_committed,
        })
    }

    async fn commit_standalone(
        &mut self,
        caller: &Caller,
        checkpoint: EncodedPayload,
        write: SessionWrite,
    ) -> Result<SessionReceipt> {
        self.authorize(caller)?;
        self.abandon_stale("").await?;
        if let Some((pending_id, _)) = self.state.first_pending_operation() {
            return Err(Error::OperationBlocked {
                operation_id: "standalone-checkpoint".to_owned(),
                pending_id: pending_id.to_owned(),
            });
        }
        self.stage(write)?;
        self.applied(Transition::CheckpointCommitted { checkpoint })
            .await
    }

    async fn abandon_stale(&mut self, except: &str) -> Result<()> {
        if self.abandonable.is_empty() {
            return Ok(());
        }
        loop {
            let Some((operation_id, operation)) = self.state.first_pending_operation() else {
                return Ok(());
            };
            if operation_id == except || self.claimed.contains_key(operation_id) {
                return Ok(());
            }
            let (operation_id, input) = (operation_id.to_owned(), operation.input.clone());
            let checkpoint = operation
                .cancellation_requires_checkpoint()
                .then(|| self.state.latest_checkpoint().cloned());
            let input = input.load(&mut *self.store, &self.state_id).await?;
            let kind = serde_json::from_str::<serde_json::Value>(input.json()?)
                .ok()
                .and_then(|input| input.get("kind")?.as_str().map(str::to_owned));
            if !kind.is_some_and(|kind| self.abandonable.contains(&kind)) {
                return Ok(());
            }
            let checkpoint = match checkpoint {
                Some(Some(checkpoint)) => Some(checkpoint),
                Some(None) => {
                    return Err(Error::CancellationCheckpointRequired { operation_id });
                }
                None => None,
            };
            tracing::warn!(
                operation = operation_id,
                "abandoning a maintenance operation left pending without a live attempt"
            );
            self.apply_terminal(Transition::OperationCancelled {
                operation_id,
                checkpoint,
            })
            .await?;
        }
    }

    async fn admit(
        &mut self,
        caller: &Caller,
        operation_id: String,
        input: EncodedPayload,
    ) -> Result<StoredAdmission> {
        self.abandon_stale(&operation_id).await?;
        if let Some(operation) = self.state.operation(&operation_id) {
            if operation.input != input {
                return Err(Error::OperationConflict { operation_id });
            }
            if self.claimed.contains_key(&operation_id) {
                return Err(Error::OperationActive { operation_id });
            }
            let operation = self.state.operation(&operation_id).ok_or_else(|| {
                Error::InvalidState(format!("operation `{operation_id}` disappeared"))
            })?;
            return match &operation.status {
                OperationStatus::Pending => {
                    self.claimed.insert(operation_id.clone(), caller.clone());
                    Ok(StoredAdmission::Pending)
                }
                OperationStatus::Completed { checkpoint, output } => {
                    Ok(StoredAdmission::Completed {
                        checkpoint: checkpoint.load(&mut *self.store, &self.state_id).await?,
                        output: output.load(&mut *self.store, &self.state_id).await?,
                    })
                }
                OperationStatus::Failed { checkpoint, error } => Ok(StoredAdmission::Failed {
                    checkpoint: checkpoint.load(&mut *self.store, &self.state_id).await?,
                    error: error.clone(),
                }),
                OperationStatus::Cancelled { .. } => Ok(StoredAdmission::Cancelled),
            };
        }
        if let Some((pending_id, _)) = self
            .state
            .first_pending_operation_where(|pending_id| !self.claimed.contains_key(pending_id))
        {
            return Err(Error::OperationBlocked {
                operation_id,
                pending_id: pending_id.to_owned(),
            });
        }
        self.apply(Transition::OperationAccepted {
            operation_id: operation_id.clone(),
            input,
        })
        .await?;
        self.claimed.insert(operation_id, caller.clone());
        Ok(StoredAdmission::Accepted)
    }

    async fn admit_automatic(
        &mut self,
        caller: &Caller,
        candidate_operation_id: String,
        input: EncodedPayload,
    ) -> Result<(String, StoredAdmission)> {
        self.abandon_stale(&candidate_operation_id).await?;
        if let Some((pending_id, operation)) = self
            .state
            .first_pending_operation_where(|pending_id| !self.claimed.contains_key(pending_id))
        {
            if operation.input != input {
                return Err(Error::OperationBlocked {
                    operation_id: candidate_operation_id,
                    pending_id: pending_id.to_owned(),
                });
            }
            let recovered_operation_id = pending_id.to_owned();
            let admission = self
                .admit(caller, recovered_operation_id.clone(), input)
                .await?;
            return Ok((recovered_operation_id, admission));
        }

        let admission = self
            .admit(caller, candidate_operation_id.clone(), input)
            .await?;
        Ok((candidate_operation_id, admission))
    }

    async fn begin_attempt(&mut self, caller: &Caller, operation_id: String) -> Result<()> {
        self.require_claimed(caller, &operation_id)?;
        self.abandon_stale(&operation_id).await?;
        if let Some((pending_id, _)) = self.state.first_pending_operation()
            && pending_id != operation_id
        {
            return Err(Error::OperationBlocked {
                operation_id,
                pending_id: pending_id.to_owned(),
            });
        }
        let operation = self.state.operation(&operation_id).ok_or_else(|| {
            Error::InvalidState(format!("operation `{operation_id}` was not accepted"))
        })?;
        if operation.status.is_terminal() {
            return Err(Error::OperationTerminal { operation_id });
        }
        if self.running.contains(&operation_id) {
            return Err(Error::AttemptActive { operation_id });
        }
        self.running.insert(operation_id.clone());
        Ok(())
    }

    async fn accept_steer(
        &mut self,
        caller: &Caller,
        operation_id: String,
        accepted_after_model_call_index: u32,
        input: EncodedPayload,
        message_id: Option<String>,
        identity: Option<EncodedPayload>,
    ) -> Result<Option<u32>> {
        self.require_claimed(caller, &operation_id)?;
        self.require_running(&operation_id)?;
        let operation = self.state.operation(&operation_id).ok_or_else(|| {
            Error::InvalidState(format!("operation `{operation_id}` was not accepted"))
        })?;
        let identity = identity.as_ref().unwrap_or(&input);
        if let Some(id) = &message_id
            && let Some(receipt) = operation.steer_receipts.get(id)
        {
            if !identity.matches_key(&receipt.input_key) {
                return Err(Error::SteerConflict {
                    message_id: id.clone(),
                });
            }
            if receipt.withdrawn {
                return Err(Error::SteerWithdrawn {
                    message_id: id.clone(),
                });
            }
            return Ok(None);
        }
        let steer_index = u32::try_from(operation.steers.len())
            .ok()
            .and_then(|length| length.checked_add(operation.retired_steers)?.checked_add(1))
            .ok_or_else(|| {
                Error::InvalidState(format!(
                    "operation `{operation_id}` exceeded the steer counter range"
                ))
            })?;
        let input_key = (identity.key() != input.key()).then(|| identity.key().to_owned());
        self.apply(Transition::SteerAccepted {
            message_id,
            operation_id,
            steer_index,
            accepted_after_model_call_index,
            input,
            input_key,
        })
        .await?;
        Ok(Some(steer_index))
    }

    fn retained_steers(&self, caller: &Caller, operation_id: &str) -> Result<Vec<StoredSteer>> {
        self.require_claimed(caller, operation_id)?;
        self.require_running(operation_id)?;
        let operation = self.state.operation(operation_id).ok_or_else(|| {
            Error::InvalidState(format!("operation `{operation_id}` was not accepted"))
        })?;
        operation
            .steers
            .iter()
            .cloned()
            .enumerate()
            .map(|(offset, state)| {
                let index = u32::try_from(offset)
                    .ok()
                    .and_then(|offset| offset.checked_add(operation.retired_steers)?.checked_add(1))
                    .ok_or_else(|| {
                        Error::InvalidState(format!(
                            "operation `{operation_id}` exceeded the steer counter range"
                        ))
                    })?;
                Ok(StoredSteer { index, state })
            })
            .collect()
    }

    async fn withdraw_steer(
        &mut self,
        caller: &Caller,
        operation_id: String,
        steer_index: u32,
    ) -> Result<()> {
        self.require_claimed(caller, &operation_id)?;
        self.require_running(&operation_id)?;
        self.apply(Transition::SteerWithdrawn {
            operation_id,
            steer_index,
        })
        .await
    }

    async fn bind_steer(
        &mut self,
        caller: &Caller,
        operation_id: String,
        steer_index: u32,
        model_call_index: u32,
    ) -> Result<()> {
        self.require_claimed(caller, &operation_id)?;
        self.require_running(&operation_id)?;
        let retained = self
            .state
            .operation(&operation_id)
            .and_then(|operation| {
                steer_index
                    .checked_sub(operation.retired_steers)
                    .and_then(|index| index.checked_sub(1))
                    .and_then(|index| {
                        usize::try_from(index)
                            .ok()
                            .and_then(|index| operation.steers.get(index))
                    })
            })
            .ok_or_else(|| {
                Error::InvalidState(format!(
                    "steer {steer_index} in operation `{operation_id}` was bound before acceptance"
                ))
            })?;
        match retained.model_call_index {
            Some(retained) if retained == model_call_index => return Ok(()),
            Some(retained) => {
                return Err(Error::InvalidState(format!(
                    "steer {steer_index} in operation `{operation_id}` changed model boundary from {retained} to {model_call_index}"
                )));
            }
            None => {}
        }
        self.apply(Transition::SteerBound {
            operation_id,
            steer_index,
            model_call_index,
        })
        .await
    }

    async fn begin_step(
        &mut self,
        caller: &Caller,
        operation_id: String,
        step_id: String,
        kind: String,
        input: EncodedPayload,
    ) -> Result<StoredBeginStep> {
        self.require_claimed(caller, &operation_id)?;
        if let Some((pending_id, _)) = self.state.first_pending_operation()
            && pending_id != operation_id
        {
            return Err(Error::OperationBlocked {
                operation_id,
                pending_id: pending_id.to_owned(),
            });
        }
        self.require_running(&operation_id)?;
        if let Some(step) = self
            .state
            .operation(&operation_id)
            .and_then(|operation| operation.steps.get(&step_id))
        {
            if step.kind != kind || step.input != input {
                return Err(Error::InvalidState(format!(
                    "step `{step_id}` in operation `{operation_id}` changed definition"
                )));
            }
            match &step.status {
                StepStatus::Completed(output) => {
                    return Ok(StoredBeginStep::Replay(
                        output.load(&mut *self.store, &self.state_id).await?,
                    ));
                }
                StepStatus::EffectPending => return Ok(StoredBeginStep::Interrupted),
            }
        }
        let entry = Transition::StepStarted {
            operation_id: operation_id.clone(),
            step_id,
            kind,
            input,
        };
        self.apply(entry).await?;
        Ok(StoredBeginStep::Execute)
    }

    async fn complete_step(
        &mut self,
        caller: &Caller,
        operation_id: String,
        step_id: String,
        output: EncodedPayload,
    ) -> Result<()> {
        self.require_claimed(caller, &operation_id)?;
        let Some(step) = self
            .state
            .operation(&operation_id)
            .and_then(|operation| operation.steps.get(&step_id))
        else {
            return Err(Error::StepNotStarted {
                operation_id,
                step_id,
            });
        };
        let status = step.status.clone();
        self.require_running(&operation_id)?;
        match status {
            StepStatus::Completed(_) => Err(Error::InvalidState(format!(
                "step `{step_id}` in operation `{operation_id}` already completed"
            ))),
            StepStatus::EffectPending => {
                self.apply(Transition::StepCompleted {
                    operation_id,
                    step_id,
                    output,
                })
                .await
            }
        }
    }

    fn defer_step(
        &mut self,
        caller: &Caller,
        operation_id: String,
        step_id: String,
        output: EncodedPayload,
    ) -> Result<()> {
        self.require_claimed(caller, &operation_id)?;
        self.require_running(&operation_id)?;
        match self
            .state
            .operation(&operation_id)
            .and_then(|operation| operation.steps.get(&step_id))
            .map(|step| &step.status)
        {
            None => Err(Error::StepNotStarted {
                operation_id,
                step_id,
            }),
            Some(StepStatus::Completed(_)) => Err(Error::InvalidState(format!(
                "step `{step_id}` in operation `{operation_id}` already completed"
            ))),
            Some(StepStatus::EffectPending) => {
                let revision = self.state.revision();
                self.state.apply_within(
                    revision,
                    Transition::StepCompleted {
                        operation_id,
                        step_id,
                        output,
                    },
                )
            }
        }
    }

    async fn complete(
        &mut self,
        caller: &Caller,
        operation_id: String,
        checkpoint: EncodedPayload,
        output: EncodedPayload,
    ) -> Result<()> {
        self.require_claimed(caller, &operation_id)?;
        self.require_running(&operation_id)?;
        let entry = Transition::OperationCompleted {
            operation_id: operation_id.clone(),
            checkpoint,
            output,
        };
        let outcome = self.apply_terminal(entry).await;
        if finishing_attempt_releases_claim(&outcome) {
            self.release_claim_if_owned(caller, &operation_id);
        }
        outcome
    }

    async fn fail(
        &mut self,
        caller: &Caller,
        operation_id: String,
        checkpoint: EncodedPayload,
        error: String,
    ) -> Result<()> {
        self.require_claimed(caller, &operation_id)?;
        self.require_running(&operation_id)?;
        let entry = Transition::OperationFailed {
            operation_id: operation_id.clone(),
            checkpoint,
            error,
        };
        let outcome = self.apply_terminal(entry).await;
        if finishing_attempt_releases_claim(&outcome) {
            self.release_claim_if_owned(caller, &operation_id);
        }
        outcome
    }

    fn require_claimed(&self, caller: &Caller, operation_id: &str) -> Result<()> {
        if self.claimed.get(operation_id) == Some(caller) {
            Ok(())
        } else {
            Err(Error::OperationNotClaimed {
                operation_id: operation_id.to_owned(),
            })
        }
    }

    fn require_running(&self, operation_id: &str) -> Result<()> {
        let operation = self.state.operation(operation_id).ok_or_else(|| {
            Error::InvalidState(format!("operation `{operation_id}` was not accepted"))
        })?;
        if operation.status.is_terminal() {
            return Err(Error::OperationTerminal {
                operation_id: operation_id.to_owned(),
            });
        }
        if !self.running.contains(operation_id) {
            return Err(Error::AttemptNotStarted {
                operation_id: operation_id.to_owned(),
            });
        }
        Ok(())
    }

    fn release_claim(&mut self, caller: &Caller, operation_id: &str) -> Result<()> {
        self.require_claimed(caller, operation_id)?;
        self.claimed.remove(operation_id);
        self.running.remove(operation_id);
        Ok(())
    }

    fn release_claim_if_owned(&mut self, caller: &Caller, operation_id: &str) {
        if self.claimed.get(operation_id) == Some(caller) {
            self.claimed.remove(operation_id);
            self.running.remove(operation_id);
        }
    }

    async fn apply(&mut self, entry: Transition) -> Result<()> {
        if let Some(revision) = self.batch {
            return if self.state.revision() == revision {
                self.state.apply_within(revision, entry)
            } else {
                self.state.apply_transition(revision, entry)
            };
        }
        self.applied(entry).await?;
        Ok(())
    }

    async fn commit_writes(
        &mut self,
        caller: &Caller,
        operation_id: String,
        writes: Vec<OwnerWrite>,
    ) -> Result<Vec<WriteOutcome>> {
        let revision = self.state.revision().checked_add(1).ok_or_else(|| {
            Error::InvalidState("state revision exceeded the u64 range".to_owned())
        })?;
        let base = self.state.clone();
        self.batch = Some(revision);
        let mut outcomes = Vec::with_capacity(writes.len());
        let mut failure = None;
        for write in writes {
            let outcome = match write {
                OwnerWrite::Advance { continuation } => {
                    match self
                        .require_claimed(caller, &operation_id)
                        .and_then(|()| self.require_running(&operation_id))
                    {
                        Ok(()) => self
                            .apply(Transition::ExecutionAdvanced {
                                operation_id: operation_id.clone(),
                                continuation,
                            })
                            .await
                            .map(|()| WriteOutcome::Advanced),
                        Err(error) => Err(error),
                    }
                }
                OwnerWrite::BeginStep {
                    step_id,
                    kind,
                    input,
                } => self
                    .begin_step(caller, operation_id.clone(), step_id, kind, input)
                    .await
                    .map(|step| WriteOutcome::Began(step.into())),
                OwnerWrite::CompleteStep { step_id, output } => self
                    .complete_step(caller, operation_id.clone(), step_id, output)
                    .await
                    .map(|()| WriteOutcome::Completed),
            };
            match outcome {
                Ok(outcome) => outcomes.push(outcome),
                Err(error) => {
                    failure = Some(error);
                    break;
                }
            }
        }
        self.batch = None;
        let mut next = std::mem::replace(&mut self.state, base);
        if let Some(error) = failure {
            return Err(error);
        }
        if next.revision() == revision {
            if let Some(limit) = self.terminal_receipt_limit {
                let _ = next.retain_terminal_receipts(limit);
            }
            self.persist(next).await?;
        }
        Ok(outcomes)
    }

    async fn applied(&mut self, entry: Transition) -> Result<SessionReceipt> {
        let expected_revision = self.state.revision().checked_add(1).ok_or_else(|| {
            Error::InvalidState("state revision exceeded the u64 range".to_owned())
        })?;
        let mut next = self.state.clone();
        next.apply_transition(expected_revision, entry)?;
        if let Some(limit) = self.terminal_receipt_limit {
            let _ = next.retain_terminal_receipts(limit);
        }
        self.persist(next).await
    }

    fn stage(&mut self, write: SessionWrite) -> Result<()> {
        if !write.events.is_empty() && !self.store.supports_events() {
            return Err(StoreError::NotCommitted(
                "this store does not record state events".to_owned(),
            )
            .into());
        }
        if write.metadata.is_some() && self.state.session().is_none() {
            return Err(Error::InvalidState(
                "session metadata requires a session record".to_owned(),
            ));
        }
        if write != SessionWrite::default() {
            self.staged.push(write);
        }
        Ok(())
    }

    async fn append(&mut self, write: SessionWrite) -> Result<SessionReceipt> {
        self.stage(write)?;
        let revision = self.state.revision().checked_add(1).ok_or_else(|| {
            Error::InvalidState("state revision exceeded the u64 range".to_owned())
        })?;
        let mut next = self.state.clone();
        next.advance_revision(revision)?;
        self.persist(next).await
    }

    async fn preview(&mut self, state: &DurableState) -> Result<Option<String>> {
        let Some((_, operation)) = state.operations().first_accepted() else {
            return Ok(None);
        };
        let key = operation.input.key();
        if let Some((cached, preview)) = &self.preview
            && cached == key
        {
            return Ok(preview.clone());
        }
        let input: serde_json::Value = operation
            .input
            .load(&mut *self.store, &self.state_id)
            .await?
            .decode()?;
        let preview = prompt_text(&input);
        self.preview = Some((key.to_owned(), preview.clone()));
        Ok(preview)
    }

    async fn persist(&mut self, mut next: DurableState) -> Result<SessionReceipt> {
        let expected_revision = self.state.revision().checked_add(1).ok_or_else(|| {
            Error::InvalidState("state revision exceeded the u64 range".to_owned())
        })?;
        if next.revision() != expected_revision {
            return Err(Error::InvalidState(format!(
                "next current state has revision {}, expected {expected_revision}",
                next.revision()
            )));
        }
        let summaries = self.store.supports_summaries();
        let preview = if summaries && next.session().is_some() {
            self.preview(&next).await.map_err(|error| {
                StoreError::NotCommitted(format!("could not prepare session summary: {error}"))
            })?
        } else {
            None
        };
        if let Some(record) = next.session_mut() {
            for metadata in self
                .staged
                .iter()
                .filter_map(|write| write.metadata.as_ref())
            {
                metadata.apply(record);
            }
        }
        let events: Vec<StoreEvent> = self
            .staged
            .iter()
            .flat_map(|write| write.events.iter().cloned())
            .collect();
        if let Some(record) = next.session_mut() {
            record.record_events(&events);
        }
        if self.terminal_receipt_limit.is_none() {
            next.seal_receipts()?;
        }
        let committed = &self.committed_records;
        let records = next.stage_records_unless(&|key| committed.contains(key));
        if let Some(session) = next.session_mut() {
            session.touch();
        }
        let payload = next.checkpoint_payload()?;
        let summary = if summaries {
            match summarize(&next, preview).map_err(|error| {
                StoreError::NotCommitted(format!("could not prepare session summary: {error}"))
            })? {
                Some(summary) => SummaryUpdate::Set(summary),
                None if is_listed(&self.state) => SummaryUpdate::Remove,
                None => SummaryUpdate::Unchanged,
            }
        } else {
            SummaryUpdate::Unchanged
        };
        let Some(owner) = self.owner.clone() else {
            return Err(Error::InvalidState(format!(
                "state {} was persisted before its owner was acquired",
                self.state_id
            )));
        };
        let mut retried = false;
        let committed = loop {
            let error = match self
                .store
                .commit(
                    &self.state_id,
                    &owner,
                    self.state.revision(),
                    StateCommit {
                        payload: &payload,
                        records: &records,
                        events: &events,
                        summary: &summary,
                    },
                )
                .await
            {
                Ok(committed) => break committed,
                Err(error @ StoreError::NotCommitted(_)) => return Err(error.into()),
                Err(error) => error,
            };
            match self
                .resolve_commit(expected_revision, &payload, events.len())
                .await
            {
                Resolution::Committed(committed) => {
                    tracing::warn!(
                        state_id = &*self.state_id,
                        revision = expected_revision,
                        %error,
                        "durability commit reported an error but the store holds its payload; treating it as committed"
                    );
                    break committed;
                }
                Resolution::Absent if !retried && !matches!(error, StoreError::Fenced) => {
                    tracing::warn!(
                        state_id = &*self.state_id,
                        revision = expected_revision,
                        %error,
                        "durability commit reported an error and the store still holds the previous revision; retrying the same conditional commit"
                    );
                    retried = true;
                }
                Resolution::Absent | Resolution::Unknown => {
                    self.poisoned = true;
                    return Err(error.into());
                }
            }
        };
        if committed.revision != expected_revision {
            self.poisoned = true;
            return Err(Error::InvalidState(format!(
                "store returned revision {} after replacing expected revision {expected_revision}",
                committed.revision
            )));
        }
        let assigned = match committed.first_event {
            None if events.is_empty() => Some(0..0),
            Some(first) if first > 0 && !events.is_empty() => u64::try_from(events.len())
                .ok()
                .and_then(|count| first.checked_add(count))
                .map(|end| first..end),
            Some(_) | None => None,
        };
        let Some(assigned) = assigned else {
            self.poisoned = true;
            return Err(Error::InvalidState(
                "store acknowledged an event sequence range that does not match the committed events".to_owned(),
            ));
        };
        self.committed_records
            .extend(records.into_iter().map(|record| record.key));
        if !assigned.is_empty() {
            self.next_event = Some(assigned.end);
        }
        self.staged.clear();
        self.state = next;
        Ok(SessionReceipt {
            revision: committed.revision,
            events: assigned,
            record: self.state.session().cloned(),
        })
    }

    async fn resolve_commit(&mut self, revision: u64, payload: &str, events: usize) -> Resolution {
        let Ok(stored) = self.store.peek(&self.state_id).await else {
            return Resolution::Unknown;
        };
        if stored.revision == self.state.revision() {
            return Resolution::Absent;
        }
        if stored.revision != revision || stored.payload.as_deref() != Some(payload) {
            return Resolution::Unknown;
        }
        match (events, self.next_event) {
            (0, _) => Resolution::Committed(Committed {
                revision,
                first_event: None,
            }),
            (_, Some(next)) => Resolution::Committed(Committed {
                revision,
                first_event: Some(next),
            }),
            (_, None) => Resolution::Unknown,
        }
    }

    async fn describe(
        &mut self,
        record: crate::catalog::SessionRecord,
    ) -> Result<crate::catalog::SessionRecord> {
        if record.session_id != *self.state_id {
            return Err(Error::InvalidState(format!(
                "session record {} does not describe state {}",
                record.session_id, self.state_id
            )));
        }
        let merged = match self.state.session() {
            Some(stored) => stored.merged(record),
            None => record,
        };
        if self.state.session() == Some(&merged) {
            return Ok(merged);
        }
        let mut next = self.state.clone();
        next.set_session(Some(merged.clone()));
        let revision = self.state.revision().checked_add(1).ok_or_else(|| {
            Error::InvalidState("state revision exceeded the u64 range".to_owned())
        })?;
        next.advance_revision(revision)?;
        self.persist(next).await?;
        Ok(self.state.session().cloned().unwrap_or(merged))
    }

    async fn discard_unused(&mut self) -> Result<()> {
        if let Some(operation_id) = self.state.operations().keys().next() {
            return Err(Error::InvalidState(format!(
                "cannot discard state {} with retained operation {operation_id}",
                self.state_id
            )));
        }
        if self.state.session().is_none() && self.state.latest_checkpoint().is_none() {
            return Ok(());
        }
        let revision = self.state.revision().checked_add(1).ok_or_else(|| {
            Error::InvalidState("state revision exceeded the u64 range".to_owned())
        })?;
        self.persist(DurableState::retracted(revision)).await?;
        Ok(())
    }

    async fn apply_terminal(&mut self, entry: Transition) -> Result<()> {
        self.apply(entry).await
    }

    async fn prune_retained(&mut self, _explicit: bool) -> Result<()> {
        let Some(limit) = self.terminal_receipt_limit else {
            return Ok(());
        };
        let mut next = self.state.clone();
        if !next.retain_terminal_receipts(limit) {
            return Ok(());
        }
        let revision = self.state.revision().checked_add(1).ok_or_else(|| {
            Error::InvalidState("state revision exceeded the u64 range".to_owned())
        })?;
        next.advance_revision(revision)?;
        self.persist(next).await?;
        Ok(())
    }
}

const fn finishing_attempt_releases_claim(outcome: &Result<()>) -> bool {
    matches!(
        outcome,
        Ok(()) | Err(Error::Store(StoreError::NotCommitted(_)))
    )
}

fn store_update_error(error: Error) -> StoreError {
    StoreError::Backend(error.to_string())
}

fn update_session(
    stored: &StoredState,
    record: Option<crate::catalog::SessionRecord>,
    initial: Option<crate::InitialSettings>,
    checkpoint: Option<EncodedPayload>,
    committed: &AtomicBool,
) -> std::result::Result<Option<StoreReplacement>, StoreError> {
    if record.is_none() && initial.is_none() && checkpoint.is_none() {
        return Ok(None);
    }
    let mut state = reduce(stored.clone()).map_err(store_update_error)?;
    let mut merged = match (state.session(), record) {
        (Some(existing), Some(record)) => Some(existing.merged(record)),
        (existing, record) => record.or_else(|| existing.cloned()),
    };
    if let (Some(merged), Some(initial)) = (&mut merged, initial)
        && merged.initial.is_none()
    {
        merged.initial = Some(initial);
    }
    let checkpoint = checkpoint.filter(|_| {
        state.session().is_none() && state.latest_checkpoint().is_none() && !state.has_operations()
    });
    let Some(checkpoint) = checkpoint else {
        if state.session() == merged.as_ref() {
            return Ok(None);
        }
        state.set_session(merged);
        return prepare_update(state).map(Some).map_err(store_update_error);
    };
    state.set_session(merged);
    let revision = state.revision().checked_add(1).ok_or_else(|| {
        store_update_error(Error::InvalidState(
            "state revision exceeded the u64 range".to_owned(),
        ))
    })?;
    state
        .apply_transition(revision, Transition::CheckpointCommitted { checkpoint })
        .map_err(store_update_error)?;
    let replacement = staged_update(state).map_err(store_update_error)?;
    committed.store(true, Ordering::Release);
    Ok(Some(replacement))
}

fn prepare_update(mut state: DurableState) -> Result<StoreReplacement> {
    let revision = state
        .revision()
        .checked_add(1)
        .ok_or_else(|| Error::InvalidState("state revision exceeded the u64 range".to_owned()))?;
    state.advance_revision(revision)?;
    staged_update(state)
}

fn staged_update(mut state: DurableState) -> Result<StoreReplacement> {
    let records = state.stage_records();
    if let Some(record) = state.session_mut() {
        record.touch();
    }
    Ok(StoreReplacement {
        payload: state.checkpoint_payload()?,
        records,
    })
}

fn reduce(stored: StoredState) -> Result<DurableState> {
    let Some(payload) = stored.payload else {
        return if stored.revision == 0 {
            Ok(DurableState::default())
        } else {
            Err(Error::InvalidState(format!(
                "store reported revision {} without a payload",
                stored.revision
            )))
        };
    };
    if stored.revision == 0 {
        return Err(Error::InvalidState(
            "store retained a payload at revision zero".to_owned(),
        ));
    }
    let RetainedCheckpoint {
        nanocode_durable_state,
        nanocode_session,
    } = serde_json::from_str(&payload).map_err(|source| Error::Decode {
        revision: stored.revision,
        source,
    })?;
    let mut state = DurableState::from_checkpoint(stored.revision, nanocode_durable_state)?;
    state.set_session(nanocode_session);
    Ok(state)
}

pub(crate) fn reduce_peeked(stored: StoredState) -> Result<DurableState> {
    reduce(stored)
}

#[derive(Clone)]
pub struct WeakDurableSession {
    commands: mpsc::WeakSender<CommandRequest>,
}

impl WeakDurableSession {
    pub async fn is_current_owner(&self) -> Result<bool> {
        let Some(commands) = self.commands.upgrade() else {
            return Ok(false);
        };
        let (result, receiver) = oneshot::channel();
        commands
            .send(Command::CurrentOwner { result }.into())
            .await
            .map_err(|_| Error::DriverStopped)?;
        match receive(receiver).await {
            Err(Error::DriverStopped) => Ok(false),
            outcome => outcome,
        }
    }
}

pub struct DurableSession {
    state_id: Arc<str>,
    store: SharedStore,
    terminal_receipt_limit: Option<usize>,
    commands: mpsc::Sender<CommandRequest>,
    releases: mpsc::UnboundedSender<ReleaseSignal>,
    caller_id: OwnerId,
    active_claims: AtomicUsize,
    described: Option<crate::SessionRecord>,
    preload: Arc<ArcSwapOption<CheckpointPreload>>,
    adopted: Arc<ArcSwapOption<crate::JournalClaim>>,
}

pub(crate) type LoadedCheckpoint = (EncodedPayload, Arc<HashSet<String>>);

struct CheckpointPreload {
    key: String,
    checkpoint: serde_json::Value,
    loaded: Option<LoadedCheckpoint>,
}

impl Clone for DurableSession {
    fn clone(&self) -> Self {
        Self {
            state_id: Arc::clone(&self.state_id),
            store: self.store.clone(),
            terminal_receipt_limit: self.terminal_receipt_limit,
            commands: self.commands.clone(),
            releases: self.releases.clone(),
            caller_id: OwnerId::new(),
            active_claims: AtomicUsize::new(0),
            described: self.described.clone(),
            preload: Arc::clone(&self.preload),
            adopted: Arc::clone(&self.adopted),
        }
    }
}

impl Drop for DurableSession {
    fn drop(&mut self) {
        if self.active_claims.load(Ordering::Acquire) > 0 {
            let _ = self
                .releases
                .send(ReleaseSignal::Direct(self.caller_id.clone()));
        }
    }
}

impl DurableSession {
    pub async fn open<S>(store: S, state_id: impl Into<String>) -> Result<Self>
    where
        S: StateStore + 'static,
    {
        Self::open_inner(store, state_id.into(), None).await
    }

    pub async fn open_with_terminal_receipt_limit<S>(
        store: S,
        state_id: impl Into<String>,
        limit: usize,
    ) -> Result<Self>
    where
        S: StateStore + 'static,
    {
        Self::open_inner(store, state_id.into(), Some(limit)).await
    }

    pub async fn prune_receipts(&self) -> Result<()> {
        let (result, receiver) = oneshot::channel();
        self.send(Command::PruneReceipts {
            caller: Caller::Direct(self.caller_id.clone()),
            result,
        })
        .await?;
        receive(receiver).await
    }

    async fn open_inner<S>(
        store: S,
        state_id: String,
        terminal_receipt_limit: Option<usize>,
    ) -> Result<Self>
    where
        S: StateStore + 'static,
    {
        let store = SharedStore::new(store)?;
        Self::open_shared(store, state_id, terminal_receipt_limit).await
    }

    pub async fn open_shared(
        store: SharedStore,
        state_id: String,
        terminal_receipt_limit: Option<usize>,
    ) -> Result<Self> {
        Self::open_shared_described(store, state_id, terminal_receipt_limit, None).await
    }

    pub async fn open_shared_described(
        store: SharedStore,
        state_id: String,
        terminal_receipt_limit: Option<usize>,
        record: Option<crate::SessionRecord>,
    ) -> Result<Self> {
        let session =
            Self::prepare_shared_described(store, state_id, terminal_receipt_limit, record).await?;
        let (result, receiver) = oneshot::channel();
        session.send(Command::Acquire { result }).await?;
        receive(receiver).await?;
        Ok(session)
    }

    pub async fn prepare_shared_described(
        store: SharedStore,
        state_id: String,
        terminal_receipt_limit: Option<usize>,
        record: Option<crate::SessionRecord>,
    ) -> Result<Self> {
        Self::prepare(store, state_id, terminal_receipt_limit, record, false)
    }

    pub(crate) fn prepare_new(store: SharedStore, record: crate::SessionRecord) -> Result<Self> {
        Self::prepare(store, record.session_id.clone(), None, Some(record), true)
    }

    fn prepare(
        store: SharedStore,
        state_id: String,
        terminal_receipt_limit: Option<usize>,
        record: Option<crate::SessionRecord>,
        create: bool,
    ) -> Result<Self> {
        if state_id.trim().is_empty() {
            return Err(Error::InvalidState(
                "state identity must not be empty".to_owned(),
            ));
        }
        if let Some(record) = &record
            && record.session_id != state_id
        {
            return Err(Error::InvalidState(
                "session record does not describe its state".into(),
            ));
        }
        let described = record.clone();
        let state_id = Arc::<str>::from(state_id);
        let (commands, receiver) = mpsc::channel(COMMAND_CAPACITY);
        let (releases, release_receiver) = mpsc::unbounded_channel();
        spawn_driver(Driver {
            store: Box::new(store.clone()),
            state_id: Arc::clone(&state_id),
            state: DurableState::default(),
            terminal_receipt_limit,
            owner: None,
            pending_record: record,
            create,
            next_agent_generation: 0,
            active_agent_generation: None,
            claimed: HashMap::new(),
            running: HashSet::new(),
            abandonable: HashSet::new(),
            poisoned: false,
            staged: Vec::new(),
            preview: None,
            batch: None,
            committed_records: HashSet::new(),
            next_event: None,
            commands: receiver,
            releases: release_receiver,
        })?;
        Ok(Self {
            state_id,
            store,
            terminal_receipt_limit,
            commands,
            releases,
            caller_id: OwnerId::new(),
            active_claims: AtomicUsize::new(0),
            described,
            preload: Arc::default(),
            adopted: Arc::default(),
        })
    }

    pub(crate) fn preload_checkpoint(
        &self,
        key: String,
        checkpoint: serde_json::Value,
        loaded: Option<LoadedCheckpoint>,
    ) {
        let loaded = loaded.filter(|(payload, _)| payload.key() == key);
        self.preload.store(Some(Arc::new(CheckpointPreload {
            key,
            checkpoint,
            loaded,
        })));
    }

    pub fn take_checkpoint_preload(&self, key: &str) -> Option<serde_json::Value> {
        self.take_checkpoint_preload_with_records(key)
            .map(|(checkpoint, _)| checkpoint)
    }

    pub fn take_checkpoint_preload_with_records(
        &self,
        key: &str,
    ) -> Option<(serde_json::Value, Option<Arc<HashSet<String>>>)> {
        self.preload
            .swap(None)
            .filter(|preloaded| preloaded.key == key)
            .map(|preloaded| {
                Arc::try_unwrap(preloaded).map_or_else(
                    |shared| {
                        (
                            shared.checkpoint.clone(),
                            shared
                                .loaded
                                .as_ref()
                                .map(|(_, records)| Arc::clone(records)),
                        )
                    },
                    |preloaded| {
                        (
                            preloaded.checkpoint,
                            preloaded.loaded.map(|(_, records)| records),
                        )
                    },
                )
            })
    }

    pub async fn describe(
        &self,
        record: crate::catalog::SessionRecord,
    ) -> Result<crate::catalog::SessionRecord> {
        let (result, receiver) = oneshot::channel();
        self.send(Command::Describe {
            caller: Caller::Direct(self.caller_id.clone()),
            record,
            result,
        })
        .await?;
        receive(receiver).await
    }

    pub async fn record(&self) -> Result<Option<crate::catalog::SessionRecord>> {
        Ok(self.state().await?.session().cloned())
    }

    pub async fn open_sibling(&self, state_id: impl Into<String>) -> Result<Self> {
        Self::open_shared(
            self.store.clone(),
            state_id.into(),
            self.terminal_receipt_limit,
        )
        .await
    }

    pub async fn is_current_owner(&self) -> Result<bool> {
        let (result, receiver) = oneshot::channel();
        self.send(Command::CurrentOwner { result }).await?;
        receive(receiver).await
    }

    #[must_use]
    pub fn downgrade(&self) -> WeakDurableSession {
        WeakDurableSession {
            commands: self.commands.downgrade(),
        }
    }

    #[must_use]
    pub fn shared_store(&self) -> SharedStore {
        self.store.clone()
    }

    pub fn child_journal(&self) -> nanocode_agent_core::backend::ChildJournal {
        crate::child_journal::child_journal(self.store.clone(), &self.state_id)
    }

    pub fn seeded_child_journal(
        &self,
    ) -> (
        nanocode_agent_core::backend::ChildJournal,
        crate::JournalSeed,
    ) {
        crate::child_journal::seeded_child_journal(self.store.clone(), &self.state_id)
    }

    pub fn adopt_child_journal(&self, seed: &crate::JournalSeed) -> Result<()> {
        if !seed.adopt(&self.state_id, &self.store) {
            return Err(Error::InvalidState(
                "the journal seed does not belong to this session".to_owned(),
            ));
        }
        self.adopted.store(Some(Arc::new(seed.claim())));
        Ok(())
    }

    fn adopted_seed(&self) -> Option<crate::child_journal::Seed> {
        self.adopted.swap(None)?.take()
    }

    #[must_use]
    pub fn state_id(&self) -> &str {
        &self.state_id
    }

    pub async fn state(&self) -> Result<DurableState> {
        let (result, receiver) = oneshot::channel();
        self.send(Command::State { result }).await?;
        receive(receiver).await
    }

    pub async fn resolve_many(&self, payloads: Vec<EncodedPayload>) -> Result<Vec<EncodedPayload>> {
        let (result, receiver) = oneshot::channel();
        self.send(Command::LoadPayloads {
            caller: None,
            payloads,
            result,
        })
        .await?;
        receive(receiver).await
    }

    pub async fn resolve(&self, payload: &EncodedPayload) -> Result<EncodedPayload> {
        let (result, receiver) = oneshot::channel();
        self.send(Command::LoadPayload {
            caller: None,
            payload: payload.clone(),
            result,
        })
        .await?;
        receive(receiver).await
    }

    pub async fn latest_checkpoint(&self) -> Result<Option<EncodedPayload>> {
        let (result, receiver) = oneshot::channel();
        self.send(Command::LatestCheckpoint { result }).await?;
        receive(receiver).await
    }

    pub async fn acquire_agent(&self) -> Result<(DurableOwner, Option<EncodedPayload>)> {
        self.acquire_agent_inner(None, None, self.adopted_seed())
            .await
            .map(|(owner, checkpoint, _)| (owner, checkpoint))
    }

    pub async fn acquire_agent_with_initial(
        &self,
        initial: crate::InitialSettings,
    ) -> Result<(DurableOwner, Option<EncodedPayload>)> {
        self.acquire_agent_inner(Some(initial), None, self.adopted_seed())
            .await
            .map(|(owner, checkpoint, _)| (owner, checkpoint))
    }

    pub async fn acquire_agent_with_initial_checkpoint(
        &self,
        initial: Option<crate::InitialSettings>,
        checkpoint: EncodedPayload,
    ) -> Result<(DurableOwner, InitialAcquisition)> {
        self.initial_acquisition(initial, checkpoint, None).await
    }

    pub async fn acquire_agent_with_initial_checkpoint_and_journal(
        &self,
        initial: Option<crate::InitialSettings>,
        checkpoint: EncodedPayload,
        journal: &crate::JournalClaim,
    ) -> Result<(DurableOwner, InitialAcquisition)> {
        let journal = journal.take();
        if let Some(journal) = &journal
            && (journal.root_state_id != *self.state_id || !journal.store.same_store(&self.store))
        {
            return Err(Error::InvalidState(
                "the journal seed does not belong to this session".to_owned(),
            ));
        }
        self.initial_acquisition(initial, checkpoint, journal).await
    }

    async fn initial_acquisition(
        &self,
        initial: Option<crate::InitialSettings>,
        checkpoint: EncodedPayload,
        journal: Option<crate::child_journal::Seed>,
    ) -> Result<(DurableOwner, InitialAcquisition)> {
        let (owner, existing, committed) = self
            .acquire_agent_inner(initial, Some(checkpoint), journal)
            .await?;
        Ok((
            owner,
            if committed {
                InitialAcquisition::Committed
            } else {
                InitialAcquisition::Existing(existing)
            },
        ))
    }

    #[must_use]
    pub const fn described_record(&self) -> Option<&crate::SessionRecord> {
        self.described.as_ref()
    }

    async fn acquire_agent_inner(
        &self,
        initial: Option<crate::InitialSettings>,
        checkpoint: Option<EncodedPayload>,
        journal: Option<crate::child_journal::Seed>,
    ) -> Result<(DurableOwner, Option<EncodedPayload>, bool)> {
        let (result, receiver) = oneshot::channel();
        let preloaded = self
            .preload
            .load()
            .as_ref()
            .and_then(|preload| preload.loaded.clone());
        self.send(Command::AcquireAgent {
            initial,
            checkpoint,
            journal,
            preloaded,
            result,
        })
        .await?;
        let acquired = receive(receiver).await?;
        let owner = DurableOwner {
            generation: acquired.generation,
            commands: self.commands.clone(),
            releases: self.releases.clone(),
            release: Arc::new(OwnerReleaseState::new()),
        };
        Ok((owner, acquired.checkpoint, acquired.initial_committed))
    }

    pub async fn admit<I>(&self, operation_id: impl Into<String>, input: &I) -> Result<Admission>
    where
        I: Serialize + ?Sized,
    {
        Ok(self
            .admit_encoded(operation_id.into(), EncodedPayload::encode(input)?)
            .await?
            .into_encoded())
    }

    pub async fn admit_typed<I, C, O>(
        &self,
        operation_id: impl Into<String>,
        input: &I,
    ) -> Result<Admission<C, O>>
    where
        I: Serialize + ?Sized,
        C: DeserializeOwned,
        O: DeserializeOwned,
    {
        self.admit_encoded(operation_id.into(), EncodedPayload::encode(input)?)
            .await?
            .decode()
    }

    pub async fn admit_automatic<I>(
        &self,
        candidate_operation_id: impl Into<String>,
        input: &I,
    ) -> Result<AutomaticAdmission>
    where
        I: Serialize + ?Sized,
    {
        let (operation_id, admission) = self
            .admit_automatic_encoded(
                candidate_operation_id.into(),
                EncodedPayload::encode(input)?,
            )
            .await?;
        Ok(AutomaticAdmission {
            operation_id,
            admission: admission.into_encoded(),
        })
    }

    pub async fn admit_automatic_typed<I, C, O>(
        &self,
        candidate_operation_id: impl Into<String>,
        input: &I,
    ) -> Result<AutomaticAdmission<C, O>>
    where
        I: Serialize + ?Sized,
        C: DeserializeOwned,
        O: DeserializeOwned,
    {
        let (operation_id, admission) = self
            .admit_automatic_encoded(
                candidate_operation_id.into(),
                EncodedPayload::encode(input)?,
            )
            .await?;
        let admission = admission.decode()?;
        Ok(AutomaticAdmission {
            operation_id,
            admission,
        })
    }

    async fn admit_encoded(
        &self,
        operation_id: String,
        input: EncodedPayload,
    ) -> Result<StoredAdmission> {
        let (result, receiver) = oneshot::channel();
        let (acknowledge, acknowledged) = oneshot::channel();
        self.send(Command::Admit {
            caller: Caller::Direct(self.caller_id.clone()),
            operation_id,
            input,
            acknowledged,
            release_commands: self.commands.clone(),
            result,
        })
        .await?;
        let admission = receive(receiver).await?;
        if matches!(
            &admission,
            StoredAdmission::Accepted | StoredAdmission::Pending
        ) {
            self.active_claims.fetch_add(1, Ordering::AcqRel);
        }
        let _ = acknowledge.send(());
        Ok(admission)
    }

    async fn admit_automatic_encoded(
        &self,
        candidate_operation_id: String,
        input: EncodedPayload,
    ) -> Result<(String, StoredAdmission)> {
        let (result, receiver) = oneshot::channel();
        let (acknowledge, acknowledged) = oneshot::channel();
        self.send(Command::AdmitAutomatic {
            caller: Caller::Direct(self.caller_id.clone()),
            candidate_operation_id,
            input,
            acknowledged,
            release_commands: self.commands.clone(),
            result,
        })
        .await?;
        let admission = receive(receiver).await?;
        if matches!(
            &admission.1,
            StoredAdmission::Accepted | StoredAdmission::Pending
        ) {
            self.active_claims.fetch_add(1, Ordering::AcqRel);
        }
        let _ = acknowledge.send(());
        Ok(admission)
    }

    pub async fn release(&self, operation_id: impl Into<String>) -> Result<()> {
        let (result, receiver) = oneshot::channel();
        self.send(Command::Release {
            caller: Caller::Direct(self.caller_id.clone()),
            operation_id: operation_id.into(),
            result,
        })
        .await?;
        let outcome = receive(receiver).await;
        if outcome.is_ok() {
            self.release_one_claim();
        }
        outcome
    }

    pub async fn begin_attempt(&self, operation_id: impl Into<String>) -> Result<()> {
        let (result, receiver) = oneshot::channel();
        self.send(Command::BeginAttempt {
            caller: Caller::Direct(self.caller_id.clone()),
            operation_id: operation_id.into(),
            result,
        })
        .await?;
        receive(receiver).await
    }

    pub async fn begin_step<I>(
        &self,
        operation_id: impl Into<String>,
        step_id: impl Into<String>,
        kind: impl Into<String>,
        input: &I,
    ) -> Result<BeginStep>
    where
        I: Serialize + ?Sized,
    {
        match self
            .begin_step_encoded(
                operation_id.into(),
                step_id.into(),
                kind.into(),
                EncodedPayload::encode(input)?,
            )
            .await?
        {
            StoredBeginStep::Execute => Ok(BeginStep::Execute),
            StoredBeginStep::Interrupted => Ok(BeginStep::Interrupted),
            StoredBeginStep::Replay(output) => Ok(BeginStep::Replay(output)),
        }
    }

    pub async fn begin_step_typed<I, O>(
        &self,
        operation_id: impl Into<String>,
        step_id: impl Into<String>,
        kind: impl Into<String>,
        input: &I,
    ) -> Result<BeginStep<O>>
    where
        I: Serialize + ?Sized,
        O: DeserializeOwned,
    {
        match self
            .begin_step_encoded(
                operation_id.into(),
                step_id.into(),
                kind.into(),
                EncodedPayload::encode(input)?,
            )
            .await?
        {
            StoredBeginStep::Execute => Ok(BeginStep::Execute),
            StoredBeginStep::Interrupted => Ok(BeginStep::Interrupted),
            StoredBeginStep::Replay(output) => Ok(BeginStep::Replay(output.decode()?)),
        }
    }

    async fn begin_step_encoded(
        &self,
        operation_id: String,
        step_id: String,
        kind: String,
        input: EncodedPayload,
    ) -> Result<StoredBeginStep> {
        let (result, receiver) = oneshot::channel();
        self.send(Command::BeginStep {
            caller: Caller::Direct(self.caller_id.clone()),
            operation_id,
            step_id,
            kind,
            input,
            result,
        })
        .await?;
        receive(receiver).await
    }

    pub async fn complete_step<T: Serialize + ?Sized>(
        &self,
        operation_id: impl Into<String>,
        step_id: impl Into<String>,
        output: &T,
    ) -> Result<()> {
        let (result, receiver) = oneshot::channel();
        self.send(Command::CompleteStep {
            caller: Caller::Direct(self.caller_id.clone()),
            operation_id: operation_id.into(),
            step_id: step_id.into(),
            output: EncodedPayload::encode(output)?,
            deferred: false,
            result,
        })
        .await?;
        receive(receiver).await
    }

    pub async fn complete<C: Serialize + ?Sized, O: Serialize + ?Sized>(
        &self,
        operation_id: impl Into<String>,
        checkpoint: &C,
        output: &O,
    ) -> Result<()> {
        let (result, receiver) = oneshot::channel();
        self.send(Command::Complete {
            caller: Caller::Direct(self.caller_id.clone()),
            operation_id: operation_id.into(),
            checkpoint: EncodedPayload::encode(checkpoint)?,
            output: EncodedPayload::encode(output)?,
            result,
        })
        .await?;
        let outcome = receive(receiver).await;
        if finishing_attempt_releases_claim(&outcome) {
            self.release_one_claim();
        }
        outcome
    }

    pub async fn fail<C: Serialize + ?Sized>(
        &self,
        operation_id: impl Into<String>,
        checkpoint: &C,
        error: impl Into<String>,
    ) -> Result<()> {
        let (result, receiver) = oneshot::channel();
        self.send(Command::Fail {
            caller: Caller::Direct(self.caller_id.clone()),
            operation_id: operation_id.into(),
            checkpoint: EncodedPayload::encode(checkpoint)?,
            error: error.into(),
            result,
        })
        .await?;
        let outcome = receive(receiver).await;
        if finishing_attempt_releases_claim(&outcome) {
            self.release_one_claim();
        }
        outcome
    }

    pub async fn fail_attempt(
        &self,
        operation_id: impl Into<String>,
        _error: impl Into<String>,
    ) -> Result<()> {
        let (result, receiver) = oneshot::channel();
        self.send(Command::FailAttempt {
            caller: Caller::Direct(self.caller_id.clone()),
            operation_id: operation_id.into(),
            result,
        })
        .await?;
        let outcome = receive(receiver).await;
        if finishing_attempt_releases_claim(&outcome) {
            self.release_one_claim();
        }
        outcome
    }

    pub async fn stage(&self, write: SessionWrite) -> Result<()> {
        let (result, receiver) = oneshot::channel();
        self.send(Command::Stage { write, result }).await?;
        receive(receiver).await
    }

    pub async fn append(&self, write: SessionWrite) -> Result<SessionReceipt> {
        let (result, receiver) = oneshot::channel();
        self.send(Command::Append { write, result }).await?;
        receive(receiver).await
    }

    pub async fn commit_checkpoint(
        &self,
        checkpoint: EncodedPayload,
        write: SessionWrite,
    ) -> Result<SessionReceipt> {
        let (result, receiver) = oneshot::channel();
        self.send(Command::CommitCheckpoint {
            caller: Caller::Direct(self.caller_id.clone()),
            checkpoint,
            write,
            result,
        })
        .await?;
        receive(receiver).await
    }

    pub async fn cancel(&self, operation_id: impl Into<String>) -> Result<()> {
        let (result, receiver) = oneshot::channel();
        self.send(Command::Cancel {
            caller: Caller::Direct(self.caller_id.clone()),
            operation_id: operation_id.into(),
            checkpoint: None,
            result,
        })
        .await?;
        let outcome = receive(receiver).await;
        if finishing_attempt_releases_claim(&outcome) {
            self.release_one_claim();
        }
        outcome
    }

    async fn send(&self, command: Command) -> Result<()> {
        self.commands
            .send(command.into())
            .await
            .map_err(|_| Error::DriverStopped)
    }

    fn release_one_claim(&self) {
        let _ = self
            .active_claims
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |claims| {
                claims.checked_sub(1)
            });
    }
}

pub struct DurableOwner {
    generation: u64,
    commands: mpsc::Sender<CommandRequest>,
    releases: mpsc::UnboundedSender<ReleaseSignal>,
    release: Arc<OwnerReleaseState>,
}

impl DurableOwner {
    pub async fn load_payloads(
        &self,
        payloads: Vec<EncodedPayload>,
    ) -> Result<Vec<EncodedPayload>> {
        let (result, receiver) = oneshot::channel();
        self.send(Command::LoadPayloads {
            caller: Some(self.caller()?),
            payloads,
            result,
        })
        .await?;
        receive(receiver).await
    }

    pub async fn load_payload(&self, payload: EncodedPayload) -> Result<EncodedPayload> {
        let (result, receiver) = oneshot::channel();
        self.send(Command::LoadPayload {
            caller: Some(self.caller()?),
            payload,
            result,
        })
        .await?;
        receive(receiver).await
    }

    pub async fn continuation(&self, operation_id: String) -> Result<Option<EncodedPayload>> {
        let (result, receiver) = oneshot::channel();
        self.send(Command::Continuation {
            caller: self.caller()?,
            operation_id,
            result,
        })
        .await?;
        receive(receiver).await
    }

    pub async fn advance(&self, operation_id: String, continuation: EncodedPayload) -> Result<()> {
        self.send_advance(operation_id, continuation, false).await
    }

    pub async fn advance_deferred(
        &self,
        operation_id: String,
        continuation: EncodedPayload,
    ) -> Result<()> {
        self.send_advance(operation_id, continuation, true).await
    }

    async fn send_advance(
        &self,
        operation_id: String,
        continuation: EncodedPayload,
        deferred: bool,
    ) -> Result<()> {
        let (result, receiver) = oneshot::channel();
        self.send(Command::Advance {
            caller: self.caller()?,
            operation_id,
            continuation,
            deferred,
            result,
        })
        .await?;
        receive(receiver).await
    }

    pub async fn abandon_stale_kinds(&self, kinds: &[&str]) -> Result<()> {
        let (result, receiver) = oneshot::channel();
        self.send(Command::Abandonable {
            caller: self.caller()?,
            kinds: kinds.iter().map(|kind| (*kind).to_owned()).collect(),
            result,
        })
        .await?;
        receive(receiver).await
    }

    pub async fn recover_failure(&self, operation_id: String) -> Result<Option<OperationStatus>> {
        let (result, receiver) = oneshot::channel();
        self.send(Command::RecoverFailure {
            caller: self.caller()?,
            operation_id,
            result,
        })
        .await?;
        receiver.await.map_err(|_| Error::DriverStopped)?
    }

    fn caller(&self) -> Result<Caller> {
        if self.release.state.load(Ordering::Acquire) != OWNER_ACTIVE {
            Err(Error::ModelOwnerFenced)
        } else {
            Ok(Caller::Agent(self.generation))
        }
    }

    async fn send(&self, command: Command) -> Result<()> {
        self.commands
            .send(command.into())
            .await
            .map_err(|_| Error::DriverStopped)
    }

    pub async fn admit_typed<I, C, O>(
        &self,
        operation_id: String,
        input: &I,
    ) -> Result<Admission<C, O>>
    where
        I: Serialize + ?Sized,
        C: DeserializeOwned,
        O: DeserializeOwned,
    {
        let (result, receiver) = oneshot::channel();
        let (acknowledge, acknowledged) = oneshot::channel();
        self.send(Command::Admit {
            caller: self.caller()?,
            operation_id,
            input: EncodedPayload::encode(input)?,
            acknowledged,
            release_commands: self.commands.clone(),
            result,
        })
        .await?;
        let admission = receive(receiver).await?;
        let _ = acknowledge.send(());
        admission.decode()
    }

    pub async fn admit_automatic_typed<I, C, O>(
        &self,
        candidate_operation_id: String,
        input: &I,
    ) -> Result<AutomaticAdmission<C, O>>
    where
        I: Serialize + ?Sized,
        C: DeserializeOwned,
        O: DeserializeOwned,
    {
        let (result, receiver) = oneshot::channel();
        let (acknowledge, acknowledged) = oneshot::channel();
        self.send(Command::AdmitAutomatic {
            caller: self.caller()?,
            candidate_operation_id,
            input: EncodedPayload::encode(input)?,
            acknowledged,
            release_commands: self.commands.clone(),
            result,
        })
        .await?;
        let (operation_id, admission) = receive(receiver).await?;
        let _ = acknowledge.send(());
        Ok(AutomaticAdmission {
            operation_id,
            admission: admission.decode()?,
        })
    }

    pub async fn release_claim(&self, operation_id: String) -> Result<()> {
        let (result, receiver) = oneshot::channel();
        self.send(Command::Release {
            caller: self.caller()?,
            operation_id,
            result,
        })
        .await?;
        receive(receiver).await
    }

    pub async fn begin_attempt(&self, operation_id: String) -> Result<()> {
        let (result, receiver) = oneshot::channel();
        self.send(Command::BeginAttempt {
            caller: self.caller()?,
            operation_id,
            result,
        })
        .await?;
        receive(receiver).await
    }

    pub async fn accept_steer<I: Serialize + ?Sized>(
        &self,
        operation_id: String,
        accepted_after_model_call_index: u32,
        input: &I,
        message_id: Option<String>,
    ) -> Result<Option<u32>> {
        let (result, receiver) = oneshot::channel();
        self.send(Command::AcceptSteer {
            message_id,
            caller: self.caller()?,
            operation_id,
            accepted_after_model_call_index,
            input: EncodedPayload::encode(input)?,
            identity: None,
            result,
        })
        .await?;
        receive(receiver).await
    }

    pub async fn accept_identified_steer<K: Serialize + ?Sized, I: Serialize + ?Sized>(
        &self,
        operation_id: String,
        message_id: String,
        accepted_after_model_call_index: u32,
        identity: &K,
        input: &I,
    ) -> Result<Option<u32>> {
        let (result, receiver) = oneshot::channel();
        self.send(Command::AcceptSteer {
            message_id: Some(message_id),
            caller: self.caller()?,
            operation_id,
            accepted_after_model_call_index,
            input: EncodedPayload::encode(input)?,
            identity: Some(EncodedPayload::encode(identity)?),
            result,
        })
        .await?;
        receive(receiver).await
    }

    pub async fn identified_steer_accepted<I: Serialize + ?Sized>(
        &self,
        message_id: String,
        input: &I,
    ) -> Result<bool> {
        let (result, receiver) = oneshot::channel();
        self.send(Command::IdentifiedSteerAccepted {
            caller: self.caller()?,
            message_id,
            input: EncodedPayload::encode(input)?,
            result,
        })
        .await?;
        receive(receiver).await
    }

    pub async fn steer_receipt<I: Serialize + ?Sized>(
        &self,
        message_id: String,
        input: &I,
    ) -> Result<SteerReceiptStatus> {
        let (result, receiver) = oneshot::channel();
        self.send(Command::SteerReceipt {
            caller: self.caller()?,
            message_id,
            input: EncodedPayload::encode(input)?,
            result,
        })
        .await?;
        receive(receiver).await
    }

    pub async fn retained_steers(&self, operation_id: String) -> Result<Vec<StoredSteer>> {
        let (result, receiver) = oneshot::channel();
        self.send(Command::RetainedSteers {
            caller: self.caller()?,
            operation_id,
            result,
        })
        .await?;
        receive(receiver).await
    }

    pub async fn withdraw_steer(&self, operation_id: String, steer_index: u32) -> Result<()> {
        let (result, receiver) = oneshot::channel();
        self.send(Command::WithdrawSteer {
            caller: self.caller()?,
            operation_id,
            steer_index,
            result,
        })
        .await?;
        receive(receiver).await
    }

    pub async fn bind_steer(
        &self,
        operation_id: String,
        steer_index: u32,
        model_call_index: u32,
    ) -> Result<()> {
        let (result, receiver) = oneshot::channel();
        self.send(Command::BindSteer {
            caller: self.caller()?,
            operation_id,
            steer_index,
            model_call_index,
            result,
        })
        .await?;
        receive(receiver).await
    }

    pub async fn begin_step<I>(
        &self,
        operation_id: String,
        step_id: String,
        kind: String,
        input: &I,
    ) -> Result<BeginStep>
    where
        I: Serialize + ?Sized,
    {
        let (result, receiver) = oneshot::channel();
        self.send(Command::BeginStep {
            caller: self.caller()?,
            operation_id,
            step_id,
            kind,
            input: EncodedPayload::encode(input)?,
            result,
        })
        .await?;
        match receive(receiver).await? {
            StoredBeginStep::Execute => Ok(BeginStep::Execute),
            StoredBeginStep::Interrupted => Ok(BeginStep::Interrupted),
            StoredBeginStep::Replay(output) => Ok(BeginStep::Replay(output)),
        }
    }

    pub async fn complete_step<T: Serialize + ?Sized>(
        &self,
        operation_id: String,
        step_id: String,
        output: &T,
    ) -> Result<()> {
        self.send_completion(
            operation_id,
            step_id,
            EncodedPayload::encode(output)?,
            false,
        )
        .await
    }

    pub async fn complete_step_deferred<T: Serialize + ?Sized>(
        &self,
        operation_id: String,
        step_id: String,
        output: &T,
    ) -> Result<()> {
        self.send_completion(operation_id, step_id, EncodedPayload::encode(output)?, true)
            .await
    }

    pub async fn complete_step_deferred_encoded(
        &self,
        operation_id: String,
        step_id: String,
        output: EncodedPayload,
    ) -> Result<()> {
        self.send_completion(operation_id, step_id, output, true)
            .await
    }

    async fn send_completion(
        &self,
        operation_id: String,
        step_id: String,
        output: EncodedPayload,
        deferred: bool,
    ) -> Result<()> {
        let (result, receiver) = oneshot::channel();
        self.send(Command::CompleteStep {
            caller: self.caller()?,
            operation_id,
            step_id,
            output,
            deferred,
            result,
        })
        .await?;
        receive(receiver).await
    }

    pub async fn commit_writes(
        &self,
        operation_id: String,
        writes: Vec<OwnerWrite>,
    ) -> Result<Vec<WriteOutcome>> {
        let (result, receiver) = oneshot::channel();
        self.send(Command::CommitWrites {
            caller: self.caller()?,
            operation_id,
            writes,
            result,
        })
        .await?;
        receive(receiver).await
    }

    pub async fn complete<O: Serialize + ?Sized>(
        &self,
        operation_id: String,
        checkpoint: EncodedPayload,
        output: &O,
    ) -> Result<()> {
        let (result, receiver) = oneshot::channel();
        self.send(Command::Complete {
            caller: self.caller()?,
            operation_id,
            checkpoint,
            output: EncodedPayload::encode(output)?,
            result,
        })
        .await?;
        receive(receiver).await
    }

    pub async fn fail(
        &self,
        operation_id: String,
        checkpoint: EncodedPayload,
        error: String,
    ) -> Result<()> {
        let (result, receiver) = oneshot::channel();
        self.send(Command::Fail {
            caller: self.caller()?,
            operation_id,
            checkpoint,
            error,
            result,
        })
        .await?;
        receive(receiver).await
    }

    pub async fn fail_attempt(&self, operation_id: String, _error: String) -> Result<()> {
        let (result, receiver) = oneshot::channel();
        self.send(Command::FailAttempt {
            caller: self.caller()?,
            operation_id,
            result,
        })
        .await?;
        receive(receiver).await
    }

    pub async fn cancel(
        &self,
        operation_id: String,
        checkpoint: Option<EncodedPayload>,
    ) -> Result<()> {
        let (result, receiver) = oneshot::channel();
        self.send(Command::Cancel {
            caller: self.caller()?,
            operation_id,
            checkpoint,
            result,
        })
        .await?;
        receive(receiver).await
    }

    pub async fn discard_unused(&self) -> Result<()> {
        let (result, receiver) = oneshot::channel();
        self.send(Command::DiscardUnused {
            caller: self.caller()?,
            result,
        })
        .await?;
        receive(receiver).await
    }

    pub async fn commit_checkpoint(&self, checkpoint: EncodedPayload) -> Result<()> {
        let (result, receiver) = oneshot::channel();
        self.send(Command::CommitCheckpoint {
            caller: self.caller()?,
            checkpoint,
            write: SessionWrite::default(),
            result,
        })
        .await?;
        receive(receiver).await?;
        Ok(())
    }

    pub async fn shutdown(&self) -> Result<()> {
        match self.release.state.compare_exchange(
            OWNER_ACTIVE,
            OWNER_RELEASING,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(OWNER_ACTIVE) => {
                if self
                    .releases
                    .send(ReleaseSignal::Agent(AgentRelease {
                        generation: self.generation,
                        state: Arc::clone(&self.release),
                    }))
                    .is_err()
                {
                    self.release.finish();
                    return Err(Error::DriverStopped);
                }
            }
            Err(OWNER_RELEASED) => return Ok(()),
            Err(OWNER_RELEASING) => {}
            Ok(_) | Err(_) => {
                return Err(Error::InvalidState(
                    "durable owner entered an invalid release state".to_owned(),
                ));
            }
        }
        let mut completed = self.release.completed.subscribe();
        if *completed.borrow() {
            return Ok(());
        }
        tokio::select! {
            changed = completed.changed() => changed.map_err(|_| Error::DriverStopped),
            () = self.commands.closed() => {
                self.release.finish();
                Err(Error::DriverStopped)
            }
        }
    }
}

impl Drop for DurableOwner {
    fn drop(&mut self) {
        if self
            .release
            .state
            .compare_exchange(
                OWNER_ACTIVE,
                OWNER_RELEASING,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
        {
            let _ = self.releases.send(ReleaseSignal::Agent(AgentRelease {
                generation: self.generation,
                state: Arc::clone(&self.release),
            }));
        }
    }
}

async fn receive<T>(receiver: oneshot::Receiver<Result<T>>) -> Result<T> {
    receiver.await.map_err(|_| Error::DriverStopped)?
}

#[cfg(not(target_family = "wasm"))]
fn spawn_claim_ack(
    commands: mpsc::Sender<CommandRequest>,
    acknowledged: oneshot::Receiver<()>,
    caller: Caller,
    operation_id: String,
) {
    if let Ok(runtime) = tokio::runtime::Handle::try_current() {
        drop(runtime.spawn(async move {
            if acknowledged.await.is_err() {
                let (result, _receiver) = oneshot::channel();
                drop(
                    commands
                        .send(
                            Command::Release {
                                caller,
                                operation_id,
                                result,
                            }
                            .into(),
                        )
                        .await,
                );
            }
        }));
    }
}

#[cfg(target_family = "wasm")]
fn spawn_claim_ack(
    commands: mpsc::Sender<CommandRequest>,
    acknowledged: oneshot::Receiver<()>,
    caller: Caller,
    operation_id: String,
) {
    nanocode_platform::spawn_local(async move {
        if acknowledged.await.is_err() {
            let (result, _receiver) = oneshot::channel();
            drop(
                commands
                    .send(
                        Command::Release {
                            caller,
                            operation_id,
                            result,
                        }
                        .into(),
                    )
                    .await,
            );
        }
    });
}

#[cfg(not(target_family = "wasm"))]
fn spawn_driver(driver: Driver) -> Result<()> {
    let runtime = tokio::runtime::Handle::try_current().map_err(|_| Error::RuntimeUnavailable)?;
    drop(runtime.spawn(driver.run()));
    Ok(())
}

#[cfg(target_family = "wasm")]
fn spawn_driver(driver: Driver) -> Result<()> {
    nanocode_platform::spawn_local(driver.run());
    Ok(())
}
