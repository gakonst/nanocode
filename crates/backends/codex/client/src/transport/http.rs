#[cfg(not(target_family = "wasm"))]
use crate::client::OpenAiAuthMode;
use crate::client::{
    OpenAiAuthSnapshot, monotonic_now_ns,
    transport::host::{HostConnectRequest, HostError, HostHttpBody, HostTransport},
};
use http::header;
#[cfg(not(target_family = "wasm"))]
use nanocode_http::layer::{self, retry};

#[cfg(not(target_family = "wasm"))]
use crate::client::RetryAfter;
use crate::client::{EncodedRequest, ResponsesError, socket::ReceivedText};

#[cfg(not(target_family = "wasm"))]
const RESPONSES_LITE_HEADER: &str = "x-openai-internal-codex-responses-lite";
#[cfg(not(target_family = "wasm"))]
const TURN_STATE_HEADER: &str = "x-codex-turn-state";
#[cfg(not(target_family = "wasm"))]
const ZSTD_LEVEL: i32 = 3;
#[cfg(not(target_family = "wasm"))]
const STREAM_IDLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

#[cfg(not(target_family = "wasm"))]
async fn idle<T>(read: impl std::future::Future<Output = T>) -> Result<T, ResponsesError> {
    nanocode_platform::timeout(STREAM_IDLE_TIMEOUT, read)
        .await
        .map_err(|_| stream_stalled())
}

#[cfg(not(target_family = "wasm"))]
async fn idle_within<T>(
    timer: &mut super::idle::IdleTimer,
    read: impl std::future::Future<Output = T>,
) -> Result<T, ResponsesError> {
    timer
        .within(STREAM_IDLE_TIMEOUT, read)
        .await
        .ok_or_else(stream_stalled)
}

#[cfg(not(target_family = "wasm"))]
fn stream_stalled() -> ResponsesError {
    ResponsesError::HttpRequest {
        detail: format!(
            "the Responses stream sent no data for {} s; the stream stalled",
            STREAM_IDLE_TIMEOUT.as_secs()
        ),
        retryable: true,
        timeout: true,
    }
}

#[derive(Clone)]
pub(crate) struct ResponsesHttp {
    #[cfg(not(target_family = "wasm"))]
    client: nanocode_http::Client,

    #[cfg(not(target_family = "wasm"))]
    chat_gateway: Option<(
        std::sync::Arc<super::chat::ChatGateway>,
        nanocode_http::Client,
    )>,
    #[cfg(target_family = "wasm")]
    chat_gateway: Option<std::sync::Arc<super::chat::ChatGateway>>,
    #[cfg(not(target_family = "wasm"))]
    layer: Option<std::sync::Arc<dyn layer::Layer>>,
    host: Option<std::sync::Arc<dyn HostTransport>>,
    gateway: Option<std::sync::Arc<super::ResponsesGateway>>,
}

pub(crate) enum ResponsesHttpStream {
    Chat(Box<super::chat::ChatStream>),
    Sse(SseStream),
}

pub(crate) struct SseStream {
    body: Option<SseBody>,
    #[cfg(not(target_family = "wasm"))]
    hook: Option<Hook>,
    decoder: SseDecoder,
    #[cfg(not(target_family = "wasm"))]
    idle: super::idle::IdleTimer,
}

enum SseBody {
    #[cfg(not(target_family = "wasm"))]
    Reqwest(nanocode_http::Response),
    #[cfg(not(target_family = "wasm"))]
    Layered(layer::Body),
    Host(Box<dyn HostHttpBody>),
}

#[cfg(not(target_family = "wasm"))]
struct Hook {
    events: Box<dyn layer::EventHook>,
    pending: Option<layer::BoxFuture<'static, Result<(), layer::Error>>>,
}

pub(crate) struct HttpMetadata {
    pub(crate) reasoning_included: bool,
    pub(crate) turn_state: Option<String>,
}

#[cfg(not(target_family = "wasm"))]
impl ResponsesHttp {
    pub(crate) async fn preconnect(&self, api_base_url: &str) -> Result<(), ResponsesError> {
        if self.host.is_some() || self.chat_gateway.is_some() || self.layer.is_some() {
            return Ok(());
        }
        self.client
            .preconnect(format!("{}/responses", api_base_url.trim_end_matches('/')))
            .await
            .map_err(map_http_error)
    }

    pub(crate) fn new(
        client: nanocode_http::Client,
        host: Option<std::sync::Arc<dyn HostTransport>>,
        chat_gateway: Option<std::sync::Arc<super::chat::ChatGateway>>,
        gateway: Option<std::sync::Arc<super::ResponsesGateway>>,
        layer: Option<std::sync::Arc<dyn layer::Layer>>,
    ) -> Self {
        Self {
            client,
            chat_gateway: chat_gateway
                .map(|gateway| (gateway, super::chat::ChatGateway::http_client())),
            layer,
            host,
            gateway,
        }
    }

    pub(crate) async fn send(
        &self,
        api_base_url: &str,
        auth: &OpenAiAuthSnapshot,
        session_id: &str,
        thread_id: &str,
        turn_state: Option<&str>,
        request: &EncodedRequest,
    ) -> Result<(ResponsesHttpStream, HttpMetadata), ResponsesError> {
        if let Some((gateway, client)) = &self.chat_gateway {
            return match self.host.as_deref() {
                Some(host) => {
                    gateway
                        .send_host(
                            host,
                            api_base_url,
                            auth,
                            session_id,
                            thread_id,
                            request.json(),
                        )
                        .await
                }
                None => {
                    gateway
                        .send(client, api_base_url, auth, request.json())
                        .await
                }
            };
        }
        if self.host.is_some() {
            return self
                .send_host(
                    api_base_url,
                    auth,
                    session_id,
                    thread_id,
                    turn_state,
                    request,
                )
                .await;
        }
        let endpoint = format!("{}/responses", api_base_url.trim_end_matches('/'));
        let mut builder = self
            .client
            .post(endpoint)
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::ACCEPT, "text/event-stream")
            .header(RESPONSES_LITE_HEADER, "true")
            .header("session-id", session_id)
            .header("thread-id", thread_id)
            .header("x-client-request-id", thread_id)
            .header(
                header::USER_AGENT,
                concat!("nanocode/", env!("CARGO_PKG_VERSION")),
            );
        let body = request.json().to_owned();
        builder = if auth.mode() == OpenAiAuthMode::ChatGpt && self.gateway.is_none() {
            let encoded = nanocode_platform::spawn_blocking(move || {
                nanocode_zstd::compress(body.as_bytes(), ZSTD_LEVEL)
            })
            .await
            .map_err(|error| encode_error(&error))?
            .map_err(|error| encode_error(&error))?;
            nanocode_trace::Span::current().record("request.zstd_bytes", encoded.len());
            builder
                .header(header::CONTENT_ENCODING, "zstd")
                .body(encoded)
        } else {
            builder.body(body)
        };
        if auth.sends_bearer() {
            builder = builder.bearer_auth(auth.bearer());
        }
        if let Some(account_id) = auth.account_id() {
            builder = builder.header("ChatGPT-Account-ID", account_id);
        }
        if auth.is_fedramp() {
            builder = builder.header("X-OpenAI-Fedramp", "true");
        }
        for gateway_header in self
            .gateway
            .as_deref()
            .map_or(&[][..], super::ResponsesGateway::headers)
        {
            builder = builder.header(gateway_header.name(), gateway_header.value());
        }
        if let Some(turn_state) =
            turn_state.and_then(|value| header::HeaderValue::from_bytes(value.as_bytes()).ok())
        {
            builder = builder.header(TURN_STATE_HEADER, turn_state);
        }
        let request = builder.build().map_err(map_http_error)?;
        if let Some(layer) = &self.layer {
            return self.send_layered(layer.as_ref(), request).await;
        }
        let response = idle(self.client.execute(request))
            .await?
            .map(nanocode_http::decode_flate_response)
            .map_err(map_http_error)?;
        let status = response.status();
        if !status.is_success() {
            let retry_after = RetryAfter::from_headers(response.headers());
            let should_retry = retry::should_retry(response.headers());

            nanocode_trace::debug!(
                target: "nanocode::responses::http",
                status = status.as_u16(),
                retry_after_deadline_ms = retry_after.map(RetryAfter::deadline_unix_ms),
                "Responses HTTPS rejection advice captured before body"
            );
            let body = response.text().await.unwrap_or_default();
            return Err(ResponsesError::http_rejected(
                status.as_u16(),
                body,
                retry_after,
                should_retry,
            ));
        }
        let metadata = HttpMetadata {
            reasoning_included: response.headers().contains_key("x-reasoning-included"),
            turn_state: response
                .headers()
                .get(TURN_STATE_HEADER)
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned),
        };
        Ok((
            ResponsesHttpStream::Sse(SseStream::new(SseBody::Reqwest(response))),
            metadata,
        ))
    }

    async fn send_layered(
        &self,
        layer: &dyn layer::Layer,
        request: nanocode_http::Request,
    ) -> Result<(ResponsesHttpStream, HttpMetadata), ResponsesError> {
        let request = layer::Request {
            method: request.method().clone(),
            url: request.url().to_string(),
            headers: request.headers().clone(),
            body: request
                .body()
                .and_then(nanocode_http::Body::as_bytes)
                .map(<[u8]>::to_vec)
                .unwrap_or_default(),
        };
        let sender = nanocode_http::ClientSender::new(&self.client);
        let response = idle(layer.send(request, layer::Next::new(&sender)))
            .await?
            .map_err(map_layer_error)?;
        let (status, headers, body, events) = response.into_parts();
        if !status.is_success() {
            let body = collect_layered(body).await.unwrap_or_default();
            return Err(ResponsesError::http_rejected(
                status.as_u16(),
                body,
                RetryAfter::from_headers(&headers),
                retry::should_retry(&headers),
            ));
        }
        let metadata = HttpMetadata {
            reasoning_included: headers.contains_key("x-reasoning-included"),
            turn_state: headers
                .get(TURN_STATE_HEADER)
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned),
        };
        let decoder = if events.is_some() {
            SseDecoder::tracking_events()
        } else {
            SseDecoder::default()
        };
        Ok((
            ResponsesHttpStream::Sse(SseStream {
                body: Some(SseBody::Layered(body)),
                hook: events.map(|events| Hook {
                    events,
                    pending: None,
                }),
                decoder,
                idle: super::idle::IdleTimer::default(),
            }),
            metadata,
        ))
    }
}

#[cfg(not(target_family = "wasm"))]
async fn collect_layered(mut body: layer::Body) -> Result<String, layer::Error> {
    use nanocode_sync::StreamExt;

    let mut bytes = Vec::new();
    while let Some(chunk) = body.next().await {
        bytes.extend(chunk?);
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

#[cfg(not(target_family = "wasm"))]
fn map_layer_error(error: layer::Error) -> ResponsesError {
    ResponsesError::HttpRequest {
        retryable: error.is_retryable(),
        timeout: error.is_timeout(),
        detail: error.detail().to_owned(),
    }
}

impl ResponsesHttpStream {
    pub(crate) fn chat(stream: super::chat::ChatStream) -> Self {
        Self::Chat(Box::new(stream))
    }

    fn host(body: Box<dyn HostHttpBody>) -> Self {
        Self::Sse(SseStream::new(SseBody::Host(body)))
    }

    pub(crate) async fn next_text(&mut self) -> Result<ReceivedText, ResponsesError> {
        match self {
            Self::Chat(chat) => chat.next_text().await,
            Self::Sse(sse) => sse.next_text().await,
        }
    }

    pub(crate) fn try_next_text(&mut self) -> Result<Option<ReceivedText>, ResponsesError> {
        match self {
            Self::Chat(chat) => Ok(chat.try_next_text()),
            Self::Sse(sse) => sse.buffered_text(),
        }
    }
}

impl SseStream {
    fn new(body: SseBody) -> Self {
        Self {
            body: Some(body),
            #[cfg(not(target_family = "wasm"))]
            hook: None,
            decoder: SseDecoder::default(),
            #[cfg(not(target_family = "wasm"))]
            idle: super::idle::IdleTimer::default(),
        }
    }

    async fn next_text(&mut self) -> Result<ReceivedText, ResponsesError> {
        loop {
            #[cfg(not(target_family = "wasm"))]
            if let Some(pending) = self.hook.as_mut().and_then(|hook| hook.pending.take()) {
                idle_within(&mut self.idle, pending)
                    .await?
                    .map_err(map_layer_error)?;
            }
            if let Some(text) = self.buffered_text()? {
                return Ok(text);
            }
            #[cfg(not(target_family = "wasm"))]
            if self
                .hook
                .as_ref()
                .is_some_and(|hook| hook.pending.is_some())
            {
                continue;
            }
            let Some(body) = self.body.as_mut() else {
                return Err(ResponsesError::UnexpectedEnd);
            };
            let pushed = match body {
                #[cfg(not(target_family = "wasm"))]
                SseBody::Reqwest(response) => idle_within(&mut self.idle, response.chunk())
                    .await?
                    .map_err(map_http_error)?
                    .map(|chunk| self.decoder.push(&chunk)),
                #[cfg(not(target_family = "wasm"))]
                SseBody::Layered(body) => {
                    use nanocode_sync::StreamExt;

                    idle_within(&mut self.idle, body.next())
                        .await?
                        .transpose()
                        .map_err(map_layer_error)?
                        .map(|chunk| self.decoder.push(&chunk))
                }
                #[cfg(not(target_family = "wasm"))]
                SseBody::Host(body) => idle_within(&mut self.idle, body.next())
                    .await?
                    .map_err(map_host_error)?
                    .map(|chunk| self.decoder.push(&chunk)),
                #[cfg(target_family = "wasm")]
                SseBody::Host(body) => body
                    .next()
                    .await
                    .map_err(map_host_error)?
                    .map(|chunk| self.decoder.push(&chunk)),
            };
            if pushed.is_none() {
                self.body = None;
                self.decoder.finish();
            }
        }
    }

    fn buffered_text(&mut self) -> Result<Option<ReceivedText>, ResponsesError> {
        let Some(text) = self.decoder.next()? else {
            return Ok(None);
        };
        #[cfg(not(target_family = "wasm"))]
        if let Some(hook) = self.hook.as_mut()
            && let Some(pending) = hook.events.event(layer::Event {
                name: self.decoder.event_name(),
                data: &text,
            })
        {
            hook.pending = Some(pending);
            return Ok(None);
        }
        let (received_ns, retry_receipt) = self
            .decoder
            .received
            .unwrap_or_else(|| (monotonic_now_ns(), super::RetryReceipt::now()));
        Ok(Some(ReceivedText {
            #[cfg(not(target_family = "wasm"))]
            text: text.into(),
            #[cfg(target_family = "wasm")]
            text,
            received_ns,
            retry_receipt,
        }))
    }
}

#[derive(Default)]
pub(crate) struct SseDecoder {
    bytes: Vec<u8>,
    cursor: usize,
    scan_start: usize,
    data: Vec<String>,
    finished: bool,
    received: Option<(u64, super::RetryReceipt)>,
    track_events: bool,
    event: Option<String>,
    dispatched_event: Option<String>,
}

impl SseDecoder {
    #[cfg(not(target_family = "wasm"))]
    fn tracking_events() -> Self {
        Self {
            track_events: true,
            ..Self::default()
        }
    }

    #[cfg(not(target_family = "wasm"))]
    fn event_name(&self) -> Option<&str> {
        self.dispatched_event.as_deref()
    }

    pub(crate) fn push(&mut self, chunk: &[u8]) {
        self.received = Some((monotonic_now_ns(), super::RetryReceipt::now()));
        self.compact();
        self.bytes.extend_from_slice(chunk);
    }

    pub(crate) fn finish(&mut self) {
        self.finished = true;
        self.compact();
        if !self.bytes.is_empty() {
            self.bytes.push(b'\n');
        }
        self.bytes.push(b'\n');
    }

    pub(crate) fn next(&mut self) -> Result<Option<String>, ResponsesError> {
        loop {
            let Some(relative_newline) = memchr::memchr(b'\n', &self.bytes[self.scan_start..])
            else {
                self.scan_start = self.bytes.len();
                return Ok(None);
            };
            let line_start = self.cursor;
            let newline = self.scan_start + relative_newline;
            self.cursor = newline + 1;
            self.scan_start = self.cursor;
            let line_end = if newline > line_start && self.bytes.get(newline - 1) == Some(&b'\r') {
                newline - 1
            } else {
                newline
            };
            let line = std::str::from_utf8(&self.bytes[line_start..line_end]).map_err(|error| {
                ResponsesError::InvalidSseUtf8 {
                    detail: error.to_string(),
                }
            })?;
            if line.is_empty() {
                let name = self.event.take();
                if self.data.is_empty() {
                    if self.finished && self.cursor == self.bytes.len() {
                        return Ok(None);
                    }
                    continue;
                }
                let event = if self.data.len() == 1 {
                    self.data.pop().expect("one SSE data line exists")
                } else {
                    let event = self.data.join("\n");
                    self.data.clear();
                    event
                };
                if event == "[DONE]" {
                    continue;
                }
                self.dispatched_event = name;
                return Ok(Some(event));
            }
            if let Some(data) = line.strip_prefix("data:") {
                self.data
                    .push(data.strip_prefix(' ').unwrap_or(data).to_owned());
            } else if self.track_events
                && let Some(name) = line.strip_prefix("event:")
            {
                self.event = Some(name.strip_prefix(' ').unwrap_or(name).to_owned());
            }
        }
    }

    fn compact(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let remaining = self.bytes.len() - self.cursor;
        self.bytes.copy_within(self.cursor.., 0);
        self.bytes.truncate(remaining);
        self.scan_start -= self.cursor;
        self.cursor = 0;
    }
}

#[cfg(not(target_family = "wasm"))]
fn encode_error(error: &dyn std::error::Error) -> ResponsesError {
    ResponsesError::HttpRequest {
        retryable: false,
        timeout: false,
        detail: format!("zstd request body: {error}"),
    }
}

#[cfg(not(target_family = "wasm"))]
pub(super) fn map_http_error(error: nanocode_http::Error) -> ResponsesError {
    ResponsesError::HttpRequest {
        retryable: nanocode_http::retryable(&error),
        timeout: error.is_timeout(),
        detail: error.to_string(),
    }
}

#[cfg(target_family = "wasm")]
impl ResponsesHttp {
    #[expect(clippy::unused_async, reason = "a WebAssembly host opens its own connections")]
    pub(crate) async fn preconnect(&self, _api_base_url: &str) -> Result<(), ResponsesError> {
        Ok(())
    }

    pub(crate) fn new(
        host: Option<std::sync::Arc<dyn HostTransport>>,
        chat_gateway: Option<std::sync::Arc<super::chat::ChatGateway>>,
        gateway: Option<std::sync::Arc<super::ResponsesGateway>>,
    ) -> Self {
        Self {
            chat_gateway,
            host,
            gateway,
        }
    }

    pub(crate) async fn send(
        &self,
        api_base_url: &str,
        auth: &OpenAiAuthSnapshot,
        session_id: &str,
        thread_id: &str,
        turn_state: Option<&str>,
        request: &EncodedRequest,
    ) -> Result<(ResponsesHttpStream, HttpMetadata), ResponsesError> {
        if let Some(gateway) = &self.chat_gateway {
            let host = self
                .host
                .as_deref()
                .ok_or(ResponsesError::HostUnavailable)?;
            return gateway
                .send_host(
                    host,
                    api_base_url,
                    auth,
                    session_id,
                    thread_id,
                    request.json(),
                )
                .await;
        }
        self.send_host(
            api_base_url,
            auth,
            session_id,
            thread_id,
            turn_state,
            request,
        )
        .await
    }
}

impl ResponsesHttp {
    async fn send_host(
        &self,
        api_base_url: &str,
        auth: &OpenAiAuthSnapshot,
        session_id: &str,
        thread_id: &str,
        turn_state: Option<&str>,
        request: &EncodedRequest,
    ) -> Result<(ResponsesHttpStream, HttpMetadata), ResponsesError> {
        let host = self
            .host
            .as_deref()
            .ok_or(ResponsesError::HostUnavailable)?;
        let endpoint = format!("{}/responses", api_base_url.trim_end_matches('/'));
        let turn_state =
            turn_state.filter(|value| header::HeaderValue::from_bytes(value.as_bytes()).is_ok());
        let response = host
            .http(
                HostConnectRequest::new_with_thread_id(
                    &endpoint,
                    auth.bearer(),
                    auth.account_id(),
                    auth.is_fedramp(),
                    session_id,
                    thread_id,
                    turn_state,
                )
                .with_headers(
                    self.gateway
                        .as_deref()
                        .map_or(&[][..], super::ResponsesGateway::headers),
                ),
                request.json(),
            )
            .await
            .map_err(map_host_error)?;
        let metadata = HttpMetadata {
            reasoning_included: response.metadata.reasoning_included(),
            turn_state: response.metadata.turn_state().map(str::to_owned),
        };
        Ok((ResponsesHttpStream::host(response.body), metadata))
    }
}

pub(crate) fn map_host_error(error: HostError) -> ResponsesError {
    match error {
        HostError::HandshakeRejected {
            status,
            body,
            retry_after,
            should_retry,
        } => ResponsesError::http_rejected(status, body, retry_after, should_retry),
        HostError::Transport {
            detail,
            reconnectable,
        } => ResponsesError::HttpRequest {
            detail,
            retryable: reconnectable,
            timeout: false,
        },
    }
}
