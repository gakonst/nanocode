use std::{future::Future, pin::Pin, time::Duration};

use futures_util::StreamExt;
use http::{HeaderMap, HeaderName, HeaderValue, header};
use nanocode_http::layer::{self, retry};

#[cfg(target_family = "wasm")]
pub(crate) mod socket;

const RESPONSES_WEBSOCKETS_BETA: &str = "responses_websockets=2026-02-06";
const RESPONSES_LITE_HEADER: &str = "x-openai-internal-codex-responses-lite";
const TURN_STATE_HEADER: &str = "x-codex-turn-state";
const USER_AGENT: &str = concat!("nanocode/", env!("CARGO_PKG_VERSION"));

#[cfg(not(target_family = "wasm"))]
pub type HostFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

#[cfg(target_family = "wasm")]
pub type HostFuture<'a, T> = Pin<Box<dyn Future<Output = T> + 'a>>;

pub trait HostTransport: Send + Sync + 'static {
    fn connect<'a>(
        &'a self,
        request: HostConnectRequest<'a>,
    ) -> HostFuture<'a, Result<ConnectedHost, HostError>>;

    fn http<'a>(
        &'a self,
        _request: HostConnectRequest<'a>,
        _body: &'a str,
    ) -> HostFuture<'a, Result<HostHttpResponse, HostError>> {
        Box::pin(async { Err(HostError::new("host HTTPS transport is unavailable")) })
    }

    fn sleep<'a>(&'a self, session_id: &'a str, duration: Duration) -> HostFuture<'a, ()>;
}

pub async fn send_http(
    sender: &dyn layer::Sender,
    request: HostConnectRequest<'_>,
    body: &str,
) -> Result<HostHttpResponse, HostError> {
    let response = sender
        .send(layer::Request {
            method: http::Method::POST,
            url: request.endpoint().to_owned(),
            headers: request.http_headers()?,
            body: body.as_bytes().to_vec(),
        })
        .await?;
    let (status, headers, mut stream, _) = response.into_parts();
    if !status.is_success() {
        let mut rejected = Vec::new();
        while let Some(chunk) = stream.next().await {
            rejected.extend(chunk?);
        }
        return Err(HostError::rejected(
            status.as_u16(),
            &headers,
            String::from_utf8_lossy(&rejected),
        ));
    }
    Ok(HostHttpResponse {
        body: Box::new(LayerBody(stream)),
        metadata: HostConnectionMetadata::from_response(status.as_u16(), &headers),
    })
}

struct LayerBody(layer::Body);

impl HostHttpBody for LayerBody {
    fn next(&mut self) -> HostFuture<'_, Result<Option<Vec<u8>>, HostError>> {
        Box::pin(async move { Ok(self.0.next().await.transpose()?) })
    }
}

fn header_value(name: &str, value: &str) -> Result<HeaderValue, HostError> {
    HeaderValue::from_str(value)
        .map_err(|error| HostError::new(format!("invalid {name} header: {error}")))
}

pub trait HostConnection: Send + 'static {
    fn send<'a>(&'a self, message: &'a str) -> HostFuture<'a, Result<(), HostError>>;

    fn next(&mut self) -> HostFuture<'_, Result<HostMessage, HostError>>;

    fn is_closed(&self) -> bool {
        false
    }

    fn close(&mut self);
}

#[derive(Clone, Copy)]
pub struct HostConnectRequest<'a> {
    endpoint: &'a str,
    bearer_token: &'a str,
    account_id: Option<&'a str>,
    fedramp: bool,
    session_id: &'a str,
    thread_id: &'a str,
    turn_state: Option<&'a str>,
    headers: &'a [super::GatewayHeader],
    responses_lite: bool,
}

impl<'a> HostConnectRequest<'a> {
    #[must_use]
    pub const fn new(
        endpoint: &'a str,
        bearer_token: &'a str,
        account_id: Option<&'a str>,
        fedramp: bool,
        session_id: &'a str,
        turn_state: Option<&'a str>,
    ) -> Self {
        Self::new_with_thread_id(
            endpoint,
            bearer_token,
            account_id,
            fedramp,
            session_id,
            session_id,
            turn_state,
        )
    }

    pub(crate) const fn new_with_thread_id(
        endpoint: &'a str,
        bearer_token: &'a str,
        account_id: Option<&'a str>,
        fedramp: bool,
        session_id: &'a str,
        thread_id: &'a str,
        turn_state: Option<&'a str>,
    ) -> Self {
        Self {
            endpoint,
            bearer_token,
            account_id,
            fedramp,
            session_id,
            thread_id,
            turn_state,
            headers: &[],
            responses_lite: true,
        }
    }

    #[must_use]
    pub const fn without_responses_lite(mut self) -> Self {
        self.responses_lite = false;
        self
    }

    pub(crate) const fn with_headers(mut self, headers: &'a [super::GatewayHeader]) -> Self {
        self.headers = headers;
        self
    }

    #[must_use]
    pub const fn headers(&self) -> &'a [super::GatewayHeader] {
        self.headers
    }

    #[must_use]
    pub const fn endpoint(&self) -> &'a str {
        self.endpoint
    }

    #[must_use]
    pub const fn bearer_token(&self) -> &'a str {
        self.bearer_token
    }

    #[must_use]
    pub const fn sends_bearer_token(&self) -> bool {
        !self.bearer_token.is_empty()
    }

    #[must_use]
    pub const fn account_id(&self) -> Option<&'a str> {
        self.account_id
    }

    #[must_use]
    pub const fn is_fedramp(&self) -> bool {
        self.fedramp
    }

    #[must_use]
    pub const fn session_id(&self) -> &'a str {
        self.session_id
    }

    #[must_use]
    pub const fn thread_id(&self) -> &'a str {
        self.thread_id
    }

    #[must_use]
    pub const fn turn_state(&self) -> Option<&'a str> {
        self.turn_state
    }

    fn common_headers(&self) -> Result<HeaderMap, HostError> {
        let mut headers = HeaderMap::new();
        if self.sends_bearer_token() {
            let mut bearer =
                header_value("authorization", &format!("Bearer {}", self.bearer_token))?;
            bearer.set_sensitive(true);
            headers.insert(header::AUTHORIZATION, bearer);
        }
        if let Some(account_id) = self.account_id {
            headers.insert(
                "chatgpt-account-id",
                header_value("chatgpt-account-id", account_id)?,
            );
        }
        if self.fedramp {
            headers.insert("x-openai-fedramp", HeaderValue::from_static("true"));
        }
        if self.responses_lite {
            headers.insert(RESPONSES_LITE_HEADER, HeaderValue::from_static("true"));
        }
        headers.insert("session-id", header_value("session-id", self.session_id)?);
        let thread = header_value("thread-id", self.thread_id)?;
        headers.insert("thread-id", thread.clone());
        headers.insert("x-client-request-id", thread);
        if let Some(turn_state) = self
            .turn_state
            .and_then(|state| HeaderValue::from_str(state).ok())
        {
            headers.insert(TURN_STATE_HEADER, turn_state);
        }
        headers.insert(header::USER_AGENT, HeaderValue::from_static(USER_AGENT));
        Ok(headers)
    }

    pub fn http_headers(&self) -> Result<HeaderMap, HostError> {
        let mut headers = self.common_headers()?;
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        headers.insert(
            header::ACCEPT,
            HeaderValue::from_static("text/event-stream"),
        );
        for gateway in self.headers {
            let name = HeaderName::from_static(gateway.name());
            let mut value = header_value(gateway.name(), gateway.value())?;
            value.set_sensitive(gateway.secret_value().is_some());
            headers.insert(name, value);
        }
        Ok(headers)
    }

    pub fn socket_headers(&self) -> Result<HeaderMap, HostError> {
        let mut headers = self.common_headers()?;
        headers.insert(
            "openai-beta",
            HeaderValue::from_static(RESPONSES_WEBSOCKETS_BETA),
        );
        headers.insert(
            "x-responsesapi-include-timing-metrics",
            HeaderValue::from_static("true"),
        );
        Ok(headers)
    }
}

pub struct ConnectedHost {
    pub(crate) connection: Box<dyn HostConnection>,
    pub(crate) metadata: HostConnectionMetadata,
}

impl ConnectedHost {
    #[must_use]
    pub fn new(connection: impl HostConnection, metadata: HostConnectionMetadata) -> Self {
        Self {
            connection: Box::new(connection),
            metadata,
        }
    }

    #[must_use]
    pub fn into_parts(self) -> (Box<dyn HostConnection>, HostConnectionMetadata) {
        (self.connection, self.metadata)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HostConnectionMetadata {
    pub(crate) status: u16,
    pub(crate) request_id: Option<String>,
    pub(crate) server_model: Option<String>,
    pub(crate) reasoning_included: bool,
    pub(crate) turn_state: Option<String>,
}

impl HostConnectionMetadata {
    #[must_use]
    pub const fn new(status: u16) -> Self {
        Self {
            status,
            request_id: None,
            server_model: None,
            reasoning_included: false,
            turn_state: None,
        }
    }

    #[must_use]
    pub fn from_response(status: u16, headers: &HeaderMap) -> Self {
        let text = |name: &str| {
            headers
                .get(name)
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned)
        };
        Self {
            status,
            request_id: text("x-request-id"),
            server_model: text("openai-model"),
            reasoning_included: headers.contains_key("x-reasoning-included"),
            turn_state: text(TURN_STATE_HEADER),
        }
    }

    #[must_use]
    pub fn with_request_id(mut self, request_id: impl Into<String>) -> Self {
        self.request_id = Some(request_id.into());
        self
    }

    #[must_use]
    pub fn with_server_model(mut self, server_model: impl Into<String>) -> Self {
        self.server_model = Some(server_model.into());
        self
    }

    #[must_use]
    pub const fn with_reasoning_included(mut self, included: bool) -> Self {
        self.reasoning_included = included;
        self
    }

    #[must_use]
    pub fn with_turn_state(mut self, turn_state: impl Into<String>) -> Self {
        self.turn_state = Some(turn_state.into());
        self
    }

    #[must_use]
    pub const fn status(&self) -> u16 {
        self.status
    }

    #[must_use]
    pub fn request_id(&self) -> Option<&str> {
        self.request_id.as_deref()
    }

    #[must_use]
    pub fn server_model(&self) -> Option<&str> {
        self.server_model.as_deref()
    }

    #[must_use]
    pub const fn reasoning_included(&self) -> bool {
        self.reasoning_included
    }

    #[must_use]
    pub fn turn_state(&self) -> Option<&str> {
        self.turn_state.as_deref()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HostMessage {
    Text(String),

    Closed { detail: String },

    Binary,
}

#[derive(Clone, Debug, thiserror::Error)]
pub enum HostError {
    #[error("{detail}")]
    Transport { detail: String, reconnectable: bool },

    #[error("WebSocket handshake was rejected with HTTP {status}: {body}")]
    HandshakeRejected {
        status: u16,

        body: String,

        retry_after: Option<super::RetryAfter>,

        should_retry: Option<bool>,
    },
}

impl HostError {
    #[must_use]
    pub fn new(detail: impl Into<String>) -> Self {
        Self::Transport {
            detail: detail.into(),
            reconnectable: false,
        }
    }

    #[must_use]
    pub fn handshake_rejected(
        status: u16,
        body: impl Into<String>,
        retry_after: Option<super::RetryAfter>,
    ) -> Self {
        Self::HandshakeRejected {
            status,
            body: body.into(),
            retry_after,
            should_retry: None,
        }
    }

    #[must_use]
    pub fn rejected(status: u16, headers: &HeaderMap, body: impl Into<String>) -> Self {
        Self::HandshakeRejected {
            status,
            body: body.into(),
            retry_after: super::RetryAfter::from_headers(headers),
            should_retry: retry::should_retry(headers),
        }
    }

    #[must_use]
    pub fn with_reconnectable(self, reconnectable: bool) -> Self {
        match self {
            Self::Transport { detail, .. } => Self::Transport {
                detail,
                reconnectable,
            },
            rejected @ Self::HandshakeRejected { .. } => rejected,
        }
    }

    #[must_use]
    pub fn detail(&self) -> &str {
        match self {
            Self::Transport { detail, .. } => detail,
            Self::HandshakeRejected { body, .. } => body,
        }
    }

    #[must_use]
    pub const fn is_reconnectable(&self) -> bool {
        matches!(
            self,
            Self::Transport {
                reconnectable: true,
                ..
            }
        )
    }
}

impl From<layer::Error> for HostError {
    fn from(error: layer::Error) -> Self {
        Self::new(error.detail()).with_reconnectable(error.is_retryable() || error.is_timeout())
    }
}

pub struct HostHttpResponse {
    pub body: Box<dyn HostHttpBody>,

    pub metadata: HostConnectionMetadata,
}

#[cfg(not(target_family = "wasm"))]
pub trait HostHttpBody: Send + 'static {
    fn next(&mut self) -> HostFuture<'_, Result<Option<Vec<u8>>, HostError>>;
}

#[cfg(target_family = "wasm")]
pub trait HostHttpBody: 'static {
    fn next(&mut self) -> HostFuture<'_, Result<Option<Vec<u8>>, HostError>>;
}
