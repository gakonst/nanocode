use super::{ClaudeError, ContentBlock, Lineage, NanocodeError, Result, StopReason, TurnResult};
use crate::{CacheTtl, MessageResponse, MessagesRequest, Speed, WireMessage};
use serde::Serialize;
use tracing::{Span, field::Empty, info_span};

pub(super) struct TurnLabels<'a> {
    pub(super) session_id: &'a str,
    pub(super) lineage: &'a Lineage,
    pub(super) model: &'a str,
    pub(super) reasoning_mode: &'a str,
    pub(super) effort: &'a str,
    pub(super) turn_index: u64,
    pub(super) prompt_bytes: usize,
}

pub(super) fn turn_span(parent: &Span, labels: &TurnLabels<'_>) -> Span {
    let parent_id = parent.id();
    let span = info_span!(
        target: "nanocode",
        parent: parent_id.clone(),
        "agent.turn",
        otel.kind = "internal",
        otel.status_code = Empty,
        session.id = labels.session_id,
        session.root_id = labels.lineage.root_session_id.as_str(),
        parent.session.id = Empty,
        agent.origin = Empty,
        agent.depth = labels.lineage.depth,
        trace.parented = parent_id.is_some(),
        model = labels.model,
        reasoning.mode = labels.reasoning_mode,
        reasoning.effort = labels.effort,
        thinking = labels.effort,
        turn.index = labels.turn_index,
        prompt.bytes = labels.prompt_bytes,
        usage.input_tokens = Empty,
        usage.cached_input_tokens = Empty,
        usage.cache_write_input_tokens = Empty,
        usage.output_tokens = Empty,
        usage.reasoning_output_tokens = Empty,
        usage.total_tokens = Empty,
        cost.usd = Empty,
        cost.status = Empty,
        cost.service_tier = Empty,
        status = Empty,
    );
    if let Some(parent_session_id) = &labels.lineage.parent_session_id {
        span.record("parent.session.id", parent_session_id.as_str());
    }
    if let Ok(serde_json::Value::String(origin)) = serde_json::to_value(labels.lineage.origin) {
        span.record("agent.origin", origin.as_str());
    }
    span
}

pub(super) fn finish_turn(span: &Span, result: &Result<TurnResult>, panicked: bool) {
    if let Some(usage) = result.as_ref().ok().and_then(TurnResult::usage) {
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
            span.record("cost.usd", cost.amount().decimal().as_str());
            span.record("cost.service_tier", cost.service_tier().as_str());
        }
    }
    if panicked {
        span.record("status", "failed");
        span.record("otel.status_code", "ERROR");
        tracing::error!(target: "nanocode", parent: span, "agent turn panicked");
        return;
    }
    finish(span, result.as_ref().err(), "agent turn failed");
}

pub(super) fn model_call_span(
    session_id: &str,
    call_index: u32,
    model: &str,
    reasoning_mode: &str,
    effort: &str,
    message_count: usize,
) -> Span {
    info_span!(
        target: "nanocode",
        "model.call",
        otel.kind = "internal",
        otel.status_code = Empty,
        session.id = session_id,
        model,
        reasoning.mode = reasoning_mode,
        reasoning.effort = effort,
        model.call_index = call_index,
        model.input.item_count = message_count,
        model.replayed = Empty,
        model.attempts = Empty,
        model.response.id = Empty,
        model.response.status = Empty,
        model.output.item_count = Empty,
        model.tool_call_count = Empty,
        status = Empty,
        duration_ns = Empty,
        input_tokens = Empty,
        cached_input_tokens = Empty,
        cache_write_input_tokens = Empty,
        output_tokens = Empty,
        total_tokens = Empty,
        time_to_first_event_ns = Empty,
        time_to_first_output_ns = Empty,
        time_to_dispatch_ns = Empty,
        prompt.tools = Empty,
        prompt.system = Empty,
        prompt.first_message = Empty,
        request.speed = Empty,
        request.cache_ttl = Empty,
        request.wire = Empty,
    )
}

pub(super) fn record_prompt<M: WireMessage>(
    span: &Span,
    request: &MessagesRequest,
    messages: &[M],
    subscription: bool,
    cache: &crate::WireCache,
) {
    if span.is_disabled() {
        return;
    }
    span.record(
        "request.wire",
        if subscription { "subscription" } else { "api" },
    );
    if let Ok(tools) = cache.tools_fingerprint(&request.tools) {
        span.record("prompt.tools", tools.as_str());
    }
    if let Ok(system) = cache.system_fingerprint(request.system.as_deref()) {
        span.record("prompt.system", system.as_str());
    }
    if let Some(first) = messages.first() {
        let encode = |message: &crate::Message| {
            serde_json::to_string(message).map_err(crate::ClaudeError::from)
        };
        if let Ok(encoded) = first.encoded(false, &encode) {
            span.record(
                "prompt.first_message",
                crate::fingerprint_bytes(encoded.as_bytes()).as_str(),
            );
        }
    }
    if let Some(speed) = request.speed {
        span.record(
            "request.speed",
            match speed {
                Speed::Standard => "standard",
                Speed::Fast => "fast",
            },
        );
    }
    if let Some(control) = &request.cache_control {
        span.record(
            "request.cache_ttl",
            match control.ttl {
                Some(CacheTtl::OneHour) => "1h",
                Some(CacheTtl::FiveMinutes) | None => "5m",
            },
        );
    }
}

pub(super) struct ModelCallTiming {
    pub(super) attempt: u32,
    pub(super) first_event_ns: u64,
    pub(super) first_output_ns: Option<u64>,
    pub(super) dispatch_ns: Option<u64>,
}

#[derive(Default, Serialize)]
pub(super) struct CallTiming {
    pub(super) replayed: bool,
    pub(super) preparation_ns: Option<u64>,
    pub(super) attempt_started_ns: Option<u64>,
    pub(super) send_ns: Option<u64>,
    pub(super) response_headers_ns: Option<u64>,
    pub(super) first_event_ns: Option<u64>,
    pub(super) first_output_ns: Option<u64>,
    pub(super) response_completed_ns: Option<u64>,
    pub(super) total_ns: u64,
    pub(super) pipeline: Option<CallPipeline>,
}

#[derive(Default, Serialize)]
pub(super) struct CallPipeline {
    pub(super) event_count: u64,
    pub(super) receive_decode_ns: u64,
    pub(super) display_delta_emit_ns: u64,
    pub(super) collect_ns: u64,
    pub(super) display_delta_count: u64,
    pub(super) display_delta_bytes: u64,
}

pub(super) fn elapsed_ns(started: nanocode_platform::Instant) -> u64 {
    u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX)
}

pub(super) fn record_model_response(
    span: &Span,
    response: &MessageResponse,
    timing: &ModelCallTiming,
) {
    let input_tokens = response
        .usage
        .input_tokens
        .saturating_add(response.usage.cache_read_input_tokens)
        .saturating_add(response.usage.cache_creation_input_tokens);
    span.record("model.replayed", timing.attempt == 0);
    if timing.attempt != 0 {
        span.record("model.attempts", timing.attempt);
        span.record("time_to_first_event_ns", timing.first_event_ns);
    }
    span.record("model.response.id", response.id.as_str());
    span.record(
        "model.response.status",
        stop_reason(response.stop_reason.unwrap_or(StopReason::Unknown)),
    );
    span.record("model.output.item_count", response.content.len());
    span.record(
        "model.tool_call_count",
        response
            .content
            .iter()
            .filter(|block| matches!(block, ContentBlock::ToolUse { .. }))
            .count(),
    );
    span.record("input_tokens", input_tokens);
    span.record(
        "cached_input_tokens",
        response.usage.cache_read_input_tokens,
    );
    span.record(
        "cache_write_input_tokens",
        response.usage.cache_creation_input_tokens,
    );
    span.record("output_tokens", response.usage.output_tokens);
    span.record(
        "total_tokens",
        input_tokens.saturating_add(response.usage.output_tokens),
    );
    if let Some(first_output_ns) = timing.first_output_ns {
        span.record("time_to_first_output_ns", first_output_ns);
    }
    if let Some(dispatch_ns) = timing.dispatch_ns {
        span.record("time_to_dispatch_ns", dispatch_ns);
    }
}

pub(super) fn finish_model_call(span: &Span, duration_ns: u64, error: Option<&NanocodeError>) {
    span.record("duration_ns", duration_ns);
    finish(span, error, "model call failed");
}

pub(super) struct CompactionLabels<'a> {
    pub(super) session_id: &'a str,
    pub(super) model: &'a str,
    pub(super) trigger: &'a str,
    pub(super) step: &'a str,
    pub(super) active_context_tokens: u64,
    pub(super) threshold: u64,
}

pub(super) fn compaction_span(labels: &CompactionLabels<'_>) -> Span {
    info_span!(
        target: "nanocode",
        "model.compaction",
        otel.kind = "internal",
        otel.status_code = Empty,
        session.id = labels.session_id,
        model = labels.model,
        compaction.trigger = labels.trigger,
        compaction.step = labels.step,
        active_context_tokens = labels.active_context_tokens,
        auto_compact_token_limit = labels.threshold,
        compacted_context_tokens = Empty,
        model.response.id = Empty,
        status = Empty,
        duration_ns = Empty,
        input_tokens = Empty,
        cached_input_tokens = Empty,
        cache_write_input_tokens = Empty,
        output_tokens = Empty,
        total_tokens = Empty,
    )
}

pub(super) fn record_compaction(span: &Span, usage: &crate::Usage, compacted_context_tokens: u64) {
    let input_tokens = usage
        .input_tokens
        .saturating_add(usage.cache_read_input_tokens)
        .saturating_add(usage.cache_creation_input_tokens);
    span.record("compacted_context_tokens", compacted_context_tokens);
    span.record("input_tokens", input_tokens);
    span.record("cached_input_tokens", usage.cache_read_input_tokens);
    span.record(
        "cache_write_input_tokens",
        usage.cache_creation_input_tokens,
    );
    span.record("output_tokens", usage.output_tokens);
    span.record(
        "total_tokens",
        input_tokens.saturating_add(usage.output_tokens),
    );
}

pub(super) fn finish_compaction(span: &Span, duration_ns: u64, error: Option<&NanocodeError>) {
    span.record("duration_ns", duration_ns);
    finish(span, error, "compaction failed");
}

pub(super) fn attempt_span(call_index: u32, attempt: u32, max_attempts: u32) -> Span {
    info_span!(
        target: "nanocode_claude",
        "messages.attempt",
        otel.kind = "client",
        otel.status_code = Empty,
        model.call_index = call_index,
        attempt,
        max_attempts,
        request.bytes = Empty,
        request.gzip_bytes = Empty,
        http.status_code = Empty,
        error.class = Empty,
        error.kind = Empty,
        error.transient = Empty,
        retry.delay_ns = Empty,
        retry.server_requested = Empty,
        stream.accepted = Empty,
        time_to_first_event_ns = Empty,
        time_to_first_output_ns = Empty,
        status = Empty,
        duration_ns = Empty,
    )
}

pub(super) struct AttemptTiming {
    pub(super) accepted: bool,
    pub(super) first_event_ns: Option<u64>,
    pub(super) first_output_ns: Option<u64>,
    pub(super) duration_ns: u64,
}

pub(super) fn record_attempt(span: &Span, timing: &AttemptTiming) {
    span.record("stream.accepted", timing.accepted);
    if let Some(first_event_ns) = timing.first_event_ns {
        span.record("time_to_first_event_ns", first_event_ns);
    }
    if let Some(first_output_ns) = timing.first_output_ns {
        span.record("time_to_first_output_ns", first_output_ns);
    }
    span.record("duration_ns", timing.duration_ns);
}

pub(super) fn complete_attempt(span: &Span) {
    span.record("status", "completed");
    span.record("otel.status_code", "OK");
}

pub(super) fn cancel_attempt(span: &Span) {
    span.record("status", "cancelled");
    span.record("otel.status_code", "ERROR");
}

pub(super) fn fail_attempt(
    span: &Span,
    error: &ClaudeError,
    retry_delay_ns: Option<u64>,
    retry_after: bool,
) {
    if let ClaudeError::Http { status, .. } = error {
        span.record("http.status_code", *status);
    }
    let (class, kind) = error_class(error);
    span.record("error.class", class);
    if let Some(kind) = kind {
        span.record("error.kind", kind);
    }
    span.record("error.transient", error.is_transient());
    span.record(
        "status",
        if retry_delay_ns.is_some() {
            "retrying"
        } else {
            "failed"
        },
    );
    span.record("otel.status_code", "ERROR");
    if let Some(delay_ns) = retry_delay_ns {
        span.record("retry.delay_ns", delay_ns);
        span.record("retry.server_requested", retry_after);
    }
    let mut message = error.to_string();
    let mut source = std::error::Error::source(error);
    while let Some(cause) = source {
        let cause_text = cause.to_string();
        if !message.contains(&cause_text) {
            message.push_str(": ");
            message.push_str(&cause_text);
        }
        source = cause.source();
    }
    tracing::warn!(target: "nanocode_claude", parent: span, error = %message, "Claude Messages attempt failed");
}

fn finish(span: &Span, error: Option<&NanocodeError>, message: &'static str) {
    let ended_as_asked = |error: &NanocodeError| {
        matches!(
            error,
            NanocodeError::TurnCancelled | NanocodeError::TurnStopped
        )
    };
    let status = match error {
        None => "completed",
        Some(error) if ended_as_asked(error) => "cancelled",
        Some(_) => "failed",
    };
    span.record("status", status);
    span.record(
        "otel.status_code",
        if error.is_none() { "OK" } else { "ERROR" },
    );
    if let Some(error) = error
        && !ended_as_asked(error)
    {
        tracing::error!(target: "nanocode", parent: span, error = %error, "{message}");
    }
}

const fn error_class(error: &ClaudeError) -> (&'static str, Option<&str>) {
    match error {
        ClaudeError::Http { .. } => ("http", None),
        ClaudeError::Transport(_) => ("transport", None),
        ClaudeError::Json(_) => ("json", None),
        ClaudeError::StreamError { kind, .. } => ("stream", Some(kind.as_str())),
        ClaudeError::Protocol(_) => ("protocol", None),
        ClaudeError::IncompleteStream => ("incomplete_stream", None),
        ClaudeError::AuthUnavailable(_) => ("auth_unavailable", None),
    }
}

pub(super) fn retry_class(error: &ClaudeError) -> &'static str {
    match error {
        ClaudeError::Http { status: 429, .. } => "http_rate_limit",
        ClaudeError::Http { status: 529, .. } => "http_overloaded",
        ClaudeError::Http {
            status: 500..=599, ..
        } => "http_server",
        ClaudeError::Http { .. } => "http",
        ClaudeError::Transport(crate::transport::HttpError::Transport {
            timeout: true, ..
        }) => "transport_timeout",
        ClaudeError::Transport(_) => "transport",
        ClaudeError::StreamError { kind, .. } => match kind.as_str() {
            "overloaded_error" => "stream_overloaded",
            "rate_limit_error" => "stream_rate_limit",
            "timeout_error" => "stream_timeout",
            "api_error" => "stream_api_error",
            _ => "stream",
        },
        ClaudeError::IncompleteStream => "incomplete_stream",
        ClaudeError::Json(_) => "json",
        ClaudeError::Protocol(_) => "protocol",
        ClaudeError::AuthUnavailable(_) => "auth_unavailable",
    }
}

const fn stop_reason(reason: StopReason) -> &'static str {
    match reason {
        StopReason::EndTurn => "end_turn",
        StopReason::MaxTokens => "max_tokens",
        StopReason::StopSequence => "stop_sequence",
        StopReason::ToolUse => "tool_use",
        StopReason::PauseTurn => "pause_turn",
        StopReason::Refusal => "refusal",
        StopReason::ModelContextWindowExceeded => "model_context_window_exceeded",
        StopReason::Unknown => "unknown",
    }
}
