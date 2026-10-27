use std::{
    collections::HashSet,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use arc_swap::{ArcSwap, ArcSwapOption};
use nanocode_sync::Slot;

use nanocode_agent_core::{
    ExecutionPolicyDisposition, NanocodeError, Result as AgentResult, SteerReceiptStatus,
};
use nanocode_durability::{
    Admission, BeginStep, Branches, DurableAgentExt, DurableOwner, DurableSession, EncodedPayload,
    InitialAcquisition, InitialSettings, JournalClaim, JournalSeed, OperationStatus, OwnerWrite,
    SessionRecord, SharedStore, WriteOutcome, agent_error,
};
use serde_json::{Value, value::RawValue};

use crate::{
    CodexBuilder,
    client::responses::{ResponseHistory, ResponseItem},
    execution::{
        ExecutionAdmission, ExecutionContinuation, ExecutionFuture, ExecutionOutput,
        ExecutionPolicy, ExecutionSteer, ExecutionStepAdmission, IdentifiedExecutionSteer,
    },
    session::{SessionId, SessionSnapshot},
};

mod context;
mod format;
mod registration;
mod session;

pub use format::CodexCheckpointFormat;
pub use session::DurableSessionExt;

const MAINTENANCE_KINDS: &[&str] = &["standalone_compaction"];

impl<F> DurableAgentExt for CodexBuilder<F> {
    async fn durability(self, state: DurableSession) -> AgentResult<Self> {
        let state_id = state.state_id().to_owned();
        let (owner, checkpoint) = state
            .acquire_agent_with_initial(InitialSettings {
                thinking: self.config.thinking,
                service_tier: self.config.service_tier,
            })
            .await
            .map_err(agent_error)?;
        owner
            .abandon_stale_kinds(MAINTENANCE_KINDS)
            .await
            .map_err(agent_error)?;
        let record = state.record().await.map_err(agent_error)?;
        let mut builder = self.child_journal(state.child_journal());

        if builder.explicit_session_id().is_none()
            && let Ok(session_id) = state_id.parse::<SessionId>()
        {
            builder = builder.session_id(session_id);
        }
        if let Some(record) = &record {
            builder = builder.lineage(record.lineage.clone());
        }
        let initial =
            record
                .as_ref()
                .and_then(|record| match (record.model.as_codex(), record.initial) {
                    (Some(model), Some(initial)) => Some((model, initial)),
                    _ => None,
                });
        let branches = Branches {
            store: state.shared_store(),
            record,
        };
        let mut known_records = HashSet::new();
        if let Some(checkpoint) = checkpoint {
            let preloaded = state
                .take_checkpoint_preload_with_records(checkpoint.key())
                .and_then(|(preloaded, records)| {
                    context::preloaded_snapshot(preloaded, records, checkpoint.key())
                });
            let (restored, keys) = match preloaded {
                Some(preloaded) => preloaded,
                None => context::load_snapshot_with_keys(
                    (&owner).into(),
                    checkpoint.decode().map_err(agent_error)?,
                )
                .await
                .map_err(agent_error)?,
            };
            if let Some(configured) = builder.resume_snapshot()
                && serde_json::to_string(configured)
                    .map_err(|error| NanocodeError::InvalidCheckpoint(error.to_string()))?
                    != serde_json::to_string(&restored)
                        .map_err(|error| NanocodeError::InvalidCheckpoint(error.to_string()))?
            {
                return Err(NanocodeError::InvalidCheckpoint(
                    "configured resume snapshot does not match the durability state".to_owned(),
                ));
            }
            known_records = keys;
            builder = builder.resume_native_snapshot(restored);
        } else if let Some(snapshot) = builder.resume_snapshot() {
            let prepared =
                context::prepare_snapshot(snapshot.clone(), &known_records).map_err(agent_error)?;
            if state
                .state()
                .await
                .map_err(agent_error)?
                .pending_operations()
                .is_empty()
            {
                owner
                    .commit_checkpoint(prepared.payload)
                    .await
                    .map_err(agent_error)?;
                known_records = prepared.keys;
            }
        } else {
            if let Some((model, initial)) = initial {
                builder = builder.initial_settings(model, initial.thinking, initial.service_tier);
            }

            builder = builder.default_prompt_cache_key(state_id.clone());
        }
        let owner = Arc::new(Slot::new(Some((owner, known_records, branches))));
        Ok(builder
            .execution_policy_factory(move || {
                let (owner, keys, branches) = owner
                    .take()
                    .ok_or_else(|| {
                        NanocodeError::InvalidExecutionPolicy(
                            "a durability-attached builder can build only one agent; attach durability again to reopen the state"
                                .to_owned(),
                        )
                    })?;
                let policy = DurableExecution::ready(owner, state_id.clone(), Some(branches));
                policy.remember(keys)?;
                let policy: Arc<dyn ExecutionPolicy> = Arc::new(policy);
                Ok(policy)
            }) )
    }
}

impl<F> CodexBuilder<F> {
    pub(crate) fn deferred_durability(
        self,
        state: &DurableSession,
        record: SessionRecord,
        notice: String,
    ) -> (Self, Arc<AtomicBool>, JournalSeed) {
        let state_id = state.state_id().to_owned();
        let (journal, seed) = state.seeded_child_journal();
        let mut builder = self.child_journal(journal);
        if builder.explicit_session_id().is_none()
            && let Ok(session_id) = state_id.parse::<SessionId>()
        {
            builder = builder.session_id(session_id);
        }
        let existing = Arc::new(AtomicBool::new(false));
        let builder = builder
            .lineage(record.lineage.clone())
            .default_prompt_cache_key(state_id)
            .initial_developer_messages(vec![notice])
            .execution_policy(Arc::new(LazyExecution::deferred(
                state.shared_store(),
                record,
                Arc::clone(&existing),
                seed.claim(),
            )));
        (builder, existing, seed)
    }
}

struct DurableExecution {
    owner: DurableOwner,
    state_id: String,
    branches: Option<Branches>,
    context_records: ArcSwap<HashSet<String>>,
    continuation_cache: Slot<context::ContinuationCache>,
}

impl DurableExecution {
    fn ready(owner: DurableOwner, state_id: String, branches: Option<Branches>) -> Self {
        Self {
            owner,
            state_id,
            branches,
            context_records: ArcSwap::default(),
            continuation_cache: Slot::default(),
        }
    }

    async fn commit_snapshot<T, F: Future<Output = nanocode_durability::Result<T>>>(
        &self,
        prepare: impl FnOnce(&mut context::ContinuationCache) -> nanocode_durability::Result<F>,
    ) -> AgentResult<T> {
        let mut cache = self.continuation_cache.take().unwrap_or_default();
        let committed = match prepare(&mut cache) {
            Ok(commit) => commit.await,
            Err(error) => Err(error),
        };
        if committed.is_ok() {
            cache.committed();
        }
        drop(self.continuation_cache.put(cache));
        committed.map_err(agent_error)
    }

    fn remember(&self, keys: HashSet<String>) -> AgentResult<()> {
        self.context_records.store(Arc::new(keys));
        Ok(())
    }
}

async fn step_admission(
    owner: &DurableOwner,
    kind: &str,
    step: BeginStep,
) -> AgentResult<ExecutionStepAdmission> {
    match step {
        BeginStep::Execute => Ok(ExecutionStepAdmission::Execute),
        BeginStep::Interrupted if kind == "tool_call" => Ok(ExecutionStepAdmission::Interrupted),
        BeginStep::Interrupted => Ok(ExecutionStepAdmission::Execute),
        BeginStep::Replay(output) => {
            let json = output.json().map_err(agent_error)?.to_owned();
            let json = if kind == "model_call" {
                context::restore_receipt(owner.into(), json)
                    .await
                    .map_err(agent_error)?
            } else {
                json
            };
            Ok(ExecutionStepAdmission::Replay(json))
        }
    }
}

impl ExecutionPolicy for DurableExecution {
    fn durable_state_id(&self) -> Option<String> {
        Some(self.state_id.clone())
    }

    fn branch(
        &self,
        child: &nanocode_agent_core::SessionInfo,
    ) -> AgentResult<Option<Arc<dyn ExecutionPolicy>>> {
        let Some(branches) = self
            .branches
            .as_ref()
            .filter(|branches| branches.record.is_some())
        else {
            return Ok(None);
        };
        let record = branches.child_record(child)?;
        Ok(Some(Arc::new(LazyExecution::new(
            branches.store.clone(),
            record,
        ))))
    }

    fn recover_failure<'a>(
        &'a self,
        operation_id: String,
        error: NanocodeError,
    ) -> ExecutionFuture<'a, NanocodeError> {
        Box::pin(async move {
            if matches!(
                error.execution_policy_disposition(),
                Some(ExecutionPolicyDisposition::Reopen | ExecutionPolicyDisposition::Fatal)
            ) {
                return error;
            }
            let owner = &self.owner;
            match owner.recover_failure(operation_id).await {
                Ok(Some(OperationStatus::Failed { error, .. })) => {
                    NanocodeError::ReplayedExecutionFailed(error)
                }
                Ok(Some(OperationStatus::Cancelled { .. })) => NanocodeError::TurnCancelled,

                Ok(Some(OperationStatus::Pending | OperationStatus::Completed { .. })) => {
                    if error.execution_policy_disposition()
                        == Some(ExecutionPolicyDisposition::Retry)
                    {
                        return error;
                    }
                    NanocodeError::execution_policy_with_disposition(
                        "durable operation recovery",
                        ExecutionPolicyDisposition::Retry,
                        error,
                    )
                }

                Ok(None) => error,
                Err(error) => agent_error(error),
            }
        })
    }

    fn shutdown<'a>(&'a self) -> ExecutionFuture<'a, AgentResult<()>> {
        Box::pin(async move { self.owner.shutdown().await.map_err(agent_error) })
    }

    fn commit_checkpoint<'a>(
        &'a self,
        snapshot: SessionSnapshot,
    ) -> ExecutionFuture<'a, AgentResult<()>> {
        Box::pin(async move {
            self.commit_snapshot(|cache| {
                let checkpoint = context::prepare_cached_snapshot(snapshot, cache)?;
                Ok(self.owner.commit_checkpoint(checkpoint))
            })
            .await
            .map(drop)
        })
    }

    fn admit<'a>(
        &'a self,
        operation_id: String,
        input_json: String,
    ) -> ExecutionFuture<'a, AgentResult<ExecutionAdmission>> {
        Box::pin(async move {
            let input = raw(input_json)?;
            let owner = &self.owner;
            let admission = owner
                .admit_typed::<_, context::Snapshot, Value>(operation_id, &input)
                .await
                .map_err(agent_error)?;
            map_admission(owner, admission).await
        })
    }

    fn admit_automatic<'a>(
        &'a self,
        candidate_operation_id: String,
        input_json: String,
    ) -> ExecutionFuture<'a, AgentResult<(String, ExecutionAdmission)>> {
        Box::pin(async move {
            let input = raw(input_json)?;
            let admission = self
                .owner
                .admit_automatic_typed::<_, context::Snapshot, Value>(
                    candidate_operation_id,
                    &input,
                )
                .await
                .map_err(agent_error)?;
            let (operation_id, admission) = admission.into_parts();
            Ok((operation_id, map_admission(&self.owner, admission).await?))
        })
    }

    fn release<'a>(&'a self, operation_id: String) -> ExecutionFuture<'a, ()> {
        Box::pin(async move {
            let _ = self.owner.release_claim(operation_id).await;
        })
    }

    fn cancel<'a>(
        &'a self,
        operation_id: String,
        snapshot: Option<SessionSnapshot>,
    ) -> ExecutionFuture<'a, AgentResult<()>> {
        Box::pin(async move {
            self.commit_snapshot(|cache| {
                let checkpoint = snapshot
                    .map(|snapshot| context::prepare_cached_snapshot(snapshot, cache))
                    .transpose()?;
                Ok(self.owner.cancel(operation_id, checkpoint))
            })
            .await
        })
    }

    fn begin_attempt<'a>(&'a self, operation_id: String) -> ExecutionFuture<'a, AgentResult<()>> {
        Box::pin(async move {
            self.owner
                .begin_attempt(operation_id)
                .await
                .map(|_| ())
                .map_err(agent_error)
        })
    }

    fn accept_steer<'a>(
        &'a self,
        operation_id: String,
        accepted_after_model_call_index: u32,
        input_json: String,
    ) -> ExecutionFuture<'a, AgentResult<u32>> {
        Box::pin(async move {
            let input = raw(input_json)?;
            self.owner
                .accept_steer(operation_id, accepted_after_model_call_index, &input, None)
                .await
                .map(|index| index.expect("unidentified steer is new"))
                .map_err(agent_error)
        })
    }

    fn supports_steer_receipts(&self) -> bool {
        true
    }

    fn identified_steer_accepted<'a>(
        &'a self,
        message_id: String,
        input_json: String,
    ) -> ExecutionFuture<'a, AgentResult<bool>> {
        Box::pin(async move {
            self.owner
                .identified_steer_accepted(message_id, &raw(input_json)?)
                .await
                .map_err(agent_error)
        })
    }

    fn steer_receipt<'a>(
        &'a self,
        message_id: String,
        input_json: String,
    ) -> ExecutionFuture<'a, AgentResult<SteerReceiptStatus>> {
        Box::pin(async move {
            self.owner
                .steer_receipt(message_id, &raw(input_json)?)
                .await
                .map_err(agent_error)
        })
    }

    fn accept_identified_steer<'a>(
        &'a self,
        operation_id: String,
        message_id: String,
        accepted_after_model_call_index: u32,
        input_json: String,
    ) -> ExecutionFuture<'a, AgentResult<Option<u32>>> {
        Box::pin(async move {
            let input = raw(input_json)?;
            self.owner
                .accept_steer(
                    operation_id,
                    accepted_after_model_call_index,
                    &input,
                    Some(message_id),
                )
                .await
                .map_err(agent_error)
        })
    }

    fn retained_steers<'a>(
        &'a self,
        operation_id: String,
    ) -> ExecutionFuture<'a, AgentResult<Vec<ExecutionSteer>>> {
        Box::pin(async move {
            self.retained_identified_steers(operation_id)
                .await
                .map(|steers| steers.into_iter().map(|(_, steer)| steer).collect())
        })
    }

    fn retained_identified_steers<'a>(
        &'a self,
        operation_id: String,
    ) -> ExecutionFuture<'a, AgentResult<Vec<IdentifiedExecutionSteer>>> {
        Box::pin(async move {
            self.owner
                .retained_steers(operation_id)
                .await
                .and_then(|steers| {
                    steers
                        .into_iter()
                        .map(|steer| {
                            Ok((
                                steer.state.message_id,
                                ExecutionSteer {
                                    index: steer.index,
                                    accepted_after_model_call_index: steer
                                        .state
                                        .accepted_after_model_call_index,
                                    model_call_index: steer.state.model_call_index,
                                    input_json: steer.state.input.json()?.to_owned(),
                                },
                            ))
                        })
                        .collect()
                })
                .map_err(agent_error)
        })
    }

    fn withdraw_steer<'a>(
        &'a self,
        operation_id: String,
        steer_index: u32,
    ) -> ExecutionFuture<'a, AgentResult<()>> {
        Box::pin(async move {
            self.owner
                .withdraw_steer(operation_id, steer_index)
                .await
                .map_err(agent_error)
        })
    }

    fn bind_steer<'a>(
        &'a self,
        operation_id: String,
        steer_index: u32,
        model_call_index: u32,
    ) -> ExecutionFuture<'a, AgentResult<()>> {
        Box::pin(async move {
            self.owner
                .bind_steer(operation_id, steer_index, model_call_index)
                .await
                .map_err(agent_error)
        })
    }

    fn continuation<'a>(
        &'a self,
        operation_id: String,
    ) -> ExecutionFuture<'a, AgentResult<Option<ExecutionContinuation>>> {
        Box::pin(async move {
            let owner = &self.owner;
            match owner
                .continuation(operation_id)
                .await
                .map_err(agent_error)?
            {
                Some(value) => {
                    let (continuation, keys) = context::load_continuation(owner.into(), value)
                        .await
                        .map_err(agent_error)?;
                    self.remember(keys)?;
                    Ok(Some(continuation))
                }
                None => Ok(None),
            }
        })
    }

    fn advance<'a>(
        &'a self,
        operation_id: String,
        continuation: ExecutionContinuation,
    ) -> ExecutionFuture<'a, AgentResult<()>> {
        Box::pin(async move {
            let prepared =
                context::prepare_continuation(continuation, &self.context_records.load())
                    .map_err(agent_error)?;
            self.owner
                .advance(operation_id, prepared.payload)
                .await
                .map_err(agent_error)?;
            self.remember(prepared.keys)
        })
    }

    fn begin_step<'a>(
        &'a self,
        operation_id: String,
        step_id: String,
        kind: String,
        input_json: String,
    ) -> ExecutionFuture<'a, AgentResult<ExecutionStepAdmission>> {
        Box::pin(async move {
            let input = raw(input_json)?;
            let step = self
                .owner
                .begin_step(operation_id, step_id, kind.clone(), &input)
                .await
                .map_err(agent_error)?;
            step_admission(&self.owner, &kind, step).await
        })
    }

    fn advance_history<'a>(
        &'a self,
        operation_id: String,
        state_json: String,
        history: ResponseHistory,
        prefix: Arc<[ResponseItem]>,
        next_step: Option<(String, String, String)>,
    ) -> ExecutionFuture<'a, AgentResult<Option<ExecutionStepAdmission>>> {
        Box::pin(async move {
            let mut cache = self.continuation_cache.take().unwrap_or_default();
            let continuation = match context::prepare_cached_continuation(
                state_json, &history, &prefix, &mut cache,
            ) {
                Ok(continuation) => continuation,
                Err(error) => {
                    drop(self.continuation_cache.put(cache));
                    return Err(agent_error(error));
                }
            };
            let mut writes = vec![OwnerWrite::Advance { continuation }];
            let kind = match next_step {
                Some((step_id, kind, input_json)) => {
                    writes.push(OwnerWrite::BeginStep {
                        step_id,
                        kind: kind.clone(),
                        input: EncodedPayload::encode(&raw(input_json)?).map_err(agent_error)?,
                    });
                    Some(kind)
                }
                None => None,
            };
            let outcomes = self.owner.commit_writes(operation_id, writes).await;
            if outcomes.is_ok() {
                cache.committed();
            }
            drop(self.continuation_cache.put(cache));
            let outcomes = outcomes.map_err(agent_error)?;
            let Some(kind) = kind else {
                return Ok(None);
            };
            match outcomes.into_iter().last() {
                Some(WriteOutcome::Began(step)) => {
                    step_admission(&self.owner, &kind, step).await.map(Some)
                }
                _ => Err(NanocodeError::InvalidExecutionPolicy(
                    "fused durable write did not report the begun step".into(),
                )),
            }
        })
    }

    fn complete_step<'a>(
        &'a self,
        operation_id: String,
        step_id: String,
        output_json: String,
    ) -> ExecutionFuture<'a, AgentResult<()>> {
        Box::pin(async move {
            let output = raw(output_json)?;
            self.owner
                .complete_step(operation_id, step_id, &output)
                .await
                .map_err(agent_error)
        })
    }

    fn complete_model_step<'a>(
        &'a self,
        operation_id: String,
        step_id: String,
        output_json: String,
    ) -> ExecutionFuture<'a, AgentResult<()>> {
        Box::pin(async move {
            let output = context::prepare_receipt(&output_json).map_err(agent_error)?;
            self.owner
                .complete_step_deferred_encoded(operation_id, step_id, output)
                .await
                .map_err(agent_error)
        })
    }

    fn complete_step_deferred<'a>(
        &'a self,
        operation_id: String,
        step_id: String,
        output_json: String,
    ) -> ExecutionFuture<'a, AgentResult<()>> {
        Box::pin(async move {
            let output = raw(output_json)?;
            self.owner
                .complete_step_deferred(operation_id, step_id, &output)
                .await
                .map_err(agent_error)
        })
    }

    fn advance_history_deferred<'a>(
        &'a self,
        operation_id: String,
        state_json: String,
        history: ResponseHistory,
        prefix: Arc<[ResponseItem]>,
    ) -> ExecutionFuture<'a, AgentResult<()>> {
        Box::pin(async move {
            let mut cache = self.continuation_cache.take().unwrap_or_default();
            let continuation =
                context::prepare_cached_continuation(state_json, &history, &prefix, &mut cache);
            drop(self.continuation_cache.put(cache));
            self.owner
                .advance_deferred(operation_id, continuation.map_err(agent_error)?)
                .await
                .map_err(agent_error)
        })
    }

    fn complete<'a>(
        &'a self,
        operation_id: String,
        snapshot: SessionSnapshot,
        output: ExecutionOutput,
    ) -> ExecutionFuture<'a, AgentResult<()>> {
        Box::pin(async move {
            self.commit_snapshot(|cache| {
                let (checkpoint, output) =
                    context::prepare_cached_completion(snapshot, &output, cache)?;
                Ok(async move {
                    self.owner
                        .complete(operation_id, checkpoint, &*output)
                        .await
                })
            })
            .await
        })
    }

    fn fail_attempt<'a>(
        &'a self,
        operation_id: String,
        error: String,
    ) -> ExecutionFuture<'a, AgentResult<()>> {
        Box::pin(async move {
            self.owner
                .fail_attempt(operation_id, error)
                .await
                .map_err(agent_error)
        })
    }

    fn fail<'a>(
        &'a self,
        operation_id: String,
        snapshot: SessionSnapshot,
        error: String,
    ) -> ExecutionFuture<'a, AgentResult<()>> {
        Box::pin(async move {
            self.commit_snapshot(|cache| {
                let checkpoint = context::prepare_cached_snapshot(snapshot, cache)?;
                Ok(self.owner.fail(operation_id, checkpoint, error))
            })
            .await
        })
    }
}

struct LazyExecution {
    store: SharedStore,
    record: SessionRecord,
    ready: tokio::sync::OnceCell<DurableExecution>,

    reopened: std::sync::atomic::AtomicBool,

    initialized: std::sync::atomic::AtomicBool,

    created_with: ArcSwapOption<crate::execution::InitialSettings>,

    existing: Option<Arc<AtomicBool>>,

    attempted: AtomicBool,

    journal: Option<JournalClaim>,
}

impl LazyExecution {
    fn new(store: SharedStore, record: SessionRecord) -> Self {
        Self {
            store,
            record,
            ready: tokio::sync::OnceCell::new(),
            reopened: std::sync::atomic::AtomicBool::new(false),
            initialized: std::sync::atomic::AtomicBool::new(false),
            created_with: ArcSwapOption::empty(),
            existing: None,
            attempted: AtomicBool::new(false),
            journal: None,
        }
    }

    fn deferred(
        store: SharedStore,
        record: SessionRecord,
        existing: Arc<AtomicBool>,
        journal: JournalClaim,
    ) -> Self {
        Self {
            existing: Some(existing),
            journal: Some(journal),
            ..Self::new(store, record)
        }
    }

    async fn get(&self) -> AgentResult<&DurableExecution> {
        self.acquire(None).await.map(|(policy, _)| policy)
    }

    async fn acquire(
        &self,
        initial: Option<EncodedPayload>,
    ) -> AgentResult<(&DurableExecution, bool)> {
        let committed = AtomicBool::new(false);
        let policy = self
            .ready
            .get_or_try_init(|| async {
                let mut record = self.record.clone();
                let mut initial_settings = None;
                if let Some(settings) = self.created_with.load_full().map(|settings| *settings) {
                    record.model = nanocode_agent_core::ModelSelection::new(
                        nanocode_agent_core::BackendId::Codex,
                        settings.model.as_str(),
                    );
                    initial_settings = Some(InitialSettings {
                        thinking: settings.thinking,
                        service_tier: settings.service_tier,
                    });
                    if self.existing.is_none() {
                        record.initial = initial_settings;
                    }
                }
                let settings = initial_settings;
                let state = DurableSession::prepare_shared_described(
                    self.store.clone(),
                    record.session_id.clone(),
                    None,
                    Some(record),
                )
                .await
                .map_err(agent_error)?;
                let (owner, checkpoint) = match initial {
                    Some(payload) => {
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
                    None => state.acquire_agent().await.map_err(agent_error)?,
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
                Ok::<_, NanocodeError>(DurableExecution::ready(
                    owner,
                    record.session_id.clone(),
                    Some(Branches {
                        store: self.store.clone(),
                        record: Some(record),
                    }),
                ))
            })
            .await?;
        Ok((policy, committed.load(Ordering::SeqCst)))
    }
}

impl ExecutionPolicy for LazyExecution {
    fn durable_state_id(&self) -> Option<String> {
        Some(self.record.session_id.clone())
    }

    fn commits_initial(&self) -> bool {
        self.existing.is_some()
    }

    fn branch(
        &self,
        child: &nanocode_agent_core::SessionInfo,
    ) -> AgentResult<Option<Arc<dyn ExecutionPolicy>>> {
        let branches = Branches {
            store: self.store.clone(),
            record: Some(self.record.clone()),
        };
        Ok(Some(Arc::new(Self::new(
            self.store.clone(),
            branches.child_record(child)?,
        ))))
    }

    fn recover_failure<'a>(
        &'a self,
        operation_id: String,
        error: NanocodeError,
    ) -> ExecutionFuture<'a, NanocodeError> {
        Box::pin(async move {
            match self.get().await {
                Ok(policy) => policy.recover_failure(operation_id, error).await,
                Err(_) => error,
            }
        })
    }

    fn shutdown<'a>(&'a self) -> ExecutionFuture<'a, AgentResult<()>> {
        Box::pin(async move {
            match self.ready.get() {
                Some(policy) => policy.shutdown().await,
                None => Ok(()),
            }
        })
    }

    fn commit_checkpoint<'a>(
        &'a self,
        snapshot: SessionSnapshot,
    ) -> ExecutionFuture<'a, AgentResult<()>> {
        Box::pin(async move { self.get().await?.commit_checkpoint(snapshot).await })
    }

    fn commit_initial_checkpoint<'a>(
        &'a self,
        snapshot: Option<SessionSnapshot>,
        settings: crate::execution::InitialSettings,
    ) -> ExecutionFuture<'a, AgentResult<()>> {
        Box::pin(async move {
            if self.initialized.load(std::sync::atomic::Ordering::SeqCst) {
                return Ok(());
            }
            if self.existing.is_some() && self.attempted.swap(true, Ordering::SeqCst) {
                return Err(NanocodeError::InvalidExecutionPolicy(
                    "the deferred durable acquisition already failed".to_owned(),
                ));
            }
            self.created_with.store(Some(Arc::new(settings)));

            let (snapshot, prepared) = match snapshot {
                Some(snapshot) if self.ready.get().is_none() => (
                    None,
                    Some(
                        context::prepare_snapshot(snapshot, &HashSet::new())
                            .map_err(agent_error)?,
                    ),
                ),
                snapshot => (snapshot, None),
            };
            let (policy, committed) = self
                .acquire(prepared.as_ref().map(|prepared| prepared.payload.clone()))
                .await?;
            if let Some(existing) = self.existing.as_ref().filter(|_| !committed) {
                existing.store(true, Ordering::SeqCst);
                return Err(NanocodeError::InvalidExecutionPolicy(
                    "the prepared durable session already holds state".to_owned(),
                ));
            }
            if self.reopened.load(std::sync::atomic::Ordering::SeqCst) {
                return Ok(());
            }
            self.initialized
                .store(true, std::sync::atomic::Ordering::SeqCst);
            if let Some(prepared) = prepared {
                if !committed {
                    policy
                        .owner
                        .commit_checkpoint(prepared.payload)
                        .await
                        .map_err(agent_error)?;
                }
                return policy.remember(prepared.keys);
            }
            match snapshot {
                Some(snapshot) => policy.commit_checkpoint(snapshot).await,
                None => Ok(()),
            }
        })
    }

    fn discard_initial_checkpoint<'a>(&'a self) -> ExecutionFuture<'a, AgentResult<()>> {
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

    fn admit<'a>(
        &'a self,
        operation_id: String,
        input_json: String,
    ) -> ExecutionFuture<'a, AgentResult<ExecutionAdmission>> {
        Box::pin(async move { self.get().await?.admit(operation_id, input_json).await })
    }

    fn admit_automatic<'a>(
        &'a self,
        candidate_operation_id: String,
        input_json: String,
    ) -> ExecutionFuture<'a, AgentResult<(String, ExecutionAdmission)>> {
        Box::pin(async move {
            self.get()
                .await?
                .admit_automatic(candidate_operation_id, input_json)
                .await
        })
    }

    fn release<'a>(&'a self, operation_id: String) -> ExecutionFuture<'a, ()> {
        Box::pin(async move {
            if let Some(policy) = self.ready.get() {
                policy.release(operation_id).await;
            }
        })
    }

    fn cancel<'a>(
        &'a self,
        operation_id: String,
        snapshot: Option<SessionSnapshot>,
    ) -> ExecutionFuture<'a, AgentResult<()>> {
        Box::pin(async move { self.get().await?.cancel(operation_id, snapshot).await })
    }

    fn begin_attempt<'a>(&'a self, operation_id: String) -> ExecutionFuture<'a, AgentResult<()>> {
        Box::pin(async move { self.get().await?.begin_attempt(operation_id).await })
    }

    fn accept_steer<'a>(
        &'a self,
        operation_id: String,
        accepted_after_model_call_index: u32,
        input_json: String,
    ) -> ExecutionFuture<'a, AgentResult<u32>> {
        Box::pin(async move {
            self.get()
                .await?
                .accept_steer(operation_id, accepted_after_model_call_index, input_json)
                .await
        })
    }

    fn supports_steer_receipts(&self) -> bool {
        true
    }

    fn identified_steer_accepted<'a>(
        &'a self,
        message_id: String,
        input_json: String,
    ) -> ExecutionFuture<'a, AgentResult<bool>> {
        Box::pin(async move {
            self.get()
                .await?
                .identified_steer_accepted(message_id, input_json)
                .await
        })
    }

    fn steer_receipt<'a>(
        &'a self,
        message_id: String,
        input_json: String,
    ) -> ExecutionFuture<'a, AgentResult<SteerReceiptStatus>> {
        Box::pin(async move {
            self.get()
                .await?
                .steer_receipt(message_id, input_json)
                .await
        })
    }

    fn accept_identified_steer<'a>(
        &'a self,
        operation_id: String,
        message_id: String,
        accepted_after_model_call_index: u32,
        input_json: String,
    ) -> ExecutionFuture<'a, AgentResult<Option<u32>>> {
        Box::pin(async move {
            self.get()
                .await?
                .accept_identified_steer(
                    operation_id,
                    message_id,
                    accepted_after_model_call_index,
                    input_json,
                )
                .await
        })
    }

    fn retained_steers<'a>(
        &'a self,
        operation_id: String,
    ) -> ExecutionFuture<'a, AgentResult<Vec<ExecutionSteer>>> {
        Box::pin(async move { self.get().await?.retained_steers(operation_id).await })
    }

    fn retained_identified_steers<'a>(
        &'a self,
        operation_id: String,
    ) -> ExecutionFuture<'a, AgentResult<Vec<IdentifiedExecutionSteer>>> {
        Box::pin(async move {
            self.get()
                .await?
                .retained_identified_steers(operation_id)
                .await
        })
    }

    fn withdraw_steer<'a>(
        &'a self,
        operation_id: String,
        steer_index: u32,
    ) -> ExecutionFuture<'a, AgentResult<()>> {
        Box::pin(async move {
            self.get()
                .await?
                .withdraw_steer(operation_id, steer_index)
                .await
        })
    }

    fn bind_steer<'a>(
        &'a self,
        operation_id: String,
        steer_index: u32,
        model_call_index: u32,
    ) -> ExecutionFuture<'a, AgentResult<()>> {
        Box::pin(async move {
            self.get()
                .await?
                .bind_steer(operation_id, steer_index, model_call_index)
                .await
        })
    }

    fn continuation<'a>(
        &'a self,
        operation_id: String,
    ) -> ExecutionFuture<'a, AgentResult<Option<ExecutionContinuation>>> {
        Box::pin(async move { self.get().await?.continuation(operation_id).await })
    }

    fn advance<'a>(
        &'a self,
        operation_id: String,
        continuation: ExecutionContinuation,
    ) -> ExecutionFuture<'a, AgentResult<()>> {
        Box::pin(async move { self.get().await?.advance(operation_id, continuation).await })
    }

    fn advance_history<'a>(
        &'a self,
        operation_id: String,
        state_json: String,
        history: ResponseHistory,
        prefix: Arc<[ResponseItem]>,
        next_step: Option<(String, String, String)>,
    ) -> ExecutionFuture<'a, AgentResult<Option<ExecutionStepAdmission>>> {
        Box::pin(async move {
            self.get()
                .await?
                .advance_history(operation_id, state_json, history, prefix, next_step)
                .await
        })
    }

    fn begin_step<'a>(
        &'a self,
        operation_id: String,
        step_id: String,
        kind: String,
        input_json: String,
    ) -> ExecutionFuture<'a, AgentResult<ExecutionStepAdmission>> {
        Box::pin(async move {
            self.get()
                .await?
                .begin_step(operation_id, step_id, kind, input_json)
                .await
        })
    }

    fn complete_step<'a>(
        &'a self,
        operation_id: String,
        step_id: String,
        output_json: String,
    ) -> ExecutionFuture<'a, AgentResult<()>> {
        Box::pin(async move {
            self.get()
                .await?
                .complete_step(operation_id, step_id, output_json)
                .await
        })
    }

    fn complete_model_step<'a>(
        &'a self,
        operation_id: String,
        step_id: String,
        output_json: String,
    ) -> ExecutionFuture<'a, AgentResult<()>> {
        Box::pin(async move {
            self.get()
                .await?
                .complete_model_step(operation_id, step_id, output_json)
                .await
        })
    }

    fn complete_step_deferred<'a>(
        &'a self,
        operation_id: String,
        step_id: String,
        output_json: String,
    ) -> ExecutionFuture<'a, AgentResult<()>> {
        Box::pin(async move {
            self.get()
                .await?
                .complete_step_deferred(operation_id, step_id, output_json)
                .await
        })
    }

    fn advance_history_deferred<'a>(
        &'a self,
        operation_id: String,
        state_json: String,
        history: ResponseHistory,
        prefix: Arc<[ResponseItem]>,
    ) -> ExecutionFuture<'a, AgentResult<()>> {
        Box::pin(async move {
            self.get()
                .await?
                .advance_history_deferred(operation_id, state_json, history, prefix)
                .await
        })
    }

    fn complete<'a>(
        &'a self,
        operation_id: String,
        snapshot: SessionSnapshot,
        output: ExecutionOutput,
    ) -> ExecutionFuture<'a, AgentResult<()>> {
        Box::pin(async move {
            self.get()
                .await?
                .complete(operation_id, snapshot, output)
                .await
        })
    }

    fn fail_attempt<'a>(
        &'a self,
        operation_id: String,
        error: String,
    ) -> ExecutionFuture<'a, AgentResult<()>> {
        Box::pin(async move { self.get().await?.fail_attempt(operation_id, error).await })
    }

    fn fail<'a>(
        &'a self,
        operation_id: String,
        snapshot: SessionSnapshot,
        error: String,
    ) -> ExecutionFuture<'a, AgentResult<()>> {
        Box::pin(async move { self.get().await?.fail(operation_id, snapshot, error).await })
    }
}

async fn map_admission(
    owner: &DurableOwner,
    admission: Admission<context::Snapshot, Value>,
) -> AgentResult<ExecutionAdmission> {
    Ok(match admission {
        Admission::Accepted => ExecutionAdmission::Execute,
        Admission::Pending => ExecutionAdmission::Resume,
        Admission::Completed { checkpoint, output } => ExecutionAdmission::Completed {
            snapshot: context::restore_snapshot(owner.into(), checkpoint)
                .await
                .map_err(agent_error)?,
            output: context::restore_output(owner.into(), output)
                .await
                .map_err(agent_error)?,
        },
        Admission::Failed { checkpoint, error } => ExecutionAdmission::Failed {
            snapshot: context::restore_snapshot(owner.into(), checkpoint)
                .await
                .map_err(agent_error)?,
            error,
        },
        Admission::Cancelled => ExecutionAdmission::Cancelled,
    })
}

fn raw(json: String) -> AgentResult<Box<RawValue>> {
    RawValue::from_string(json).map_err(NanocodeError::ExecutionPayload)
}
