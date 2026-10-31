use std::{
    borrow::Cow,
    num::NonZeroU32,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
    },
};

use crate::client::{
    AgentEventKind, EventError, EventSink, Model, ResponseEvent, ResponseItem, ResponsesTransport,
    Thinking,
    pricing::ServiceTier,
    responses::{RequestProfile, ResponseHistory, ResponsesInput, WarmupResponse},
    session::state::RequestHistory,
    tower::transport_policy::SessionTransport,
};
use nanocode_platform::Instant;
use serde::Serialize;
use tokio::sync::mpsc;

use crate::client::stream::{CompactionOutput, GenerationOutput};

const RESPONSE_MAX_ATTEMPTS: NonZeroU32 = NonZeroU32::new(5).unwrap();

const COMPACTION_MAX_ATTEMPTS: u32 = 3;

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ResponsesAttemptKind {
    Warmup,

    Generation,

    Compaction,

    Connect,
}

impl ResponsesAttemptKind {
    pub(crate) const fn phase(self) -> &'static str {
        match self {
            Self::Warmup => "warmup",
            Self::Generation => "generation",
            Self::Compaction => "compaction",
            Self::Connect => "connect",
        }
    }
}

#[derive(Clone)]
pub(crate) struct ResponsesObserver {
    pub(crate) events: EventSink,
    pub(crate) raw_api_events: bool,
    pub(crate) tool_input_deltas: bool,
    pub(crate) stats: Arc<TransportStats>,
    response_events: Option<mpsc::Sender<ResponseEvent>>,
}

impl ResponsesObserver {
    pub(crate) fn emit<P: Serialize>(
        &self,
        kind: AgentEventKind,
        payload: P,
    ) -> Result<(), EventError> {
        self.events.emit(kind, payload)
    }

    pub(crate) const fn wants_responses(&self) -> bool {
        self.response_events.is_some()
    }

    pub(crate) async fn emit_response(&self, event: ResponseEvent) {
        if let Some(events) = &self.response_events {
            drop(events.send(event).await);
        }
    }
}

#[derive(Default)]
pub struct TransportStats {
    pub(crate) connection_attempts: AtomicU32,
    pub(crate) websocket_reconnects: AtomicU32,
    pub(crate) response_attempts: AtomicU32,
    pub(crate) response_retries: AtomicU32,
    pub(crate) connection_duration_ns: AtomicU64,
    pub(crate) retry_backoff_duration_ns: AtomicU64,
}

#[derive(Clone, Copy, Default)]
pub struct TransportStatsSnapshot {
    connection_attempts: u32,
    websocket_reconnects: u32,
    response_attempts: u32,
    response_retries: u32,
    connection_duration_ns: u64,
    retry_backoff_duration_ns: u64,
}

impl TransportStats {
    #[must_use]
    pub fn snapshot(&self) -> TransportStatsSnapshot {
        TransportStatsSnapshot {
            connection_attempts: self.connection_attempts.load(Ordering::Relaxed),
            websocket_reconnects: self.websocket_reconnects.load(Ordering::Relaxed),
            response_attempts: self.response_attempts.load(Ordering::Relaxed),
            response_retries: self.response_retries.load(Ordering::Relaxed),
            connection_duration_ns: self.connection_duration_ns.load(Ordering::Relaxed),
            retry_backoff_duration_ns: self.retry_backoff_duration_ns.load(Ordering::Relaxed),
        }
    }

    #[must_use]
    pub fn since(&self, before: TransportStatsSnapshot) -> TransportStatsDelta {
        let after = self.snapshot();
        TransportStatsDelta {
            connection_attempts: after
                .connection_attempts
                .saturating_sub(before.connection_attempts),
            websocket_reconnects: after
                .websocket_reconnects
                .saturating_sub(before.websocket_reconnects),
            response_attempts: after
                .response_attempts
                .saturating_sub(before.response_attempts),
            response_retries: after
                .response_retries
                .saturating_sub(before.response_retries),
            connection_duration_ns: after
                .connection_duration_ns
                .saturating_sub(before.connection_duration_ns),
            retry_backoff_duration_ns: after
                .retry_backoff_duration_ns
                .saturating_sub(before.retry_backoff_duration_ns),
        }
    }
}

#[derive(Clone, Copy, Default)]
pub struct TransportStatsDelta {
    pub connection_attempts: u32,

    pub websocket_reconnects: u32,

    pub response_attempts: u32,

    pub response_retries: u32,

    pub connection_duration_ns: u64,

    pub retry_backoff_duration_ns: u64,
}

#[derive(Clone)]
pub struct ResponsesAttempt {
    pub(crate) kind: ResponsesAttemptKind,
    pub(crate) call_index: Option<u32>,
    full_history: ResponseHistory,
    incremental_history: ResponseHistory,
    incremental_start: usize,
    tail: Option<ResponseItem>,
    previous_response_id: Option<String>,
    model: Model,
    thinking: Thinking,
    service_tier: ServiceTier,
    pub(crate) profile: Arc<RequestProfile>,
    pub(crate) observer: ResponsesObserver,
    pub(crate) attempt: u32,
    pub(crate) max_attempts: u32,
    full_replay: bool,
    pub(crate) logical_turn: u64,
    session_transport: Arc<SessionTransport>,
    pub(crate) model_call_started_at: Option<Instant>,
    input_trace: Option<InputTrace>,
    pub(crate) origin: nanocode_http::layer::origin::Origin,
}

#[derive(Clone)]
struct InputTrace {
    span: tracing::Span,
    full_replay: bool,
    traced: Arc<AtomicBool>,
}

impl ResponsesAttempt {
    fn warmup(
        model: Model,
        thinking: Thinking,
        service_tier: ServiceTier,
        profile: Arc<RequestProfile>,
        observer: ResponsesObserver,
        session_transport: Arc<SessionTransport>,
    ) -> Self {
        Self {
            kind: ResponsesAttemptKind::Warmup,
            call_index: None,
            full_history: ResponseHistory::default(),
            incremental_history: ResponseHistory::default(),
            incremental_start: 0,
            tail: None,
            previous_response_id: None,
            model,
            thinking,
            service_tier,
            profile,
            observer,
            attempt: 1,
            max_attempts: 1,
            full_replay: false,
            logical_turn: 0,
            session_transport,
            model_call_started_at: None,
            input_trace: None,
            origin: nanocode_http::layer::origin::captured(),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn generation(
        call_index: u32,
        full_history: ResponseHistory,
        incremental_history: ResponseHistory,
        incremental_start: usize,
        previous_response_id: Option<&str>,
        model: Model,
        thinking: Thinking,
        service_tier: ServiceTier,
        profile: Arc<RequestProfile>,
        observer: ResponsesObserver,
        session_transport: Arc<SessionTransport>,
    ) -> Self {
        Self {
            kind: ResponsesAttemptKind::Generation,
            call_index: Some(call_index),
            full_history,
            incremental_history,
            incremental_start,
            tail: None,
            previous_response_id: previous_response_id.map(str::to_owned),
            model,
            thinking,
            service_tier,
            profile,
            observer,
            attempt: 1,
            max_attempts: RESPONSE_MAX_ATTEMPTS.get(),
            full_replay: previous_response_id.is_none(),
            logical_turn: 0,
            session_transport,
            model_call_started_at: None,
            input_trace: None,
            origin: nanocode_http::layer::origin::captured(),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn compaction(
        call_index: u32,
        full_history: ResponseHistory,
        incremental_history: ResponseHistory,
        incremental_start: usize,
        previous_response_id: Option<&str>,
        trigger: ResponseItem,
        model: Model,
        thinking: Thinking,
        service_tier: ServiceTier,
        profile: Arc<RequestProfile>,
        observer: ResponsesObserver,
        session_transport: Arc<SessionTransport>,
    ) -> Self {
        Self {
            kind: ResponsesAttemptKind::Compaction,
            call_index: Some(call_index),
            full_history,
            incremental_history,
            incremental_start,
            tail: Some(trigger),
            previous_response_id: previous_response_id.map(str::to_owned),
            model,
            thinking,
            service_tier,
            profile,
            observer,
            attempt: 1,
            max_attempts: COMPACTION_MAX_ATTEMPTS,
            full_replay: previous_response_id.is_none(),
            logical_turn: 0,
            session_transport,
            model_call_started_at: None,
            input_trace: None,
            origin: nanocode_http::layer::origin::captured(),
        }
    }

    #[must_use]
    pub const fn with_model_call_started_at(mut self, started_at: Instant) -> Self {
        self.model_call_started_at = Some(started_at);
        self
    }

    #[doc(hidden)]
    #[must_use]
    pub fn with_input_trace(mut self, span: Option<tracing::Span>) -> Self {
        self.input_trace = span.map(|span| InputTrace {
            span,
            full_replay: self.full_replay,
            traced: Arc::default(),
        });
        self
    }

    pub(crate) fn trace_encoded_input(&self, request: &str) {
        let Some(trace) = &self.input_trace else {
            return;
        };
        if trace.traced.swap(true, Ordering::Relaxed) {
            return;
        }
        let input = if trace.full_replay == self.full_replay {
            encoded_input(request).map(Cow::Borrowed)
        } else {
            serde_json::to_string(&ResponsesInput::history_suffix(
                &[],
                &self.incremental_history,
                self.incremental_start,
                self.tail.as_ref(),
            ))
            .ok()
            .map(Cow::Owned)
        };
        let Some(input) = input else {
            return;
        };
        trace.span.record("model.input.bytes", input.len());
        trace.span.in_scope(|| {
            tracing::info!(
                target: "nanocode",
                content_kind = "model.input",
                content = &*input,
                "trace content"
            );
        });
    }

    pub(crate) fn input(&self) -> ResponsesInput<'_> {
        if matches!(
            self.kind,
            ResponsesAttemptKind::Warmup | ResponsesAttemptKind::Connect
        ) {
            return ResponsesInput::new(self.profile.prefix(), &[], None);
        }
        if self.full_replay {
            ResponsesInput::history(
                self.profile.prefix(),
                &self.full_history,
                self.tail.as_ref(),
            )
        } else {
            ResponsesInput::history_suffix(
                &[],
                &self.incremental_history,
                self.incremental_start,
                self.tail.as_ref(),
            )
        }
    }

    pub(crate) fn freeform_calls(
        &self,
        gateway: Option<&crate::client::transport::ResponsesGateway>,
    ) -> Option<crate::client::transport::freeform::FreeformCalls> {
        gateway.filter(|gateway| gateway.sends_custom_tools_as_functions(self.model))?;
        Some(crate::client::transport::freeform::FreeformCalls::new(
            ResponsesInput::history(
                self.profile.prefix(),
                &self.full_history,
                self.tail.as_ref(),
            )
            .iter(),
        ))
    }

    #[must_use]
    pub const fn kind(&self) -> ResponsesAttemptKind {
        self.kind
    }

    #[must_use]
    pub const fn model_call_index(&self) -> Option<u32> {
        self.call_index
    }

    #[must_use]
    pub const fn thinking(&self) -> Thinking {
        self.thinking
    }

    #[must_use]
    pub const fn model(&self) -> Model {
        self.model
    }

    #[must_use]
    pub const fn service_tier(&self) -> ServiceTier {
        self.service_tier.effective_for_model(self.model)
    }

    #[must_use]
    pub const fn fast_mode(&self) -> bool {
        !matches!(self.service_tier, ServiceTier::Standard)
    }

    #[must_use]
    pub const fn attempt(&self) -> u32 {
        self.attempt
    }

    pub fn input_items(&self) -> impl Iterator<Item = &ResponseItem> {
        self.input().iter()
    }

    pub async fn emit(&self, event: ResponseEvent) {
        self.observer.emit_response(event).await;
    }

    #[must_use]
    pub fn input_item_count(&self) -> usize {
        self.input().len()
    }

    #[must_use]
    pub fn previous_response_id(&self) -> Option<&str> {
        (!self.full_replay)
            .then_some(self.previous_response_id.as_deref())
            .flatten()
    }

    #[must_use]
    pub const fn is_full_replay(&self) -> bool {
        self.full_replay
    }

    pub(crate) const fn replay_mode(&self) -> &'static str {
        if self.full_replay {
            "full_history"
        } else {
            "incremental"
        }
    }

    pub(crate) const fn prepare_retry(&mut self) -> bool {
        if self.attempt >= self.max_attempts {
            return false;
        }
        self.attempt += 1;
        self.full_replay = true;
        true
    }

    pub(crate) const fn prepare_transport_fallback(&mut self) {
        self.attempt = 1;
        self.full_replay = true;
    }

    pub(crate) fn effective_transport(&self, preferred: ResponsesTransport) -> ResponsesTransport {
        self.session_transport.effective(preferred)
    }

    pub(crate) fn activate_https_fallback(&self) -> bool {
        self.session_transport.activate_https_fallback()
    }

    pub(crate) fn limit_attempts(&mut self, max_attempts: NonZeroU32) {
        self.max_attempts = self.max_attempts.min(max_attempts.get());
    }

    pub(crate) const fn force_full_replay(&mut self) {
        self.full_replay = true;
    }
}

fn encoded_input(request: &str) -> Option<&str> {
    const INPUT_KEY: &str = "\"input\":";
    let start = request.find(INPUT_KEY)? + INPUT_KEY.len();
    let mut deserializer = serde_json::Deserializer::from_str(&request[start..]);
    <&serde_json::value::RawValue as serde::Deserialize>::deserialize(&mut deserializer)
        .ok()
        .map(serde_json::value::RawValue::get)
}

pub enum ResponsesOutput {
    Warmup(WarmupResponse),

    Generation(GenerationOutput),

    Compaction(CompactionOutput),

    Connected,
}

pub struct ResponsesServiceResponse {
    pub(crate) output: ResponsesOutput,
    pub(crate) attempt: u32,
    pub(crate) connection_generation: u32,
    pub(crate) server_reasoning_included: bool,
}

impl ResponsesServiceResponse {
    #[must_use]
    pub const fn new(output: ResponsesOutput) -> Self {
        Self {
            output,
            attempt: 1,
            connection_generation: 0,
            server_reasoning_included: false,
        }
    }

    #[must_use]
    pub const fn with_attempt(mut self, attempt: u32) -> Self {
        self.attempt = attempt;
        self
    }

    #[must_use]
    pub const fn with_connection_generation(mut self, connection_generation: u32) -> Self {
        self.connection_generation = connection_generation;
        self
    }

    #[must_use]
    pub const fn with_server_reasoning_included(mut self, included: bool) -> Self {
        self.server_reasoning_included = included;
        self
    }

    #[must_use]
    pub const fn attempt(&self) -> u32 {
        self.attempt
    }

    #[must_use]
    pub const fn connection_generation(&self) -> u32 {
        self.connection_generation
    }

    #[must_use]
    pub const fn server_reasoning_included(&self) -> bool {
        self.server_reasoning_included
    }

    #[must_use]
    pub fn into_output(self) -> ResponsesOutput {
        self.output
    }
}

#[derive(Clone, Default)]
pub struct TransportFallback(Arc<SessionTransport>);

#[must_use]
pub struct ResponsesAttemptFactory {
    profile: Arc<RequestProfile>,
    observer: ResponsesObserver,
    logical_turn: u64,
    session_transport: Arc<SessionTransport>,
}

impl ResponsesAttemptFactory {
    pub fn with_transport_fallback(mut self, fallback: &TransportFallback) -> Self {
        self.session_transport = Arc::clone(&fallback.0);
        self
    }

    pub fn new(profile: RequestProfile, events: EventSink, stats: Arc<TransportStats>) -> Self {
        Self {
            profile: Arc::new(profile),
            observer: ResponsesObserver {
                events,
                raw_api_events: true,
                tool_input_deltas: false,
                stats,
                response_events: None,
            },
            logical_turn: 0,
            session_transport: Arc::new(SessionTransport::new()),
        }
    }

    pub fn set_events(&mut self, events: EventSink) {
        self.observer.events = events;
    }

    pub(crate) fn with_response_events(
        mut self,
        response_events: mpsc::Sender<ResponseEvent>,
    ) -> Self {
        self.observer.response_events = Some(response_events);
        self
    }

    pub fn for_logical_turn(&self, logical_turn: u64) -> Self {
        Self {
            profile: Arc::new((*self.profile).clone().with_logical_turn(logical_turn)),
            observer: self.observer.clone(),
            logical_turn,
            session_transport: Arc::clone(&self.session_transport),
        }
    }

    #[must_use]
    pub fn profile(&self) -> &RequestProfile {
        &self.profile
    }

    #[doc(hidden)]
    pub fn with_request_content(
        &self,
        prompt_cache_key: String,
        prefix: Arc<[ResponseItem]>,
        model_id_prefix: Option<String>,
        reasoning_mode: crate::client::ReasoningMode,
        store_responses: bool,
    ) -> Self {
        Self {
            profile: Arc::new((*self.profile).clone().with_request_content(
                prompt_cache_key,
                prefix,
                model_id_prefix,
                reasoning_mode,
                store_responses,
            )),
            observer: self.observer.clone(),
            logical_turn: self.logical_turn,
            session_transport: Arc::clone(&self.session_transport),
        }
    }

    #[must_use]
    pub fn warmup(
        &self,
        model: Model,
        thinking: Thinking,
        service_tier: impl Into<ServiceTier>,
    ) -> ResponsesAttempt {
        let mut attempt = ResponsesAttempt::warmup(
            model,
            thinking,
            service_tier.into(),
            Arc::clone(&self.profile),
            self.observer.clone(),
            Arc::clone(&self.session_transport),
        );
        attempt.logical_turn = self.logical_turn;
        attempt
    }

    #[must_use]
    pub fn connect(
        &self,
        model: Model,
        thinking: Thinking,
        service_tier: impl Into<ServiceTier>,
    ) -> ResponsesAttempt {
        let mut attempt = self.warmup(model, thinking, service_tier);
        attempt.kind = ResponsesAttemptKind::Connect;
        attempt
    }

    #[must_use]
    pub fn generation(
        &self,
        call_index: u32,
        history: &RequestHistory,
        model: Model,
        thinking: Thinking,
        service_tier: impl Into<ServiceTier>,
    ) -> ResponsesAttempt {
        let mut attempt = ResponsesAttempt::generation(
            call_index,
            history.full.clone(),
            history.incremental.clone(),
            history.incremental_start,
            history.previous_response_id.as_deref(),
            model,
            thinking,
            service_tier.into(),
            Arc::clone(&self.profile),
            self.observer.clone(),
            Arc::clone(&self.session_transport),
        );
        attempt.logical_turn = self.logical_turn;
        attempt
    }

    #[must_use]
    pub fn compaction(
        &self,
        call_index: u32,
        history: &RequestHistory,
        trigger: ResponseItem,
        model: Model,
        thinking: Thinking,
        service_tier: impl Into<ServiceTier>,
    ) -> ResponsesAttempt {
        let mut attempt = ResponsesAttempt::compaction(
            call_index,
            history.full.clone(),
            history.incremental.clone(),
            history.incremental_start,
            history.previous_response_id.as_deref(),
            trigger,
            model,
            thinking,
            service_tier.into(),
            Arc::clone(&self.profile),
            self.observer.clone(),
            Arc::clone(&self.session_transport),
        );
        attempt.logical_turn = self.logical_turn;
        attempt
    }
}
