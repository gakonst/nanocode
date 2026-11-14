use std::time::Duration;

use crate::client::EventError;
use serde::Serialize;

use crate::client::{ResponsesError, RetryAdvice};

#[derive(Debug)]
pub struct ResponsesServiceError {
    pub(crate) source: ResponsesServiceErrorSource,
    pub(crate) phase: FailurePhase,
    class: &'static str,
    pub(crate) retry_advice: Option<RetryAdvice>,
    pub(crate) connection_generation: u32,
    connection_renewal: bool,
}

impl ResponsesServiceError {
    const fn new(
        source: ResponsesServiceErrorSource,
        phase: FailurePhase,
        class: &'static str,
        retry_advice: Option<RetryAdvice>,
        connection_generation: u32,
    ) -> Self {
        Self {
            source,
            phase,
            class,
            retry_advice,
            connection_generation,
            connection_renewal: false,
        }
    }

    pub(crate) fn with_connection_renewal(mut self, first_event: bool) -> Self {
        self.connection_renewal = first_event
            && self
                .responses_error()
                .is_some_and(ResponsesError::is_connection_expired);
        self
    }

    pub(crate) const fn can_renew_connection(&self) -> bool {
        self.connection_renewal
    }

    pub(crate) fn responses(
        source: ResponsesError,
        phase: FailurePhase,
        connection_generation: u32,
    ) -> Self {
        let class = source.class();
        let retry_advice = source.retry_advice();
        Self::new(
            ResponsesServiceErrorSource::Responses(source),
            phase,
            class,
            retry_advice,
            connection_generation,
        )
    }

    pub(crate) const fn event(
        source: EventError,
        phase: FailurePhase,
        connection_generation: u32,
    ) -> Self {
        Self::new(
            ResponsesServiceErrorSource::Event(source),
            phase,
            "output",
            None,
            connection_generation,
        )
    }

    pub(crate) const fn invalid_attempt_state(
        detail: &'static str,
        phase: FailurePhase,
        connection_generation: u32,
    ) -> Self {
        Self::new(
            ResponsesServiceErrorSource::InvalidAttemptState { detail },
            phase,
            "protocol",
            None,
            connection_generation,
        )
    }

    pub(crate) const fn invalid_compaction(count: usize) -> Self {
        Self::new(
            ResponsesServiceErrorSource::InvalidCompactionOutput { count },
            FailurePhase::Completion,
            "protocol",
            None,
            0,
        )
    }

    pub(crate) const fn with_connection_generation(mut self, connection_generation: u32) -> Self {
        self.connection_generation = connection_generation;
        self
    }

    pub(crate) fn with_request_input(self, request: &crate::client::ResponsesAttempt) -> Self {
        match self.source {
            ResponsesServiceErrorSource::Responses(source) => {
                let source = source.with_request_input(request.input_items());
                let class = source.class();
                let retry_advice = if matches!(source, ResponsesError::InvalidToolSchema { .. }) {
                    None
                } else {
                    self.retry_advice
                };
                Self::new(
                    ResponsesServiceErrorSource::Responses(source),
                    self.phase,
                    class,
                    retry_advice,
                    self.connection_generation,
                )
            }
            _ => self,
        }
    }

    pub(crate) fn redact(&mut self, credentials: &crate::client::transport::error::Credentials) {
        if let ResponsesServiceErrorSource::Responses(source) = &mut self.source {
            source.redact(credentials);
        }
    }

    #[must_use]
    pub const fn error_class(&self) -> &'static str {
        self.class
    }

    #[must_use]
    pub const fn is_retryable(&self) -> bool {
        self.retry_advice.is_some()
    }

    #[must_use]
    pub fn is_checkpoint_missing(&self) -> bool {
        self.responses_error()
            .is_some_and(ResponsesError::is_checkpoint_missing)
    }

    #[must_use]
    pub fn is_context_window_exceeded(&self) -> bool {
        self.responses_error()
            .is_some_and(ResponsesError::is_context_window_exceeded)
    }

    #[must_use]
    pub fn server_retry_after(&self) -> Option<Duration> {
        self.retry_advice
            .and_then(|advice| advice.server_delay)
            .map(crate::client::RetryAfter::remaining_delay)
    }

    #[must_use]
    pub const fn responses_error(&self) -> Option<&ResponsesError> {
        match &self.source {
            ResponsesServiceErrorSource::Responses(error) => Some(error),
            ResponsesServiceErrorSource::Event(_)
            | ResponsesServiceErrorSource::InvalidAttemptState { .. }
            | ResponsesServiceErrorSource::InvalidCompactionOutput { .. } => None,
        }
    }
}

impl std::fmt::Display for ResponsesServiceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.source.fmt(formatter)
    }
}

impl std::error::Error for ResponsesServiceError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

impl From<ResponsesError> for ResponsesServiceError {
    fn from(error: ResponsesError) -> Self {
        let phase = match error {
            ResponsesError::UnexpectedEnd
            | ResponsesError::Closed { .. }
            | ResponsesError::Receive { .. }
            | ResponsesError::ReceiveTimeout { .. } => FailurePhase::Receive,
            ResponsesError::HttpRequest { .. } | ResponsesError::InvalidSseUtf8 { .. } => {
                FailurePhase::Receive
            }
            ResponsesError::Api { .. }
            | ResponsesError::ContextWindowExceeded { .. }
            | ResponsesError::InvalidToolSchema { .. } => FailurePhase::Api,
            ResponsesError::HttpRejected { .. } => FailurePhase::Api,
            ResponsesError::HostUnavailable
            | ResponsesError::HandshakeTimeout { .. }
            | ResponsesError::Handshake { .. }
            | ResponsesError::HandshakeRejected { .. } => FailurePhase::Connect,
            ResponsesError::Send { .. } | ResponsesError::SendTimeout { .. } => FailurePhase::Send,
            _ => FailurePhase::Protocol,
        };
        Self::responses(error, phase, 0)
    }
}

impl From<EventError> for ResponsesServiceError {
    fn from(error: EventError) -> Self {
        Self::event(error, FailurePhase::Output, 0)
    }
}

#[derive(Debug)]
pub(crate) enum ResponsesServiceErrorSource {
    Responses(ResponsesError),
    Event(EventError),
    InvalidAttemptState { detail: &'static str },
    InvalidCompactionOutput { count: usize },
}

impl std::fmt::Display for ResponsesServiceErrorSource {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Responses(value) => std::fmt::Display::fmt(value, formatter),
            Self::Event(value) => std::fmt::Display::fmt(value, formatter),
            Self::InvalidAttemptState { detail } => {
                write!(formatter, "invalid Responses attempt state: {detail}")
            }
            Self::InvalidCompactionOutput { count } => write!(
                formatter,
                "remote compaction returned {count} compaction items; expected exactly one"
            ),
        }
    }
}

impl std::error::Error for ResponsesServiceErrorSource {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Responses(value) => std::error::Error::source(value),
            Self::Event(value) => std::error::Error::source(value),
            _ => None,
        }
    }
}

impl From<ResponsesError> for ResponsesServiceErrorSource {
    fn from(source: ResponsesError) -> Self {
        Self::Responses(source)
    }
}

impl From<EventError> for ResponsesServiceErrorSource {
    fn from(source: EventError) -> Self {
        Self::Event(source)
    }
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum FailurePhase {
    Connect,
    Encode,
    Send,
    Receive,
    Api,
    Protocol,
    Completion,
    Output,
}
