use std::{
    collections::HashSet,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use nanocode_agent_core::{
    Agent, AgentEvents, BackendRequest, Result as AgentResult, SteerReceiptStatus,
    backend::BackendFuture,
};
use nanocode_durability::{
    Admission, BeginStep, Branches, CheckpointFormat, DurableAgentExt, DurableBackendRegistration,
    DurableOwner, DurableSession, EncodedPayload, InitialAcquisition, InitialSettings,
    JournalClaim, JournalSeed, OwnerWrite, PayloadReader, SessionRecord, SharedStore, WriteOutcome,
    agent_error,
};
use serde_json::{Value, value::RawValue};

use crate::{
    Claude, ClaudeBuilder,
    execution::{
        Admission as ClaudeAdmission, BeginRequest, ClaudeExecutionPolicy, ClaudeSteer,
        EncodedState, JournalWrite, PolicyFuture, Settlement, Step,
    },
};

mod format;

pub use format::ClaudeCheckpointFormat;

const MAINTENANCE_KINDS: &[&str] = &["compact", "session_end"];

impl DurableBackendRegistration for ClaudeBuilder {
    fn checkpoint_format(&self) -> Option<Arc<dyn CheckpointFormat>> {
        Some(Arc::new(ClaudeCheckpointFormat))
    }

    fn build_durable(
        &self,
        request: BackendRequest,
        state: DurableSession,
    ) -> BackendFuture<AgentResult<(Agent, AgentEvents)>> {
        let prepared = self.clone().prepare(request);
        Box::pin(async move { prepared?.durability(state).await?.build() })
    }
}

impl DurableBackendRegistration for Claude {
    fn checkpoint_format(&self) -> Option<Arc<dyn CheckpointFormat>> {
        Some(Arc::new(ClaudeCheckpointFormat))
    }

    fn build_durable(
        &self,
        request: BackendRequest,
        state: DurableSession,
    ) -> BackendFuture<AgentResult<(Agent, AgentEvents)>> {
        Agent::builder(self.clone()).build_durable(request, state)
    }
}

impl DurableAgentExt for ClaudeBuilder {
    async fn durability(self, state: DurableSession) -> AgentResult<Self> {
        let state_id = state.state_id().to_owned();
        let journal = state.child_journal();
        let (owner, checkpoint) = match self.durable_initial_settings() {
            Ok(initial) => state.acquire_agent_with_initial(initial).await,
            Err(error) => {
                if state
                    .record()
                    .await
                    .map_err(agent_error)?
                    .is_some_and(|record| record.initial.is_none())
                {
                    return Err(error);
                }
                state.acquire_agent().await
            }
        }
        .map_err(agent_error)?;
        owner
            .abandon_stale_kinds(MAINTENANCE_KINDS)
            .await
            .map_err(agent_error)?;
        let preloaded =
            state.take_checkpoint_preload(checkpoint.as_ref().map_or("", EncodedPayload::key));
        let record = state.record().await.map_err(agent_error)?;
        let branches = Branches {
            store: state.shared_store(),
            record,
        };

        let builder = match &branches.record {
            Some(record) => self.lineage(record.lineage.clone()),
            None => self,
        };
        let builder = if checkpoint.is_none()
            && let Some(record) = &branches.record
            && let Some(initial) = record.initial
        {
            record
                .model
                .capabilities(nanocode_agent_core::ModelTransport::Native)
                .check_service_tier(initial.service_tier)?;
            builder
                .thinking(initial.thinking)?
                .fast_mode(initial.service_tier != nanocode_agent_core::ServiceTier::Standard)
        } else {
            builder
        };
        let checkpoint = match (checkpoint, preloaded) {
            (Some(_), Some(preloaded)) => {
                Some(hydrate(PayloadReader::from(&owner), preloaded).await?)
            }
            (Some(value), None) => Some(
                hydrate(
                    PayloadReader::from(&owner),
                    value.decode().map_err(agent_error)?,
                )
                .await?,
            ),
            (None, _) => {
                let imported = builder.configured_checkpoint()?;
                if let Some(checkpoint) = &imported
                    && state
                        .state()
                        .await
                        .map_err(agent_error)?
                        .pending_operations()
                        .is_empty()
                {
                    owner
                        .commit_checkpoint(EncodedPayload::encode(checkpoint).map_err(agent_error)?)
                        .await
                        .map_err(agent_error)?;
                }
                imported
            }
        };
        builder.child_journal(journal).execution_policy(
            Arc::new(ClaudeExecution {
                state_id,
                owner,
                session: state,
                branches,
            }),
            checkpoint,
        )
    }
}

impl ClaudeBuilder {
    pub(crate) fn deferred_durability(
        self,
        state: &DurableSession,
        record: SessionRecord,
        notice: String,
    ) -> AgentResult<(Self, Arc<AtomicBool>, JournalSeed)> {
        let initial = self.durable_initial_settings()?;
        record
            .model
            .capabilities(nanocode_agent_core::ModelTransport::Native)
            .check_service_tier(initial.service_tier)?;
        let existing = Arc::new(AtomicBool::new(false));
        let (journal, seed) = state.seeded_child_journal();
        let builder = self
            .lineage(record.lineage.clone())
            .thinking(initial.thinking)?
            .fast_mode(initial.service_tier != nanocode_agent_core::ServiceTier::Standard)
            .initial_developer_notices(vec![(nanocode_platform::uuid_v7().to_string(), notice)])
            .child_journal(journal)
            .execution_policy(
                Arc::new(LazyClaudeExecution::deferred(
                    state.shared_store(),
                    record,
                    initial,
                    Arc::clone(&existing),
                    seed.claim(),
                )),
                None,
            )?;
        Ok((builder, existing, seed))
    }
}

struct ClaudeExecution {
    state_id: String,
    owner: DurableOwner,
    session: DurableSession,
    branches: Branches,
}

struct LazyClaudeExecution {
    store: SharedStore,
    record: SessionRecord,
    ready: tokio::sync::OnceCell<ClaudeExecution>,

    reopened: std::sync::atomic::AtomicBool,

    initialized: std::sync::atomic::AtomicBool,

    model: nanocode_sync::Atom<Option<nanocode_agent_core::ModelSelection>>,

    initial: Option<InitialSettings>,

    existing: Option<Arc<AtomicBool>>,

    attempted: AtomicBool,

    journal: Option<JournalClaim>,
}

impl LazyClaudeExecution {
    fn new(store: SharedStore, record: SessionRecord) -> Self {
        Self {
            store,
            record,
            ready: tokio::sync::OnceCell::new(),
            reopened: std::sync::atomic::AtomicBool::new(false),
            initialized: std::sync::atomic::AtomicBool::new(false),
            model: nanocode_sync::Atom::default(),
            initial: None,
            existing: None,
            attempted: AtomicBool::new(false),
            journal: None,
        }
    }

    fn deferred(
        store: SharedStore,
        record: SessionRecord,
        initial: InitialSettings,
        existing: Arc<AtomicBool>,
        journal: JournalClaim,
    ) -> Self {
        Self {
            initial: Some(initial),
            existing: Some(existing),
            journal: Some(journal),
            ..Self::new(store, record)
        }
    }

    async fn get(&self) -> AgentResult<&ClaudeExecution> {
        self.acquire(None).await.map(|(policy, _)| policy)
    }

    async fn acquire(
        &self,
        initial: Option<EncodedPayload>,
    ) -> AgentResult<(&ClaudeExecution, bool)> {
        let committed = AtomicBool::new(false);
        let policy = self
            .ready
            .get_or_try_init(|| async {
                let mut record = self.record.clone();
                if let Some(model) = Option::clone(&self.model.get()) {
                    record.model = model;
                }
                let state = DurableSession::prepare_shared_described(
                    self.store.clone(),
                    record.session_id.clone(),
                    None,
                    Some(record),
                )
                .await
                .map_err(agent_error)?;
                let (owner, checkpoint) = match (initial, self.initial) {
                    (Some(payload), settings) => {
                        match match &self.journal {
                            Some(journal) => {
                                state
                                    .acquire_agent_with_initial_checkpoint_and_journal(
                                        settings, payload, journal,
                                    )
                                    .await
                            }
                            None => {
                                state
                                    .acquire_agent_with_initial_checkpoint(settings, payload)
                                    .await
                            }
                        }
                        .map_err(agent_error)?
                        {
                            (owner, InitialAcquisition::Committed) => {
                                committed.store(true, Ordering::SeqCst);
                                (owner, None)
                            }
                            (owner, InitialAcquisition::Existing(checkpoint)) => {
                                (owner, checkpoint)
                            }
                        }
                    }
                    (None, Some(settings)) => state
                        .acquire_agent_with_initial(settings)
                        .await
                        .map_err(agent_error)?,
                    (None, None) => state.acquire_agent().await.map_err(agent_error)?,
                };
                owner
                    .abandon_stale_kinds(MAINTENANCE_KINDS)
                    .await
                    .map_err(agent_error)?;
                let record = state.record().await.map_err(agent_error)?.ok_or_else(|| {
                    agent_error(nanocode_durability::Error::InvalidState(
                        "described durable session has no record".to_owned(),
                    ))
                })?;
                self.reopened.store(
                    checkpoint.is_some()
                        || !state
                            .state()
                            .await
                            .map_err(agent_error)?
                            .pending_operations()
                            .is_empty(),
                    std::sync::atomic::Ordering::SeqCst,
                );
                Ok::<_, nanocode_agent_core::NanocodeError>(ClaudeExecution {
                    state_id: record.session_id.clone(),
                    owner,
                    session: state,
                    branches: Branches {
                        store: self.store.clone(),
                        record: Some(record),
                    },
                })
            })
            .await?;
        Ok((policy, committed.load(Ordering::SeqCst)))
    }
}

fn branch_child(
    branches: &Branches,
    child: &nanocode_agent_core::SessionInfo,
) -> AgentResult<Option<Arc<dyn ClaudeExecutionPolicy>>> {
    if branches.record.is_none() {
        return Ok(None);
    }
    Ok(Some(Arc::new(LazyClaudeExecution::new(
        branches.store.clone(),
        branches.child_record(child)?,
    ))))
}

impl ClaudeExecutionPolicy for LazyClaudeExecution {
    fn state_id(&self) -> &str {
        &self.record.session_id
    }

    fn admit(
        &self,
        id: String,
        input: Value,
        automatic: bool,
    ) -> PolicyFuture<'_, (String, ClaudeAdmission)> {
        Box::pin(async move { self.get().await?.admit(id, input, automatic).await })
    }

    fn begin_attempt(&self, id: String) -> PolicyFuture<'_, ()> {
        Box::pin(async move { self.get().await?.begin_attempt(id).await })
    }

    fn supports_steering(&self) -> bool {
        true
    }

    fn identified_steer_accepted(
        &self,
        message_id: String,
        input_json: String,
    ) -> PolicyFuture<'_, bool> {
        Box::pin(async move {
            self.get()
                .await?
                .identified_steer_accepted(message_id, input_json)
                .await
        })
    }

    fn steer_receipt(
        &self,
        message_id: String,
        input_json: String,
    ) -> PolicyFuture<'_, SteerReceiptStatus> {
        Box::pin(async move {
            self.get()
                .await?
                .steer_receipt(message_id, input_json)
                .await
        })
    }

    fn accept_steer(
        &self,
        id: String,
        message_id: Option<String>,
        after: u32,
        identity_json: String,
        input_json: String,
    ) -> PolicyFuture<'_, Option<u32>> {
        Box::pin(async move {
            self.get()
                .await?
                .accept_steer(id, message_id, after, identity_json, input_json)
                .await
        })
    }

    fn retained_steers(&self, id: String) -> PolicyFuture<'_, Vec<ClaudeSteer>> {
        Box::pin(async move { self.get().await?.retained_steers(id).await })
    }

    fn withdraw_steer(&self, id: String, index: u32) -> PolicyFuture<'_, ()> {
        Box::pin(async move { self.get().await?.withdraw_steer(id, index).await })
    }

    fn bind_steer(&self, id: String, index: u32, boundary: u32) -> PolicyFuture<'_, ()> {
        Box::pin(async move { self.get().await?.bind_steer(id, index, boundary).await })
    }

    fn continuation(&self, id: String) -> PolicyFuture<'_, Option<Value>> {
        Box::pin(async move { self.get().await?.continuation(id).await })
    }

    fn advance(&self, id: String, state: Value) -> PolicyFuture<'_, ()> {
        Box::pin(async move { self.get().await?.advance(id, state).await })
    }

    fn advance_encoded(&self, id: String, state: Box<RawValue>) -> PolicyFuture<'_, ()> {
        Box::pin(async move { self.get().await?.advance_encoded(id, state).await })
    }

    fn begin_step(
        &self,
        id: String,
        step_id: String,
        kind: String,
        input: Value,
    ) -> PolicyFuture<'_, Step> {
        Box::pin(async move { self.get().await?.begin_step(id, step_id, kind, input).await })
    }

    fn commit_writes<'a>(
        &'a self,
        id: String,
        writes: Vec<JournalWrite>,
        begin: Option<BeginRequest<'a>>,
    ) -> PolicyFuture<'a, Option<Step>> {
        Box::pin(async move { self.get().await?.commit_writes(id, writes, begin).await })
    }

    fn defer_writes(&self, id: String, writes: Vec<JournalWrite>) -> PolicyFuture<'_, ()> {
        Box::pin(async move { self.get().await?.defer_writes(id, writes).await })
    }

    fn complete_step(&self, id: String, step_id: String, output: Value) -> PolicyFuture<'_, ()> {
        Box::pin(async move { self.get().await?.complete_step(id, step_id, output).await })
    }

    fn complete(&self, id: String, checkpoint: Value, output: Value) -> PolicyFuture<'_, ()> {
        Box::pin(async move { self.get().await?.complete(id, checkpoint, output).await })
    }

    fn fail(&self, id: String, checkpoint: Value, error: String) -> PolicyFuture<'_, ()> {
        Box::pin(async move { self.get().await?.fail(id, checkpoint, error).await })
    }

    fn fail_attempt(&self, id: String, checkpoint: Value, error: String) -> PolicyFuture<'_, ()> {
        Box::pin(async move { self.get().await?.fail_attempt(id, checkpoint, error).await })
    }

    fn cancel(&self, id: String, checkpoint: Value) -> PolicyFuture<'_, ()> {
        Box::pin(async move { self.get().await?.cancel(id, checkpoint).await })
    }

    fn pages_messages(&self) -> bool {
        true
    }

    fn settle_encoded(
        &self,
        id: String,
        checkpoint: EncodedState,
        settlement: Settlement,
    ) -> PolicyFuture<'_, ()> {
        Box::pin(async move {
            self.get()
                .await?
                .settle_encoded(id, checkpoint, settlement)
                .await
        })
    }

    fn release(&self, id: String) -> PolicyFuture<'_, ()> {
        Box::pin(async move {
            match self.ready.get() {
                Some(policy) => policy.release(id).await,
                None => Ok(()),
            }
        })
    }

    fn shutdown(&self) -> PolicyFuture<'_, ()> {
        Box::pin(async move {
            match self.ready.get() {
                Some(policy) => policy.shutdown().await,
                None => Ok(()),
            }
        })
    }

    fn checkpoint(&self, state: Value) -> PolicyFuture<'_, ()> {
        Box::pin(async move { self.get().await?.checkpoint(state).await })
    }

    fn developer_notices(&self, id: String) -> PolicyFuture<'_, Vec<(String, String)>> {
        Box::pin(async move { self.get().await?.developer_notices(id).await })
    }

    fn initial_checkpoint(
        &self,
        state: Value,
        model: nanocode_agent_core::ModelSelection,
    ) -> PolicyFuture<'_, ()> {
        Box::pin(async move {
            if self.initialized.load(std::sync::atomic::Ordering::SeqCst) {
                return Ok(());
            }
            if self.existing.is_some() && self.attempted.swap(true, Ordering::SeqCst) {
                return Err(nanocode_agent_core::NanocodeError::InvalidExecutionPolicy(
                    "the deferred durable acquisition already failed".to_owned(),
                ));
            }
            self.model.set(Some(model));

            let prepared = self
                .ready
                .get()
                .is_none()
                .then(|| payload(&state))
                .transpose()?;
            let (policy, committed) = self.acquire(prepared).await?;
            if let Some(existing) = self.existing.as_ref().filter(|_| !committed) {
                existing.store(true, Ordering::SeqCst);
                return Err(nanocode_agent_core::NanocodeError::InvalidExecutionPolicy(
                    "the prepared durable session already holds state".to_owned(),
                ));
            }
            if self.reopened.load(std::sync::atomic::Ordering::SeqCst) {
                return Ok(());
            }
            if !committed {
                policy.checkpoint(state).await?;
            }
            self.initialized
                .store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        })
    }

    fn discard_initial(&self) -> PolicyFuture<'_, ()> {
        Box::pin(async move {
            let Some(policy) = self.ready.get() else {
                return Ok(());
            };
            if !self.initialized.load(std::sync::atomic::Ordering::SeqCst) {
                return Ok(());
            }
            policy.owner.discard_unused().await.map_err(agent_error)
        })
    }

    fn branch(
        &self,
        child: &nanocode_agent_core::SessionInfo,
    ) -> AgentResult<Option<Arc<dyn ClaudeExecutionPolicy>>> {
        branch_child(
            &Branches {
                store: self.store.clone(),
                record: Some(self.record.clone()),
            },
            child,
        )
    }
}

impl ClaudeExecutionPolicy for ClaudeExecution {
    fn state_id(&self) -> &str {
        &self.state_id
    }

    fn branch(
        &self,
        child: &nanocode_agent_core::SessionInfo,
    ) -> AgentResult<Option<Arc<dyn ClaudeExecutionPolicy>>> {
        branch_child(&self.branches, child)
    }

    fn admit(
        &self,
        id: String,
        input: Value,
        automatic: bool,
    ) -> PolicyFuture<'_, (String, ClaudeAdmission)> {
        Box::pin(async move {
            let (id, admission) = if automatic {
                self.owner
                    .admit_automatic_typed::<_, Value, Value>(id, &input)
                    .await
                    .map_err(agent_error)?
                    .into_parts()
            } else {
                let admission = self
                    .owner
                    .admit_typed::<_, Value, Value>(id.clone(), &input)
                    .await
                    .map_err(agent_error)?;
                (id, admission)
            };
            let reader = PayloadReader::from(&self.owner);
            let admission = match admission {
                Admission::Completed { checkpoint, output } => ClaudeAdmission::Completed {
                    checkpoint: hydrate(reader, checkpoint).await?,
                    output: format::hydrate_output(reader, output)
                        .await
                        .map_err(agent_error)?,
                },
                Admission::Failed { checkpoint, error } => ClaudeAdmission::Failed {
                    checkpoint: hydrate(reader, checkpoint).await?,
                    error,
                },
                admission => map_admission(admission),
            };
            Ok((id, admission))
        })
    }

    fn begin_attempt(&self, id: String) -> PolicyFuture<'_, ()> {
        Box::pin(async move { self.owner.begin_attempt(id).await.map_err(agent_error) })
    }

    fn supports_steering(&self) -> bool {
        true
    }

    fn identified_steer_accepted(
        &self,
        message_id: String,
        input_json: String,
    ) -> PolicyFuture<'_, bool> {
        Box::pin(async move {
            let input: Box<serde_json::value::RawValue> = serde_json::from_str(&input_json)
                .map_err(|error| {
                    nanocode_agent_core::NanocodeError::InvalidRequest(error.to_string())
                })?;
            self.owner
                .identified_steer_accepted(message_id, &input)
                .await
                .map_err(agent_error)
        })
    }

    fn steer_receipt(
        &self,
        message_id: String,
        input_json: String,
    ) -> PolicyFuture<'_, SteerReceiptStatus> {
        Box::pin(async move {
            let input: Box<RawValue> = serde_json::from_str(&input_json).map_err(|error| {
                nanocode_agent_core::NanocodeError::InvalidRequest(error.to_string())
            })?;
            self.owner
                .steer_receipt(message_id, &input)
                .await
                .map_err(agent_error)
        })
    }

    fn accept_steer(
        &self,
        id: String,
        message_id: Option<String>,
        after: u32,
        identity_json: String,
        input_json: String,
    ) -> PolicyFuture<'_, Option<u32>> {
        Box::pin(async move {
            let raw = |json: &str| {
                serde_json::from_str::<Box<RawValue>>(json).map_err(|error| {
                    nanocode_agent_core::NanocodeError::InvalidRequest(error.to_string())
                })
            };
            let input = raw(&input_json)?;
            match message_id {
                Some(message_id) => {
                    self.owner
                        .accept_identified_steer(
                            id,
                            message_id,
                            after,
                            &raw(&identity_json)?,
                            &input,
                        )
                        .await
                }
                None => self.owner.accept_steer(id, after, &input, None).await,
            }
            .map_err(agent_error)
        })
    }
    fn retained_steers(&self, id: String) -> PolicyFuture<'_, Vec<ClaudeSteer>> {
        Box::pin(async move {
            self.owner
                .retained_steers(id)
                .await
                .and_then(|steers| {
                    steers
                        .into_iter()
                        .map(|steer| {
                            Ok(ClaudeSteer {
                                message_id: steer.state.message_id,
                                index: steer.index,
                                accepted_after_model_call_index: steer
                                    .state
                                    .accepted_after_model_call_index,
                                model_call_index: steer.state.model_call_index,
                                input_json: steer.state.input.json()?.to_owned(),
                            })
                        })
                        .collect()
                })
                .map_err(agent_error)
        })
    }
    fn withdraw_steer(&self, id: String, index: u32) -> PolicyFuture<'_, ()> {
        Box::pin(async move {
            self.owner
                .withdraw_steer(id, index)
                .await
                .map_err(agent_error)
        })
    }
    fn bind_steer(&self, id: String, index: u32, boundary: u32) -> PolicyFuture<'_, ()> {
        Box::pin(async move {
            self.owner
                .bind_steer(id, index, boundary)
                .await
                .map_err(agent_error)
        })
    }
    fn continuation(&self, id: String) -> PolicyFuture<'_, Option<Value>> {
        Box::pin(async move {
            let Some(value) = self.owner.continuation(id).await.map_err(agent_error)? else {
                return Ok(None);
            };
            format::hydrate_cursor(
                PayloadReader::from(&self.owner),
                value.decode().map_err(agent_error)?,
            )
            .await
            .map(Some)
            .map_err(agent_error)
        })
    }

    fn advance_encoded(&self, id: String, state: Box<RawValue>) -> PolicyFuture<'_, ()> {
        Box::pin(async move {
            let state = EncodedPayload::from_json(Arc::from(state.get()));
            self.owner.advance(id, state).await.map_err(agent_error)
        })
    }

    fn begin_step_encoded(
        &self,
        id: String,
        step_id: String,
        kind: String,
        input: Box<RawValue>,
    ) -> PolicyFuture<'_, Step> {
        Box::pin(async move {
            let begun = self
                .owner
                .begin_step(id, step_id, kind.clone(), &*input)
                .await;
            self.admitted(begun, &kind).await
        })
    }

    fn commit_writes<'a>(
        &'a self,
        id: String,
        writes: Vec<JournalWrite>,
        begin: Option<BeginRequest<'a>>,
    ) -> PolicyFuture<'a, Option<Step>> {
        Box::pin(async move {
            let mut fused = writes.iter().map(owner_write).collect::<Vec<_>>();
            let Some(begin) = begin else {
                if !fused.is_empty() {
                    self.owner
                        .commit_writes(id, fused)
                        .await
                        .map_err(agent_error)?;
                }
                return Ok(None);
            };
            fused.push(OwnerWrite::BeginStep {
                step_id: begin.step_id.clone(),
                kind: begin.kind.clone(),
                input: EncodedPayload::from_json(Arc::from(begin.input.get())),
            });
            let begun = match self.owner.commit_writes(id.clone(), fused).await {
                Ok(mut outcomes) => match outcomes.pop() {
                    Some(WriteOutcome::Began(begun)) => Ok(begun),
                    _ => Err(nanocode_durability::Error::InvalidState(
                        "a fused step admission returned no step outcome".to_owned(),
                    )),
                },
                Err(nanocode_durability::Error::InvalidState(message))
                    if let Some(wire) = begin.wire =>
                {
                    if !writes.is_empty() {
                        self.owner
                            .commit_writes(id.clone(), writes.iter().map(owner_write).collect())
                            .await
                            .map_err(agent_error)?;
                    }
                    let state = self.session.state().await.map_err(agent_error)?;
                    let Some(step) = state
                        .operation(&id)
                        .and_then(|operation| operation.steps.get(&begin.step_id))
                        .filter(|step| step.kind == begin.kind)
                    else {
                        return Err(agent_error(nanocode_durability::Error::InvalidState(
                            message,
                        )));
                    };
                    let digest = wire
                        .sha256_durable_input()
                        .map_err(crate::agent::provider_error)?;
                    let earlier = if EncodedPayload::encode(&*digest)
                        .map_err(agent_error)?
                        .matches_key(step.input.key())
                    {
                        digest
                    } else {
                        let legacy = wire
                            .legacy_durable_input()
                            .map_err(crate::agent::provider_error)?;
                        if !EncodedPayload::encode(&*legacy)
                            .map_err(agent_error)?
                            .matches_key(step.input.key())
                        {
                            return Err(agent_error(nanocode_durability::Error::InvalidState(
                                message,
                            )));
                        }
                        legacy
                    };
                    self.owner
                        .begin_step(id, begin.step_id, begin.kind.clone(), &*earlier)
                        .await
                }
                Err(error) => Err(error),
            };
            self.admitted(begun, &begin.kind).await.map(Some)
        })
    }

    fn defer_writes(&self, id: String, writes: Vec<JournalWrite>) -> PolicyFuture<'_, ()> {
        Box::pin(async move {
            for write in &writes {
                match owner_write(write) {
                    OwnerWrite::Advance { continuation } => {
                        self.owner.advance_deferred(id.clone(), continuation).await
                    }
                    OwnerWrite::CompleteStep { step_id, output } => {
                        self.owner
                            .complete_step_deferred_encoded(id.clone(), step_id, output)
                            .await
                    }
                    OwnerWrite::BeginStep { .. } => Err(nanocode_durability::Error::InvalidState(
                        "a deferred journal write cannot begin a step".to_owned(),
                    )),
                }
                .map_err(agent_error)?;
            }
            Ok(())
        })
    }

    fn advance(&self, id: String, state: Value) -> PolicyFuture<'_, ()> {
        Box::pin(async move {
            self.owner
                .advance(id, payload(&state)?)
                .await
                .map_err(agent_error)
        })
    }

    fn begin_step(
        &self,
        id: String,
        step_id: String,
        kind: String,
        input: Value,
    ) -> PolicyFuture<'_, Step> {
        Box::pin(async move {
            let begun = self
                .owner
                .begin_step(id, step_id, kind.clone(), &input)
                .await;
            self.admitted(begun, &kind).await
        })
    }

    fn complete_step(&self, id: String, step_id: String, output: Value) -> PolicyFuture<'_, ()> {
        Box::pin(async move {
            self.owner
                .complete_step(id, step_id, &output)
                .await
                .map_err(agent_error)
        })
    }

    fn complete(&self, id: String, checkpoint: Value, output: Value) -> PolicyFuture<'_, ()> {
        Box::pin(async move {
            self.owner
                .complete(id, payload(&checkpoint)?, &output)
                .await
                .map_err(agent_error)
        })
    }

    fn fail(&self, id: String, checkpoint: Value, error: String) -> PolicyFuture<'_, ()> {
        Box::pin(async move {
            self.owner
                .fail(id, payload(&checkpoint)?, error)
                .await
                .map_err(agent_error)
        })
    }

    fn fail_attempt(&self, id: String, _checkpoint: Value, error: String) -> PolicyFuture<'_, ()> {
        Box::pin(async move {
            self.owner
                .fail_attempt(id, error)
                .await
                .map_err(agent_error)
        })
    }

    fn cancel(&self, id: String, checkpoint: Value) -> PolicyFuture<'_, ()> {
        Box::pin(async move {
            self.owner
                .cancel(id, Some(payload(&checkpoint)?))
                .await
                .map_err(agent_error)
        })
    }

    fn pages_messages(&self) -> bool {
        true
    }

    fn settle_encoded(
        &self,
        id: String,
        checkpoint: EncodedState,
        settlement: Settlement,
    ) -> PolicyFuture<'_, ()> {
        Box::pin(async move {
            let checkpoint = EncodedPayload::from_json(Arc::from(checkpoint.json.get()))
                .with_payloads(checkpoint.records);
            match settlement {
                Settlement::Complete(output) => self.owner.complete(id, checkpoint, &*output).await,
                Settlement::Fail(error) => self.owner.fail(id, checkpoint, error).await,
                Settlement::FailAttempt(error) => self.owner.fail_attempt(id, error).await,
                Settlement::Cancel => self.owner.cancel(id, Some(checkpoint)).await,
            }
            .map_err(agent_error)
        })
    }

    fn cancel_unstarted(&self, id: String) -> PolicyFuture<'_, bool> {
        Box::pin(async move {
            match self.owner.cancel(id, None).await {
                Ok(()) => Ok(true),
                Err(nanocode_durability::Error::CancellationCheckpointRequired { .. }) => Ok(false),
                Err(error) => Err(agent_error(error)),
            }
        })
    }

    fn release(&self, id: String) -> PolicyFuture<'_, ()> {
        Box::pin(async move { self.owner.release_claim(id).await.map_err(agent_error) })
    }

    fn shutdown(&self) -> PolicyFuture<'_, ()> {
        Box::pin(async move { self.owner.shutdown().await.map_err(agent_error) })
    }

    fn developer_notices(&self, id: String) -> PolicyFuture<'_, Vec<(String, String)>> {
        Box::pin(async move {
            let state = self.session.state().await.map_err(agent_error)?;
            let operation = state.operation(&id).ok_or_else(|| {
                nanocode_agent_core::NanocodeError::InvalidCheckpoint(
                    "Claude notice operation is missing".into(),
                )
            })?;
            let mut notices = Vec::new();
            for (step_id, step) in &operation.steps {
                if step.kind != "developer_notice" {
                    continue;
                }
                let notice = self
                    .owner
                    .load_payload(step.input.clone())
                    .await
                    .map_err(agent_error)?
                    .decode()
                    .map_err(agent_error)?;
                if matches!(step.status, nanocode_durability::StepStatus::EffectPending) {
                    self.owner
                        .complete_step(id.clone(), step_id.clone(), &Value::Null)
                        .await
                        .map_err(agent_error)?;
                }
                notices.push(notice);
            }
            Ok(notices)
        })
    }

    fn checkpoint(&self, state: Value) -> PolicyFuture<'_, ()> {
        Box::pin(async move {
            self.owner
                .commit_checkpoint(payload(&state)?)
                .await
                .map_err(agent_error)
        })
    }
}

async fn hydrate(reader: PayloadReader<'_>, checkpoint: Value) -> AgentResult<Value> {
    format::hydrate_snapshot(reader, checkpoint, &mut HashSet::new())
        .await
        .map_err(agent_error)
}

fn owner_write(write: &JournalWrite) -> OwnerWrite {
    match write {
        JournalWrite::Advance(state) => OwnerWrite::Advance {
            continuation: EncodedPayload::from_json(Arc::from(state.json.get()))
                .with_payloads(state.records.clone()),
        },
        JournalWrite::Complete {
            step_id,
            output,
            records,
        } => OwnerWrite::CompleteStep {
            step_id: step_id.clone(),
            output: EncodedPayload::from_json(Arc::from(output.get()))
                .with_payloads(records.clone()),
        },
    }
}

impl ClaudeExecution {
    async fn admitted(
        &self,
        begun: nanocode_durability::Result<BeginStep>,
        kind: &str,
    ) -> AgentResult<Step> {
        match begun.map_err(agent_error)? {
            BeginStep::Execute => Ok(Step::Execute),
            BeginStep::Interrupted if kind == "tool" => Ok(Step::Interrupted),
            BeginStep::Interrupted => Ok(Step::Execute),
            BeginStep::Replay(value) if kind == "model" || kind == "model_call" => {
                format::hydrate_receipt(
                    PayloadReader::from(&self.owner),
                    value.decode().map_err(agent_error)?,
                )
                .await
                .map(Step::Replay)
                .map_err(agent_error)
            }
            BeginStep::Replay(value) => Ok(Step::Replay(value.decode().map_err(agent_error)?)),
        }
    }
}

fn payload(value: &Value) -> AgentResult<EncodedPayload> {
    EncodedPayload::encode(value).map_err(agent_error)
}

fn map_admission(admission: Admission<Value, Value>) -> ClaudeAdmission {
    match admission {
        Admission::Accepted => ClaudeAdmission::Execute,
        Admission::Pending => ClaudeAdmission::Resume,
        Admission::Completed { checkpoint, output } => {
            ClaudeAdmission::Completed { checkpoint, output }
        }
        Admission::Failed { checkpoint, error } => ClaudeAdmission::Failed { checkpoint, error },
        Admission::Cancelled => ClaudeAdmission::Cancelled,
    }
}
