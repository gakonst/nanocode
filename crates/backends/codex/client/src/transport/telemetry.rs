use std::time::Duration;

use nanocode_platform::Instant;
use serde::{Serialize, Serializer};
use serde_json::{Value, value::RawValue};

use crate::client::{attempt::ResponsesAttemptKind, service_error::FailurePhase};

pub const TRANSPORT: &str = "responses_websocket_v2";

pub(crate) enum ApiEventData<'a> {
    Encoded(&'a str),
    Received(&'a str),
}

impl Serialize for ApiEventData<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let (Self::Encoded(text) | Self::Received(text)) = self;
        if memchr::memchr2(b'\n', b'\r', text.as_bytes()).is_some() {
            let value = serde_json::from_str::<Value>(text).map_err(serde::ser::Error::custom)?;
            value.serialize(serializer)
        } else {
            serde_json::from_str::<&RawValue>(text)
                .map_err(serde::ser::Error::custom)?
                .serialize(serializer)
        }
    }
}

#[derive(Serialize)]
pub(crate) struct ApiEvent<'a> {
    pub(crate) direction: &'static str,
    pub(crate) transport: &'static str,
    pub(crate) phase: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) model_call_index: Option<u32>,
    pub(crate) event: ApiEventData<'a>,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ConnectionPurpose {
    Initial,
    WarmupFallback,
    Reconnect,
}

#[derive(Serialize)]
pub(crate) struct AttemptStarted<'a> {
    pub(crate) phase: ResponsesAttemptKind,
    pub(crate) model_call_index: Option<u32>,
    pub(crate) attempt: u32,
    pub(crate) max_attempts: u32,
    pub(crate) replay_mode: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) previous_response_id: Option<&'a str>,
    pub(crate) connection_generation: u32,
}

#[derive(Serialize)]
pub(crate) struct AttemptFailed<'a> {
    pub(crate) phase: ResponsesAttemptKind,
    pub(crate) model_call_index: Option<u32>,
    pub(crate) attempt: u32,
    pub(crate) max_attempts: u32,
    pub(crate) duration_ns: u64,
    pub(crate) failure_phase: FailurePhase,
    pub(crate) error_class: &'static str,
    pub(crate) retryable: bool,
    pub(crate) connection_generation: u32,
    pub(crate) error: &'a str,
}

#[derive(Serialize)]
pub(crate) struct AttemptRetrying<'a> {
    pub(crate) phase: ResponsesAttemptKind,
    pub(crate) model_call_index: Option<u32>,
    pub(crate) attempt: u32,
    pub(crate) next_attempt: u32,
    pub(crate) max_attempts: u32,
    pub(crate) failure_phase: FailurePhase,
    pub(crate) error_class: &'static str,
    pub(crate) delay_ns: u64,
    pub(crate) server_requested_delay: bool,
    pub(crate) opens_new_socket: bool,
    pub(crate) replay_mode: &'static str,
    pub(crate) connection_generation: u32,
    pub(crate) error: &'a str,
}

#[derive(Serialize)]
pub(crate) struct ConnectionStarted<'a> {
    pub(crate) transport: &'static str,
    pub(crate) websocket_url: &'a str,
    pub(crate) attempt: u32,
    pub(crate) purpose: ConnectionPurpose,
    pub(crate) connection_generation: u32,
}

#[derive(Serialize)]
pub(crate) struct ConnectionCompleted<'a> {
    pub(crate) transport: &'static str,
    pub(crate) attempt: u32,
    pub(crate) purpose: ConnectionPurpose,
    pub(crate) duration_ns: u64,
    pub(crate) http_status: u16,
    pub(crate) request_id: Option<&'a str>,
    pub(crate) server_model: Option<&'a str>,
    pub(crate) server_reasoning_included: bool,
    pub(crate) connection_generation: u32,
}

#[derive(Serialize)]
pub(crate) struct ConnectionFailed<'a> {
    pub(crate) transport: &'static str,
    pub(crate) attempt: u32,
    pub(crate) purpose: ConnectionPurpose,
    pub(crate) duration_ns: u64,
    pub(crate) error: &'a str,
    pub(crate) connection_generation: u32,
}

pub(crate) fn elapsed_ns(started_at: Instant) -> u64 {
    duration_ns(started_at.elapsed())
}

pub(crate) fn duration_ns(duration: Duration) -> u64 {
    u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX)
}

pub(crate) fn display_endpoint(endpoint: &str) -> &str {
    endpoint.split_once('?').map_or(endpoint, |(base, _)| base)
}
