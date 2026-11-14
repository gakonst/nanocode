use super::*;

#[derive(Deserialize)]
pub(super) struct RecordedModelResult {
    response: TurnResult,
    attempt: u32,
    connection_generation: u32,
    server_reasoning_included: bool,
    duration_ns: u64,
    #[serde(default)]
    preparation_ns: Option<u64>,
}

impl Serialize for RecordedModelResult {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        struct RecordedResponse<'a> {
            id: &'a str,
            #[serde(skip_serializing_if = "Option::is_none")]
            reported_model: Option<&'a str>,
            status: &'a str,
            end_turn: Option<bool>,
            output_items: &'a [ResponseItem],
            usage: Option<&'a Usage>,
            time_to_first_event_ns: u64,
            time_to_first_output_ns: Option<u64>,
        }

        #[derive(Serialize)]
        struct Record<'a> {
            response: RecordedResponse<'a>,
            attempt: u32,
            connection_generation: u32,
            server_reasoning_included: bool,
            duration_ns: u64,
            preparation_ns: Option<u64>,
        }

        let response = &self.response;
        Record {
            response: RecordedResponse {
                id: &response.id,
                reported_model: response.reported_model.as_deref(),
                status: &response.status,
                end_turn: response.end_turn,
                output_items: &response.output_items,
                usage: response.usage.as_ref(),
                time_to_first_event_ns: response.time_to_first_event_ns,
                time_to_first_output_ns: response.time_to_first_output_ns,
            },
            attempt: self.attempt,
            connection_generation: self.connection_generation,
            server_reasoning_included: self.server_reasoning_included,
            duration_ns: self.duration_ns,
            preparation_ns: self.preparation_ns,
        }
        .serialize(serializer)
    }
}

pub(super) struct ModelCallOutcome {
    pub(super) request: RequestHistory,
    pub(super) response: TurnResult,
    pub(super) transport_continuation_valid: bool,
    pub(super) server_reasoning_included: bool,
}

impl ModelRun {
    pub(super) async fn perform_model_call(
        &mut self,
        call_index: u32,
        conversation: &ConversationState,
        factory: &ResponsesAttemptFactory,
        admitted: Option<crate::agent::ExecutionStep<RecordedModelResult>>,
    ) -> Result<ModelCallOutcome> {
        let step_id = model_step_id(call_index);
        let model = self.model;
        let thinking = self.thinking;
        let reasoning_mode = self.config.reasoning_mode;
        let service_tier = self.service_tier;
        let request_history = conversation.managed.generation_request();
        let sent = evict_images(&request_history);
        let previous_response_id = sent.previous_response_id();
        let started_at = Instant::now();
        self.stats.model_calls += 1;
        self.prompt_cache.publish_request(thinking, service_tier);
        self.events.emit(
            AgentEventKind::ModelCallStarted,
            ModelCallStarted {
                call_index,
                model: model.as_str(),
                reasoning_mode: reasoning_mode.as_str(),
                effort: thinking.as_str(),
                previous_response_id,
                prompt_cache_key: self
                    .config
                    .chat_gateway
                    .is_none()
                    .then(|| factory.profile().prompt_cache_key()),
            },
        )?;
        let effort = conversation.reasoning.request_effort(
            model,
            thinking,
            self.config.supports_reasoning_effort_updates(model),
        );
        let build = |history: &RequestHistory| {
            factory
                .generation(call_index, history, model, effort, service_tier)
                .with_model_call_started_at(started_at)
        };
        let span = model_call_span(
            &self.provider_session_id,
            call_index,
            model.as_str(),
            reasoning_mode.as_str(),
            thinking.as_str(),
            previous_response_id.is_some(),
            build(&sent).input_item_count(),
        );
        let trace = trace_content_enabled().then(|| span.clone());
        let execution_steps = self.execution_steps.clone();
        let recovered = if let Some(steps) = &execution_steps {
            let admitted = match admitted {
                Some(admitted) => admitted,
                None => {
                    steps
                        .begin::<_, RecordedModelResult>(&step_id, "model_call", &())
                        .await?
                }
            };
            match admitted {
                crate::agent::ExecutionStep::Interrupted => {
                    return Err(NanocodeError::InvalidExecutionPolicy(
                        "non-tool step was admitted with an interrupted tool outcome".into(),
                    ));
                }
                crate::agent::ExecutionStep::Execute => None,
                crate::agent::ExecutionStep::Replay(mut output) => {
                    output.response.restore_derived_fields();
                    Some(output)
                }
            }
        } else {
            None
        };
        let replayed = recovered.is_some();
        let (recorded_result, transport_continuation_valid) = if let Some(output) = recovered {
            (output, false)
        } else {
            let request = build(&inline_images(&self.images, sent).await?).with_input_trace(trace);
            let preparation_ns = elapsed_ns(started_at);
            let success = match self.client.execute(request).instrument(span.clone()).await {
                Ok(success) => success,
                Err(error) => {
                    span.record("status", "failed");
                    span.record("otel.status_code", "ERROR");
                    span.record("duration_ns", elapsed_ns(started_at));
                    return self.model_call_failed(
                        call_index,
                        started_at,
                        NanocodeError::from(error),
                    );
                }
            };
            let attempt = success.attempt();
            let connection_generation = success.connection_generation();
            let server_reasoning_included = success.server_reasoning_included();
            let ResponsesOutput::Generation(response) = success.into_output() else {
                span.record("status", "failed");
                span.record("otel.status_code", "ERROR");
                return Err(NanocodeError::InvalidAttemptState {
                    detail: "generation returned a non-generation response",
                });
            };
            let output = RecordedModelResult {
                response,
                attempt,
                connection_generation,
                server_reasoning_included,
                duration_ns: elapsed_ns(started_at),
                preparation_ns: Some(preparation_ns),
            };
            validate_provider_response_id(&output.response.id)?;
            if let Some(steps) = &execution_steps {
                steps.complete_model(&step_id, &output).await?;
            }
            (output, true)
        };
        let RecordedModelResult {
            response,
            attempt,
            connection_generation,
            server_reasoning_included,
            duration_ns,
            preparation_ns,
        } = recorded_result;
        validate_provider_response_id(&response.id)?;
        record_model_response(&span, &response);
        span.record("status", "completed");
        span.record("otel.status_code", "OK");
        span.record("duration_ns", duration_ns);
        if let Some(usage) = &response.usage {
            record_usage(&span, usage, model, service_tier);
        }
        self.stats.model_duration_ns += duration_ns;
        if let Some(usage) = &response.usage {
            self.stats.usage.add(usage, model, service_tier);
        }
        self.stats.last_response_id = transport_continuation_valid.then(|| response.id.clone());
        self.events.emit(
            AgentEventKind::ModelCallCompleted,
            ModelCallCompleted {
                call_index,
                model: model.as_str(),
                response_id: &response.id,
                attempt,
                connection_generation,
                status: &response.status,
                duration_ns,
                time_to_first_event_ns: response.time_to_first_event_ns,
                time_to_first_output_ns: response.time_to_first_output_ns,
                tool_calls: response.code_calls.len(),
                usage: response.usage.as_ref(),
                timing: ModelCallTiming::new(
                    replayed,
                    preparation_ns,
                    duration_ns,
                    &response.timing,
                    &response.pipeline_stats,
                ),
            },
        )?;
        Ok(ModelCallOutcome {
            request: request_history,
            response,
            transport_continuation_valid,
            server_reasoning_included,
        })
    }

    pub(super) fn model_call_failed<T>(
        &mut self,
        call_index: u32,
        started_at: Instant,
        error: crate::NanocodeError,
    ) -> Result<T> {
        let duration_ns = elapsed_ns(started_at);
        self.stats.model_duration_ns += duration_ns;
        let message = error.to_string();
        self.events.emit(
            AgentEventKind::ModelCallFailed,
            ModelCallFailed {
                call_index,
                model: self.model.as_str(),
                duration_ns,
                error: &message,
            },
        )?;
        Err(error)
    }
}

pub(super) fn model_step_id(call_index: u32) -> String {
    format!("model-{call_index}")
}

pub(super) fn validate_provider_response_id(response_id: &str) -> Result<()> {
    if response_id.trim().is_empty() {
        return Err(NanocodeError::MalformedResponse {
            detail: "completed turn did not have a response ID",
        });
    }
    Ok(())
}

pub(super) fn unsupported_tool_message(tools: &ToolRuntime, call: &CodeCall) -> Option<String> {
    let qualified_name = qualified_tool_name(call);
    if (call.namespace.is_none() && matches!(call.name.as_str(), "exec" | "wait"))
        || tools.contains(&qualified_name)
    {
        return None;
    }
    Some(match &call.kind {
        CodeCallKind::Custom => format!("unsupported custom tool call: {qualified_name}"),
        CodeCallKind::Function => format!("unsupported call: {qualified_name}"),
        CodeCallKind::ToolSearch => return None,
    })
}

pub(super) fn qualified_tool_name(call: &CodeCall) -> String {
    crate::client::tower::qualified_tool_name(&call.name, call.namespace.as_deref())
}

pub(super) fn trace_content_enabled() -> bool {
    nanocode_trace::enabled!(target: "nanocode", nanocode_trace::Level::INFO)
}

pub(super) fn serialize_trace_content<T: Serialize + ?Sized>(value: &T) -> Option<String> {
    trace_content_enabled()
        .then(|| serde_json::to_string(value).ok())
        .flatten()
}

pub(super) fn record_tool_span_terminal(
    span: &nanocode_trace::Span,
    status: &'static str,
    otel_status: &'static str,
    duration_ns: u64,
    output: &ToolOutputBody,
) {
    if let Some(content) = serialize_trace_content(output) {
        record_span_content(span, "tool.output", &content);
    }
    span.record("status", status);
    span.record("otel.status_code", otel_status);
    span.record("duration_ns", duration_ns);
}

pub(super) fn model_tool_span(session_id: &str, call: &CodeCall, call_index: u32) -> nanocode_trace::Span {
    let qualified_name = qualified_tool_name(call);
    info_span!(
        target: "nanocode",
        "tool.call",
        otel.kind = "internal",
        otel.status_code = nanocode_trace::field::Empty,
        session.id = session_id,
        tool.name = %qualified_name,
        tool.call_id = %call.call_id,
        tool.arguments.bytes = call.input.len(),
        model.call_index = call_index,
        status = nanocode_trace::field::Empty,
        duration_ns = nanocode_trace::field::Empty,
    )
}

pub(super) fn owned_code_context(
    call: &CodeCall,
    history: Option<Arc<SharedHistory>>,
    session_id: &str,
    turn_id: &str,
    model: Model,
    host_context: Option<&str>,
    instruction_revision: Option<u64>,
) -> Result<Option<OwnedToolContext>> {
    if call.name != "exec" {
        return Ok(None);
    }
    let history = history.ok_or(NanocodeError::MalformedResponse {
        detail: "exec call did not have an owned history snapshot",
    })?;
    Ok(Some(
        OwnedToolContext::new(
            model.as_str(),
            session_id,
            &call.call_id,
            Arc::default(),
            DEFAULT_TOOL_OUTPUT_TOKENS,
        )
        .with_history(history)
        .with_instruction_revision(instruction_revision)
        .with_host_context(host_context.map(Arc::from))
        .with_turn_id(Some(Arc::from(turn_id))),
    ))
}

pub(super) fn record_span_content(span: &nanocode_trace::Span, kind: &'static str, content: &str) {
    span.in_scope(|| {
        info!(
            target: "nanocode",
            content_kind = kind,
            content,
            "trace content"
        );
    });
}

pub(super) fn record_indexed_span_content(
    span: &nanocode_trace::Span,
    kind: &'static str,
    index: usize,
    content: &str,
) {
    span.in_scope(|| {
        info!(
            target: "nanocode",
            content_kind = kind,
            output.index = index,
            content,
            "trace content"
        );
    });
}

#[allow(clippy::too_many_arguments)]
pub(super) fn model_call_span(
    session_id: &str,
    call_index: u32,
    model: &str,
    reasoning_mode: &str,
    reasoning_effort: &str,
    previous_response: bool,
    input_item_count: usize,
) -> nanocode_trace::Span {
    info_span!(
        target: "nanocode",
        "model.call",
        otel.kind = "internal",
        otel.status_code = nanocode_trace::field::Empty,
        session.id = session_id,
        model,
        reasoning.mode = reasoning_mode,
        reasoning.effort = reasoning_effort,
        model.call_index = call_index,
        previous_response,
        model.input.item_count = input_item_count,
        model.input.bytes = nanocode_trace::field::Empty,
        model.response.id = nanocode_trace::field::Empty,
        model.response.status = nanocode_trace::field::Empty,
        model.response.end_turn = nanocode_trace::field::Empty,
        model.output.item_count = nanocode_trace::field::Empty,
        model.output.bytes = nanocode_trace::field::Empty,
        model.tool_call_count = nanocode_trace::field::Empty,
        assistant.output.bytes = nanocode_trace::field::Empty,
        status = nanocode_trace::field::Empty,
        duration_ns = nanocode_trace::field::Empty,
        input_tokens = nanocode_trace::field::Empty,
        cached_input_tokens = nanocode_trace::field::Empty,
        cache_write_input_tokens = nanocode_trace::field::Empty,
        output_tokens = nanocode_trace::field::Empty,
        reasoning_output_tokens = nanocode_trace::field::Empty,
        total_tokens = nanocode_trace::field::Empty,
        cost.usd = nanocode_trace::field::Empty,
        cost.service_tier = nanocode_trace::field::Empty,
        reasoning.summary_count = nanocode_trace::field::Empty,
        time_to_first_event_ns = nanocode_trace::field::Empty,
        time_to_first_output_ns = nanocode_trace::field::Empty,
        stream.display_delta.count = nanocode_trace::field::Empty,
        stream.display_delta.bytes = nanocode_trace::field::Empty,
        stream.inter_delta_gap.max_ns = nanocode_trace::field::Empty,
        stream.inter_delta_stall_100ms.count = nanocode_trace::field::Empty,
    )
}

pub(super) fn warmup_span(config: &ModelConfig, model: Model) -> nanocode_trace::Span {
    info_span!(
        target: "nanocode",
        "model.warmup",
        otel.kind = "internal",
        otel.status_code = nanocode_trace::field::Empty,
        model = model.as_str(),
        system_prompt.bytes = config.system_prompt().len(),
        warmup.source = nanocode_trace::field::Empty,
        status = nanocode_trace::field::Empty,
        duration_ns = nanocode_trace::field::Empty,
        input_tokens = nanocode_trace::field::Empty,
        cached_input_tokens = nanocode_trace::field::Empty,
        cache_write_input_tokens = nanocode_trace::field::Empty,
        output_tokens = nanocode_trace::field::Empty,
        reasoning_output_tokens = nanocode_trace::field::Empty,
        total_tokens = nanocode_trace::field::Empty,
        cost.usd = nanocode_trace::field::Empty,
        cost.service_tier = nanocode_trace::field::Empty,
    )
}

pub(super) fn compaction_span(
    after_model_call_index: u32,
    input_item_count: usize,
    effort: Thinking,
) -> nanocode_trace::Span {
    info_span!(
        target: "nanocode",
        "model.compaction",
        otel.kind = "internal",
        otel.status_code = nanocode_trace::field::Empty,
        after_model_call_index,
        reasoning.effort = effort.as_str(),
        model.input.item_count = input_item_count,
        model.input.bytes = nanocode_trace::field::Empty,
        model.response.id = nanocode_trace::field::Empty,
        status = nanocode_trace::field::Empty,
        duration_ns = nanocode_trace::field::Empty,
        input_tokens = nanocode_trace::field::Empty,
        cached_input_tokens = nanocode_trace::field::Empty,
        cache_write_input_tokens = nanocode_trace::field::Empty,
        output_tokens = nanocode_trace::field::Empty,
        reasoning_output_tokens = nanocode_trace::field::Empty,
        total_tokens = nanocode_trace::field::Empty,
        cost.usd = nanocode_trace::field::Empty,
        cost.service_tier = nanocode_trace::field::Empty,
    )
}

pub(super) fn record_usage(
    span: &nanocode_trace::Span,
    usage: &Usage,
    model: Model,
    service_tier: ServiceTier,
) {
    let cached_input_tokens = usage
        .input_tokens_details
        .as_ref()
        .map_or(0, |details| details.cached_tokens);
    let cache_write_input_tokens = usage
        .input_tokens_details
        .as_ref()
        .map_or(0, |details| details.cache_write_tokens);
    let reasoning_output_tokens = usage
        .output_tokens_details
        .as_ref()
        .map_or(0, |details| details.reasoning_tokens);
    span.record("input_tokens", usage.input_tokens);
    span.record("cached_input_tokens", cached_input_tokens);
    span.record("cache_write_input_tokens", cache_write_input_tokens);
    span.record("output_tokens", usage.output_tokens);
    span.record("reasoning_output_tokens", reasoning_output_tokens);
    span.record("total_tokens", usage.total_tokens);
    let estimate = estimate_for_model(usage, model, service_tier);
    let amount = estimate.amount().decimal();
    span.record("cost.usd", amount.as_str());
    span.record("cost.service_tier", estimate.service_tier().as_str());
}

pub(super) fn record_turn_usage(span: &nanocode_trace::Span, usage: &TurnUsage) {
    span.record("usage.input_tokens", usage.input_tokens());
    span.record("usage.cached_input_tokens", usage.cached_input_tokens());
    span.record(
        "usage.cache_write_input_tokens",
        usage.cache_write_input_tokens(),
    );
    span.record("usage.output_tokens", usage.output_tokens());
    span.record(
        "usage.reasoning_output_tokens",
        usage.reasoning_output_tokens(),
    );
    span.record("usage.total_tokens", usage.total_tokens());
    span.record("cost.status", usage.cost_status().as_str());
    if let Some(cost) = usage.estimated_cost() {
        let amount = cost.amount().decimal();
        span.record("cost.usd", amount.as_str());
        span.record("cost.service_tier", cost.service_tier().as_str());
    }
}

pub(super) fn record_model_response(span: &nanocode_trace::Span, response: &TurnResult) {
    span.record("model.response.id", response.id.as_str());
    span.record("model.response.status", response.status.as_str());
    if let Some(end_turn) = response.end_turn {
        span.record("model.response.end_turn", end_turn);
    }
    span.record("model.output.item_count", response.output_items.len());
    span.record("model.tool_call_count", response.code_calls.len());
    let trace_content = trace_content_enabled();
    let mut output_bytes = usize::from(trace_content).saturating_mul(2);
    let mut serialized_items = 0_usize;
    let mut summary_count = 0_usize;
    for (index, item) in response.output_items.iter().enumerate() {
        let kind = if let ResponseItem::Reasoning { summary, .. } = item {
            summary_count = summary_count.saturating_add(summary.len());
            "reasoning"
        } else {
            "model.output_item"
        };
        if trace_content && let Ok(content) = serde_json::to_string(item) {
            output_bytes = output_bytes
                .saturating_add(usize::from(serialized_items != 0))
                .saturating_add(content.len());
            serialized_items = serialized_items.saturating_add(1);
            record_indexed_span_content(span, kind, index, &content);
        }
    }
    span.record("model.output.bytes", output_bytes);
    if let Some(message) = &response.final_message {
        span.record("assistant.output.bytes", message.len());
    }
    span.record("reasoning.summary_count", summary_count);
    span.record("time_to_first_event_ns", response.time_to_first_event_ns);
    if let Some(time_to_first_output_ns) = response.time_to_first_output_ns {
        span.record("time_to_first_output_ns", time_to_first_output_ns);
    }
    span.record(
        "stream.display_delta.count",
        response.pipeline_stats.display_delta_count,
    );
    span.record(
        "stream.display_delta.bytes",
        response.pipeline_stats.display_delta_bytes,
    );
    span.record(
        "stream.inter_delta_gap.max_ns",
        response.pipeline_stats.inter_delta_gap_max_ns,
    );
    span.record(
        "stream.inter_delta_stall_100ms.count",
        response.pipeline_stats.inter_delta_stall_100ms_count,
    );
}

pub(super) fn request_profile(
    session_id: &str,
    prompt_cache_key: &str,
    tool_specs: Vec<ToolDefinition>,
    code_mode_tool_names: Vec<(String, String)>,
    system_prompt: &str,
    model: Model,
) -> Result<RequestProfile> {
    let tool_specs = match model {
        Model::Custom(_) => unknown_model_tools(tool_specs),
        _ => tool_specs,
    };
    let prefix = shared_request_prefix(prompt_cache_key, tool_specs, system_prompt)
        .map_err(NanocodeError::SerializePromptPrefix)?;
    Ok(with_code_mode_tool_names(
        RequestProfile::new(session_id, prompt_cache_key, prefix),
        code_mode_tool_names,
    ))
}

fn unknown_model_tools(specs: Vec<ToolDefinition>) -> Vec<ToolDefinition> {
    specs
        .into_iter()
        .filter(|spec| {
            !matches!(
                spec,
                ToolDefinition::Function { name, .. } | ToolDefinition::Custom { name, .. }
                    if matches!(&**name, "apply_patch" | "update_plan")
            )
        })
        .map(|mut spec| {
            if let ToolDefinition::Function {
                name, parameters, ..
            } = &mut spec
                && &**name == "view_image"
            {
                let mut schema = parameters.as_value().clone();
                if let Some(properties) = schema
                    .get_mut("properties")
                    .and_then(serde_json::Value::as_object_mut)
                {
                    properties.remove("detail");
                }
                *parameters = schema.into();
            }
            spec
        })
        .collect()
}

pub(super) fn attempt_factory(
    events: &EventSink,
    transport_stats: &Arc<TransportStats>,
    provider_session_id: &str,
    prompt_cache: &ModelPromptCache,
    model: Model,
    tools: &ToolRuntime,
    system_prompt: &str,
) -> Result<ResponsesAttemptFactory> {
    let (tool_specs, code_mode_tool_names) = model_tool_contract(tools, events.request_id());
    let prompt_cache_key = prompt_cache.key(model, &tool_specs, system_prompt)?;
    Ok(ResponsesAttemptFactory::new(
        request_profile(
            provider_session_id,
            &prompt_cache_key,
            tool_specs,
            code_mode_tool_names,
            system_prompt,
            model,
        )?
        .with_thread_id(events.request_id()),
        events.clone(),
        Arc::clone(transport_stats),
    ))
}

pub(super) const fn status(success: bool) -> &'static str {
    if success { "completed" } else { "failed" }
}

pub(super) const fn otel_status(success: bool) -> &'static str {
    if success { "OK" } else { "ERROR" }
}
