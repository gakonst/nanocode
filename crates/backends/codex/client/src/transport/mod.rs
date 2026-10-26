mod api_error;
pub(crate) mod chat;
#[cfg(not(target_family = "wasm"))]
pub(crate) mod connector;
#[cfg(not(target_family = "wasm"))]
pub use connector::connect_async;
#[cfg(not(target_family = "wasm"))]
mod crypto;
#[cfg(not(target_family = "wasm"))]
pub(crate) mod deflate;
pub(crate) mod error;
pub(crate) mod freeform;
pub(crate) mod gateway;

pub mod host;
pub(crate) mod idle;
mod retry_after;
#[cfg(target_family = "wasm")]
pub(crate) use host::socket;
pub(crate) mod http;
pub(crate) mod platform;
#[cfg(not(target_family = "wasm"))]
pub(crate) mod socket;
pub(crate) mod telemetry;
mod wire;

use std::{fmt, str::FromStr};

pub use crate::client::tower::attempt::{
    TransportStats, TransportStatsDelta, TransportStatsSnapshot,
};
pub use chat::{ChatGateway, DEFAULT_STALL_TIMEOUT, OPENROUTER_API_BASE_URL};
#[cfg(not(target_family = "wasm"))]
pub use crypto::install_default_rustls_crypto_provider;
pub use error::{ResponsesError, RetryAdvice};
pub use gateway::{GatewayHeader, ResponsesGateway};
pub use nanocode_http::layer;
pub use retry_after::RetryAfter;
pub(crate) use retry_after::RetryReceipt;
pub use telemetry::TRANSPORT;
pub use wire::EncodedRequest;

pub(crate) enum SendOutcome {
    Sent,
    Disconnected,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ResponsesTransport {
    #[default]
    WebSocket,

    Https,
}

impl ResponsesTransport {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::WebSocket => "responses_websocket_v2",
            Self::Https => "responses_https_sse",
        }
    }
}

impl fmt::Display for ResponsesTransport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WebSocket => formatter.write_str("websocket"),
            Self::Https => formatter.write_str("https"),
        }
    }
}

impl FromStr for ResponsesTransport {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "websocket" | "ws" => Ok(Self::WebSocket),
            "https" | "http" => Ok(Self::Https),
            _ => Err(format!(
                "invalid Responses transport {value:?}; expected websocket or https"
            )),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ResponsesHistory {
    #[default]
    Incremental,

    FullReplay,
}

impl fmt::Display for ResponsesHistory {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Incremental => formatter.write_str("incremental"),
            Self::FullReplay => formatter.write_str("full-replay"),
        }
    }
}

impl FromStr for ResponsesHistory {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "incremental" => Ok(Self::Incremental),
            "full-replay" | "replay" => Ok(Self::FullReplay),
            _ => Err(format!(
                "invalid Responses history policy {value:?}; expected incremental or full-replay"
            )),
        }
    }
}
