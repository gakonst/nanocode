mod branch;
mod control;
mod telemetry;

use super::execution::{
    AdmittedExecution, ExecutionFuture, ExecutionTurn, SteerDelivery, SteerQueue, SteerReceipt,
    SteerSender,
};
use super::spawn::{validate_model_reasoning_mode, validate_model_thinking};
use super::*;
pub(super) use branch::{AgentOrigin, BranchSpawner};
pub(super) use control::DriverShutdown;
use control::{
    TurnDefaults, begin_shutdown, cancel_queued_turn, handle_idle_command, model_change_locked,
    queued_execution_operation, queued_prompt,
};
use telemetry::{ReasoningSettings, agent_compact_span, agent_turn_span, emit_replayed_terminal};

pub(super) struct AgentDriver {
    pub(super) commands: mpsc::Receiver<Command>,
    pub(super) events: EventSink,
    pub(super) client: ResponsesClientBox,
    pub(super) transport_stats: Arc<TransportStats>,
    pub(super) tools: Tools,
    pub(super) workspace: Option<Arc<str>>,
    pub(super) spawner: BranchSpawner,
    pub(super) initial_model: Option<PreparedCheckpoint>,
    pub(super) origin: AgentOrigin,
    pub(super) checkpoints: Arc<CheckpointSource>,
    pub(super) execution: Execution,

    pub(super) initial_persisted: Option<oneshot::Sender<Result<()>>>,
}

impl AgentDriver {
    #[allow(clippy::too_many_lines)]
    pub(super) async fn run(mut self) -> Result<()> {
        let session_id = self.events.request_id().to_owned();
        let thread_model = self.spawner.config.model;
        let mut default_thinking = self.spawner.config.thinking;
        let mut default_service_tier = self.spawner.config.service_tier;
        let inherited_checkpoint = self.initial_model.as_ref().map(|initial| {
            Arc::new(
                CommittedSession::new(
                    Arc::clone(&self.spawner.lineage_id),
                    thread_model,
                    default_thinking,
                    default_service_tier,
                    self.spawner.config.reasoning_mode,
                    initial.checkpoint.clone(),
                )
                .with_retained_snapshot(self.spawner.restored_snapshot.take()),
            )
        });
        let prompt_cache = ModelPromptCache::new(
            self.spawner.prompt_cache_key.clone(),
            Arc::clone(&self.spawner.lineage_id),
            self.spawner.shared_prompt_cache.clone(),
        );
        let mut model = if let Some(initial) = self.initial_model.take() {
            ModelRun::from_checkpoint(
                self.events.clone(),
                Arc::clone(&self.spawner.config),
                self.client,
                Arc::clone(&self.transport_stats),
                self.tools.clone(),
                prompt_cache.clone(),
                initial,
                self.spawner.host_context.as_ref().map(Arc::clone),
            )
        } else {
            ModelRun::new(
                self.events.clone(),
                Arc::clone(&self.spawner.provider_session_id),
                Arc::clone(&self.spawner.config),
                self.client,
                Arc::clone(&self.transport_stats),
                self.tools.clone(),
                prompt_cache.clone(),
                self.spawner.context_source.clone(),
                self.spawner.host_context.as_ref().map(Arc::clone),
            )
        };
        model.set_before_compaction(self.spawner.before_compaction.clone());
        model.set_images(self.spawner.images.clone());
        if let Some(connect) = model.preconnect() {
            super::executor::spawn_driver(connect)?;
        }
        let mut turn_index = 0_u64;
        let mut logical_turn_index = 0_u64;
        let mut latest_fork_checkpoint = inherited_checkpoint;
        let mut queued_turns = VecDeque::new();
        let mut pending_compact = None;
        let mut pending_developer_messages: Vec<(String, super::turn::DeveloperMessageResult)> =
            Vec::new();
        let mut developer_checkpoint_ready = true;
        let mut commands_open = true;
        let mut detached = false;
        let mut shutdown_failures = Vec::new();
        let mut initial_notices = Ok(());
        for text in std::mem::take(&mut self.spawner.initial_developer_messages) {
            match model.append_developer_message(text, self.workspace.as_deref()) {
                Ok(snapshot) => {
                    latest_fork_checkpoint = Some(Arc::new(CommittedSession::new(
                        Arc::clone(&self.spawner.lineage_id),
                        thread_model,
                        default_thinking,
                        default_service_tier,
                        self.spawner.config.reasoning_mode,
                        snapshot,
                    )));
                }
                Err(error) => {
                    initial_notices = Err(error);
                    break;
                }
            }
        }
        if let Some(persisted) = self.initial_persisted.take() {
            let child = match self.origin.start {
                crate::session::SessionStart::New(origin) => !matches!(origin, crate::Origin::Root),
                crate::session::SessionStart::Restore => true,
                crate::session::SessionStart::Resume => false,
            };
            let outcome = if let Err(error) = initial_notices {
                Err(error)
            } else if (child || self.execution.commits_initial()) && self.execution.has_policy() {
                self.execution
                    .commit_initial_checkpoint(
                        latest_fork_checkpoint.as_deref(),
                        crate::execution::InitialSettings {
                            model: thread_model,
                            thinking: default_thinking,
                            service_tier: default_service_tier,
                        },
                    )
                    .await
            } else {
                Ok(())
            };
            drop(persisted.send(outcome));
        }
        loop {
            let command = loop {
                if let Some(queued) = queued_turns.pop_front() {
                    match queued {
                        QueuedTurn::Pending {
                            key,
                            prompt,
                            execution_operation,
                            thinking,
                            service_tier,
                            parent,
                            events,
                            result,
                        } => {
                            break Command::Prompt {
                                key,
                                prompt,
                                execution_operation,
                                accepted: None,
                                cancel_on_admission: false,
                                thinking: Some(thinking),
                                service_tier: Some(service_tier),
                                parent,
                                events,
                                result,
                            };
                        }
                        QueuedTurn::Cancelled {
                            key,
                            prompt,
                            execution_operation,
                            cancellation_committed,
                            thinking,
                            service_tier,
                            parent,
                            events,
                            result,
                        } => {
                            if execution_operation
                                .as_ref()
                                .is_some_and(ExecutionOperation::is_recovered)
                            {
                                break Command::Prompt {
                                    key,
                                    prompt,
                                    execution_operation,
                                    accepted: None,
                                    cancel_on_admission: true,
                                    thinking: Some(thinking),
                                    service_tier: Some(service_tier),
                                    parent,
                                    events,
                                    result,
                                };
                            }
                            let cancelled: ExecutionFuture<'_, ()> = Box::pin(async {
                                turn_index += 1;
                                let prompt_content = tracing::enabled!(
                                    target: "nanocode",
                                    tracing::Level::INFO
                                )
                                .then(|| serde_json::to_string(&prompt).ok())
                                .flatten();
                                let turn_span = agent_turn_span(
                                    parent.as_ref(),
                                    session_id.as_str(),
                                    self.spawner.lineage_id.as_ref(),
                                    &self.origin,
                                    ReasoningSettings {
                                        model: thread_model,
                                        mode: self.spawner.config.reasoning_mode,
                                        effort: thinking,
                                    },
                                    turn_index,
                                    prompt.text_bytes(),
                                );
                                drop(parent);
                                turn_span.record("status", "cancelled");
                                turn_span.record("otel.status_code", "ERROR");
                                if let Some(prompt_content) = &prompt_content {
                                    turn_span.in_scope(|| {
                                        info!(
                                            target: "nanocode",
                                            content_kind = "prompt",
                                            content = prompt_content.as_str(),
                                            "turn content"
                                        );
                                    });
                                }
                                let _guard = turn_span.enter();
                                let persisted = if cancellation_committed {
                                    Ok(())
                                } else if let Some(operation) = &execution_operation {
                                    self.execution
                                        .cancel_operation(operation.id(), &prompt)
                                        .await
                                } else {
                                    Ok(())
                                };
                                developer_checkpoint_ready &= persisted.is_ok();
                                let outcome = match persisted {
                                    Ok(()) => {
                                        model.set_events(events);
                                        let emitted = model.emit_cancelled_before_start(
                                            &prompt,
                                            self.workspace.as_deref(),
                                            thinking,
                                            service_tier,
                                            &NanocodeError::TurnCancelled,
                                        );
                                        model.set_events(self.events.clone());
                                        emitted.and(Err(NanocodeError::TurnCancelled))
                                    }
                                    Err(error) => {
                                        model.set_events(events);
                                        let emitted = model.emit_failed_before_start(
                                            &prompt,
                                            self.workspace.as_deref(),
                                            thinking,
                                            service_tier,
                                            &error,
                                        );
                                        model.set_events(self.events.clone());
                                        emitted.and(Err(error))
                                    }
                                };
                                let outcome = self
                                    .execution
                                    .recover_failure(
                                        execution_operation.as_ref().map(ExecutionOperation::id),
                                        outcome,
                                    )
                                    .await;
                                if !commands_open
                                    && let Err(error) = &outcome
                                    && !matches!(error, NanocodeError::TurnCancelled)
                                {
                                    shutdown_failures.push(error.to_string());
                                }
                                if commands_open && outcome_requires_reopen(&outcome) {
                                    begin_shutdown(
                                        &mut self.commands,
                                        &mut queued_turns,
                                        default_thinking,
                                        default_service_tier,
                                    )
                                    .await;
                                    commands_open = false;
                                }
                                drop(_guard);
                                drop(result.send(outcome));
                            });
                            cancelled.await;
                            continue;
                        }
                        QueuedTurn::Stopped {
                            prompt,
                            operation_id,
                            thinking,
                            service_tier,
                            events,
                            result,
                        } => {
                            self.execution.release_claim(&operation_id).await;
                            model.set_events(events);
                            let emitted = model.emit_cancelled_before_start(
                                &prompt,
                                self.workspace.as_deref(),
                                thinking,
                                service_tier,
                                &NanocodeError::TurnStopped,
                            );
                            model.set_events(self.events.clone());
                            drop(result.send(emitted.and(Err(NanocodeError::TurnStopped))));
                            continue;
                        }
                    }
                }

                if let Some((parent, result)) = pending_compact.take() {
                    break Command::Compact { parent, result };
                }

                if developer_checkpoint_ready && !pending_developer_messages.is_empty() {
                    let delivered: ExecutionFuture<'_, ()> = Box::pin(async {
                        let mut developer_messages = pending_developer_messages.drain(..);
                        while let Some((text, message_result)) = developer_messages.next() {
                            if message_result
                                .prefix()
                                .is_some_and(|prefix| model.developer_notice_current(prefix, &text))
                            {
                                message_result.skip();
                                continue;
                            }
                            let committed = commit_developer_message(
                                &mut model,
                                &self.execution,
                                Arc::clone(&self.spawner.lineage_id),
                                (
                                    thread_model,
                                    default_thinking,
                                    default_service_tier,
                                    self.spawner.config.reasoning_mode,
                                ),
                                text,
                                self.workspace.as_deref(),
                            )
                            .await;
                            let committed = match committed {
                                Ok(checkpoint) => {
                                    if let Some(checkpoint) = checkpoint {
                                        latest_fork_checkpoint = Some(checkpoint);
                                    }
                                    Ok(())
                                }
                                Err(error) => {
                                    model = model_from_checkpoint(
                                        &self.events,
                                        &self.transport_stats,
                                        &self.tools,
                                        &self.spawner,
                                        &prompt_cache,
                                        latest_fork_checkpoint.as_deref(),
                                    );
                                    Err(error)
                                }
                            };
                            let message_reopens =
                                complete_developer_message(message_result, committed, || {
                                    agent_session_context(
                                        latest_fork_checkpoint.as_deref(),
                                        self.workspace.as_deref(),
                                        &self.spawner.context_source,
                                    )
                                });
                            if message_reopens {
                                begin_shutdown(
                                    &mut self.commands,
                                    &mut queued_turns,
                                    default_thinking,
                                    default_service_tier,
                                )
                                .await;
                                commands_open = false;
                                for (_, pending_result) in developer_messages {
                                    pending_result.fail(NanocodeError::AgentStopped);
                                }
                                break;
                            }
                        }
                    });
                    delivered.await;
                }
                if commands_open {
                    let Some(command) = self.commands.recv().await else {
                        commands_open = false;
                        continue;
                    };
                    let mut reopen = false;
                    let Some(command) = accept_execution_command(
                        &self.execution,
                        &self.checkpoints,
                        &self.spawner.config,
                        default_thinking,
                        command,
                        &mut reopen,
                    )
                    .await
                    else {
                        if reopen {
                            begin_shutdown(
                                &mut self.commands,
                                &mut queued_turns,
                                default_thinking,
                                default_service_tier,
                            )
                            .await;
                            commands_open = false;
                        }
                        continue;
                    };
                    if let Command::Shutdown = command {
                        begin_shutdown(
                            &mut self.commands,
                            &mut queued_turns,
                            default_thinking,
                            default_service_tier,
                        )
                        .await;
                        commands_open = false;
                        continue;
                    }
                    break command;
                }
                model.shutdown().await;
                return if shutdown_failures.is_empty() {
                    Ok(())
                } else {
                    Err(NanocodeError::InvalidExecutionPolicy(format!(
                        "shutdown could not durably settle accepted work: {}",
                        shutdown_failures.join("; ")
                    )))
                };
            };
            let command = if matches!(command, Command::RoutePrompt { .. }) {
                let mut reopen = false;
                let Some(command) = accept_idle_route(
                    &self.execution,
                    &self.checkpoints,
                    &self.spawner.config,
                    default_thinking,
                    command,
                    &mut reopen,
                )
                .await
                else {
                    if reopen {
                        begin_shutdown(
                            &mut self.commands,
                            &mut queued_turns,
                            default_thinking,
                            default_service_tier,
                        )
                        .await;
                        commands_open = false;
                    }
                    continue;
                };
                command
            } else {
                command
            };
            let Command::Prompt {
                key,
                prompt,
                execution_operation,
                accepted: _,
                cancel_on_admission,
                thinking,
                service_tier,
                parent,
                events,
                result,
            } = command
            else {
                if let Command::SetModel {
                    model: requested_model,
                    result,
                } = command
                {
                    let outcome = if requested_model == thread_model {
                        Ok(())
                    } else {
                        Err(model_change_locked())
                    };
                    drop(result.send(outcome));
                    continue;
                }
                if let Command::SetThinking { thinking, result } = command {
                    let outcome = validate_model_thinking(thread_model, thinking).map(|()| {
                        default_thinking = thinking;
                    });
                    drop(result.send(outcome));
                    continue;
                }
                if let Command::SetAutoCompactTokenLimit { limit, result } = command {
                    Arc::make_mut(&mut self.spawner.config).auto_compact_token_limit = limit;
                    drop(result.send(Ok(())));
                    continue;
                }
                if let Command::SetServiceTier {
                    service_tier,
                    result,
                } = command
                {
                    let outcome = crate::ModelSelection::Codex(thread_model)
                        .capabilities(crate::ModelTransport::Native)
                        .check_service_tier(service_tier)
                        .map(|()| default_service_tier = service_tier);
                    drop(result.send(outcome));
                    continue;
                }
                if let Command::Preconnect { result } = command {
                    let outcome = model
                        .preconnect()
                        .map_or(Ok(()), super::executor::spawn_driver);
                    drop(result.send(outcome));
                    continue;
                }
                if let Command::AppendDeveloperMessage { text, result } = command {
                    let appended: ExecutionFuture<'_, ()> = Box::pin(async {
                        if !pending_developer_messages.is_empty() {
                            pending_developer_messages.push((text, result));
                            return;
                        }
                        if result
                            .prefix()
                            .is_some_and(|prefix| model.developer_notice_current(prefix, &text))
                        {
                            result.skip();
                            return;
                        }
                        let committed =
                            match model.append_developer_message(text, self.workspace.as_deref()) {
                                Ok(snapshot) => {
                                    let checkpoint = Arc::new(CommittedSession::new(
                                        Arc::clone(&self.spawner.lineage_id),
                                        thread_model,
                                        default_thinking,
                                        default_service_tier,
                                        self.spawner.config.reasoning_mode,
                                        snapshot,
                                    ));
                                    match self.execution.commit_checkpoint(&checkpoint).await {
                                        Ok(()) => {
                                            latest_fork_checkpoint = Some(checkpoint);
                                            Ok(())
                                        }
                                        Err(error) => {
                                            model = model_from_checkpoint(
                                                &self.events,
                                                &self.transport_stats,
                                                &self.tools,
                                                &self.spawner,
                                                &prompt_cache,
                                                latest_fork_checkpoint.as_deref(),
                                            );
                                            Err(error)
                                        }
                                    }
                                }
                                Err(error) => Err(error),
                            };
                        let reopen = complete_developer_message(result, committed, || {
                            agent_session_context(
                                latest_fork_checkpoint.as_deref(),
                                self.workspace.as_deref(),
                                &self.spawner.context_source,
                            )
                        });
                        if reopen {
                            begin_shutdown(
                                &mut self.commands,
                                &mut queued_turns,
                                default_thinking,
                                default_service_tier,
                            )
                            .await;
                            commands_open = false;
                        }
                    });
                    appended.await;
                    continue;
                }
                if let Command::Context { result } = command {
                    drop(result.send(agent_session_context(
                        latest_fork_checkpoint.as_deref(),
                        self.workspace.as_deref(),
                        &self.spawner.context_source,
                    )));
                    continue;
                }
                if let Command::SideQuestion { question, result } = command {
                    let checkpoint = match latest_fork_checkpoint.clone() {
                        Some(checkpoint) => Ok(checkpoint),
                        None => model
                            .side_checkpoint(self.workspace.as_deref())
                            .map(|snapshot| {
                                Arc::new(CommittedSession::new(
                                    Arc::clone(&self.spawner.lineage_id),
                                    thread_model,
                                    default_thinking,
                                    default_service_tier,
                                    self.spawner.config.reasoning_mode,
                                    snapshot,
                                ))
                            }),
                    };
                    match checkpoint {
                        Ok(checkpoint) => handle_idle_command(
                            Command::SideQuestion { question, result },
                            Some(&checkpoint),
                            &self.spawner,
                            TurnDefaults {
                                model: thread_model,
                                thinking: default_thinking,
                                service_tier: default_service_tier,
                            }
                            .latest_request(&prompt_cache),
                            session_id.as_str(),
                            self.workspace.clone(),
                            &self.execution,
                        ),
                        Err(error) => drop(result.send(Err(error))),
                    }
                    continue;
                }
                if let Command::Compact { parent, result } = command {
                    let compaction: ExecutionFuture<'_, ()> = Box::pin(async {
                        logical_turn_index = logical_turn_index.saturating_add(1);
                        let span = agent_compact_span(
                            parent.as_ref(),
                            session_id.as_str(),
                            self.spawner.lineage_id.as_ref(),
                            &self.origin,
                        );
                        drop(parent);
                        let compact_started = nanocode_platform::Instant::now();
                        let compact_base_checkpoint = latest_fork_checkpoint.clone();
                        let admitted = self
                            .execution
                            .admit_compaction(
                                compact_base_checkpoint.as_deref(),
                                thread_model,
                                default_thinking,
                                default_service_tier,
                                self.workspace.as_deref(),
                            )
                            .await;
                        let (compaction_operation_id, compaction_input, admission) = match admitted
                        {
                            Ok(admitted) => admitted,
                            Err(error) => {
                                let reopen = matches!(
                                    error.execution_policy_disposition(),
                                    Some(crate::ExecutionPolicyDisposition::Reopen)
                                );
                                span.record("status", "failed");
                                span.record("otel.status_code", "ERROR");
                                span.record(
                                    "duration_ns",
                                    u64::try_from(compact_started.elapsed().as_nanos())
                                        .unwrap_or(u64::MAX),
                                );
                                drop(result.send(Err(error)));
                                if reopen {
                                    begin_shutdown(
                                        &mut self.commands,
                                        &mut queued_turns,
                                        default_thinking,
                                        default_service_tier,
                                    )
                                    .await;
                                    commands_open = false;
                                }
                                return;
                            }
                        };
                        if !matches!(
                            admission,
                            AdmittedExecution::Execute | AdmittedExecution::Resume
                        ) {
                            let error = NanocodeError::InvalidExecutionPolicy(
                            "standalone compaction identity resolved to an unexpected terminal operation"
                                .to_owned(),
                        );
                            span.record("status", "failed");
                            span.record("otel.status_code", "ERROR");
                            span.record(
                                "duration_ns",
                                u64::try_from(compact_started.elapsed().as_nanos())
                                    .unwrap_or(u64::MAX),
                            );
                            drop(result.send(Err(error)));
                            return;
                        }
                        let execution_turn = self.execution.start_compaction(
                            default_thinking,
                            compaction_operation_id.clone(),
                            compaction_input,
                        );
                        if let Err(error) = execution_turn.begin().await {
                            if let Some(operation_id) = &compaction_operation_id {
                                self.execution.release_claim(operation_id).await;
                            }
                            let reopen = matches!(
                                error.execution_policy_disposition(),
                                Some(crate::ExecutionPolicyDisposition::Reopen)
                            );
                            span.record("status", "failed");
                            span.record("otel.status_code", "ERROR");
                            span.record(
                                "duration_ns",
                                u64::try_from(compact_started.elapsed().as_nanos())
                                    .unwrap_or(u64::MAX),
                            );
                            drop(result.send(Err(error)));
                            if reopen {
                                begin_shutdown(
                                    &mut self.commands,
                                    &mut queued_turns,
                                    default_thinking,
                                    default_service_tier,
                                )
                                .await;
                                commands_open = false;
                            }
                            return;
                        }
                        model.set_host_context(
                            self.spawner
                                .host_context
                                .as_ref()
                                .map(Arc::clone)
                                .or_else(|| compaction_operation_id.as_deref().map(Arc::from)),
                        );
                        let execution_steps = execution_turn.steps();
                        let mut compact_replaced = false;
                        let (cancel_compaction, mut cancel_compaction_rx) = oneshot::channel();
                        let mut cancel_compaction = Some(cancel_compaction);
                        let mut execution: ExecutionFuture<'_, Result<ModelCompactOutcome>> =
                            Box::pin(
                                model
                                    .compact(
                                        self.workspace.clone(),
                                        default_thinking,
                                        default_service_tier,
                                        logical_turn_index,
                                        &mut cancel_compaction_rx,
                                        execution_steps,
                                    )
                                    .instrument(span.clone()),
                            );
                        let completed = loop {
                            if !commands_open {
                                break execution.as_mut().await;
                            }
                            tokio::select! {
                                biased;
                                outcome = &mut execution => break outcome,
                                command = self.commands.recv() => {
                                    let mut reopen = false;
                                    let command = match command {
                                        Some(command) => match accept_execution_command(
                                            &self.execution,
                                            &self.checkpoints,
                                            &self.spawner.config,
                                            default_thinking,
                                            command,
                                            &mut reopen,
                                        ).await {
                                            Some(command) => Some(command),
                                            None if reopen => None,
                                            None => continue,
                                        },
                                        None => None,
                                    };
                                    if reopen {
                                        if let Some(cancel) = cancel_compaction.take() {
                                            let _ = cancel.send(());
                                        }
                                        begin_shutdown(
                                            &mut self.commands,
                                            &mut queued_turns,
                                            default_thinking,
                                            default_service_tier,
                                        )
                                        .await;
                                        commands_open = false;
                                        break execution.as_mut().await;
                                    }
                                    match command {
                                        Some(Command::Prompt {
                                            key,
                                            prompt,
                                            execution_operation,
                                            accepted: _,
                                            cancel_on_admission,
                                            thinking: _,
                                            service_tier: _,
                                            parent,
                                            events,
                                            result,
                                        }) => {
                                            queued_turns.push_back(queued_prompt(
                                                key,
                                                prompt,
                                                execution_operation,
                                                cancel_on_admission,
                                                default_thinking,
                                                default_service_tier,
                                                parent,
                                                events,
                                                result,
                                            ));
                                        }
                                        Some(command @ Command::RoutePrompt { .. }) => {
                                            let mut reopen = false;
                                            let Some(Command::Prompt {
                                                key,
                                                prompt,
                                                execution_operation,
                                                accepted: _,
                                                cancel_on_admission,
                                                thinking: _,
                                                service_tier: _,
                                                parent,
                                                events,
                                                result,
                                            }) = accept_idle_route(
                                                &self.execution,
                                                &self.checkpoints,
                                                &self.spawner.config,
                                                default_thinking,
                                                command,
                                                &mut reopen,
                                            )
                                            .await
                                            else {
                                                if reopen {
                                                    if let Some(cancel) = cancel_compaction.take() {
                                                        let _ = cancel.send(());
                                                    }
                                                    begin_shutdown(
                                                        &mut self.commands,
                                                        &mut queued_turns,
                                                        default_thinking,
                                                        default_service_tier,
                                                    )
                                                    .await;
                                                    commands_open = false;
                                                    break execution.as_mut().await;
                                                }
                                                continue;
                                            };
                                            queued_turns.push_back(queued_prompt(
                                                key,
                                                prompt,
                                                execution_operation,
                                                cancel_on_admission,
                                                default_thinking,
                                                default_service_tier,
                                                parent,
                                                events,
                                                result,
                                            ));
                                        }
                                        Some(Command::Compact { parent, result }) => {
                                            compact_replaced = true;
                                            pending_compact = Some((parent, result));
                                            if let Some(cancel) = cancel_compaction.take() {
                                                let _ = cancel.send(());
                                            }
                                            break execution.as_mut().await;
                                        }
                                        Some(Command::CancelActive { result }) => {
                                            drop(result.send(Err(NanocodeError::TurnNotCancellable)));
                                        }
                                        Some(Command::Cancel { key, result }) => {
                                            let outcome = match queued_execution_operation(
                                                &queued_turns,
                                                key,
                                            ) {
                                                Some((operation, prompt)) => {
                                                    let recovered = operation
                                                        .as_ref()
                                                        .is_some_and(ExecutionOperation::is_recovered);
                                                    let (persisted, committed) = match operation.as_ref() {
                                                        Some(operation) if !recovered => (
                                                            self.execution
                                                                .cancel_operation(
                                                                    operation.id(),
                                                                    &prompt,
                                                                )
                                                                .await,
                                                            true,
                                                        ),

                                                        Some(operation) => (
                                                            Ok(()),
                                                            self.execution
                                                                .cancel_unstarted(operation.id())
                                                                .await,
                                                        ),
                                                        None => (Ok(()), false),
                                                    };
                                                    persisted.and_then(|()| {
                                                        if settle_cancelled_queued_turn(
                                                            &mut queued_turns,
                                                            key,
                                                            recovered,
                                                            committed,
                                                            &self.spawner.config,
                                                        ) {
                                                            Ok(())
                                                        } else {
                                                            Err(NanocodeError::TurnNotCancellable)
                                                        }
                                                    })
                                                }
                                                None => Err(NanocodeError::TurnNotCancellable),
                                            };
                                            let reopen = outcome_requires_reopen(&outcome);
                                            drop(result.send(outcome));
                                            if reopen {
                                                if let Some(cancel) = cancel_compaction.take() {
                                                    let _ = cancel.send(());
                                                }
                                                begin_shutdown(
                                                    &mut self.commands,
                                                    &mut queued_turns,
                                                    default_thinking,
                                                    default_service_tier,
                                                )
                                                .await;
                                                commands_open = false;
                                                break execution.as_mut().await;
                                            }
                                        }
                                        Some(Command::WithdrawSteer { result, .. }) => {
                                            drop(result.send(Ok(false)));
                                        }
                                        Some(Command::SteerWithId { result, .. } | Command::Steer { result, .. }) => {
                                            drop(result.send(Err(NanocodeError::TurnNotSteerable)));
                                        }
                                        Some(command @ Command::SideQuestion { .. }) => {
                                            handle_idle_command(
                                                command,
                                                latest_fork_checkpoint.as_ref(),
                                                &self.spawner,
                                                TurnDefaults {
                                                    model: thread_model,
                                                    thinking: default_thinking,
                                                    service_tier: default_service_tier,
                                                }
                                                .latest_request(&prompt_cache),
                                                session_id.as_str(),
                                                self.workspace.clone(),
                                                &self.execution,
                                            );
                                        }
                                        Some(command @ (Command::ChildSnapshot { .. } | Command::Fork { .. } | Command::Spawn { .. } | Command::SpawnBatch { .. })) => {
                                            handle_idle_command(
                                                command,
                                                latest_fork_checkpoint.as_ref(),
                                                &self.spawner,
                                                TurnDefaults {
                                                    model: thread_model,
                                                    thinking: default_thinking,
                                                    service_tier: default_service_tier,
                                                },
                                                session_id.as_str(),
                                                self.workspace.clone(),
                                                &self.execution,
                                            );
                                        }
                                        Some(Command::SetThinking { thinking, result }) => {
                                            let outcome = validate_model_thinking(
                                                thread_model,
                                                thinking,
                                            )
                                            .map(|()| {
                                                default_thinking = thinking;
                                            });
                                            drop(result.send(outcome));
                                        }
                                        Some(Command::Preconnect { result }) => {
                                            drop(result.send(Ok(())));
                                        }
                                        Some(Command::SetAutoCompactTokenLimit { limit, result }) => {
                                            Arc::make_mut(&mut self.spawner.config).auto_compact_token_limit = limit;
                                            drop(result.send(Ok(())));
                                        }
                                        Some(Command::SetServiceTier { service_tier, result }) => {
                                            let outcome = crate::ModelSelection::Codex(thread_model)
                                                .capabilities(crate::ModelTransport::Native)
                                                .check_service_tier(service_tier)
                                                .map(|()| default_service_tier = service_tier);
                                            drop(result.send(outcome));
                                        }
                                        Some(Command::SetModel { result, .. }) => {
                                            drop(result.send(Err(model_change_locked())));
                                        }
                                        Some(Command::AppendDeveloperMessage { text, result }) => {
                                            pending_developer_messages.push((text, result));
                                        }
                                        Some(Command::Context { result }) => {
                                            drop(result.send(agent_session_context(
                                                latest_fork_checkpoint.as_deref(),
                                                self.workspace.as_deref(),
                                                &self.spawner.context_source,
                                            )));
                                        }
                                        Some(Command::Shutdown) => {
                                            if let Some(cancel) = cancel_compaction.take() {
                                                let _ = cancel.send(());
                                            }
                                            begin_shutdown(
                                                &mut self.commands,
                                                &mut queued_turns,
                                                default_thinking,
                                                default_service_tier,
                                            )
                                            .await;
                                            commands_open = false;
                                            break execution.as_mut().await;
                                        }
                                        None => {
                                            commands_open = false;
                                            detached = true;
                                            break execution.as_mut().await;
                                        }
                                    }
                                }
                            }
                        };
                        drop(execution);
                        let mut compact_checkpoint_committed = false;
                        let outcome = match completed {
                            Ok(ModelCompactOutcome::Completed(checkpoint)) => {
                                let checkpoint = Arc::new(CommittedSession::new(
                                    Arc::clone(&self.spawner.lineage_id),
                                    thread_model,
                                    default_thinking,
                                    default_service_tier,
                                    self.spawner.config.reasoning_mode,
                                    checkpoint,
                                ));
                                let persisted = self
                                    .execution
                                    .persist_compaction(
                                        &checkpoint,
                                        execution_turn.completed_without_message(),
                                    )
                                    .await;
                                if persisted.is_ok() {
                                    compact_checkpoint_committed = true;
                                    latest_fork_checkpoint = Some(checkpoint);
                                }
                                persisted
                            }
                            Ok(ModelCompactOutcome::Cancelled(checkpoint)) => {
                                let checkpoint = Arc::new(CommittedSession::new(
                                    Arc::clone(&self.spawner.lineage_id),
                                    thread_model,
                                    default_thinking,
                                    default_service_tier,
                                    self.spawner.config.reasoning_mode,
                                    checkpoint,
                                ));
                                let execution_turn = execution_turn.interrupted();
                                let execution_turn = if compact_replaced {
                                    execution_turn.replaced()
                                } else {
                                    execution_turn
                                };
                                let persisted = self
                                    .execution
                                    .persist(&checkpoint, execution_turn)
                                    .instrument(span.clone())
                                    .await;
                                match persisted {
                                    Ok(()) => {
                                        compact_checkpoint_committed = true;
                                        latest_fork_checkpoint = Some(checkpoint);
                                        model.replace_client((self.spawner.service_factory)(
                                            Arc::clone(&self.spawner.config),
                                        ));
                                        Err(NanocodeError::TurnCancelled)
                                    }
                                    Err(error) => Err(error),
                                }
                            }
                            Ok(ModelCompactOutcome::Failed { error, checkpoint }) => {
                                let checkpoint = Arc::new(CommittedSession::new(
                                    Arc::clone(&self.spawner.lineage_id),
                                    thread_model,
                                    default_thinking,
                                    default_service_tier,
                                    self.spawner.config.reasoning_mode,
                                    checkpoint,
                                ));
                                let persisted = self
                                    .execution
                                    .persist(
                                        &checkpoint,
                                        execution_turn.failed(
                                            error.to_string(),
                                            matches!(
                                                error.execution_policy_disposition(),
                                                Some(crate::ExecutionPolicyDisposition::Retry)
                                            ),
                                        ),
                                    )
                                    .instrument(span.clone())
                                    .await;
                                match persisted {
                                    Ok(()) => {
                                        if error.execution_policy_disposition()
                                            != Some(crate::ExecutionPolicyDisposition::Retry)
                                        {
                                            compact_checkpoint_committed = true;
                                            latest_fork_checkpoint = Some(checkpoint);
                                        }
                                        Err(error)
                                    }
                                    Err(error) => Err(error),
                                }
                            }
                            Err(error) => Err(error),
                        };
                        let outcome = self
                            .execution
                            .recover_failure(compaction_operation_id.as_deref(), outcome)
                            .await;
                        if !compact_checkpoint_committed && outcome.is_err() {
                            latest_fork_checkpoint = compact_base_checkpoint;
                            model = model_from_checkpoint(
                                &self.events,
                                &self.transport_stats,
                                &self.tools,
                                &self.spawner,
                                &prompt_cache,
                                latest_fork_checkpoint.as_deref(),
                            );
                        }
                        span.record(
                            "status",
                            if matches!(&outcome, Err(NanocodeError::TurnCancelled)) {
                                "cancelled"
                            } else if outcome.is_ok() {
                                "completed"
                            } else {
                                "failed"
                            },
                        );
                        span.record(
                            "otel.status_code",
                            if outcome.is_ok() { "OK" } else { "ERROR" },
                        );
                        span.record(
                            "duration_ns",
                            u64::try_from(compact_started.elapsed().as_nanos()).unwrap_or(u64::MAX),
                        );
                        let reopen_after_compaction = outcome_requires_reopen(&outcome);
                        developer_checkpoint_ready =
                            compact_checkpoint_committed && !reopen_after_compaction;
                        if !developer_checkpoint_ready && let Err(error) = &outcome {
                            let detail = error.to_string();
                            for (_, message_result) in pending_developer_messages.drain(..) {
                                message_result.fail(NanocodeError::InvalidExecutionPolicy(format!(
                                "developer message was not committed because the preceding compaction failed: {detail}"
                            )));
                            }
                        }
                        drop(result.send(outcome));
                        if commands_open && reopen_after_compaction {
                            begin_shutdown(
                                &mut self.commands,
                                &mut queued_turns,
                                default_thinking,
                                default_service_tier,
                            )
                            .await;
                            commands_open = false;
                        }
                    });
                    compaction.await;
                    continue;
                }
                handle_idle_command(
                    command,
                    latest_fork_checkpoint.as_ref(),
                    &self.spawner,
                    TurnDefaults {
                        model: thread_model,
                        thinking: default_thinking,
                        service_tier: default_service_tier,
                    },
                    session_id.as_str(),
                    self.workspace.clone(),
                    &self.execution,
                );
                continue;
            };
            let recovered_operation = execution_operation
                .as_ref()
                .is_some_and(ExecutionOperation::is_recovered);
            let execution_operation = execution_operation.map(ExecutionOperation::into_id);
            let thinking = thinking.unwrap_or(default_thinking);
            let service_tier = service_tier.unwrap_or(default_service_tier);
            if let Err(error) = validate_model_thinking(thread_model, thinking) {
                if let Some(operation_id) = &execution_operation {
                    self.execution.release_claim(operation_id).await;
                }
                let outcome = self
                    .execution
                    .recover_failure(execution_operation.as_deref(), Err(error))
                    .await;
                drop(result.send(outcome));
                continue;
            }
            if cancel_on_admission && !recovered_operation {
                queued_turns.push_front(queued_prompt(
                    key,
                    prompt,
                    execution_operation.map(ExecutionOperation::Admitted),
                    true,
                    thinking,
                    service_tier,
                    parent,
                    events,
                    result,
                ));
                continue;
            }
            developer_checkpoint_ready = false;
            model.set_host_context(
                self.spawner
                    .host_context
                    .as_ref()
                    .map(Arc::clone)
                    .or_else(|| execution_operation.as_deref().map(Arc::from)),
            );
            turn_index += 1;
            logical_turn_index = logical_turn_index.saturating_add(1);
            let prompt_content = tracing::enabled!(
                target: "nanocode",
                tracing::Level::INFO
            )
            .then(|| serde_json::to_string(&prompt).ok())
            .flatten();
            let turn_span = agent_turn_span(
                parent.as_ref(),
                session_id.as_str(),
                self.spawner.lineage_id.as_ref(),
                &self.origin,
                ReasoningSettings {
                    model: thread_model,
                    mode: self.spawner.config.reasoning_mode,
                    effort: thinking,
                },
                turn_index,
                prompt.text_bytes(),
            );
            drop(parent);
            if let Some(prompt_content) = &prompt_content {
                turn_span.in_scope(|| {
                    info!(
                        target: "nanocode",
                        content_kind = "prompt",
                        content = prompt_content.as_str(),
                        "turn content"
                    );
                });
            }
            let execution_turn = self.execution.start_turn(
                &prompt,
                thinking,
                execution_operation.clone(),
                events.turn_id(),
            );
            let retained_steers = match execution_turn.begin().await {
                Ok(steers) => steers,
                Err(error) => {
                    if let Some(operation_id) = &execution_operation {
                        self.execution.release_claim(operation_id).await;
                    }
                    model.set_events(events);
                    let outcome = self
                        .execution
                        .recover_failure(execution_operation.as_deref(), Err(error))
                        .await;
                    let emitted = match &outcome {
                        Err(error) => model.emit_failed_before_start(
                            &prompt,
                            self.workspace.as_deref(),
                            thinking,
                            service_tier,
                            error,
                        ),
                        Ok(_) => Ok(()),
                    };
                    model.set_events(self.events.clone());
                    let outcome = emitted.and(outcome);
                    let reopen = outcome.as_ref().is_err_and(|error| {
                        error.execution_policy_disposition()
                            == Some(crate::ExecutionPolicyDisposition::Reopen)
                    });
                    drop(result.send(outcome));
                    if reopen {
                        begin_shutdown(
                            &mut self.commands,
                            &mut queued_turns,
                            default_thinking,
                            default_service_tier,
                        )
                        .await;
                        commands_open = false;
                    }
                    continue;
                }
            };
            let execution_base_checkpoint = execution_operation
                .as_ref()
                .map(|_| latest_fork_checkpoint.clone());
            let execution_steps = execution_turn.steps();
            let (steer_preempt, steer_preempt_rx) = watch::channel(0_u64);
            let steer_rx: SteerQueue = Arc::default();
            let steers = Arc::downgrade(&steer_rx);
            let mut accepted_steers = retained_steers
                .iter()
                .filter(|steer| steer.model_call_index.is_none())
                .map(|steer| {
                    (
                        steer.message_id.clone(),
                        SteerReceipt {
                            durable_index: steer.durable_index,
                            delivery: Arc::clone(&steer.delivery),
                        },
                    )
                })
                .collect();
            let mut steer_ids = std::collections::HashMap::<String, (String, bool)>::new();
            let model_call_index = Arc::new(std::sync::atomic::AtomicU32::new(1));
            let (cancel, cancel_rx) = oneshot::channel();
            let (fork_snapshots, mut fork_snapshot_rx) = watch::channel(None);
            let mut fork_snapshots_open = true;
            let mut cancel = Some(cancel);
            if cancel_on_admission && let Some(cancel) = cancel.take() {
                let _ = cancel.send(());
            }
            let mut cancel_result = None;
            let input_events = events.clone();
            model.set_events(events);
            let mut execution: ExecutionFuture<'_, Result<ModelTurnOutcome>> = Box::pin(
                model
                    .execute(
                        prompt,
                        self.workspace.clone(),
                        thinking,
                        service_tier,
                        logical_turn_index,
                        TurnSteering {
                            preempt: steer_preempt_rx,
                            instant_tool_steering: self.spawner.instant_tool_steering,
                            receiver: steer_rx,
                            retained: retained_steers,
                            model_call_index: Arc::clone(&model_call_index),
                        },
                        cancel_rx,
                        fork_snapshots,
                        execution_steps,
                    )
                    .instrument(turn_span.clone()),
            );
            let completed = loop {
                if !commands_open {
                    break execution.as_mut().await;
                }
                tokio::select! {
                    biased;
                    changed = fork_snapshot_rx.changed(), if fork_snapshots_open => {
                        if changed.is_err() {
                            fork_snapshots_open = false;
                            continue;
                        }
                        let snapshot = fork_snapshot_rx.borrow_and_update().clone();
                        if let Some(snapshot) = snapshot {
                            latest_fork_checkpoint = Some(Arc::new(CommittedSession::new(
                                Arc::clone(&self.spawner.lineage_id),
                                thread_model,
                                default_thinking,
                                default_service_tier,
                    self.spawner.config.reasoning_mode,
                                snapshot,
                            )));
                        }
                    }
                    outcome = &mut execution => break outcome,
                    command = self.commands.recv() => {
                        let mut reopen = false;
                        let command = match command {
                            Some(command) => match accept_execution_command(
                                &self.execution,
                                &self.checkpoints,
                                &self.spawner.config,
                                default_thinking,
                                command,
                                &mut reopen,
                            ).await {
                                Some(command) => Some(command),
                                None if reopen => None,
                                None => continue,
                            },
                            None => None,
                        };
                        if reopen {
                            if let Some(cancel) = cancel.take() {
                                let _ = cancel.send(());
                            }
                            begin_shutdown(
                                &mut self.commands,
                                &mut queued_turns,
                                default_thinking,
                                default_service_tier,
                            )
                            .await;
                            commands_open = false;
                            break execution.as_mut().await;
                        }
                        match command {
                            Some(Command::Prompt {
                                key,
                                prompt,
                                execution_operation,
                                accepted: _,
                                cancel_on_admission,
                                thinking: _,
                                service_tier: _,
                                parent,
                                events,
                                result,
                            }) => {
                                queued_turns.push_back(queued_prompt(
                                    key,
                                    prompt,
                                    execution_operation,
                                    cancel_on_admission,
                                    default_thinking,
                                    default_service_tier,
                                    parent,
                                    events,
                                    result,
                                ));
                            }
                            Some(Command::Steer { key: target, prompt, result }) => {
                                if target != key {
                                    drop(result.send(Err(NanocodeError::TurnNotSteerable)));
                                    continue;
                                }
                                let outcome = accept_turn_steer(
                                    &self.execution,
                                    &input_events,
                                    &steers,
                                    &steer_preempt,
                                    &mut accepted_steers,
                                    None,
                                    &execution_turn,
                                    &model_call_index,
                                    prompt,
                                )
                                .await;
                                let reopen = outcome_requires_reopen(&outcome);
                                drop(result.send(outcome));
                                if reopen {
                                    if let Some(cancel) = cancel.take() {
                                        let _ = cancel.send(());
                                    }
                                    begin_shutdown(
                                        &mut self.commands,
                                        &mut queued_turns,
                                        default_thinking,
                                        default_service_tier,
                                    )
                                    .await;
                                    commands_open = false;
                                    break execution.as_mut().await;
                                }
                            }
                            Some(Command::SteerWithId { key: target, id, prompt, result }) => {
                                if target != key {
                                    drop(result.send(Err(NanocodeError::TurnNotSteerable)));
                                    continue;
                                }
                                let receipt = match (!execution_turn.supports_steer_receipts()).then(|| serde_json::to_string(&prompt)).transpose() {
                                    Ok(receipt) => receipt,
                                    Err(error) => {
                                        drop(result.send(Err(NanocodeError::InvalidRequest(error.to_string()))));
                                        continue;
                                    }
                                };
                                if let Some(input) = &receipt && let Some((known, withdrawn)) = steer_ids.get(&id) {
                                    drop(result.send(if known != input {
                                        Err(NanocodeError::InvalidRequest(format!("steering message `{id}` already has different input")))
                                    } else if *withdrawn {
                                        Err(NanocodeError::InvalidRequest(format!("steering message `{id}` was withdrawn")))
                                    } else {
                                        Ok(())
                                    }));
                                    continue;
                                }
                                let outcome = accept_turn_steer(&self.execution, &input_events, &steers, &steer_preempt, &mut accepted_steers, Some(id.clone()), &execution_turn, &model_call_index, prompt).await;
                                if let Some(input) = receipt && outcome.is_ok() {
                                    steer_ids.insert(id, (input, false));
                                }
                                let reopen = outcome_requires_reopen(&outcome);
                                drop(result.send(outcome));
                                if reopen {
                                    if let Some(cancel) = cancel.take() { let _ = cancel.send(()); }
                                    begin_shutdown(&mut self.commands, &mut queued_turns, default_thinking, default_service_tier).await;
                                    commands_open = false;
                                    break execution.as_mut().await;
                                }
                            }
                            Some(Command::WithdrawSteer { key: target, id, result }) => {
                                let outcome = if target == key {
                                    match accepted_steers.last() {
                                        Some((Some(retained_id), steer)) if *retained_id == id => execution_turn.withdraw_steer(steer).await,
                                        _ => Ok(false),
                                    }
                                } else { Ok(false) };
                                if matches!(outcome, Ok(true)) {
                                    if let Some((_, withdrawn)) = steer_ids.get_mut(&id) {
                                        *withdrawn = true;
                                    }
                                    accepted_steers.pop();
                                }
                                let reopen = outcome_requires_reopen(&outcome);
                                drop(result.send(outcome));
                                if reopen {
                                    if let Some(cancel) = cancel.take() { let _ = cancel.send(()); }
                                    begin_shutdown(&mut self.commands, &mut queued_turns, default_thinking, default_service_tier).await;
                                    commands_open = false;
                                    break execution.as_mut().await;
                                }
                            }
                            Some(Command::RoutePrompt {
                                key: route_key,
                                prompt,
                                request_id,
                                cancel_on_admission,
                                parent,
                                events,
                                turn_result,
                                route_result,
                            }) => {
                                if let Some(id) = &request_id {
                                    match self.execution.identified_steer_accepted(id.clone(), &prompt).await {
                                        Ok(true) => {
                                            drop(route_result.send(Ok(PromptRouteKind::Steered)));
                                            continue;
                                        }
                                        Ok(false) => {}
                                        Err(error) => {
                                            drop(route_result.send(Err(error)));
                                            continue;
                                        }
                                    }
                                }
                                let outcome = accept_turn_steer(
                                    &self.execution,
                                    &input_events,
                                    &steers,
                                    &steer_preempt,
                                    &mut accepted_steers,
                                    request_id.clone(),
                                    &execution_turn,
                                    &model_call_index,
                                    prompt.clone(),
                                )
                                .await
                                .map(|()| PromptRouteKind::Steered);
                                if matches!(outcome, Err(NanocodeError::TurnNotSteerable)) {
                                    let mut reopen = false;
                                    let routed = accept_idle_route(
                                        &self.execution,
                                        &self.checkpoints,
                                        &self.spawner.config,
                                        default_thinking,
                                        Command::RoutePrompt { key: route_key, prompt, request_id, cancel_on_admission, parent, events, turn_result, route_result },
                                        &mut reopen,
                                    )
                                    .await;
                                    if let Some(Command::Prompt { key, prompt, execution_operation, cancel_on_admission, parent, events, result, .. }) = routed {
                                        queued_turns.push_back(queued_prompt(
                                            key,
                                            prompt,
                                            execution_operation,
                                            cancel_on_admission,
                                            default_thinking,
                                            default_service_tier,
                                            parent,
                                            events,
                                            result,
                                        ));
                                    }
                                    if reopen {
                                        if let Some(cancel) = cancel.take() {
                                            let _ = cancel.send(());
                                        }
                                        begin_shutdown(
                                            &mut self.commands,
                                            &mut queued_turns,
                                            default_thinking,
                                            default_service_tier,
                                        )
                                        .await;
                                        commands_open = false;
                                        break execution.as_mut().await;
                                    }
                                    continue;
                                }
                                let reopen = outcome_requires_reopen(&outcome);
                                drop(route_result.send(outcome));
                                if reopen {
                                    if let Some(cancel) = cancel.take() {
                                        let _ = cancel.send(());
                                    }
                                    begin_shutdown(
                                        &mut self.commands,
                                        &mut queued_turns,
                                        default_thinking,
                                        default_service_tier,
                                    )
                                    .await;
                                    commands_open = false;
                                    break execution.as_mut().await;
                                }
                            }
                            Some(Command::Cancel { key: target, result: cancellation }) => {
                                if target != key {
                                    let outcome = match queued_execution_operation(
                                        &queued_turns,
                                        target,
                                    ) {
                                        Some((operation, prompt)) => {
                                            let recovered = operation
                                                .as_ref()
                                                .is_some_and(ExecutionOperation::is_recovered);
                                            let (persisted, committed) = match operation.as_ref() {
                                                Some(operation) if !recovered => (
                                                    self.execution
                                                        .cancel_operation(operation.id(), &prompt)
                                                        .await,
                                                    true,
                                                ),

                                                Some(operation) => (
                                                    Ok(()),
                                                    self.execution
                                                        .cancel_unstarted(operation.id())
                                                        .await,
                                                ),
                                                None => (Ok(()), false),
                                            };
                                            persisted.and_then(|()| {
                                                if settle_cancelled_queued_turn(
                                                    &mut queued_turns,
                                                    target,
                                                    recovered,
                                                    committed,
                                                    &self.spawner.config,
                                                ) {
                                                    Ok(())
                                                } else {
                                                    Err(NanocodeError::TurnNotCancellable)
                                                }
                                            })
                                        }
                                        None => Err(NanocodeError::TurnNotCancellable),
                                    };
                                    let reopen = outcome_requires_reopen(&outcome);
                                    drop(cancellation.send(outcome));
                                    if reopen {
                                        if let Some(cancel) = cancel.take() {
                                            let _ = cancel.send(());
                                        }
                                        begin_shutdown(
                                            &mut self.commands,
                                            &mut queued_turns,
                                            default_thinking,
                                            default_service_tier,
                                        )
                                        .await;
                                        commands_open = false;
                                        break execution.as_mut().await;
                                    }
                                    continue;
                                }
                                let Some(cancel) = cancel.take() else {
                                    drop(cancellation.send(Err(
                                        NanocodeError::TurnNotCancellable,
                                    )));
                                    continue;
                                };
                                let _ = cancel.send(());
                                cancel_result = Some(cancellation);
                                break execution.as_mut().await;
                            }
                            Some(Command::CancelActive { result: cancellation }) => {
                                let Some(cancel) = cancel.take() else {
                                    drop(cancellation.send(Err(
                                        NanocodeError::TurnNotCancellable,
                                    )));
                                    continue;
                                };
                                let _ = cancel.send(());
                                cancel_result = Some(cancellation);
                                break execution.as_mut().await;
                            }
                            Some(Command::SideQuestion { question, result }) => {
                                let side_checkpoint = prompt_cache
                                    .live()
                                    .or_else(|| fork_snapshot_rx.borrow().clone())
                                    .map(|snapshot| {
                                        Arc::new(CommittedSession::new(
                                            Arc::clone(&self.spawner.lineage_id),
                                            thread_model,
                                            default_thinking,
                                            default_service_tier,
                    self.spawner.config.reasoning_mode,
                                            snapshot,
                                        ))
                                    })
                                    .or_else(|| latest_fork_checkpoint.clone());
                                handle_idle_command(
                                    Command::SideQuestion { question, result },
                                    side_checkpoint.as_ref(),
                                    &self.spawner,
                                    TurnDefaults {
                                        model: thread_model,
                                        thinking: default_thinking,
                                        service_tier: default_service_tier,
                                    }
                                    .latest_request(&prompt_cache),
                                    session_id.as_str(),
                                    self.workspace.clone(),
                                    &self.execution,
                                );
                            }
                            Some(command @ (Command::ChildSnapshot { .. } | Command::Fork { .. } | Command::Spawn { .. } | Command::SpawnBatch { .. })) => {
                                if let Some(snapshot) =
                                    fork_snapshot_rx.borrow_and_update().clone()
                                {
                                    latest_fork_checkpoint =
                                        Some(Arc::new(CommittedSession::new(
                                            Arc::clone(&self.spawner.lineage_id),
                                            thread_model,
                                            default_thinking,
                                            default_service_tier,
                    self.spawner.config.reasoning_mode,
                                            snapshot,
                                        )));
                                }
                                handle_idle_command(
                                    command,
                                    latest_fork_checkpoint.as_ref(),
                                    &self.spawner,
                                    TurnDefaults {
                                        model: thread_model,
                                        thinking: default_thinking,
                                        service_tier: default_service_tier,
                                    },
                                    session_id.as_str(),
                                    self.workspace.clone(),
                                    &self.execution,
                                );
                            }
                            Some(Command::SetThinking { thinking, result }) => {
                                let outcome = validate_model_thinking(
                                    thread_model,
                                    thinking,
                                )
                                .map(|()| {
                                    default_thinking = thinking;
                                });
                                drop(result.send(outcome));
                            }
                            Some(Command::Preconnect { result }) => {
                                drop(result.send(Ok(())));
                            }
                            Some(Command::SetAutoCompactTokenLimit { limit, result }) => {
                                Arc::make_mut(&mut self.spawner.config).auto_compact_token_limit = limit;
                                drop(result.send(Ok(())));
                            }
                            Some(Command::SetServiceTier { service_tier, result }) => {
                                let outcome = crate::ModelSelection::Codex(thread_model)
                                    .capabilities(crate::ModelTransport::Native)
                                    .check_service_tier(service_tier)
                                    .map(|()| default_service_tier = service_tier);
                                drop(result.send(outcome));
                            }
                            Some(Command::SetModel { result, .. }) => {
                                drop(result.send(Err(model_change_locked())));
                            }
                            Some(Command::AppendDeveloperMessage { text, result }) => {
                                pending_developer_messages.push((text, result));
                            }
                            Some(Command::Context { result }) => {
                                let checkpoint = fork_snapshot_rx
                                    .borrow_and_update()
                                    .clone()
                                    .map(|checkpoint| {
                                        Arc::new(CommittedSession::new(
                                            Arc::clone(&self.spawner.lineage_id),
                                            thread_model,
                                            default_thinking,
                                            default_service_tier,
                    self.spawner.config.reasoning_mode,
                                            checkpoint,
                                        ))
                                    })
                                    .or_else(|| latest_fork_checkpoint.clone());
                                drop(result.send(agent_session_context(
                                    checkpoint.as_deref(),
                                    self.workspace.as_deref(),
                                    &self.spawner.context_source,
                                )));
                            }
                            Some(Command::Compact { parent, result }) => {
                                pending_compact = Some((parent, result));
                                if let Some(cancel) = cancel.take() {
                                    let _ = cancel.send(());
                                }
                                break execution.as_mut().await;
                            }
                            Some(Command::Shutdown) => {
                                if let Some(cancel) = cancel.take() {
                                    let _ = cancel.send(());
                                }
                                begin_shutdown(
                                    &mut self.commands,
                                    &mut queued_turns,
                                    default_thinking,
                                    default_service_tier,
                                )
                                .await;
                                commands_open = false;
                            }
                            None => {
                                commands_open = false;
                                detached = true;
                            }
                        }
                    }
                }
            };
            drop(execution);
            let mut terminal_failure_committed = false;
            let mut provider_requires_stop = false;
            let (outcome, was_cancelled, cancellation_persisted): (
                Result<TurnResult>,
                bool,
                Option<Result<()>>,
            ) = match completed {
                Ok(ModelTurnOutcome::Completed(completed)) => {
                    let CompletedModelTurn {
                        final_message,
                        usage,
                        checkpoint,
                    } = completed;
                    let checkpoint = Arc::new(CommittedSession::new(
                        Arc::clone(&self.spawner.lineage_id),
                        thread_model,
                        default_thinking,
                        default_service_tier,
                        self.spawner.config.reasoning_mode,
                        checkpoint,
                    ));
                    let execution_turn =
                        execution_turn.completed(final_message.clone(), usage.clone());
                    let committed = self
                        .execution
                        .persist(&checkpoint, execution_turn)
                        .instrument(turn_span.clone())
                        .await;
                    if committed.is_ok() {
                        terminal_failure_committed = true;
                        latest_fork_checkpoint = Some(Arc::clone(&checkpoint));
                    }
                    let persisted = committed;
                    (
                        persisted.map(|()| TurnResult {
                            request_id: execution_operation.clone(),
                            turn_id: None,
                            final_message,
                            usage: Some(usage),
                            boundary: Some(self.checkpoints.live(
                                checkpoint,
                                thinking,
                                service_tier,
                            )),
                        }),
                        false,
                        None,
                    )
                }
                Ok(ModelTurnOutcome::Cancelled(_)) if !commands_open && !detached => {
                    if let Some(operation_id) = &execution_operation {
                        self.execution.release_claim(operation_id).await;
                    }
                    model.replace_client((self.spawner.service_factory)(Arc::clone(
                        &self.spawner.config,
                    )));
                    (Err(NanocodeError::TurnStopped), true, None)
                }
                Ok(ModelTurnOutcome::Cancelled(checkpoint)) => {
                    let checkpoint = Arc::new(CommittedSession::new(
                        Arc::clone(&self.spawner.lineage_id),
                        thread_model,
                        default_thinking,
                        default_service_tier,
                        self.spawner.config.reasoning_mode,
                        checkpoint,
                    ));
                    let execution_turn = execution_turn.interrupted();
                    let committed = self
                        .execution
                        .persist(&checkpoint, execution_turn)
                        .instrument(turn_span.clone())
                        .await;
                    if committed.is_ok() {
                        terminal_failure_committed = true;
                        latest_fork_checkpoint = Some(Arc::clone(&checkpoint));
                    }
                    let (persisted, cancellation_persisted) = match committed {
                        Ok(()) => (Ok(()), Ok(())),
                        Err(error) => (Err(duplicate_policy_error(&error)), Err(error)),
                    };
                    model.replace_client((self.spawner.service_factory)(Arc::clone(
                        &self.spawner.config,
                    )));
                    (
                        persisted.and(Err(NanocodeError::TurnCancelled)),
                        true,
                        Some(cancellation_persisted),
                    )
                }
                Ok(ModelTurnOutcome::Failed { error, checkpoint }) => {
                    provider_requires_stop = error_requires_stop(&error);
                    let checkpoint = Arc::new(CommittedSession::new(
                        Arc::clone(&self.spawner.lineage_id),
                        thread_model,
                        default_thinking,
                        default_service_tier,
                        self.spawner.config.reasoning_mode,
                        checkpoint,
                    ));
                    match error.execution_policy_disposition() {
                        Some(crate::ExecutionPolicyDisposition::Reopen) => {
                            (Err(error), false, None)
                        }
                        disposition => {
                            let retryable = error
                                .responses_error()
                                .is_some_and(|source| source.retry_advice().is_some())
                                || matches!(
                                    disposition,
                                    Some(crate::ExecutionPolicyDisposition::Retry)
                                );
                            let execution_turn =
                                execution_turn.failed(error.to_string(), retryable);
                            let committed = self
                                .execution
                                .persist(&checkpoint, execution_turn)
                                .instrument(turn_span.clone())
                                .await;
                            if committed.is_ok() && !retryable {
                                latest_fork_checkpoint = Some(checkpoint);
                                terminal_failure_committed = true;
                            }
                            let persisted = committed;
                            (persisted.and(Err(error)), false, None)
                        }
                    }
                }
                Err(error) => {
                    provider_requires_stop = error_requires_stop(&error);
                    match error.execution_policy_disposition() {
                        Some(crate::ExecutionPolicyDisposition::Reopen) => {
                            (Err(error), false, None)
                        }
                        _ => {
                            let persisted = self
                                .execution
                                .fail_without_checkpoint(execution_turn)
                                .instrument(turn_span.clone())
                                .await;
                            (persisted.and(Err(error)), false, None)
                        }
                    }
                }
            };
            let outcome = match outcome {
                Err(NanocodeError::TurnStopped) if !commands_open => {
                    Err(NanocodeError::TurnStopped)
                }
                outcome => {
                    self.execution
                        .recover_failure(execution_operation.as_deref(), outcome)
                        .await
                }
            };

            let terminal = match &outcome {
                Ok(_) => "completed",
                Err(NanocodeError::TurnCancelled | NanocodeError::TurnStopped) => "cancelled",
                Err(_) => "failed",
            };
            let emitted = model.emit_terminal(terminal);
            let outcome = match outcome {
                Err(error)
                    if matches!(
                        error.execution_policy_disposition(),
                        Some(
                            crate::ExecutionPolicyDisposition::Retry
                                | crate::ExecutionPolicyDisposition::Reopen
                        )
                    ) =>
                {
                    Err(error)
                }
                outcome => emitted.and(outcome),
            };
            if !commands_open
                && !terminal_failure_committed
                && let Err(error) = &outcome
                && !matches!(
                    error,
                    NanocodeError::TurnCancelled | NanocodeError::TurnStopped
                )
            {
                shutdown_failures.push(error.to_string());
            }
            if outcome.is_err()
                && !terminal_failure_committed
                && let Some(base_checkpoint) = execution_base_checkpoint
            {
                latest_fork_checkpoint = base_checkpoint.clone();
                model = model_from_checkpoint(
                    &self.events,
                    &self.transport_stats,
                    &self.tools,
                    &self.spawner,
                    &prompt_cache,
                    base_checkpoint.as_deref(),
                );
            }
            model.set_events(self.events.clone());
            turn_span.record(
                "status",
                if was_cancelled {
                    "cancelled"
                } else if outcome.is_ok() {
                    "completed"
                } else {
                    "failed"
                },
            );
            turn_span.record(
                "otel.status_code",
                if outcome.is_ok() { "OK" } else { "ERROR" },
            );
            let reopen_after_turn = requires_reopen_after_turn(provider_requires_stop, &outcome);
            if commands_open && reopen_after_turn {
                begin_shutdown(
                    &mut self.commands,
                    &mut queued_turns,
                    default_thinking,
                    default_service_tier,
                )
                .await;
                commands_open = false;
            }
            drop(result.send(outcome));
            if reopen_after_turn {
                for (_, message_result) in pending_developer_messages.drain(..) {
                    message_result.fail(NanocodeError::AgentStopped);
                }
            }
            developer_checkpoint_ready =
                !reopen_after_turn && (terminal_failure_committed || execution_operation.is_none());
            if commands_open && reopen_after_turn {
                begin_shutdown(
                    &mut self.commands,
                    &mut queued_turns,
                    default_thinking,
                    default_service_tier,
                )
                .await;
                commands_open = false;
            }
            if let Some(cancel_result) = cancel_result {
                let outcome = cancellation_persisted.unwrap_or_else(|| {
                    if was_cancelled {
                        Ok(())
                    } else {
                        Err(NanocodeError::TurnNotCancellable)
                    }
                });
                drop(cancel_result.send(outcome));
            }
        }
    }
}

fn duplicate_policy_error(error: &NanocodeError) -> NanocodeError {
    match error {
        NanocodeError::ExecutionPolicy {
            layer,
            disposition,
            source,
        } => NanocodeError::ExecutionPolicy {
            layer,
            disposition: *disposition,
            source: Arc::clone(source),
        },
        error => NanocodeError::InvalidExecutionPolicy(error.to_string()),
    }
}

fn complete_developer_message(
    result: super::turn::DeveloperMessageResult,
    committed: Result<()>,
    context: impl FnOnce() -> Result<AgentSessionContext>,
) -> bool {
    match result {
        super::turn::DeveloperMessageResult::Context(result) => {
            let outcome = committed.and_then(|()| context());
            let reopen = outcome_requires_reopen(&outcome);
            drop(result.send(outcome));
            reopen
        }
        super::turn::DeveloperMessageResult::Notice { result, .. } => {
            let outcome = committed.map(|()| true);
            let reopen = outcome_requires_reopen(&outcome);
            drop(result.send(outcome));
            reopen
        }
    }
}

fn outcome_requires_reopen<T>(outcome: &Result<T>) -> bool {
    outcome.as_ref().is_err_and(|error| {
        matches!(
            error.execution_policy_disposition(),
            Some(crate::ExecutionPolicyDisposition::Reopen)
        ) || error_requires_stop(error)
    })
}

fn requires_reopen_after_turn<T>(provider_requires_stop: bool, outcome: &Result<T>) -> bool {
    provider_requires_stop || outcome_requires_reopen(outcome)
}

fn error_requires_stop(error: &NanocodeError) -> bool {
    match error {
        NanocodeError::CompactionFailed {
            requires_session_stop,
            ..
        } => *requires_session_stop,
        NanocodeError::Shutdown(source) => error_requires_stop(source),
        _ => error
            .responses_error()
            .is_some_and(|source| source.is_misalignment_policy_violation()),
    }
}

#[allow(clippy::too_many_arguments)]
async fn accept_turn_steer(
    execution: &Execution,
    events: &EventSink,
    steers: &SteerSender,
    preempt: &watch::Sender<u64>,
    accepted: &mut Vec<(Option<String>, SteerReceipt)>,
    id: Option<String>,
    execution_turn: &ExecutionTurn,
    model_call_index: &std::sync::atomic::AtomicU32,
    prompt: Prompt,
) -> Result<()> {
    let steers = steers.upgrade().ok_or(NanocodeError::TurnNotSteerable)?;
    accepted.retain(|receipt| receipt.1.delivery.get() == SteerDelivery::Pending);

    let call_index = model_call_index.load(std::sync::atomic::Ordering::Acquire);
    let Some(steer) = execution_turn
        .accept_steer(prompt.clone(), id.clone(), call_index)
        .await?
    else {
        return Ok(());
    };
    execution
        .accepted_input(events, &prompt, "steer", id.as_deref())
        .await?;
    accepted.push((
        id,
        SteerReceipt {
            durable_index: steer.durable_index,
            delivery: Arc::clone(&steer.delivery),
        },
    ));
    steers.push(steer);
    preempt.send_modify(|generation| *generation = generation.wrapping_add(1));
    Ok(())
}

async fn commit_developer_message(
    model: &mut ModelRun,
    execution: &Execution,
    lineage_id: Arc<str>,
    (model_name, thinking, service_tier, reasoning_mode): (
        Model,
        Thinking,
        ServiceTier,
        ReasoningMode,
    ),
    text: String,
    workspace: Option<&str>,
) -> Result<Option<Arc<CommittedSession>>> {
    let snapshot = model.append_developer_message(text, workspace)?;
    let checkpoint = Arc::new(CommittedSession::new(
        lineage_id,
        model_name,
        thinking,
        service_tier,
        reasoning_mode,
        snapshot,
    ));
    execution.commit_checkpoint(&checkpoint).await?;
    Ok(Some(checkpoint))
}

fn model_from_checkpoint(
    events: &EventSink,
    transport_stats: &Arc<TransportStats>,
    tools: &Tools,
    spawner: &BranchSpawner,
    prompt_cache: &ModelPromptCache,
    checkpoint: Option<&CommittedSession>,
) -> ModelRun {
    let client = (spawner.service_factory)(Arc::clone(&spawner.config));
    let mut model = if let Some(checkpoint) = checkpoint {
        let prepared = prepare_checkpoint(
            checkpoint.model().clone(),
            &spawner.config,
            tools,
            spawner.context_source.clone(),
        );
        ModelRun::from_checkpoint(
            events.clone(),
            Arc::clone(&spawner.config),
            client,
            Arc::clone(transport_stats),
            tools.clone(),
            prompt_cache.clone(),
            prepared,
            spawner.host_context.as_ref().map(Arc::clone),
        )
    } else {
        ModelRun::new(
            events.clone(),
            Arc::clone(&spawner.provider_session_id),
            Arc::clone(&spawner.config),
            client,
            Arc::clone(transport_stats),
            tools.clone(),
            prompt_cache.clone(),
            spawner.context_source.clone(),
            spawner.host_context.as_ref().map(Arc::clone),
        )
    };
    model.set_before_compaction(spawner.before_compaction.clone());
    model.set_images(spawner.images.clone());
    model
}

fn settle_cancelled_queued_turn(
    queued_turns: &mut VecDeque<QueuedTurn>,
    target: TurnKey,
    recovered: bool,
    committed: bool,
    config: &ModelConfig,
) -> bool {
    let durable = queued_turns.iter().position(|queued| {
        matches!(
            queued,
            QueuedTurn::Pending { key, execution_operation: Some(_), .. } if *key == target
        )
    });
    match durable {
        Some(position) if committed => {
            let Some(QueuedTurn::Pending {
                thinking,
                events,
                result,
                ..
            }) = queued_turns.remove(position)
            else {
                return false;
            };
            let outcome = emit_replayed_terminal(
                &events,
                config,
                thinking,
                "cancelled",
                &TurnUsage::default(),
            )
            .and(Err(NanocodeError::TurnCancelled));
            drop(result.send(outcome));
            true
        }
        _ => cancel_queued_turn(queued_turns, target, !recovered),
    }
}

async fn accept_execution_command(
    execution: &Execution,
    checkpoints: &Arc<CheckpointSource>,
    config: &ModelConfig,
    default_thinking: Thinking,
    command: Command,
    reopen: &mut bool,
) -> Option<Command> {
    if let Command::Prompt {
        prompt,
        events,
        execution_operation: None,
        ..
    } = &command
    {
        if let Err(error) = execution
            .accepted_input(events, prompt, "prompt", None)
            .await
        {
            if let Command::Prompt { result, .. } = command {
                drop(result.send(Err(error)));
            }
            return None;
        }
        return Some(command);
    }
    let Command::Prompt {
        key,
        prompt,
        execution_operation: Some(operation),
        accepted: Some(accepted),
        cancel_on_admission,
        thinking,
        service_tier,
        parent,
        events,
        result,
    } = command
    else {
        return Some(command);
    };
    let replay_thinking = thinking.unwrap_or(default_thinking);
    let caller = matches!(
        operation,
        ExecutionOperation::Caller(_) | ExecutionOperation::AdmittedCaller(_)
    );
    let admission = match operation {
        ExecutionOperation::Caller(operation_id) => execution
            .admit(&operation_id, &prompt)
            .await
            .map(|admission| (operation_id, admission)),
        ExecutionOperation::Automatic(candidate_operation_id) => {
            execution
                .admit_automatic(candidate_operation_id, &prompt)
                .await
        }
        ExecutionOperation::Admitted(operation_id)
        | ExecutionOperation::AdmittedCaller(operation_id) => {
            Ok((operation_id, AdmittedExecution::Execute))
        }
        ExecutionOperation::Recovered(operation_id) => {
            Ok((operation_id, AdmittedExecution::Resume))
        }
    };
    match admission {
        Ok((operation_id, AdmittedExecution::Execute)) => {
            if let Err(error) = execution
                .accepted_input(&events, &prompt, "prompt", Some(&operation_id))
                .await
            {
                execution.release_claim(&operation_id).await;
                drop(accepted.send(Err(error)));
                return None;
            }
            drop(accepted.send(Ok(operation_id.clone())));
            Some(Command::Prompt {
                key,
                prompt,
                execution_operation: Some(if caller {
                    ExecutionOperation::AdmittedCaller(operation_id)
                } else {
                    ExecutionOperation::Admitted(operation_id)
                }),
                accepted: None,
                cancel_on_admission,
                thinking,
                service_tier,
                parent,
                events,
                result,
            })
        }
        Ok((operation_id, AdmittedExecution::Resume)) => {
            if let Err(error) = execution
                .accepted_input(&events, &prompt, "prompt", Some(&operation_id))
                .await
            {
                execution.release_claim(&operation_id).await;
                drop(accepted.send(Err(error)));
                return None;
            }
            drop(accepted.send(Ok(operation_id.clone())));
            Some(Command::Prompt {
                key,
                prompt,
                execution_operation: Some(ExecutionOperation::Recovered(operation_id)),
                accepted: None,
                cancel_on_admission,
                thinking,
                service_tier,
                parent,
                events,
                result,
            })
        }
        Ok((operation_id, AdmittedExecution::Completed { output, snapshot })) => {
            if accepted.send(Ok(operation_id.clone())).is_err() {
                return None;
            }
            if let Err(error) =
                emit_replayed_terminal(&events, config, replay_thinking, "completed", &output.usage)
            {
                drop(result.send(Err(error)));
                return None;
            }
            drop(result.send(Ok(TurnResult {
                request_id: Some(operation_id),
                turn_id: None,
                final_message: output.final_message,
                usage: Some(output.usage),
                boundary: Some(checkpoints.replayed(
                    snapshot,
                    config.model,
                    replay_thinking,
                    service_tier.unwrap_or(config.service_tier),
                )),
            })));
            None
        }
        Ok((operation_id, AdmittedExecution::Failed { error })) => {
            if accepted.send(Ok(operation_id)).is_err() {
                return None;
            }
            if let Err(event_error) = emit_replayed_terminal(
                &events,
                config,
                replay_thinking,
                "failed",
                &TurnUsage::default(),
            ) {
                drop(result.send(Err(event_error)));
                return None;
            }
            drop(result.send(Err(NanocodeError::ReplayedExecutionFailed(error))));
            None
        }
        Ok((operation_id, AdmittedExecution::Cancelled)) => {
            if accepted.send(Ok(operation_id)).is_err() {
                return None;
            }
            if let Err(error) = emit_replayed_terminal(
                &events,
                config,
                replay_thinking,
                "cancelled",
                &TurnUsage::default(),
            ) {
                drop(result.send(Err(error)));
                return None;
            }
            drop(result.send(Err(NanocodeError::TurnCancelled)));
            None
        }
        Err(error) => {
            *reopen = matches!(
                error.execution_policy_disposition(),
                Some(crate::ExecutionPolicyDisposition::Reopen)
            );
            drop(accepted.send(Err(error)));
            None
        }
    }
}

async fn accept_idle_route(
    execution: &Execution,
    checkpoints: &Arc<CheckpointSource>,
    config: &ModelConfig,
    default_thinking: Thinking,
    command: Command,
    reopen: &mut bool,
) -> Option<Command> {
    let Command::RoutePrompt {
        key,
        prompt,
        request_id,
        cancel_on_admission,
        parent,
        events,
        turn_result,
        route_result,
    } = command
    else {
        return Some(command);
    };
    if let Some(id) = &request_id {
        match execution
            .identified_steer_accepted(id.clone(), &prompt)
            .await
        {
            Ok(true) => {
                drop(route_result.send(Ok(PromptRouteKind::Steered)));
                return None;
            }
            Ok(false) => {}
            Err(error) => {
                drop(route_result.send(Err(error)));
                return None;
            }
        }
    }
    if !execution.identifies_prompts() {
        if let Err(error) = execution
            .accepted_input(&events, &prompt, "prompt", None)
            .await
        {
            drop(route_result.send(Err(error)));
            return None;
        }
        drop(route_result.send(Ok(PromptRouteKind::Started { request_id: None })));
        return Some(Command::Prompt {
            key,
            prompt,
            execution_operation: None,
            accepted: None,
            cancel_on_admission,
            thinking: None,
            service_tier: None,
            parent,
            events,
            result: turn_result,
        });
    }

    let caller = request_id.is_some();
    let admission = execution
        .admit_automatic(
            request_id.unwrap_or_else(|| SessionId::new().to_string()),
            &prompt,
        )
        .await;
    match admission {
        Ok((operation_id, AdmittedExecution::Execute)) => {
            if let Err(error) = execution
                .accepted_input(&events, &prompt, "prompt", Some(&operation_id))
                .await
            {
                execution.release_claim(&operation_id).await;
                drop(route_result.send(Err(error)));
                return None;
            }
            drop(route_result.send(Ok(PromptRouteKind::Started {
                request_id: Some(operation_id.clone()),
            })));
            Some(Command::Prompt {
                key,
                prompt,
                execution_operation: Some(if caller {
                    ExecutionOperation::AdmittedCaller(operation_id)
                } else {
                    ExecutionOperation::Admitted(operation_id)
                }),
                accepted: None,
                cancel_on_admission,
                thinking: None,
                service_tier: None,
                parent,
                events,
                result: turn_result,
            })
        }
        Ok((operation_id, AdmittedExecution::Resume)) => {
            if let Err(error) = execution
                .accepted_input(&events, &prompt, "prompt", Some(&operation_id))
                .await
            {
                execution.release_claim(&operation_id).await;
                drop(route_result.send(Err(error)));
                return None;
            }
            drop(route_result.send(Ok(PromptRouteKind::Started {
                request_id: Some(operation_id.clone()),
            })));
            Some(Command::Prompt {
                key,
                prompt,
                execution_operation: Some(ExecutionOperation::Recovered(operation_id)),
                accepted: None,
                cancel_on_admission,
                thinking: None,
                service_tier: None,
                parent,
                events,
                result: turn_result,
            })
        }
        Ok((operation_id, AdmittedExecution::Completed { output, snapshot })) => {
            if route_result
                .send(Ok(PromptRouteKind::Started {
                    request_id: Some(operation_id.clone()),
                }))
                .is_err()
            {
                return None;
            }
            if let Err(error) = emit_replayed_terminal(
                &events,
                config,
                default_thinking,
                "completed",
                &output.usage,
            ) {
                drop(turn_result.send(Err(error)));
                return None;
            }
            drop(turn_result.send(Ok(TurnResult {
                request_id: Some(operation_id),
                turn_id: None,
                final_message: output.final_message,
                usage: Some(output.usage),
                boundary: Some(checkpoints.replayed(
                    snapshot,
                    config.model,
                    default_thinking,
                    config.service_tier,
                )),
            })));
            None
        }
        Ok((operation_id, AdmittedExecution::Failed { error })) => {
            if route_result
                .send(Ok(PromptRouteKind::Started {
                    request_id: Some(operation_id),
                }))
                .is_err()
            {
                return None;
            }
            if let Err(event_error) = emit_replayed_terminal(
                &events,
                config,
                default_thinking,
                "failed",
                &TurnUsage::default(),
            ) {
                drop(turn_result.send(Err(event_error)));
                return None;
            }
            drop(turn_result.send(Err(NanocodeError::ReplayedExecutionFailed(error))));
            None
        }
        Ok((operation_id, AdmittedExecution::Cancelled)) => {
            if route_result
                .send(Ok(PromptRouteKind::Started {
                    request_id: Some(operation_id),
                }))
                .is_err()
            {
                return None;
            }
            if let Err(error) = emit_replayed_terminal(
                &events,
                config,
                default_thinking,
                "cancelled",
                &TurnUsage::default(),
            ) {
                drop(turn_result.send(Err(error)));
                return None;
            }
            drop(turn_result.send(Err(NanocodeError::TurnCancelled)));
            None
        }
        Err(error) => {
            *reopen = matches!(
                error.execution_policy_disposition(),
                Some(crate::ExecutionPolicyDisposition::Reopen)
            );
            drop(route_result.send(Err(error)));
            None
        }
    }
}

pub(super) fn agent_session_context(
    checkpoint: Option<&CommittedSession>,
    configured_workspace: Option<&str>,
    context_source: &ContextSource,
) -> Result<AgentSessionContext> {
    let workspace = checkpoint
        .map(|checkpoint| checkpoint.model().workspace().to_owned())
        .or_else(|| configured_workspace.map(str::to_owned))
        .map_or_else(|| context_source.resolve_workspace(None), Ok)?;
    Ok(AgentSessionContext::from_backend(
        workspace,
        checkpoint.map_or_else(Vec::new, |checkpoint| checkpoint.model().snapshot_history()),
    ))
}

use crate::ErrorExt as _;
