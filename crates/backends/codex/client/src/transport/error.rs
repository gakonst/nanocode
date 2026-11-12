use std::{borrow::Cow, sync::Arc};

use nanocode_http::layer::retry;

use super::RetryAfter;

use super::api_error::{
    api_error_has_code, api_error_is_checkpoint_missing, api_error_is_connection_expired,
    api_error_is_terminal, invalid_tool_schema_path, retryable_api_error,
    retryable_api_error_received,
};

#[derive(Debug)]
pub enum ResponsesError {
    Authorization {
        detail: String,
    },

    HostUnavailable,

    InvalidUrl {
        detail: String,
    },

    InvalidAuthorization {
        detail: String,
    },

    InvalidSessionId {
        detail: String,
    },

    HandshakeTimeout {
        seconds: u64,
    },

    Handshake {
        detail: String,
        reconnectable: bool,
    },

    HandshakeRejected {
        status: u16,

        body: String,

        retry_after: Option<RetryAfter>,

        should_retry: Option<bool>,
    },

    Send {
        detail: String,
        reconnectable: bool,
    },

    SendTimeout {
        seconds: u64,
    },

    UnexpectedEnd,

    Receive {
        detail: String,
        reconnectable: bool,
    },

    InvalidJson(serde_json::Error),

    UnexpectedBinary,

    EncodeRequest(serde_json::Error),

    InvalidPayload {
        source: serde_json::Error,

        event: String,
    },

    Closed {
        detail: String,
    },

    Api {
        event: String,

        retry_after: Option<RetryAfter>,
    },

    ContextWindowExceeded {
        event: String,
    },

    InvalidImageRequest {
        event: String,
    },

    InvalidToolSchema {
        source: Box<Self>,

        definition: Box<serde_json::Value>,
    },

    HttpRequest {
        detail: String,

        retryable: bool,

        timeout: bool,
    },

    HttpRejected {
        status: u16,

        body: String,

        retry_after: Option<RetryAfter>,

        should_retry: Option<bool>,
    },

    InvalidSseUtf8 {
        detail: String,
    },

    GatewayUnsupported {
        gateway: &'static str,

        operation: &'static str,
    },
}

impl std::fmt::Display for ResponsesError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Authorization { detail } => write!(
                formatter,
                "failed to resolve OpenAI authorization: {detail}"
            ),
            Self::HostUnavailable => {
                formatter.write_str("the Responses host transport is not configured")
            }
            Self::InvalidUrl { detail } => {
                write!(formatter, "invalid Responses WebSocket URL: {detail}")
            }
            Self::InvalidAuthorization { detail } => {
                write!(formatter, "invalid OpenAI authorization header: {detail}")
            }
            Self::InvalidSessionId { detail } => write!(
                formatter,
                "invalid Responses session identifier header: {detail}"
            ),
            Self::HandshakeTimeout { seconds } => write!(
                formatter,
                "Responses WebSocket handshake exceeded {seconds} seconds"
            ),
            Self::Handshake { detail, .. } => {
                write!(formatter, "Responses WebSocket handshake failed: {detail}")
            }
            Self::HandshakeRejected { status, body, .. } => write!(
                formatter,
                "Responses WebSocket handshake was rejected with HTTP {status}: {body}"
            ),
            Self::Send { detail, .. } => write!(
                formatter,
                "failed to send a Responses WebSocket frame: {detail}"
            ),
            Self::SendTimeout { seconds } => write!(
                formatter,
                "sending a Responses WebSocket frame exceeded {seconds} seconds"
            ),
            Self::UnexpectedEnd => {
                formatter.write_str("Responses WebSocket closed without a close frame")
            }
            Self::Receive { detail, .. } => write!(
                formatter,
                "failed to receive a Responses WebSocket frame: {detail}"
            ),
            Self::InvalidJson(..) => {
                formatter.write_str("Responses WebSocket event was not valid JSON")
            }
            Self::UnexpectedBinary => formatter
                .write_str("Responses WebSocket returned a binary data frame; expected JSON text"),
            Self::EncodeRequest(..) => {
                formatter.write_str("failed to encode a Responses WebSocket request")
            }
            Self::InvalidPayload { event, .. } => write!(
                formatter,
                "Responses API event did not match its declared type: {event}"
            ),
            Self::Closed { detail } => write!(formatter, "Responses WebSocket closed {detail}"),
            Self::Api { event, .. } => {
                write!(formatter, "Responses API returned an error event: {event}")
            }
            Self::ContextWindowExceeded { .. } => {
                formatter.write_str("Responses input exceeded the model context window")
            }
            Self::InvalidImageRequest { event } => write!(
                formatter,
                "Responses API rejected invalid image data: {event}"
            ),
            Self::InvalidToolSchema { source, .. } => write!(formatter, "{source}"),
            Self::HttpRequest { detail, .. } => {
                write!(formatter, "Responses HTTPS request failed: {detail}")
            }
            Self::HttpRejected { status, body, .. } => write!(
                formatter,
                "Responses HTTPS request was rejected with HTTP {status}: {body}"
            ),
            Self::InvalidSseUtf8 { detail } => write!(
                formatter,
                "Responses HTTPS stream contained invalid UTF-8: {detail}"
            ),
            Self::GatewayUnsupported { gateway, operation } => write!(
                formatter,
                "the {gateway} Responses gateway does not document {operation}"
            ),
        }
    }
}

impl std::error::Error for ResponsesError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidJson(value) => Some(value),
            Self::EncodeRequest(value) => Some(value),
            Self::InvalidPayload { source, .. } => Some(source),
            Self::InvalidToolSchema { source, .. } => Some(source),
            _ => None,
        }
    }
}

const INVALID_IMAGE_ERROR: &str = "The image data you provided does not represent a valid image";

impl ResponsesError {
    #[must_use]
    pub fn retry_advice(&self) -> Option<RetryAdvice> {
        let (class, server_delay) = match self {
            Self::Handshake {
                reconnectable: true,
                ..
            } => ("handshake_transport", None),
            Self::HandshakeTimeout { .. } => ("handshake_timeout", None),

            Self::HandshakeRejected {
                status,
                body,
                retry_after,
                should_retry,
            } if should_retry.unwrap_or_else(|| {
                (*status == 403 || retry::retryable_status(*status)) && !api_error_is_terminal(body)
            }) =>
            {
                let class = match status {
                    403 => "handshake_forbidden",
                    429 => "handshake_rate_limit",
                    500..=599 => "handshake_server",
                    _ => "handshake_rejected",
                };
                (class, *retry_after)
            }
            Self::SendTimeout { .. } => ("send_timeout", None),
            Self::Send {
                reconnectable: true,
                ..
            } => ("send_transport", None),
            Self::UnexpectedEnd | Self::Closed { .. } => ("premature_close", None),
            Self::Receive {
                reconnectable: true,
                ..
            } => ("receive_transport", None),
            Self::Api { event, retry_after } => (retryable_api_error(event)?.0, *retry_after),

            Self::HttpRequest { detail, .. } if invalid_provider_stream(detail) => return None,
            Self::HttpRequest { timeout: true, .. } => ("https_timeout", None),
            Self::HttpRequest {
                retryable: true, ..
            } => ("https_transport", None),
            Self::HttpRejected {
                status,
                body,
                retry_after,
                should_retry,
            } if should_retry.unwrap_or_else(|| {
                retry::retryable_status(*status) && !api_error_is_terminal(body)
            }) =>
            {
                let class = match status {
                    429 => "https_rate_limit",
                    500..=599 => "https_server",
                    _ => "https_rejected",
                };
                (class, *retry_after)
            }
            _ => return None,
        };
        Some(RetryAdvice {
            class,
            server_delay,
        })
    }

    #[must_use]
    pub const fn retry_after(&self) -> Option<RetryAfter> {
        match self {
            Self::Api { retry_after, .. }
            | Self::HttpRejected { retry_after, .. }
            | Self::HandshakeRejected { retry_after, .. } => *retry_after,
            _ => None,
        }
    }

    pub(crate) fn is_terminal_provider_error(&self) -> bool {
        match self {
            Self::Api { event, .. } => api_error_is_terminal(event),
            Self::HttpRejected {
                body, should_retry, ..
            }
            | Self::HandshakeRejected {
                body, should_retry, ..
            } => *should_retry != Some(true) && api_error_is_terminal(body),
            _ => false,
        }
    }

    pub(crate) fn is_patient(&self) -> bool {
        match self {
            Self::HttpRejected { status, .. } | Self::HandshakeRejected { status, .. } => {
                retry::patient_status(*status)
            }
            Self::Api { event, .. } => retryable_api_error(event)
                .is_some_and(|(class, _)| matches!(class, "api_rate_limit" | "api_overload")),
            _ => false,
        }
    }

    #[must_use]
    pub fn class(&self) -> &'static str {
        match self {
            Self::Authorization { .. } => "authorization",
            Self::HostUnavailable => "host_unavailable",
            Self::InvalidUrl { .. } => "invalid_url",
            Self::InvalidAuthorization { .. } => "invalid_authorization",
            Self::InvalidSessionId { .. } => "invalid_session_id",
            Self::HandshakeTimeout { .. } => "handshake_timeout",
            Self::Handshake { .. } => "handshake",
            Self::HandshakeRejected { .. } => "handshake_rejected",
            Self::Send { .. } => "send",
            Self::SendTimeout { .. } => "send_timeout",
            Self::UnexpectedEnd => "premature_close",
            Self::Receive { .. } => "receive",
            Self::InvalidJson(_) => "invalid_json",
            Self::UnexpectedBinary => "unexpected_binary",
            Self::EncodeRequest(_) => "encode_request",
            Self::InvalidPayload { .. } => "invalid_payload",
            Self::Closed { .. } => "closed",
            Self::Api { event, .. } if api_error_is_checkpoint_missing(event) => {
                "checkpoint_missing"
            }
            Self::Api { event, .. }
                if api_error_has_code(event, "misalignment_policy_violation") =>
            {
                "misalignment_policy_violation"
            }
            Self::Api { .. } => "api",
            Self::ContextWindowExceeded { .. } => "context_window_exceeded",
            Self::InvalidImageRequest { .. } => "invalid_image_request",
            Self::InvalidToolSchema { .. } => "invalid_tool_schema",
            Self::HttpRequest { detail, .. } if invalid_provider_stream(detail) => {
                "invalid_provider_stream"
            }
            Self::HttpRequest { timeout: true, .. } => "https_timeout",
            Self::HttpRequest { .. } => "https_transport",
            Self::HttpRejected { body, .. } if api_error_is_checkpoint_missing(body) => {
                "checkpoint_missing"
            }
            Self::HttpRejected { body, .. }
                if api_error_has_code(body, "misalignment_policy_violation") =>
            {
                "misalignment_policy_violation"
            }
            Self::HttpRejected { status: 429, .. } => "https_rate_limit",
            Self::HttpRejected { status, .. } if (500..=599).contains(status) => "https_server",
            Self::HttpRejected { .. } => "https_rejected",
            Self::InvalidSseUtf8 { .. } => "invalid_sse_utf8",
            Self::GatewayUnsupported { .. } => "gateway_unsupported",
        }
    }

    #[must_use]
    pub fn is_checkpoint_missing(&self) -> bool {
        match self {
            Self::Api { event, .. } => api_error_is_checkpoint_missing(event),
            Self::HttpRejected { body, .. } => api_error_is_checkpoint_missing(body),
            _ => false,
        }
    }

    pub(crate) fn is_connection_expired(&self) -> bool {
        matches!(self, Self::Api { event, .. } if api_error_is_connection_expired(event))
    }

    #[must_use]
    pub const fn is_context_window_exceeded(&self) -> bool {
        matches!(self, Self::ContextWindowExceeded { .. })
    }

    #[must_use]
    pub fn invalid_tool_schema(&self) -> Option<&serde_json::Value> {
        match self {
            Self::InvalidToolSchema { definition, .. } => Some(definition),
            _ => None,
        }
    }

    pub(crate) fn with_request_input<'a>(
        self,
        mut input: impl Iterator<Item = &'a crate::client::ResponseItem>,
    ) -> Self {
        let event = match &self {
            Self::Api { event, .. } => event,
            Self::HttpRejected {
                status: 400, body, ..
            } => body,
            _ => return self,
        };
        let Some(indices) = invalid_tool_schema_path(event) else {
            return self;
        };
        let Some(crate::client::ResponseItem::ToolSearchOutput { tools, .. }) =
            input.nth(indices[0])
        else {
            return self;
        };
        let Some(tool) = tools.get(indices[1]) else {
            return self;
        };
        let mut definition = tool.as_value();
        for index in &indices[2..] {
            if definition["type"] != "namespace" {
                return self;
            }
            let Some(child) = definition
                .get("tools")
                .and_then(serde_json::Value::as_array)
                .and_then(|tools| tools.get(*index))
            else {
                return self;
            };
            definition = child;
        }
        if definition["type"] != "function" || !definition["parameters"].is_object() {
            return self;
        }
        Self::InvalidToolSchema {
            source: Box::new(self),
            definition: Box::new(definition.clone()),
        }
    }

    #[must_use]
    pub fn is_misalignment_policy_violation(&self) -> bool {
        match self {
            Self::Api { event, .. } => api_error_has_code(event, "misalignment_policy_violation"),
            Self::HttpRejected { body, .. } => {
                api_error_has_code(body, "misalignment_policy_violation")
            }
            _ => false,
        }
    }

    pub(crate) fn http_rejected(
        status: u16,
        body: String,
        retry_after: Option<RetryAfter>,
        should_retry: Option<bool>,
    ) -> Self {
        if status == 400 {
            let classified = Self::api_event(body.clone());
            if matches!(
                classified,
                Self::InvalidImageRequest { .. } | Self::ContextWindowExceeded { .. }
            ) {
                return classified;
            }
        }
        Self::HttpRejected {
            status,
            body,
            retry_after,
            should_retry,
        }
    }

    #[must_use]
    pub fn api_event(event: String) -> Self {
        Self::api_event_received(event, super::RetryReceipt::now())
    }

    pub(crate) fn api_event_received(event: String, received: super::RetryReceipt) -> Self {
        if [
            "misalignment_policy_violation",
            "cyber_policy",
            "bio_policy",
        ]
        .iter()
        .any(|code| api_error_has_code(&event, code))
        {
            return Self::Api {
                event,
                retry_after: None,
            };
        }

        let invalid_image_url = serde_json::from_str::<serde_json::Value>(&event)
            .ok()
            .is_some_and(|event| {
                let error = event
                    .get("error")
                    .or_else(|| event.pointer("/response/error"));
                error.is_some_and(|error| {
                    error.get("type").and_then(serde_json::Value::as_str)
                        == Some("invalid_request_error")
                        && error.get("code").and_then(serde_json::Value::as_str)
                            == Some("invalid_value")
                        && error
                            .get("param")
                            .and_then(serde_json::Value::as_str)
                            .is_some_and(|param| {
                                param.starts_with("input[") && param.ends_with(".image_url")
                            })
                })
            });
        if invalid_image_url
            || api_error_has_code(&event, "invalid_image")
            || event.contains(INVALID_IMAGE_ERROR)
        {
            Self::InvalidImageRequest { event }
        } else if api_error_has_code(&event, "context_length_exceeded") {
            Self::ContextWindowExceeded { event }
        } else {
            let retry_after =
                retryable_api_error_received(&event, received).and_then(|(_, advice)| advice);
            Self::Api { event, retry_after }
        }
    }
}

#[derive(Clone, Default)]
pub(crate) struct Credentials(Vec<Arc<str>>);

impl Credentials {
    pub(crate) const fn new() -> Self {
        Self(Vec::new())
    }

    pub(crate) fn insert(&mut self, secret: &Arc<str>) {
        if secret.is_empty() || self.0.iter().any(|known| known == secret) {
            return;
        }
        self.0.push(Arc::clone(secret));
        self.0
            .sort_unstable_by_key(|known| std::cmp::Reverse(known.len()));
    }

    pub(crate) fn extend(&mut self, other: &Self) {
        for secret in &other.0 {
            self.insert(secret);
        }
    }

    pub(crate) fn redact<'a>(&self, text: &'a str) -> Cow<'a, str> {
        let mut text = Cow::Borrowed(text);
        for secret in &self.0 {
            if text.contains(&**secret) {
                text = Cow::Owned(text.replace(&**secret, "[redacted]"));
            }
        }
        text
    }

    fn redact_in_place(&self, text: &mut String) {
        if let Cow::Owned(redacted) = self.redact(text) {
            *text = redacted;
        }
    }
}

impl ResponsesError {
    pub(crate) fn redact(&mut self, credentials: &Credentials) {
        match self {
            Self::Authorization { detail }
            | Self::InvalidUrl { detail }
            | Self::InvalidAuthorization { detail }
            | Self::InvalidSessionId { detail }
            | Self::Handshake { detail, .. }
            | Self::Send { detail, .. }
            | Self::Receive { detail, .. }
            | Self::Closed { detail }
            | Self::HttpRequest { detail, .. }
            | Self::InvalidSseUtf8 { detail } => credentials.redact_in_place(detail),
            Self::HandshakeRejected { body, .. } | Self::HttpRejected { body, .. } => {
                credentials.redact_in_place(body);
            }
            Self::InvalidPayload { event, .. }
            | Self::Api { event, .. }
            | Self::ContextWindowExceeded { event }
            | Self::InvalidImageRequest { event } => credentials.redact_in_place(event),
            Self::InvalidToolSchema { source, .. } => source.redact(credentials),
            Self::HostUnavailable
            | Self::HandshakeTimeout { .. }
            | Self::SendTimeout { .. }
            | Self::UnexpectedEnd
            | Self::InvalidJson(_)
            | Self::UnexpectedBinary
            | Self::EncodeRequest(_)
            | Self::GatewayUnsupported { .. } => {}
        }
    }
}

fn invalid_provider_stream(detail: &str) -> bool {
    detail.lines().next().is_some_and(|line| {
        matches!(
            line.trim(),
            "Responses: invalid provider stream" | "Error: Responses: invalid provider stream"
        )
    })
}

#[derive(Clone, Copy, Debug)]
pub struct RetryAdvice {
    pub class: &'static str,

    pub server_delay: Option<RetryAfter>,
}
