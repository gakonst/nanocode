use std::{
    convert::Infallible,
    error::Error,
    fmt,
    future::{Future, IntoFuture},
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};

use crate::client::call::Service;
use futures_util::{Stream, future::poll_fn};
use nanocode_platform::Instant;
use tokio::sync::mpsc;
use tracing::{Instrument, info_span};

use crate::client::{
    ContentItem, EventSink, MessageRole, ResponseEvent, ResponseItem, ResponsesAttempt,
    ResponsesAttemptFactory, ResponsesOutput, ResponsesServiceError, ResponsesServiceResponse,
    Usage, pricing::ServiceTier,
};

use super::{
    builder::{ResponseTurn, Session},
    compaction,
    context::{assign_missing_response_item_ids, is_canonical_context_item},
    state::RejectedRequestRepair,
};

#[derive(Clone, Debug)]
pub struct ResponseInput {
    items: Vec<ResponseItem>,
}

impl ResponseInput {
    #[must_use]
    pub fn content(content: impl IntoIterator<Item = ContentItem>) -> Self {
        Self {
            items: vec![ResponseItem::message(MessageRole::User, content)],
        }
    }

    #[must_use]
    pub fn estimated_tokens(&self) -> u64 {
        self.items
            .iter()
            .map(compaction::estimate_item_tokens)
            .fold(0, u64::saturating_add)
    }

    #[must_use]
    pub fn items(items: impl IntoIterator<Item = ResponseItem>) -> Self {
        Self {
            items: items.into_iter().collect(),
        }
    }
}

impl From<String> for ResponseInput {
    fn from(text: String) -> Self {
        Self::content([ContentItem::InputText {
            text: text.into_boxed_str(),
        }])
    }
}

impl From<&str> for ResponseInput {
    fn from(text: &str) -> Self {
        Self::from(text.to_owned())
    }
}

#[derive(Clone)]
pub struct CompletedResponse {
    output: Arc<[ResponseItem]>,
    output_text: Arc<str>,
    reported_model: Option<String>,
    usage: Option<Usage>,
    estimated_cost: Option<crate::client::EstimatedUsdCost>,
    cost_status: crate::client::CostStatus,
    end_turn: Option<bool>,
}

impl CompletedResponse {
    #[must_use]
    pub fn output(&self) -> &[ResponseItem] {
        &self.output
    }

    #[must_use]
    pub fn output_text(&self) -> &str {
        &self.output_text
    }

    #[must_use]
    pub fn reported_model(&self) -> Option<&str> {
        self.reported_model.as_deref()
    }

    pub fn tool_calls(&self) -> impl Iterator<Item = &ResponseItem> {
        self.output.iter().filter(|item| {
            matches!(
                item,
                ResponseItem::FunctionCall { .. }
                    | ResponseItem::CustomToolCall { .. }
                    | ResponseItem::LocalShellCall { .. }
                    | ResponseItem::ToolSearchCall { .. }
            )
        })
    }

    #[must_use]
    pub const fn usage(&self) -> Option<&Usage> {
        self.usage.as_ref()
    }

    #[must_use]
    pub const fn estimated_cost(&self) -> Option<&crate::client::EstimatedUsdCost> {
        self.estimated_cost.as_ref()
    }

    #[must_use]
    pub const fn cost_status(&self) -> crate::client::CostStatus {
        self.cost_status
    }

    #[must_use]
    pub const fn end_turn(&self) -> Option<bool> {
        self.end_turn
    }
}

impl fmt::Debug for CompletedResponse {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CompletedResponse")
            .field("output_items", &self.output.len())
            .field("output_text", &self.output_text)
            .field("end_turn", &self.end_turn)
            .finish_non_exhaustive()
    }
}

#[derive(Clone)]
pub struct CompletedCompaction {
    usage: Option<Usage>,
    estimated_cost: Option<crate::client::EstimatedUsdCost>,
    cost_status: crate::client::CostStatus,
}

impl CompletedCompaction {
    #[must_use]
    pub const fn usage(&self) -> Option<&Usage> {
        self.usage.as_ref()
    }

    #[must_use]
    pub const fn estimated_cost(&self) -> Option<&crate::client::EstimatedUsdCost> {
        self.estimated_cost.as_ref()
    }

    #[must_use]
    pub const fn cost_status(&self) -> crate::client::CostStatus {
        self.cost_status
    }
}

#[cfg(not(target_family = "wasm"))]
type ResponseRun<'a> =
    Pin<Box<dyn Future<Output = Result<CompletedResponse, ResponseError>> + Send + 'a>>;
#[cfg(target_family = "wasm")]
type ResponseRun<'a> = Pin<Box<dyn Future<Output = Result<CompletedResponse, ResponseError>> + 'a>>;

#[must_use = "a response does no work unless it is streamed or awaited"]
pub struct Response<'a> {
    events: mpsc::Receiver<ResponseEvent>,
    run: ResponseRun<'a>,
    progress: Progress,
}

enum Progress {
    Running {
        completed: bool,
    },
    Succeeded {
        result: CompletedResponse,
        completed: bool,
    },
    Failed {
        error: ResponseError,
        reported: bool,
    },
}

impl<'a> Response<'a> {
    pub(super) fn new(events: mpsc::Receiver<ResponseEvent>, run: ResponseRun<'a>) -> Self {
        Self {
            events,
            run,
            progress: Progress::Running { completed: false },
        }
    }
}

impl Stream for Response<'_> {
    type Item = Result<ResponseEvent, ResponseError>;

    fn poll_next(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = &mut *self;
        loop {
            if let Poll::Ready(Some(event)) = this.events.poll_recv(context) {
                if matches!(event, ResponseEvent::Completed { .. })
                    && let Progress::Running { completed } | Progress::Succeeded { completed, .. } =
                        &mut this.progress
                {
                    *completed = true;
                }
                return Poll::Ready(Some(Ok(event)));
            }
            return match &mut this.progress {
                Progress::Running { completed } => match this.run.as_mut().poll(context) {
                    Poll::Ready(Ok(result)) => {
                        this.progress = Progress::Succeeded {
                            result,
                            completed: *completed,
                        };
                        continue;
                    }
                    Poll::Ready(Err(error)) => {
                        this.progress = Progress::Failed {
                            error,
                            reported: false,
                        };
                        continue;
                    }
                    Poll::Pending => Poll::Pending,
                },
                Progress::Succeeded {
                    result,
                    completed: completed @ false,
                } => {
                    *completed = true;
                    Poll::Ready(Some(Ok(ResponseEvent::Completed {
                        usage: result.usage.clone(),
                        end_turn: result.end_turn,
                    })))
                }
                Progress::Failed {
                    error,
                    reported: reported @ false,
                } => {
                    *reported = true;
                    Poll::Ready(Some(Err(error.clone())))
                }
                Progress::Succeeded { .. } | Progress::Failed { .. } => Poll::Ready(None),
            };
        }
    }
}

#[cfg(not(target_family = "wasm"))]
type ResponseIntoFuture<'a> =
    Pin<Box<dyn Future<Output = Result<CompletedResponse, ResponseError>> + Send + 'a>>;
#[cfg(target_family = "wasm")]
type ResponseIntoFuture<'a> =
    Pin<Box<dyn Future<Output = Result<CompletedResponse, ResponseError>> + 'a>>;

impl<'a> IntoFuture for Response<'a> {
    type Output = Result<CompletedResponse, ResponseError>;
    type IntoFuture = ResponseIntoFuture<'a>;

    fn into_future(mut self) -> Self::IntoFuture {
        Box::pin(async move {
            while poll_fn(|context| Pin::new(&mut self).poll_next(context))
                .await
                .is_some()
            {}
            match self.progress {
                Progress::Succeeded { result, .. } => Ok(result),
                Progress::Failed { error, .. } => Err(error),
                Progress::Running { .. } => Err(ResponseError::protocol(
                    "response stream ended without a terminal service result",
                )),
            }
        })
    }
}

#[derive(Clone)]
pub struct ResponseError {
    inner: Arc<ResponseErrorInner>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ResponseErrorKind {
    ContextWindowExceeded,

    Service,

    Protocol,
}

enum ResponseErrorInner {
    Source {
        kind: ResponseErrorKind,
        error: Arc<dyn Error + Send + Sync>,
    },
    Protocol(Arc<str>),
}

impl ResponseError {
    #[must_use]
    pub fn service(error: impl Error + Send + Sync + 'static) -> Self {
        let kind = if error_chain_responses_error(&error)
            .is_some_and(crate::client::ResponsesError::is_context_window_exceeded)
        {
            ResponseErrorKind::ContextWindowExceeded
        } else {
            ResponseErrorKind::Service
        };
        let error: Arc<dyn Error + Send + Sync> = Arc::new(error);
        Self {
            inner: Arc::new(ResponseErrorInner::Source { kind, error }),
        }
    }

    fn protocol(detail: impl Into<Arc<str>>) -> Self {
        Self {
            inner: Arc::new(ResponseErrorInner::Protocol(detail.into())),
        }
    }

    #[must_use]
    pub fn kind(&self) -> ResponseErrorKind {
        match self.inner.as_ref() {
            ResponseErrorInner::Source { kind, .. } => *kind,
            ResponseErrorInner::Protocol(_) => ResponseErrorKind::Protocol,
        }
    }

    #[must_use]
    pub fn responses_error(&self) -> Option<&crate::client::ResponsesError> {
        self.source().and_then(error_chain_responses_error)
    }

    #[must_use]
    pub fn is_context_window_exceeded(&self) -> bool {
        matches!(self.kind(), ResponseErrorKind::ContextWindowExceeded)
    }
}

impl From<ResponsesServiceError> for ResponseError {
    fn from(error: ResponsesServiceError) -> Self {
        Self::service(error)
    }
}

impl nanocode_agent_types::BackendFailure for ResponseError {
    fn requires_image_repair(&self) -> bool {
        matches!(
            self.responses_error(),
            Some(crate::client::ResponsesError::InvalidImageRequest { .. })
        )
    }
    fn retryable(&self) -> bool {
        self.responses_error()
            .is_some_and(|source| source.retry_advice().is_some())
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

impl From<ResponseError> for nanocode_agent_types::NanocodeError {
    fn from(error: ResponseError) -> Self {
        Self::Response(nanocode_agent_types::BackendError(Box::new(error)))
    }
}

impl From<crate::client::ResponsesError> for ResponseError {
    fn from(error: crate::client::ResponsesError) -> Self {
        Self::service(error)
    }
}

impl From<crate::client::call::BoxError> for ResponseError {
    fn from(error: crate::client::call::BoxError) -> Self {
        let kind = if error_chain_responses_error(error.as_ref())
            .is_some_and(crate::client::ResponsesError::is_context_window_exceeded)
        {
            ResponseErrorKind::ContextWindowExceeded
        } else {
            ResponseErrorKind::Service
        };
        let error: Arc<dyn Error + Send + Sync> = Arc::from(error);
        Self {
            inner: Arc::new(ResponseErrorInner::Source { kind, error }),
        }
    }
}

impl From<Infallible> for ResponseError {
    fn from(error: Infallible) -> Self {
        match error {}
    }
}

impl fmt::Display for ResponseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.inner.as_ref() {
            ResponseErrorInner::Source {
                kind: ResponseErrorKind::ContextWindowExceeded,
                ..
            } => formatter.write_str("Responses input exceeded the model context window"),
            ResponseErrorInner::Source { error, .. } => error.fmt(formatter),
            ResponseErrorInner::Protocol(detail) => {
                write!(formatter, "invalid Responses state: {detail}")
            }
        }
    }
}

impl fmt::Debug for ResponseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ResponseError")
            .field("message", &self.to_string())
            .finish_non_exhaustive()
    }
}

impl Error for ResponseError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self.inner.as_ref() {
            ResponseErrorInner::Source { error, .. } => Some(error.as_ref()),
            ResponseErrorInner::Protocol(_) => None,
        }
    }
}

fn error_chain_responses_error<'a>(
    mut error: &'a (dyn Error + 'static),
) -> Option<&'a crate::client::ResponsesError> {
    loop {
        if let Some(service) = error.downcast_ref::<ResponsesServiceError>()
            && let Some(error) = service.responses_error()
        {
            return Some(error);
        }
        if let Some(error) = error.downcast_ref::<crate::client::ResponsesError>() {
            return Some(error);
        }
        let source = error.source()?;
        error = source;
    }
}

pub(super) async fn run_create<S>(
    turn: &mut ResponseTurn<'_, S>,
    input: ResponseInput,
    sink: EventSink,
    response_events: mpsc::Sender<ResponseEvent>,
) -> Result<CompletedResponse, ResponseError>
where
    S: Service<ResponsesAttempt, Response = ResponsesServiceResponse>,
    S::Error: Into<ResponseError>,
{
    let span = response_call_span(
        turn.session,
        turn.logical_turn,
        "response.create",
        input.items.len(),
    );
    let started_at = Instant::now();
    let mut cancellation = ResponseCallCancellation::new(span.clone(), started_at);
    let result = run_create_inner(turn, input, sink, response_events)
        .instrument(span.clone())
        .await;
    cancellation.complete();
    finish_create_span(&span, started_at, &result);
    result
}

async fn run_create_inner<S>(
    turn: &mut ResponseTurn<'_, S>,
    mut input: ResponseInput,
    sink: EventSink,
    response_events: mpsc::Sender<ResponseEvent>,
) -> Result<CompletedResponse, ResponseError>
where
    S: Service<ResponsesAttempt, Response = ResponsesServiceResponse>,
    S::Error: Into<ResponseError>,
{
    if input.items.is_empty() {
        return Err(ResponseError::protocol(
            "response.create input must contain at least one item",
        ));
    }

    let session = &mut *turn.session;
    let call_index = session.next_call_index;
    session.next_call_index = session.next_call_index.saturating_add(1);
    assign_missing_response_item_ids(&mut input.items);
    let observed_canonical_context = input
        .items
        .iter()
        .filter(|item| is_canonical_context_item(item))
        .cloned()
        .collect::<Vec<_>>();
    let reinject_canonical_context =
        session.canonical_context_reinjection_pending && observed_canonical_context.is_empty();
    let mut candidate = session.state.clone();
    if reinject_canonical_context {
        candidate.append(session.canonical_context.iter().cloned());
    }
    candidate.append_client(input.items);
    let request_history = candidate.generation_request();

    let factory = ResponsesAttemptFactory::new(
        session.profile.clone(),
        sink,
        Arc::clone(&session.transport_stats),
    )
    .with_response_events(response_events)
    .for_logical_turn(turn.logical_turn);
    let request = factory.generation(
        call_index,
        &request_history,
        session.model,
        session.thinking,
        session.service_tier,
    );
    let success = execute(session, request).await?;
    let server_reasoning_included = success.server_reasoning_included();
    let ResponsesOutput::Generation(response) = success.into_output() else {
        return Err(ResponseError::protocol(
            "response.create returned a non-generation output",
        ));
    };

    candidate
        .complete_generation(
            request_history,
            Some(response.id),
            response.output_items.iter().cloned(),
            response.usage.as_ref(),
            server_reasoning_included,
        )
        .and_then(|()| candidate.commit())
        .map_err(|error| ResponseError::protocol(error.to_string()))?;
    session.state = candidate;
    if !observed_canonical_context.is_empty() {
        session.canonical_context = observed_canonical_context;
    }
    session.canonical_context_reinjection_pending = false;

    let output: Arc<[ResponseItem]> = response.output_items.into();
    let output_text = response
        .final_message
        .unwrap_or_else(|| output_text(&output))
        .into();
    let (estimated_cost, cost_status) =
        estimate_cost(response.usage.as_ref(), session.model, session.service_tier);
    let completed = CompletedResponse {
        output,
        output_text,
        reported_model: response.reported_model,
        usage: response.usage,
        estimated_cost,
        cost_status,
        end_turn: response.end_turn,
    };
    turn.completed_generation = true;
    Ok(completed)
}

pub(super) async fn run_compact<S>(
    turn: &mut ResponseTurn<'_, S>,
) -> Result<CompletedCompaction, ResponseError>
where
    S: Service<ResponsesAttempt, Response = ResponsesServiceResponse>,
    S::Error: Into<ResponseError>,
{
    let span = response_call_span(
        turn.session,
        turn.logical_turn,
        "response.compact",
        turn.session.history_len(),
    );
    let started_at = Instant::now();
    let mut cancellation = ResponseCallCancellation::new(span.clone(), started_at);
    let result = run_compact_inner(turn).instrument(span.clone()).await;
    cancellation.complete();
    finish_compaction_span(&span, started_at, &result);
    result
}

struct ResponseCallCancellation {
    span: tracing::Span,
    started_at: Instant,
    completed: bool,
}

impl ResponseCallCancellation {
    const fn new(span: tracing::Span, started_at: Instant) -> Self {
        Self {
            span,
            started_at,
            completed: false,
        }
    }

    const fn complete(&mut self) {
        self.completed = true;
    }
}

impl Drop for ResponseCallCancellation {
    fn drop(&mut self) {
        if self.completed {
            return;
        }
        self.span.record("error.class", "cancelled");
        self.span.record("status", "cancelled");
        self.span.record("otel.status_code", "ERROR");
        self.span.record(
            "duration_ns",
            u64::try_from(self.started_at.elapsed().as_nanos()).unwrap_or(u64::MAX),
        );
        tracing::warn!(
            target: "nanocode_codex",
            parent: &self.span,
            "Responses call cancelled before a provider terminal event"
        );
    }
}

async fn run_compact_inner<S>(
    turn: &mut ResponseTurn<'_, S>,
) -> Result<CompletedCompaction, ResponseError>
where
    S: Service<ResponsesAttempt, Response = ResponsesServiceResponse>,
    S::Error: Into<ResponseError>,
{
    let session = &mut *turn.session;
    let call_index = session.next_call_index;
    session.next_call_index = session.next_call_index.saturating_add(1);
    let (sink, events) = EventSink::channel(session.profile.session_id().to_owned());
    drop(events);
    let factory = ResponsesAttemptFactory::new(
        session.profile.clone(),
        sink,
        Arc::clone(&session.transport_stats),
    )
    .for_logical_turn(turn.logical_turn);
    let mut request_history = session.state.compaction_request();
    request_history.fit_context_window(session.profile.prefix(), session.context_window_tokens);
    let request = factory.compaction(
        call_index,
        &request_history,
        compaction::trigger(),
        session.model,
        session.thinking,
        session.service_tier,
    );
    let success = execute(session, request).await?;
    let server_reasoning_included = success.server_reasoning_included();
    let ResponsesOutput::Compaction(response) = success.into_output() else {
        return Err(ResponseError::protocol(
            "response.compact returned a non-compaction output",
        ));
    };

    let mut candidate = session.state.clone();
    candidate.observe_server_reasoning(server_reasoning_included);
    let mid_turn = turn.completed_generation;
    let canonical_context = if mid_turn {
        session.canonical_context.clone()
    } else {
        Vec::new()
    };
    candidate.install_compaction(response.item, canonical_context, session.profile.prefix());
    session.state = candidate;
    session.canonical_context_reinjection_pending = !mid_turn;

    let (estimated_cost, cost_status) =
        estimate_cost(response.usage.as_ref(), session.model, session.service_tier);
    Ok(CompletedCompaction {
        usage: response.usage,
        estimated_cost,
        cost_status,
    })
}

async fn execute<S>(
    session: &mut Session<S>,
    request: ResponsesAttempt,
) -> Result<ResponsesServiceResponse, ResponseError>
where
    S: Service<ResponsesAttempt, Response = ResponsesServiceResponse>,
    S::Error: Into<ResponseError>,
{
    let error: ResponseError = match session.client.execute(request).await {
        Ok(success) => return Ok(success),
        Err(error) => error.into(),
    };
    if let Some(source) = error.responses_error() {
        session
            .state
            .repair_rejected_request(RejectedRequestRepair::for_error(source));
    }
    Err(error)
}

fn response_call_span<S>(
    session: &Session<S>,
    logical_turn: u64,
    method: &'static str,
    input_item_count: usize,
) -> tracing::Span {
    info_span!(
        target: "nanocode_codex",
        "responses.call",
        otel.kind = "client",
        otel.status_code = tracing::field::Empty,
        session.id = %session.id,
        response.method = method,
        turn.index = logical_turn,
        model.call_index = session.next_call_index,
        model.input.item_count = input_item_count,
        model.output.item_count = tracing::field::Empty,
        response.end_turn = tracing::field::Empty,
        usage.input_tokens = tracing::field::Empty,
        usage.cached_input_tokens = tracing::field::Empty,
        usage.cache_write_input_tokens = tracing::field::Empty,
        usage.output_tokens = tracing::field::Empty,
        usage.reasoning_output_tokens = tracing::field::Empty,
        usage.total_tokens = tracing::field::Empty,
        cost.usd = tracing::field::Empty,
        cost.status = tracing::field::Empty,
        cost.service_tier = tracing::field::Empty,
        error.class = tracing::field::Empty,
        status = tracing::field::Empty,
        duration_ns = tracing::field::Empty,
    )
}

fn finish_create_span(
    span: &tracing::Span,
    started_at: Instant,
    result: &Result<CompletedResponse, ResponseError>,
) {
    match result {
        Ok(response) => {
            span.record("model.output.item_count", response.output.len());
            if let Some(end_turn) = response.end_turn {
                span.record("response.end_turn", end_turn);
            }
            record_response_usage_and_cost(
                span,
                response.usage.as_ref(),
                response.cost_status,
                response.estimated_cost.as_ref(),
            );
            finish_response_span(span, started_at, None);
        }
        Err(error) => finish_response_span(span, started_at, Some(error)),
    }
}

fn finish_compaction_span(
    span: &tracing::Span,
    started_at: Instant,
    result: &Result<CompletedCompaction, ResponseError>,
) {
    match result {
        Ok(response) => {
            record_response_usage_and_cost(
                span,
                response.usage.as_ref(),
                response.cost_status,
                response.estimated_cost.as_ref(),
            );
            finish_response_span(span, started_at, None);
        }
        Err(error) => finish_response_span(span, started_at, Some(error)),
    }
}

fn record_response_usage_and_cost(
    span: &tracing::Span,
    usage: Option<&Usage>,
    cost_status: crate::client::CostStatus,
    estimated_cost: Option<&crate::client::EstimatedUsdCost>,
) {
    if let Some(usage) = usage {
        span.record("usage.input_tokens", usage.input_tokens);
        span.record(
            "usage.cached_input_tokens",
            usage
                .input_tokens_details
                .as_ref()
                .map_or(0, |details| details.cached_tokens),
        );
        span.record(
            "usage.cache_write_input_tokens",
            usage
                .input_tokens_details
                .as_ref()
                .map_or(0, |details| details.cache_write_tokens),
        );
        span.record("usage.output_tokens", usage.output_tokens);
        span.record(
            "usage.reasoning_output_tokens",
            usage
                .output_tokens_details
                .as_ref()
                .map_or(0, |details| details.reasoning_tokens),
        );
        span.record("usage.total_tokens", usage.total_tokens);
    }
    span.record("cost.status", cost_status.as_str());
    if let Some(estimated_cost) = estimated_cost {
        span.record("cost.usd", tracing::field::display(estimated_cost.amount()));
        span.record("cost.service_tier", estimated_cost.service_tier().as_str());
    }
}

fn finish_response_span(span: &tracing::Span, started_at: Instant, error: Option<&ResponseError>) {
    span.record(
        "duration_ns",
        u64::try_from(started_at.elapsed().as_nanos()).unwrap_or(u64::MAX),
    );
    if let Some(error) = error {
        let error_class = match error.kind() {
            ResponseErrorKind::ContextWindowExceeded => "context_window_exceeded",
            ResponseErrorKind::Service => "service",
            ResponseErrorKind::Protocol => "protocol",
        };
        span.record("error.class", error_class);
        span.record("status", "failed");
        span.record("otel.status_code", "ERROR");
        tracing::error!(
            target: "nanocode_codex",
            parent: span,
            error = %error,
            error.class = error_class,
            "Responses call failed"
        );
    } else {
        span.record("status", "completed");
        span.record("otel.status_code", "OK");
    }
}

pub(super) fn estimate_cost(
    usage: Option<&Usage>,
    model: crate::client::Model,
    service_tier: ServiceTier,
) -> (
    Option<crate::client::EstimatedUsdCost>,
    crate::client::CostStatus,
) {
    match usage {
        Some(usage) => (
            Some(crate::client::pricing::estimate_for_model(
                usage,
                model,
                service_tier,
            )),
            if usage.provider_cost().is_some() {
                crate::client::CostStatus::ReportedByProvider
            } else {
                crate::client::CostStatus::EstimatedFromUsage
            },
        ),
        None => (None, crate::client::CostStatus::UsageNotReported),
    }
}

fn output_text(items: &[ResponseItem]) -> String {
    items
        .iter()
        .filter_map(|item| {
            let ResponseItem::Message { content, .. } = item else {
                return None;
            };
            Some(content.iter().filter_map(|content| {
                let ContentItem::OutputText { text, .. } = content else {
                    return None;
                };
                Some(text.as_ref())
            }))
        })
        .flatten()
        .collect()
}
