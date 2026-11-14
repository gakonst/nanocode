use std::{
    hash::{DefaultHasher, Hash, Hasher},
    num::NonZeroU32,
    sync::{Arc, atomic::Ordering},
    time::Duration,
};

use crate::client::AgentEventKind;
use crate::client::call::{Policy, Retry};
use nanocode_platform::Instant;

use crate::client::{
    ModelConfig, ResponsesError, ResponsesTransport,
    attempt::{ResponsesAttempt, ResponsesAttemptKind, ResponsesServiceResponse},
    service::ResponsesService,
    service_error::{FailurePhase, ResponsesServiceError},
    telemetry::{AttemptRetrying, duration_ns, elapsed_ns},
};

use super::delay::{RetryDelay, RetryFuture};

#[derive(Clone, Debug)]
pub struct ResponsesRetryPolicy {
    max_attempts: NonZeroU32,
    delay: RetryDelay,
    standard_transport: Option<ResponsesTransport>,
}

impl ResponsesRetryPolicy {
    pub const DEFAULT_MAX_ATTEMPTS: NonZeroU32 = NonZeroU32::new(5).unwrap();

    #[must_use]
    pub const fn new(max_attempts: NonZeroU32) -> Self {
        Self {
            max_attempts,
            delay: RetryDelay::unconfigured(),
            standard_transport: None,
        }
    }

    #[allow(clippy::missing_const_for_fn)]
    pub(crate) fn for_config(max_attempts: NonZeroU32, config: &ModelConfig) -> Self {
        Self {
            max_attempts,
            delay: RetryDelay::from_config(config),
            standard_transport: None,
        }
    }

    pub(crate) const fn with_standard_transport_fallback(
        mut self,
        transport: ResponsesTransport,
    ) -> Self {
        self.standard_transport = Some(transport);
        self
    }
}

impl Default for ResponsesRetryPolicy {
    fn default() -> Self {
        Self::new(Self::DEFAULT_MAX_ATTEMPTS)
    }
}

impl Policy<ResponsesAttempt, ResponsesServiceResponse, ResponsesServiceError>
    for ResponsesRetryPolicy
{
    type Future = RetryFuture;

    fn retry(
        &mut self,
        request: &mut ResponsesAttempt,
        result: &mut Result<ResponsesServiceResponse, ResponsesServiceError>,
    ) -> Option<Self::Future> {
        if let Some(future) = self.retry_attempt(request, result) {
            return Some(future);
        }
        let failure = result.as_ref().err()?;
        if !matches!(request.kind, ResponsesAttemptKind::Generation)
            || request.reconnects >= STREAM_MAX_RETRIES
            || !failure.responses_error().is_some_and(reconnectable)
        {
            return None;
        }
        let delay = reconnect_delay(request.reconnects + 1);
        let message = failure.source.to_string();
        if let Err(error) = request.observer.emit(
            AgentEventKind::ModelAttemptRetrying,
            AttemptRetrying {
                phase: request.kind,
                model_call_index: request.call_index,
                attempt: request.attempt,
                next_attempt: 1,
                max_attempts: request.max_attempts,
                failure_phase: failure.phase,
                error_class: "reconnect",
                delay_ns: duration_ns(delay),
                server_requested_delay: false,
                opens_new_socket: true,
                replay_mode: "full_history",
                connection_generation: failure.connection_generation,
                error: &message,
            },
        ) {
            *result = Err(ResponsesServiceError::event(
                error,
                FailurePhase::Output,
                failure.connection_generation,
            ));
            return None;
        }
        request.prepare_reconnect();
        nanocode_trace::warn!(
            target: "nanocode_codex",
            phase = request.kind.phase(),
            model.call_index = request.call_index,
            reconnect = request.reconnects,
            delay_ms = u64::try_from(delay.as_millis()).unwrap_or(u64::MAX),
            "reconnecting Responses stream"
        );
        request
            .observer
            .stats
            .response_retries
            .fetch_add(1, Ordering::Relaxed);
        Some(wait_for_retry(
            self.delay.clone_for_retry(),
            request.profile.thread_id().to_owned(),
            Arc::clone(&request.observer.stats),
            None,
            delay,
        ))
    }

    fn clone_request(&mut self, request: &ResponsesAttempt) -> Option<ResponsesAttempt> {
        let mut request = request.clone();
        request.limit_attempts(self.max_attempts);
        Some(request)
    }
}

impl ResponsesRetryPolicy {
    fn retry_attempt(
        &mut self,
        request: &mut ResponsesAttempt,
        result: &mut Result<ResponsesServiceResponse, ResponsesServiceError>,
    ) -> Option<RetryFuture> {
        request.limit_attempts(self.max_attempts);
        let failure = result.as_ref().err()?;
        let checkpoint_missing =
            failure.is_checkpoint_missing() && request.previous_response_id().is_some();
        let advice = failure.retry_advice;
        if failure
            .responses_error()
            .is_some_and(ResponsesError::is_terminal_provider_error)
        {
            return None;
        }
        let server_deadline = failure
            .responses_error()
            .and_then(ResponsesError::retry_after);

        let upgrade_required = matches!(
            failure.responses_error(),
            Some(ResponsesError::HandshakeRejected { status: 426, .. })
        );
        let exhausted_websocket_budget = !matches!(
            request.kind,
            ResponsesAttemptKind::Warmup | ResponsesAttemptKind::Connect
        ) && advice.is_some()
            && request.attempt >= request.max_attempts;
        let fallback_reason = if upgrade_required {
            Some("upgrade_required")
        } else if exhausted_websocket_budget {
            Some("retry_exhausted")
        } else {
            None
        };
        if let Some(fallback_reason) = fallback_reason
            && matches!(self.standard_transport, Some(ResponsesTransport::WebSocket))
            && request.activate_https_fallback()
        {
            let failed_attempt = request.attempt;
            let message = failure.source.to_string();
            nanocode_trace::warn!(
                target: "nanocode_codex",
                previous_transport = "responses_websocket_v2",
                next_transport = "responses_https_sse",
                reason = fallback_reason,
                phase = request.kind.phase(),
                model.call_index = request.call_index,
                attempt = failed_attempt,
                "falling back from WebSocket to HTTPS Responses transport"
            );
            if matches!(
                request.kind,
                ResponsesAttemptKind::Warmup | ResponsesAttemptKind::Connect
            ) {
                return None;
            }
            if let Err(error) = request.observer.emit(
                AgentEventKind::ModelAttemptRetrying,
                AttemptRetrying {
                    phase: request.kind,
                    model_call_index: request.call_index,
                    attempt: failed_attempt,
                    next_attempt: 1,
                    max_attempts: request.max_attempts,
                    failure_phase: failure.phase,
                    error_class: "websocket_fallback",
                    delay_ns: duration_ns(
                        server_deadline
                            .map_or(Duration::ZERO, crate::client::RetryAfter::remaining_delay),
                    ),
                    server_requested_delay: server_deadline.is_some(),
                    opens_new_socket: false,
                    replay_mode: "full_history",
                    connection_generation: failure.connection_generation,
                    error: &message,
                },
            ) {
                *result = Err(ResponsesServiceError::event(
                    error,
                    FailurePhase::Output,
                    failure.connection_generation,
                ));
                return None;
            }
            request.prepare_transport_fallback();
            request
                .observer
                .stats
                .response_retries
                .fetch_add(1, Ordering::Relaxed);
            return Some(wait_for_retry(
                self.delay.clone_for_retry(),
                request.profile.thread_id().to_owned(),
                Arc::clone(&request.observer.stats),
                server_deadline,
                Duration::ZERO,
            ));
        }
        if matches!(request.kind, ResponsesAttemptKind::Connect) {
            return None;
        }
        if !checkpoint_missing && advice.is_none() {
            return None;
        }
        let patient = !checkpoint_missing
            && failure
                .responses_error()
                .is_some_and(ResponsesError::is_patient);
        let server_delay = advice
            .and_then(|advice| advice.server_delay)
            .map(crate::client::RetryAfter::remaining_delay);
        let backoff = || {
            retry_delay(
                request.attempt,
                request.call_index,
                request.profile.thread_id(),
            )
        };
        let delay = if checkpoint_missing {
            Duration::ZERO
        } else if patient {
            server_delay.map_or_else(backoff, |delay| delay.max(backoff()))
        } else {
            server_delay.unwrap_or_else(backoff)
        };
        if !request.may_retry(patient, delay) {
            return None;
        }
        let error_class = if checkpoint_missing {
            "checkpoint_missing"
        } else {
            advice.map_or("unknown", |advice| advice.class)
        };
        let message = failure.source.to_string();
        if let Err(error) = request.observer.emit(
            AgentEventKind::ModelAttemptRetrying,
            AttemptRetrying {
                phase: request.kind,
                model_call_index: request.call_index,
                attempt: request.attempt,
                next_attempt: request.attempt + 1,
                max_attempts: request.max_attempts,
                failure_phase: failure.phase,
                error_class,
                delay_ns: duration_ns(delay),
                server_requested_delay: advice.is_some_and(|advice| advice.server_delay.is_some()),
                opens_new_socket: !checkpoint_missing,
                replay_mode: "full_history",
                connection_generation: failure.connection_generation,
                error: &message,
            },
        ) {
            *result = Err(ResponsesServiceError::event(
                error,
                FailurePhase::Output,
                failure.connection_generation,
            ));
            return None;
        }
        request
            .observer
            .stats
            .response_retries
            .fetch_add(1, Ordering::Relaxed);
        nanocode_trace::warn!(
            target: "nanocode_codex",
            phase = request.kind.phase(),
            model.call_index = request.call_index,
            attempt = request.attempt,
            next_attempt = request.attempt + 1,
            error.class = error_class,
            delay_ms = u64::try_from(delay.as_millis()).unwrap_or(u64::MAX),
            server_requested_delay = advice.is_some_and(|advice| advice.server_delay.is_some()),
            "retrying Responses attempt"
        );
        if !request.prepare_retry(patient, delay) {
            return None;
        }
        let stats = Arc::clone(&request.observer.stats);
        let delay_runtime = self.delay.clone_for_retry();
        let thread_id = request.profile.thread_id().to_owned();
        Some(wait_for_retry(
            delay_runtime,
            thread_id,
            stats,
            if checkpoint_missing || patient {
                None
            } else {
                server_deadline
            },
            delay,
        ))
    }
}

const STREAM_MAX_RETRIES: u32 = 5;

fn reconnectable(error: &ResponsesError) -> bool {
    if error.is_terminal_provider_error() {
        return false;
    }
    match error {
        ResponsesError::HttpRejected { status, .. }
        | ResponsesError::HandshakeRejected { status, .. } => !matches!(status, 400 | 429),
        ResponsesError::Api { .. } => true,
        _ => error.retry_advice().is_some(),
    }
}

fn reconnect_delay(reconnect: u32) -> Duration {
    Duration::from_millis(200 * 2_u64.pow(reconnect.saturating_sub(1)))
}

fn wait_for_retry(
    runtime: RetryDelay,
    thread_id: String,
    stats: Arc<crate::client::TransportStats>,
    deadline: Option<crate::client::RetryAfter>,
    local_delay: Duration,
) -> RetryFuture {
    Box::pin(async move {
        let started_at = Instant::now();
        runtime
            .wait(
                thread_id,
                deadline.map_or(local_delay, crate::client::RetryAfter::remaining_delay),
            )
            .await;
        stats
            .retry_backoff_duration_ns
            .fetch_add(elapsed_ns(started_at), Ordering::Relaxed);
    })
}

pub type DefaultResponsesService = Retry<ResponsesRetryPolicy, ResponsesService>;

fn retry_delay(attempt: u32, call_index: Option<u32>, thread_id: &str) -> Duration {
    let base_ms = 1_000;
    let exponent = attempt.saturating_sub(1).min(4);
    let raw_ms = base_ms * 2_u64.pow(exponent);
    let mut hasher = DefaultHasher::new();
    (thread_id, call_index, attempt).hash(&mut hasher);
    let seed = hasher.finish();
    let jitter_percent = 90 + seed % 21;
    Duration::from_millis(raw_ms * jitter_percent / 100)
}
