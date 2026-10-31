use super::*;
use crate::agent::execution::ExecutionFuture;
use crate::client::transport::layer::origin;

impl ModelRun {
    fn start_timing(&mut self) {
        self.started_at = Instant::now();
        self.operation_started_at = nanocode_platform::system_time()
            .duration_since(UNIX_EPOCH)
            .ok();
        self.elapsed_before_attempt = Duration::ZERO;
    }

    pub(crate) async fn compact(
        &mut self,
        requested_workspace: Option<Arc<str>>,
        thinking: Thinking,
        service_tier: ServiceTier,
        logical_turn: u64,
        cancel: &mut tokio::sync::oneshot::Receiver<()>,
        execution_steps: Option<ExecutionSteps>,
    ) -> Result<ModelCompactOutcome> {
        let configured = (Arc::clone(&self.config), self.model);
        let origin = origin::Origin {
            context: self.host_context.clone(),
            session: Some(Arc::from(self.events.request_id())),
        };
        let execution: ExecutionFuture<'_, Result<ModelCompactOutcome>> =
            Box::pin(self.compact_inner(
                requested_workspace,
                thinking,
                service_tier,
                logical_turn,
                cancel,
                execution_steps,
            ));
        let outcome = origin.scope(execution).await;
        self.restore_runtime(configured, logical_turn)?;
        outcome
    }

    async fn compact_inner(
        &mut self,
        requested_workspace: Option<Arc<str>>,
        thinking: Thinking,
        service_tier: ServiceTier,
        logical_turn: u64,
        cancel: &mut tokio::sync::oneshot::Receiver<()>,
        execution_steps: Option<ExecutionSteps>,
    ) -> Result<ModelCompactOutcome> {
        self.execution_steps = execution_steps;
        self.thinking = thinking;
        self.service_tier = service_tier;
        self.start_timing();
        self.stats = RunStats::default();
        self.transport_baseline = self.transport_stats.snapshot();
        let restored = self
            .restore_execution(requested_workspace.as_deref(), logical_turn)
            .await?;
        let resumed = restored.is_some();
        let mut session = match restored {
            Some((session, ExecutionPhase::Compact)) => session,
            Some(_) => {
                return Err(NanocodeError::InvalidExecutionPolicy(
                    "invalid compaction continuation".into(),
                ));
            }
            None => match self.session.take() {
                Some(session) => session,
                None => self.empty_session(requested_workspace.as_deref())?,
            },
        };
        session.factory = session.factory.for_logical_turn(logical_turn);
        if let Err(error) = session.validate_workspace(requested_workspace.as_deref()) {
            let checkpoint =
                Self::checkpoint_from_session(&session, false, self.global_instructions.clone());
            self.session = Some(session);
            return Ok(ModelCompactOutcome::Failed { error, checkpoint });
        }
        session
            .conversation
            .prepare_request_policy(self.continuation_policy());

        if !resumed {
            self.retain_execution(&mut session, ExecutionPhase::Compact)
                .await?;
        }
        let active_context_tokens = session.conversation.active_context_tokens();
        let auto_compact_token_limit = self
            .auto_compact_token_limit()
            .unwrap_or(self.config.context_window_tokens);
        let compacted = {
            let compaction = self.perform_compaction(
                self.stats.model_calls,
                session.conversation.managed.compaction_request(),
                active_context_tokens,
                auto_compact_token_limit,
                &session.factory,
                session.conversation.reasoning.request_effort(
                    self.model,
                    self.thinking,
                    self.config.supports_reasoning_effort_updates(self.model),
                ),
            );
            tokio::pin!(compaction);
            tokio::select! {
                biased;
                _ = &mut *cancel => None,
                outcome = &mut compaction => Some(outcome),
            }
        };
        let Some(compacted) = compacted else {
            session.conversation.reset_for_full_request();
            let checkpoint =
                Self::checkpoint_from_session(&session, false, self.global_instructions.clone());
            self.session = Some(session);
            return Ok(ModelCompactOutcome::Cancelled(checkpoint));
        };
        let (item, _usage, server_reasoning_included) = match compacted {
            Ok(compacted) => compacted,
            Err(error) => {
                session
                    .conversation
                    .managed
                    .repair_rejected_request(error.rejected_request_repair());
                session.conversation.reset_for_full_request();
                let checkpoint = Self::checkpoint_from_session(
                    &session,
                    false,
                    self.global_instructions.clone(),
                );
                self.session = Some(session);
                return Ok(ModelCompactOutcome::Failed { error, checkpoint });
            }
        };
        session
            .conversation
            .observe_server_reasoning(server_reasoning_included);
        session
            .conversation
            .install_pre_turn_compaction(item, session.factory.profile().prefix());
        session.conversation.commit_tail();
        session.context.require_full_reinjection();
        session.preserve_inherited_delta = false;
        self.force_compaction = false;
        let checkpoint =
            Self::checkpoint_from_session(&session, false, self.global_instructions.clone());
        self.session = Some(session);
        Ok(ModelCompactOutcome::Completed(checkpoint))
    }

    pub(crate) fn emit_cancelled_before_start(
        &mut self,
        task: &Prompt,
        workspace: Option<&str>,
        thinking: Thinking,
        service_tier: ServiceTier,
        error: &NanocodeError,
    ) -> Result<()> {
        self.thinking = thinking;
        self.service_tier = service_tier;
        self.start_timing();
        self.stats = RunStats::default();
        self.events.emit(
            AgentEventKind::RunStarted,
            RunStarted {
                mode: "openai_model",
                model: self.model.as_str(),
                reasoning_mode: self.config.reasoning_mode.as_str(),
                effort: self.thinking.as_str(),
                transport: self.config.responses_transport.as_str(),
                orchestration: ModelConfig::orchestration(),
                websocket_url: display_endpoint(self.responses_endpoint()),
                workspace,
                instruction_bytes: task.text_bytes(),
            },
        )?;
        let message = error.to_string();
        self.events
            .emit(AgentEventKind::RunError, RunError { message: &message })?;
        let usage = self.stats.turn_usage();
        record_turn_usage(&tracing::Span::current(), &usage);
        self.events.emit(
            AgentEventKind::RunFailed,
            terminal_payload(
                "cancelled",
                self.started_at.elapsed(),
                &self.config,
                self.model,
                self.thinking,
                &self.stats,
                &usage,
            ),
        )?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn emit_failed_before_start(
        &mut self,
        task: &Prompt,
        workspace: Option<&str>,
        thinking: Thinking,
        service_tier: ServiceTier,
        error: &NanocodeError,
    ) -> Result<()> {
        if matches!(
            error.execution_policy_disposition(),
            Some(
                crate::ExecutionPolicyDisposition::Retry
                    | crate::ExecutionPolicyDisposition::Reopen
            )
        ) {
            return Ok(());
        }
        self.thinking = thinking;
        self.service_tier = service_tier;
        self.start_timing();
        self.stats = RunStats::default();
        self.events.emit(
            AgentEventKind::RunStarted,
            RunStarted {
                mode: "openai_model",
                model: self.model.as_str(),
                reasoning_mode: self.config.reasoning_mode.as_str(),
                effort: self.thinking.as_str(),
                transport: self.config.responses_transport.as_str(),
                orchestration: ModelConfig::orchestration(),
                websocket_url: display_endpoint(self.responses_endpoint()),
                workspace,
                instruction_bytes: task.text_bytes(),
            },
        )?;
        let message = error.to_string();
        self.events
            .emit(AgentEventKind::RunError, RunError { message: &message })?;
        self.emit_terminal("failed")
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn execute(
        &mut self,
        task: Prompt,
        workspace: Option<Arc<str>>,
        thinking: Thinking,
        service_tier: ServiceTier,
        logical_turn: u64,
        steering: TurnSteering,
        mut cancel: tokio::sync::oneshot::Receiver<()>,
        fork_snapshots: watch::Sender<Option<ModelCheckpoint>>,
        execution_steps: Option<ExecutionSteps>,
    ) -> Result<ModelTurnOutcome> {
        self.execution_steps = execution_steps;
        self.instruction_revision = task.instruction_revision();
        self.thinking = thinking;
        self.service_tier = service_tier;
        self.start_timing();
        self.stats = RunStats::default();
        if let Some(tools) = &self.active_tools {
            tools.begin_turn();
        }
        self.transport_baseline = self.transport_stats.snapshot();
        self.events.emit(
            AgentEventKind::RunStarted,
            RunStarted {
                mode: "openai_model",
                model: self.model.as_str(),
                reasoning_mode: self.config.reasoning_mode.as_str(),
                effort: self.thinking.as_str(),
                transport: self.config.responses_transport.as_str(),
                orchestration: ModelConfig::orchestration(),
                websocket_url: display_endpoint(self.responses_endpoint()),
                workspace: workspace.as_deref(),
                instruction_bytes: task.text_bytes(),
            },
        )?;

        let configured = (Arc::clone(&self.config), self.model);
        let origin = origin::Origin {
            context: self.host_context.clone(),
            session: Some(Arc::from(self.events.request_id())),
        };
        let execution: ExecutionFuture<'_, Result<ModelTaskOutcome>> = Box::pin(self.execute_task(
            task,
            workspace,
            logical_turn,
            steering,
            &mut cancel,
            &fork_snapshots,
        ));
        let outcome = origin.scope(execution).await;
        self.restore_runtime(configured, logical_turn)?;
        match outcome {
            Ok(ModelTaskOutcome::Completed(message)) => {
                self.terminate_turn_cells().await;
                self.record_transport();
                let usage = self.stats.turn_usage();
                record_turn_usage(&tracing::Span::current(), &usage);
                let checkpoint = self.commit_checkpoint()?;
                Ok(ModelTurnOutcome::Completed(CompletedModelTurn {
                    final_message: message,
                    usage,
                    checkpoint,
                }))
            }
            Ok(ModelTaskOutcome::Cancelled) => {
                self.terminate_turn_cells().await;
                let checkpoint = self.commit_cancelled_checkpoint().await?;
                let error = NanocodeError::TurnCancelled;
                let message = error.to_string();
                self.events
                    .emit(AgentEventKind::RunError, RunError { message: &message })?;
                self.record_transport();
                let usage = self.stats.turn_usage();
                record_turn_usage(&tracing::Span::current(), &usage);
                Ok(ModelTurnOutcome::Cancelled(checkpoint))
            }
            Err(error) => {
                if self.execution_steps.is_some()
                    && error.execution_policy_disposition()
                        == Some(crate::ExecutionPolicyDisposition::Reopen)
                {
                    if let Some(tools) = &self.active_tools {
                        tools.cancel_turn().await;
                    }
                    return Err(error);
                }
                if error
                    .responses_error()
                    .is_some_and(ResponsesError::is_context_window_exceeded)
                {
                    self.force_compaction = true;
                }
                let checkpoint = if self.active_tool_calls.is_empty() {
                    self.detach_turn_cells().await;
                    self.finish_active_tool_batch_wall();

                    if let Some(session) = &mut self.session {
                        session
                            .conversation
                            .managed
                            .repair_rejected_request(error.rejected_request_repair());
                        session.conversation.commit_interrupted();
                        session.preserve_inherited_delta = false;
                    }
                    self.session.as_ref().map(|session| {
                        Self::checkpoint_from_session(
                            session,
                            false,
                            self.global_instructions.clone(),
                        )
                    })
                } else {
                    self.terminate_turn_cells().await;
                    Some(self.commit_interrupted_checkpoint()?)
                };
                let message = error.to_string();
                self.events
                    .emit(AgentEventKind::RunError, RunError { message: &message })?;
                self.record_transport();
                let usage = self.stats.turn_usage();
                record_turn_usage(&tracing::Span::current(), &usage);
                match checkpoint {
                    Some(checkpoint) => Ok(ModelTurnOutcome::Failed { error, checkpoint }),
                    None => Err(error),
                }
            }
        }
    }

    async fn detach_turn_cells(&mut self) {
        let Some(tools) = &self.active_tools else {
            return;
        };
        let events = self.events.clone();
        let indices = self.tool_call_indices.clone();
        tools
            .detach_turn_with_updates(&mut |origin| {
                Box::new(DetachedNestedToolEvents::new(
                    events.clone(),
                    indices.clone(),
                    origin,
                ))
            })
            .await;
    }

    async fn terminate_turn_cells(&mut self) {
        let Some(tools) = &self.active_tools else {
            return;
        };
        let mut observer = TurnTeardownEvents::new(
            &self.events,
            &self.tool_call_indices,
            &self.active_tool_calls,
        );
        tools.cancel_turn_with_updates(&mut observer).await;
        let terminated = std::mem::take(&mut observer.terminated);
        drop(observer);
        self.terminated_cell_output.extend(terminated);
    }

    pub(crate) fn emit_terminal(&self, status: &'static str) -> Result<()> {
        let usage = self.stats.turn_usage();
        let kind = if status == "completed" {
            AgentEventKind::RunCompleted
        } else {
            AgentEventKind::RunFailed
        };
        self.events.emit(
            kind,
            terminal_payload(
                status,
                self.elapsed_before_attempt
                    .saturating_add(self.started_at.elapsed()),
                &self.config,
                self.model,
                self.thinking,
                &self.stats,
                &usage,
            ),
        )?;
        Ok(())
    }

    pub(super) async fn prepare_follow_on_turn(
        &mut self,
        session: &mut ModelSessionState,
        task: &Prompt,
        cancel: &mut tokio::sync::oneshot::Receiver<()>,
    ) -> Result<bool> {
        let compacted = {
            let compaction = self.maybe_compact(
                self.stats.model_calls,
                &mut session.conversation,
                &session.factory,
                CompactionContext {
                    snapshot: session.context.snapshot(),
                    phase: CompactionPhase::PreTurn,
                },
            );
            tokio::pin!(compaction);
            tokio::select! {
                biased;
                _ = &mut *cancel => None,
                outcome = &mut compaction => Some(outcome?),
            }
        };
        let Some(compacted) = compacted else {
            let user_content = self
                .images
                .prepare_user_input(&task.instruction)
                .await
                .map_err(nanocode_durability::agent_store_error)?;
            session
                .conversation
                .append(prompt_messages(task, user_content));
            return Ok(false);
        };
        if compacted || session.preserve_inherited_delta {
            session.preserve_inherited_delta = false;
        } else {
            session.conversation.clear_delta();
        }
        if compacted {
            session.context.require_full_reinjection();
        }
        let current_context = session.context.capture(
            session.tools.working_directory(),
            session.tools.default_shell_name(),
            self.context_source.execution_environment(),
        );
        let canonical_context = current_context.full_item();
        let host_instructions = self.config.host_instructions();
        if let Some(update) = session.context.update(current_context) {
            if update.full {
                session.conversation.append_canonical_context(
                    developer_context(&self.config, &self.tools),
                    update.item,
                );
                session
                    .context
                    .observe_host_instructions(&host_instructions);
            } else {
                session.conversation.append([update.item]);
                if session
                    .context
                    .observe_host_instructions(&host_instructions)
                {
                    session
                        .conversation
                        .append([host_context(host_instructions)]);
                }
                session
                    .conversation
                    .set_canonical_context(canonical_context);
            }
        } else {
            session
                .conversation
                .set_canonical_context(canonical_context);
            if session
                .context
                .observe_host_instructions(&host_instructions)
            {
                session
                    .conversation
                    .append([host_context(host_instructions)]);
            }
        }
        let user_content = self
            .images
            .prepare_user_input(&task.instruction)
            .await
            .map_err(nanocode_durability::agent_store_error)?;
        session
            .conversation
            .append(prompt_messages(task, user_content));
        Ok(true)
    }

    pub(super) async fn execute_task(
        &mut self,
        task: Prompt,
        requested_workspace: Option<Arc<str>>,
        logical_turn: u64,
        steering: TurnSteering,
        cancel: &mut tokio::sync::oneshot::Receiver<()>,
        fork_snapshots: &watch::Sender<Option<ModelCheckpoint>>,
    ) -> Result<ModelTaskOutcome> {
        let restored = self
            .restore_execution(requested_workspace.as_deref(), logical_turn)
            .await?;
        let resumed = restored.is_some();
        let phase;
        let mut session = if let Some((session, saved_phase)) = restored {
            phase = saved_phase;
            self.session = None;
            session
        } else if let Some(mut session) = self.session.take() {
            session.factory = session.factory.for_logical_turn(logical_turn);
            if let Err(error) = session.validate_workspace(requested_workspace.as_deref()) {
                self.session = Some(session);
                return Err(error);
            }
            session
                .conversation
                .prepare_request_policy(self.continuation_policy());
            phase = ExecutionPhase::PrepareTurn;
            session
        } else {
            let workspace = requested_workspace.map_or_else(
                || self.context_source.resolve_workspace(None),
                |workspace| Ok(workspace.to_string()),
            )?;
            let selected_agents_md = self
                .context_source
                .project_instructions(&workspace)
                .map(Arc::<str>::from);
            let tools = tool_runtime(&workspace, &self.config, &self.tools);
            let tool_control = tools.control();
            tool_control.begin_turn();
            self.active_tools = Some(tool_control);
            let factory = self.attempt_factory(&tools)?.for_logical_turn(logical_turn);
            let user_content = self
                .images
                .prepare_user_input(&task.instruction)
                .await
                .map_err(nanocode_durability::agent_store_error)?;
            let mut context = ContextState::new(selected_agents_md, ContextBaseline::Missing);
            context.observe_host_instructions(&self.config.host_instructions());
            let context_snapshot = context.capture(
                tools.working_directory(),
                tools.default_shell_name(),
                self.context_source.execution_environment(),
            );
            let mut history = task_input(
                &self.config,
                &self.tools,
                &task,
                user_content,
                &context_snapshot,
            );
            let mut pending = std::mem::take(&mut self.pending_developer_messages);
            for item in &mut pending {
                assign_missing_response_item_id(item);
            }
            let client_authored = pending
                .iter()
                .filter_map(|item| item.id().map(ToString::to_string))
                .collect();
            history.splice(2..2, pending);
            context.establish(context_snapshot);
            let mut conversation = ConversationState::new(history)?;
            conversation
                .managed
                .restore_client_authored(client_authored);
            let mut session = ModelSessionState {
                workspace,
                tools,
                factory,
                conversation,
                context,
                preserve_inherited_delta: false,
            };
            session
                .conversation
                .prepare_request_policy(self.continuation_policy());
            Self::publish_fork_snapshot(
                &mut session,
                fork_snapshots,
                self.global_instructions.as_ref(),
            );
            phase = ExecutionPhase::Warmup;
            session
        };

        if !resumed {
            if phase == ExecutionPhase::Warmup && !self.sends_warmup() {
                session.conversation.commit_tail();
            } else {
                self.retain_execution(&mut session, phase).await?;
            }
        }
        match phase {
            ExecutionPhase::PrepareTurn => {
                match self
                    .prepare_follow_on_turn(&mut session, &task, cancel)
                    .await
                {
                    Ok(true) => {}
                    Ok(false) => {
                        self.session = Some(session);
                        return Ok(ModelTaskOutcome::Cancelled);
                    }
                    Err(error) => {
                        self.session = Some(session);
                        return Err(error);
                    }
                }
            }
            ExecutionPhase::Warmup => {
                let request_effort = session.conversation.reasoning.request_effort(
                    self.model,
                    self.thinking,
                    self.config.supports_reasoning_effort_updates(self.model),
                );
                let warmup = {
                    let warmup = self.perform_warmup(&session.factory, request_effort);
                    tokio::pin!(warmup);
                    tokio::select! {
                        biased;
                        _ = &mut *cancel => None,
                        outcome = &mut warmup => Some(outcome),
                    }
                };
                let Some(warmup) = warmup else {
                    self.session = Some(session);
                    return Ok(ModelTaskOutcome::Cancelled);
                };
                match warmup {
                    Ok(outcome) => {
                        if outcome.baseline_established {
                            session.conversation.reasoning.pin(
                                self.model,
                                self.thinking,
                                self.config.supports_reasoning_effort_updates(self.model),
                            );
                        }
                        session
                            .conversation
                            .observe_server_reasoning(outcome.server_reasoning_included);
                        if let Some(response_id) = outcome.response_id {
                            session.conversation.set_previous_response_id(response_id);
                        } else {
                            session.conversation.reset_for_full_request();
                            self.stats.last_response_id = None;
                        }
                    }
                    Err(error)
                        if error
                            .responses_error()
                            .is_some_and(|source| source.is_misalignment_policy_violation()) =>
                    {
                        self.session = Some(session);
                        return Err(error);
                    }
                    Err(error) if error.responses_error().is_some() => {
                        session.conversation.reset_for_full_request();
                        self.stats.last_response_id = None;
                    }
                    Err(error) => {
                        self.session = Some(session);
                        return Err(error);
                    }
                }
            }
            ExecutionPhase::Generate => {}
            ExecutionPhase::Compact => {
                return Err(NanocodeError::InvalidExecutionPolicy(
                    "invalid turn continuation".into(),
                ));
            }
        }

        let outcome = {
            let task = self.drive_session(
                &mut session,
                steering,
                resumed && phase == ExecutionPhase::Generate,
                fork_snapshots,
            );
            tokio::pin!(task);
            tokio::select! {
                biased;
                _ = &mut *cancel => None,
                outcome = &mut task => Some(outcome),
            }
        };
        self.session = Some(session);
        match outcome {
            Some(outcome) => outcome.map(ModelTaskOutcome::Completed),
            None => Ok(ModelTaskOutcome::Cancelled),
        }
    }

    pub(super) fn continuation_policy(&self) -> ContinuationPolicy {
        ContinuationPolicy {
            model: self.model,
            thinking: self.thinking,
            service_tier: self.service_tier,
            reasoning_effort_updates: self.config.supports_reasoning_effort_updates(self.model),
        }
    }

    pub(super) fn commit_checkpoint(&mut self) -> Result<ModelCheckpoint> {
        let session = self
            .session
            .as_mut()
            .ok_or(NanocodeError::InvalidAttemptState {
                detail: "completed turn did not have a model session",
            })?;
        if self.stats.last_response_id.is_some() {
            session.conversation.commit()?;
        } else {
            session.conversation.commit_tail();
        }
        Ok(Self::checkpoint_from_session(
            session,
            false,
            self.global_instructions.clone(),
        ))
    }

    pub(super) fn commit_interrupted_checkpoint(&mut self) -> Result<ModelCheckpoint> {
        let mut aborted_outputs = Vec::with_capacity(self.active_tool_calls.len());
        let mut terminated = std::mem::take(&mut self.terminated_cell_output);
        for call in std::mem::take(&mut self.active_tool_calls) {
            let completed = call.completion.take();
            if let Some(completed) = completed {
                aborted_outputs.extend(self.finish_completed_tool_call(completed, &call.progress)?);
                continue;
            }
            let active_nested_tool_calls = self.finish_active_tool_progress(&call.progress);
            for nested_call in active_nested_tool_calls {
                let started_after_ns = nested_call.started_after_ns(call.started_at);
                self.emit_cancelled_tool_result(
                    &nested_call.call_id,
                    &nested_call.tool,
                    nested_call.started_at,
                    Some(started_after_ns),
                    false,
                    None,
                    None,
                )?;
            }
            self.finish_cancelled_tool_work(&call);
            let partial = match terminated
                .iter()
                .position(|(origin, _)| *origin == call.call_id)
            {
                Some(index) => Some(terminated.remove(index).1),
                None if call.name == "wait" && !terminated.is_empty() => {
                    Some(terminated.remove(0).1)
                }
                None => None,
            };
            let output = self.emit_cancelled_tool_result(
                &call.call_id,
                &call.name,
                call.started_at,
                None,
                call.shell_abort_format,
                Some(&call.span),
                partial.as_deref(),
            )?;
            aborted_outputs.push(match call.kind {
                CodeCallKind::Custom => custom_tool_output(call.call_id, output),
                CodeCallKind::Function => function_tool_output(call.call_id, output),
                CodeCallKind::ToolSearch => tool_search_output(call.call_id.clone(), Vec::new()),
            });
        }
        self.finish_active_tool_batch_wall();
        let session = self
            .session
            .as_mut()
            .ok_or(NanocodeError::InvalidAttemptState {
                detail: "interrupted turn did not have a model session",
            })?;
        session.conversation.append(aborted_outputs);
        session.conversation.append([turn_aborted()]);
        session.conversation.commit_interrupted();
        Ok(Self::checkpoint_from_session(
            session,
            false,
            self.global_instructions.clone(),
        ))
    }

    async fn commit_cancelled_checkpoint(&mut self) -> Result<ModelCheckpoint> {
        self.commit_interrupted_checkpoint()
    }

    pub(super) fn checkpoint_from_session(
        session: &ModelSessionState,
        preserve_inherited_delta: bool,
        global_instructions: Option<Arc<str>>,
    ) -> ModelCheckpoint {
        ModelCheckpoint {
            workspace: session.workspace.clone(),
            provider_session_id: Arc::from(session.factory.profile().session_id()),
            conversation: session.conversation.clone(),
            request_prefix: session.factory.profile().shared_prefix(),
            prompt_cache_key: Arc::from(session.factory.profile().prompt_cache_key()),
            preserve_inherited_delta,
            global_instructions,
            context_baseline: session.context.baseline(),
        }
    }

    pub(super) fn publish_fork_snapshot(
        session: &mut ModelSessionState,
        snapshots: &watch::Sender<Option<ModelCheckpoint>>,
        global_instructions: Option<&Arc<str>>,
    ) {
        session.conversation.commit_tail();
        snapshots.send_replace(Some(Self::live_checkpoint(session, global_instructions)));
    }

    fn live_checkpoint(
        session: &ModelSessionState,
        global_instructions: Option<&Arc<str>>,
    ) -> ModelCheckpoint {
        ModelCheckpoint {
            workspace: session.workspace.clone(),
            provider_session_id: Arc::from(session.factory.profile().session_id()),
            conversation: session.conversation.clone(),
            request_prefix: session.factory.profile().shared_prefix(),
            prompt_cache_key: Arc::from(session.factory.profile().prompt_cache_key()),
            preserve_inherited_delta: true,
            global_instructions: global_instructions.cloned(),
            context_baseline: session.context.baseline(),
        }
    }

    pub(super) async fn drive_session(
        &mut self,
        session: &mut ModelSessionState,
        steering: TurnSteering,
        resumed: bool,
        fork_snapshots: &watch::Sender<Option<ModelCheckpoint>>,
    ) -> Result<String> {
        let TurnSteering {
            receiver: steers,
            retained: retained_steers,
            mut preempt,
            instant_tool_steering,
            model_call_index,
        } = steering;

        let mut can_drain_steers = false;
        let next_call = self.stats.model_calls + 1;
        let mut pending_steers = retained_steers
            .into_iter()
            .filter(|steer| {
                !resumed
                    || !steer
                        .model_call_index
                        .is_some_and(|index| index <= next_call)
            })
            .collect::<VecDeque<_>>();
        model_call_index.store(next_call, std::sync::atomic::Ordering::Release);
        self.prompt_cache.publish_live(None);
        let mut first_batch = true;
        let mut reprompted = false;
        loop {
            let call_index = self.stats.model_calls + 1;
            if can_drain_steers {
                let delivered = self.stats.steers;
                model_call_index.store(call_index, std::sync::atomic::Ordering::Release);
                pending_steers.extend(steers.drain());
                self.drain_steers(&mut session.conversation, &mut pending_steers, call_index)
                    .await?;
                if self.stats.steers != delivered {
                    session.tools.control().settle_preempt();
                }
            }

            if first_batch && resumed {
                session.conversation.reasoning.pin(
                    self.model,
                    self.thinking,
                    self.config.supports_reasoning_effort_updates(self.model),
                );
            }
            let mut admitted = None;
            if !first_batch || !resumed {
                let history_len = session.conversation.managed.history().len();
                let update = session.conversation.reasoning.sampling_update(
                    self.model,
                    self.thinking,
                    self.config.supports_reasoning_effort_updates(self.model),
                    history_len,
                );
                if let Some(update) = update {
                    session.conversation.append([update]);
                }
                admitted = self.retain_before_model_call(session, call_index).await?;
            }
            first_batch = false;
            Self::publish_fork_snapshot(session, fork_snapshots, self.global_instructions.as_ref());
            let ModelCallOutcome {
                request,
                response,
                transport_continuation_valid,
                server_reasoning_included,
            } = self
                .perform_model_call(
                    call_index,
                    &session.conversation,
                    &session.factory,
                    admitted,
                )
                .await?;
            let TurnResult {
                id,
                end_turn,
                final_message,
                code_calls,
                output_items,
                usage,
                ..
            } = response;
            let previous_history = session.conversation.shared_history();
            session
                .conversation
                .managed
                .complete_generation(
                    request,
                    transport_continuation_valid.then_some(id),
                    output_items,
                    usage.as_ref(),
                    server_reasoning_included,
                )
                .map_err(|_| NanocodeError::MalformedResponse {
                    detail: "completed turn did not have a response ID",
                })?;
            session.conversation.reasoning.reconcile_history_repair(
                previous_history.iter(),
                session.conversation.managed.history(),
            );
            can_drain_steers = true;

            if code_calls.is_empty() {
                if end_turn == Some(false) {
                    session.conversation.clear_delta();
                    self.maybe_compact(
                        call_index,
                        &mut session.conversation,
                        &session.factory,
                        CompactionContext {
                            snapshot: session.context.snapshot(),
                            phase: CompactionPhase::MidTurn,
                        },
                    )
                    .await?;
                    continue;
                }
                loop {
                    let mut live_steers = VecDeque::new();
                    while let Some(steer) = pending_steers.pop_front() {
                        if steer.delivery.get() != crate::agent::execution::SteerDelivery::Withdrawn
                        {
                            live_steers.push_back(steer);
                        }
                    }
                    pending_steers = live_steers;
                    let queued = steers.drain();
                    if queued.is_empty() {
                        break;
                    }
                    pending_steers.extend(queued);
                }
                if !pending_steers.is_empty() {
                    session.conversation.clear_delta();
                    self.maybe_compact(
                        call_index,
                        &mut session.conversation,
                        &session.factory,
                        CompactionContext {
                            snapshot: session.context.snapshot(),
                            phase: CompactionPhase::MidTurn,
                        },
                    )
                    .await?;
                    continue;
                }
                if let Some(message) = final_message {
                    return Ok(if message.trim().is_empty() {
                        "The model completed without emitting assistant text.".to_owned()
                    } else {
                        message
                    });
                }
                if !reprompted {
                    reprompted = true;
                    session.conversation.append([empty_response()]);
                    session.conversation.clear_delta();
                    continue;
                }
                return Err(NanocodeError::MalformedResponse {
                    detail: "model completed without a final message or exec call",
                });
            }

            session.conversation.clear_delta();
            self.prompt_cache.publish_live(Some(Self::live_checkpoint(
                session,
                self.global_instructions.as_ref(),
            )));
            let history = code_calls
                .iter()
                .any(|call| call.name == "exec")
                .then(|| session.exec_history());
            {
                let tool_control = session.tools.control();
                let tools = self.execute_model_tools(
                    &session.tools,
                    &mut session.conversation,
                    call_index,
                    code_calls,
                    history,
                    session.factory.profile().turn_id(),
                );
                tokio::pin!(tools);
                loop {
                    tokio::select! {
                        biased;
                        outcome = &mut tools => { outcome?; break; }
                        changed = preempt.changed(), if instant_tool_steering => {
                            if changed.is_err() { break tools.await?; }

                            let pending = steers.any(|steer| {
                                steer.delivery.get() == crate::agent::execution::SteerDelivery::Pending
                            });
                            if pending {
                                tool_control.preempt_turn().await;
                            }
                        }
                    }
                }
            }
            self.prompt_cache.publish_live(None);
            self.maybe_compact(
                call_index,
                &mut session.conversation,
                &session.factory,
                CompactionContext {
                    snapshot: session.context.snapshot(),
                    phase: CompactionPhase::MidTurn,
                },
            )
            .await?;
        }
    }

    pub(super) async fn drain_steers(
        &mut self,
        conversation: &mut ConversationState,
        pending_steers: &mut VecDeque<QueuedSteer>,
        model_call_index: u32,
    ) -> Result<()> {
        if let Some(bound) = pending_steers
            .front()
            .and_then(|steer| steer.model_call_index)
            && bound < model_call_index
        {
            return Err(NanocodeError::InvalidExecutionPolicy(format!(
                "retained steer was bound to model call {bound}, reached {model_call_index}"
            )));
        }
        while pending_steers.front().is_some_and(|steer| {
            steer.model_call_index == Some(model_call_index)
                || (steer.model_call_index.is_none()
                    && steer.accepted_after_model_call_index < model_call_index)
        }) {
            let steer = pending_steers
                .pop_front()
                .expect("eligible steering input disappeared");
            if !steer.delivery.consume().await {
                continue;
            }
            if steer.model_call_index.is_none()
                && let (Some(steps), Some(index)) = (&self.execution_steps, steer.durable_index)
            {
                steps.bind_steer(index, model_call_index).await?;
            }
            if trace_content_enabled()
                && let Ok(content) = serde_json::to_string(&steer.prompt)
            {
                info!(
                    target: "nanocode",
                    content_kind = "steer",
                    content = content.as_str(),
                    "turn content"
                );
            }
            if let Some(revision) = steer.prompt.instruction_revision() {
                self.instruction_revision = Some(revision);
            }
            let instruction_bytes = steer.prompt.text_bytes();
            let user_content = self
                .images
                .prepare_user_input(&steer.prompt.instruction)
                .await
                .map_err(nanocode_durability::agent_store_error)?;
            conversation.append(prompt_messages(&steer.prompt, user_content));
            self.stats.steers += 1;
            self.events.emit(
                AgentEventKind::RunSteered,
                RunSteered {
                    steer_index: self.stats.steers,
                    instruction_bytes,
                    message_id: steer.message_id.as_deref(),
                },
            )?;
        }
        Ok(())
    }
}

use crate::ErrorExt as _;
