use super::*;

#[derive(Deserialize, Serialize)]
pub(super) struct WarmupExecution {
    pub(super) response_id: String,
    pub(super) attempt: u32,
    pub(super) connection_generation: u32,
    pub(super) usage: Option<Usage>,
    pub(super) server_reasoning_included: bool,

    #[serde(skip)]
    transport_continuation_valid: bool,
}

pub(super) struct WarmupOutcome {
    pub(super) baseline_established: bool,
    pub(super) response_id: Option<String>,
    pub(super) server_reasoning_included: bool,
}

struct CompactionLifecycle<'a> {
    events: &'a EventSink,
    stats: &'a mut RunStats,
    span: nanocode_trace::Span,
    after_model_call_index: u32,
    started_at: Instant,
    finished: bool,
}

impl CompactionLifecycle<'_> {
    fn fail(&mut self, error: &str) -> Result<()> {
        self.finished = true;
        let duration_ns = elapsed_ns(self.started_at);
        self.span.record("status", "failed");
        self.span.record("otel.status_code", "ERROR");
        self.span.record("duration_ns", duration_ns);
        self.stats.model_duration_ns += duration_ns;
        self.stats.compaction_duration_ns += duration_ns;
        self.events.emit(
            AgentEventKind::ModelCompactionFailed,
            CompactionFailed {
                after_model_call_index: self.after_model_call_index,
                duration_ns,
                error,
            },
        )?;
        Ok(())
    }
}

impl Drop for CompactionLifecycle<'_> {
    fn drop(&mut self) {
        if !self.finished {
            let _ = self.fail("compaction cancelled");
        }
    }
}

#[derive(Deserialize, Serialize)]
#[serde(untagged)]
enum RecordedCompactionOutcome {
    Success(RecordedCompactionResult),
    Failure {
        compaction_error: String,
        #[serde(default)]
        requires_session_stop: bool,
        #[serde(default)]
        recovery: crate::error::CompactionRecovery,
    },
}

#[derive(Deserialize, Serialize)]
struct RecordedCompactionResult {
    response_id: String,
    status: String,
    item: ResponseItem,
    usage: Option<Usage>,
    attempt: u32,
    connection_generation: u32,
    server_reasoning_included: bool,
    duration_ns: u64,
    time_to_first_event_ns: u64,
    time_to_first_output_ns: Option<u64>,
}

pub(super) enum ModelTaskOutcome {
    Completed(String),
    Cancelled,
}

#[derive(Clone, Copy)]
pub(super) enum CompactionPhase {
    PreTurn,
    MidTurn,
}

#[derive(Clone, Copy)]
pub(super) struct CompactionEffort {
    pub(super) request: Thinking,
    pub(super) effort: Thinking,
    pub(super) update: bool,
}

pub(super) struct CompactionContext<'a> {
    pub(super) snapshot: Option<&'a ContextSnapshot>,
    pub(super) phase: CompactionPhase,
}

impl ModelRun {
    pub(super) fn auto_compact_token_limit(&self) -> Option<u64> {
        let window = self.config.context_window_tokens;
        self.config
            .auto_compact_token_limit
            .map(|limit| limit.min(window))
            .or_else(|| compaction::auto_compact_token_limit(self.model.as_str(), window))
    }

    pub(super) fn compaction_effort(
        &self,
        reasoning: &crate::reasoning::ReasoningState,
    ) -> CompactionEffort {
        let supported = self.config.supports_reasoning_effort_updates(self.model);
        let request = reasoning.request_effort(self.model, self.thinking, supported);
        let current = reasoning.effective_effort(self.model, self.thinking, supported);
        let effort = self
            .compaction_thinking
            .filter(|effort| supported && self.model.supports_thinking(*effort))
            .unwrap_or(current);
        CompactionEffort {
            request,
            effort,
            update: effort != current,
        }
    }

    pub(super) async fn maybe_compact(
        &mut self,
        after_model_call_index: u32,
        conversation: &mut ConversationState,
        factory: &ResponsesAttemptFactory,
        context: CompactionContext<'_>,
    ) -> Result<bool> {
        let CompactionContext { snapshot, phase } = context;
        let Some(auto_compact_token_limit) = self.auto_compact_token_limit() else {
            return Ok(false);
        };
        let active_context_tokens = conversation.active_context_tokens();
        if !self.force_compaction && active_context_tokens < auto_compact_token_limit {
            return Ok(false);
        }
        let (item, _usage, server_reasoning_included) = self
            .perform_compaction(
                after_model_call_index,
                conversation.managed.compaction_request(),
                active_context_tokens,
                auto_compact_token_limit,
                factory,
                self.compaction_effort(&conversation.reasoning),
            )
            .await?;
        conversation.observe_server_reasoning(server_reasoning_included);
        match phase {
            CompactionPhase::PreTurn => {
                conversation.install_pre_turn_compaction(item, factory.profile().prefix());
            }
            CompactionPhase::MidTurn => {
                let snapshot = snapshot.ok_or(NanocodeError::InvalidAttemptState {
                    detail: "mid-turn compaction is missing its context snapshot",
                })?;
                let canonical_context = snapshot.full_item();
                conversation.install_mid_turn_compaction(
                    item,
                    developer_context(&self.config, &self.tools),
                    canonical_context,
                    factory.profile().prefix(),
                );
            }
        }
        self.force_compaction = false;
        Ok(true)
    }

    pub(super) fn sends_warmup(&self) -> bool {
        !matches!(self.config.responses_transport, ResponsesTransport::Https)
            && self.config.websocket_warmup
    }

    pub(super) async fn perform_warmup(
        &mut self,
        factory: &ResponsesAttemptFactory,
        request_effort: Thinking,
    ) -> Result<WarmupOutcome> {
        if !self.sends_warmup() {
            return Ok(WarmupOutcome {
                baseline_established: false,
                response_id: None,
                server_reasoning_included: false,
            });
        }
        let started_at = Instant::now();
        self.events.emit(
            AgentEventKind::ModelWarmupStarted,
            WarmupStarted {
                model: self.model.as_str(),
                prompt_cache_key: factory.profile().prompt_cache_key(),
            },
        )?;
        let span = warmup_span(&self.config, self.model);
        if let Some(content) = serialize_trace_content(factory.profile().prefix()) {
            record_span_content(&span, "model.input", &content);
        }

        let shared_prompt_cache = self
            .execution_steps
            .is_none()
            .then(|| self.prompt_cache.shared().cloned())
            .flatten();
        let mut shared_prefix_warmed = false;
        let outcome = if let Some(cache) = shared_prompt_cache {
            match cache.entry(self.model, factory.profile()).await {
                Ok(entry) => {
                    let mut execution = None;
                    let initialized = entry
                        .get_or_try_init(|| async {
                            let completed =
                                self.execute_warmup(factory, &span, request_effort).await?;
                            execution = Some(completed);
                            Ok(())
                        })
                        .await;
                    initialized.map(|()| {
                        shared_prefix_warmed = execution.is_none();
                        execution.flatten()
                    })
                }
                Err(error) => Err(error),
            }
        } else {
            self.execute_warmup(factory, &span, request_effort).await
        };
        let execution = match outcome {
            Ok(outcome) => outcome,
            Err(error) => {
                span.record("status", "failed");
                span.record("otel.status_code", "ERROR");
                span.record("duration_ns", elapsed_ns(started_at));
                return self.warmup_failed(started_at, error);
            }
        };
        let duration_ns = elapsed_ns(started_at);
        if execution.is_none() && !shared_prefix_warmed {
            span.record("status", "failed");
            span.record("otel.status_code", "ERROR");
            span.record("duration_ns", duration_ns);
            self.stats.warmup_duration_ns += duration_ns;
            self.events.emit(
                AgentEventKind::ModelWarmupFailed,
                WarmupFailed {
                    duration_ns,
                    error: "replayed a failed warmup",
                },
            )?;
            return Ok(WarmupOutcome {
                baseline_established: false,
                response_id: None,
                server_reasoning_included: false,
            });
        }
        let (response_id, source, attempt, connection_generation, usage, server_reasoning_included) =
            if let Some(execution) = execution {
                if let Some(usage) = &execution.usage {
                    self.stats
                        .warmup_usage
                        .add(usage, self.model, self.service_tier);
                }
                (
                    execution
                        .transport_continuation_valid
                        .then_some(execution.response_id),
                    "response",
                    Some(execution.attempt),
                    Some(execution.connection_generation),
                    execution.usage,
                    execution.server_reasoning_included,
                )
            } else {
                (None, "shared_prefix", None, None, None, false)
            };
        span.record("warmup.source", source);
        if let Some(usage) = &usage {
            record_usage(&span, usage, self.model, self.service_tier);
        }
        span.record("status", "completed");
        span.record("otel.status_code", "OK");
        span.record("duration_ns", duration_ns);
        self.stats.warmup_duration_ns += duration_ns;
        self.stats.last_response_id.clone_from(&response_id);
        self.events.emit(
            AgentEventKind::ModelWarmupCompleted,
            WarmupCompleted {
                response_id: response_id.as_deref(),
                source,
                attempt,
                connection_generation,
                duration_ns,
                usage: usage.as_ref(),
            },
        )?;
        Ok(WarmupOutcome {
            baseline_established: true,
            response_id,
            server_reasoning_included,
        })
    }

    pub(super) async fn execute_warmup(
        &mut self,
        factory: &ResponsesAttemptFactory,
        span: &nanocode_trace::Span,
        request_effort: Thinking,
    ) -> Result<Option<WarmupExecution>> {
        if let Some(steps) = &self.execution_steps
            && let crate::agent::ExecutionStep::Replay(output) = steps
                .begin::<_, Option<WarmupExecution>>("warmup", "warmup", &())
                .await?
        {
            return Ok(output);
        }
        let success = match self
            .client
            .execute(factory.warmup(self.model, request_effort, self.service_tier))
            .instrument(span.clone())
            .await
        {
            Ok(success) => success,
            Err(error) => {
                let error = NanocodeError::from(error);
                if !error
                    .responses_error()
                    .is_some_and(|source| source.is_misalignment_policy_violation())
                    && let Some(steps) = &self.execution_steps
                {
                    steps.complete("warmup", &None::<WarmupExecution>).await?;
                }
                return Err(error);
            }
        };
        let attempt = success.attempt();
        let connection_generation = success.connection_generation();
        let server_reasoning_included = success.server_reasoning_included();
        let ResponsesOutput::Warmup(response) = success.into_output() else {
            span.record("status", "failed");
            span.record("otel.status_code", "ERROR");
            return Err(NanocodeError::InvalidAttemptState {
                detail: "warmup returned a non-warmup response",
            });
        };
        let output = WarmupExecution {
            response_id: response.id,
            attempt,
            connection_generation,
            usage: response.usage,
            server_reasoning_included,
            transport_continuation_valid: true,
        };
        if let Some(steps) = &self.execution_steps {
            steps.complete("warmup", &Some(&output)).await?;
        }
        Ok(Some(output))
    }

    pub(super) fn warmup_failed<T>(
        &mut self,
        started_at: Instant,
        error: NanocodeError,
    ) -> Result<T> {
        let duration_ns = elapsed_ns(started_at);
        self.stats.warmup_duration_ns += duration_ns;
        let message = error.to_string();
        self.events.emit(
            AgentEventKind::ModelWarmupFailed,
            WarmupFailed {
                duration_ns,
                error: &message,
            },
        )?;
        Err(error)
    }

    pub(super) async fn perform_compaction(
        &mut self,
        after_model_call_index: u32,
        mut history: RequestHistory,
        active_context_tokens: u64,
        auto_compact_token_limit: u64,
        factory: &ResponsesAttemptFactory,
        effort: CompactionEffort,
    ) -> Result<(ResponseItem, Option<Usage>, bool)> {
        let step_id = format!("compaction-{after_model_call_index}");
        let model = self.model;
        let service_tier = self.service_tier;

        if let Some(hook) = &self.before_compaction {
            use crate::execution::{BeforeCompactionRequest, CompactionReceipt};
            let preservation_step = format!("before-{step_id}");
            let operation = self.execution_steps.as_ref().map_or_else(
                || factory.profile().turn_id(),
                |steps| steps.operation_id().to_owned(),
            );
            let boundary_id = format!(
                "{}:{operation}:{preservation_step}",
                self.events.request_id()
            );
            let request = BeforeCompactionRequest::from_history(
                boundary_id,
                self.events.request_id().to_owned(),
                self.provider_session_id.to_string(),
                history.full(),
            );
            let recovered = if let Some(steps) = &self.execution_steps {
                match steps
                    .begin::<_, CompactionReceipt>(
                        &preservation_step,
                        "before_compaction",
                        &request,
                    )
                    .await?
                {
                    crate::agent::ExecutionStep::Replay(receipt) => {
                        receipt.validate()?;
                        true
                    }
                    crate::agent::ExecutionStep::Interrupted => {
                        return Err(NanocodeError::InvalidExecutionPolicy(
                            "non-tool step was admitted with an interrupted tool outcome".into(),
                        ));
                    }
                    crate::agent::ExecutionStep::Execute => false,
                }
            } else {
                false
            };
            if !recovered {
                let receipt = hook.preserve(request).await?;
                receipt.validate()?;
                if let Some(steps) = &self.execution_steps {
                    steps.complete(&preservation_step, &receipt).await?;
                }
            }
        }
        history.fit_context_window(
            factory.profile().prefix(),
            self.config.context_window_tokens,
        );
        if effort.update {
            history.push(ResponseItem::configuration_update(effort.effort));
        }
        let history = evict_images(&history);
        let started_at = Instant::now();
        self.stats.compactions += 1;
        self.events.emit(
            AgentEventKind::ModelCompactionStarted,
            CompactionStarted {
                after_model_call_index,
                active_context_tokens,
                auto_compact_token_limit,
                previous_response_id: history.previous_response_id(),
            },
        )?;
        let build = |history: &RequestHistory| {
            factory.compaction(
                after_model_call_index,
                history,
                compaction::trigger(),
                model,
                effort.request,
                service_tier,
            )
        };
        let span = compaction_span(
            after_model_call_index,
            build(&history).input_item_count(),
            effort.effort,
        );
        let trace = trace_content_enabled().then(|| span.clone());
        let images = self.images.clone();
        let mut lifecycle = CompactionLifecycle {
            events: &self.events,
            stats: &mut self.stats,
            span: span.clone(),
            after_model_call_index,
            started_at,
            finished: false,
        };
        let result = async {
            let execution_steps = self.execution_steps.clone();
            let recovered = if let Some(steps) = &execution_steps {
                match steps
                    .begin::<_, RecordedCompactionOutcome>(&step_id, "compaction", &())
                    .await?
                {
                    crate::agent::ExecutionStep::Interrupted => {
                        return Err(NanocodeError::InvalidExecutionPolicy(
                            "non-tool step was admitted with an interrupted tool outcome".into(),
                        ));
                    }
                    crate::agent::ExecutionStep::Execute => None,
                    crate::agent::ExecutionStep::Replay(output) => Some(output),
                }
            } else {
                None
            };
            let recorded_result = if let Some(output) = recovered {
                output
            } else {
                let request =
                    build(&inline_images(&images, history).await?).with_input_trace(trace);
                let success = match self.client.execute(request).instrument(span.clone()).await {
                    Ok(success) => success,
                    Err(error) => {
                        let error = NanocodeError::from(error);
                        let Some(source) = error.responses_error() else {
                            return Err(error);
                        };
                        if source.retry_advice().is_some() {
                            return Err(error);
                        }
                        let requires_session_stop = source.is_misalignment_policy_violation();
                        let recovery = if error.requires_image_repair() {
                            crate::error::CompactionRecovery::ReplaceRejectedImages
                        } else {
                            crate::error::CompactionRecovery::None
                        };
                        let compaction_error = error.to_string();
                        if let Some(steps) = &execution_steps {
                            steps
                                .complete(
                                    &step_id,
                                    &RecordedCompactionOutcome::Failure {
                                        compaction_error: compaction_error.clone(),
                                        requires_session_stop,
                                        recovery,
                                    },
                                )
                                .await?;
                        }
                        return Err(NanocodeError::CompactionFailed {
                            detail: compaction_error,
                            requires_session_stop,
                            recovery,
                        });
                    }
                };
                let attempt = success.attempt();
                let connection_generation = success.connection_generation();
                let server_reasoning_included = success.server_reasoning_included();
                let ResponsesOutput::Compaction(response) = success.into_output() else {
                    let error = NanocodeError::InvalidAttemptState {
                        detail: "compaction returned a non-compaction response",
                    };
                    return Err(error);
                };
                let output = RecordedCompactionResult {
                    response_id: response.id,
                    status: response.status,
                    item: response.item,
                    usage: response.usage,
                    attempt,
                    connection_generation,
                    server_reasoning_included,
                    duration_ns: elapsed_ns(started_at),
                    time_to_first_event_ns: response.time_to_first_event_ns,
                    time_to_first_output_ns: response.time_to_first_output_ns,
                };
                validate_provider_response_id(&output.response_id)?;
                let output = RecordedCompactionOutcome::Success(output);
                if let Some(steps) = &execution_steps {
                    steps.complete(&step_id, &output).await?;
                }
                output
            };
            let recorded_result = match recorded_result {
                RecordedCompactionOutcome::Success(output) => output,
                RecordedCompactionOutcome::Failure {
                    compaction_error,
                    requires_session_stop,
                    recovery,
                } => {
                    return Err(NanocodeError::CompactionFailed {
                        detail: compaction_error,
                        requires_session_stop,
                        recovery,
                    });
                }
            };
            let RecordedCompactionResult {
                response_id,
                status,
                item,
                usage,
                attempt,
                connection_generation,
                server_reasoning_included,
                duration_ns,
                time_to_first_event_ns,
                time_to_first_output_ns,
            } = recorded_result;
            validate_provider_response_id(&response_id)?;
            span.record("model.response.id", response_id.as_str());
            if let Some(content) = serialize_trace_content(&item) {
                record_span_content(&span, "model.output_item", &content);
            }
            span.record("status", "completed");
            span.record("otel.status_code", "OK");
            span.record("duration_ns", duration_ns);
            self.events.emit(
                AgentEventKind::ModelCompactionCompleted,
                CompactionCompleted {
                    after_model_call_index,
                    response_id: &response_id,
                    attempt,
                    connection_generation,
                    status: &status,
                    duration_ns,
                    time_to_first_event_ns,
                    time_to_first_output_ns,
                    usage: usage.as_ref(),
                },
            )?;
            lifecycle.finished = true;
            lifecycle.stats.model_duration_ns += duration_ns;
            lifecycle.stats.compaction_duration_ns += duration_ns;
            if let Some(usage) = &usage {
                record_usage(&span, usage, model, self.service_tier);
                lifecycle.stats.usage.add(usage, model, self.service_tier);
            }
            lifecycle.stats.last_response_id = Some(response_id);
            Ok((item, usage, server_reasoning_included))
        }
        .await;
        if let Err(error) = &result {
            lifecycle.fail(&error.to_string())?;
        }
        result
    }
}

use crate::ErrorExt as _;
