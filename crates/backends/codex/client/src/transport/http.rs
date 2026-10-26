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
    tokio::time::timeout(STREAM_IDLE_TIMEOUT, read)
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
    client: reqwest::Client,

    #[cfg(not(target_family = "wasm"))]
    chat_gateway: Option<(std::sync::Arc<super::chat::ChatGateway>, reqwest::Client)>,
    #[cfg(target_family = "wasm")]
    chat_gateway: Option<std::sync::Arc<super::chat::ChatGateway>>,
    #[cfg(not(target_family = "wasm"))]
    layer: Option<std::sync::Arc<dyn layer::Layer>>,
    host: Option<std::sync::Arc<dyn HostTransport>>,
    gateway: Option<std::sync::Arc<super::ResponsesGateway>>,
}

pub(crate) struct ResponsesHttpStream {
    #[cfg(not(target_family = "wasm"))]
    response: Option<reqwest::Response>,

    chat: Option<Box<super::chat::ChatStream>>,
    #[cfg(not(target_family = "wasm"))]
    host: Option<Box<dyn HostHttpBody>>,
    #[cfg(not(target_family = "wasm"))]
    layered: Option<LayeredBody>,
    #[cfg(target_family = "wasm")]
    response: Option<Box<dyn HostHttpBody>>,
    decoder: SseDecoder,
    ended: bool,
    #[cfg(not(target_family = "wasm"))]
    idle: super::idle::IdleTimer,
}

#[cfg(not(target_family = "wasm"))]
struct LayeredBody {
    body: layer::Body,
    events: Option<Box<dyn layer::EventHook>>,
    pending: Option<layer::BoxFuture<'static, Result<(), layer::Error>>>,
}

pub(crate) struct HttpMetadata {
    pub(crate) reasoning_included: bool,
    pub(crate) turn_state: Option<String>,
}

#[cfg(not(target_family = "wasm"))]
impl ResponsesHttp {
    pub(crate) fn new(
        client: reqwest::Client,
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
                            request.raw().get(),
                        )
                        .await
                }
                None => {
                    gateway
                        .send(client, api_base_url, auth, request.raw().get())
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
        let body = request.raw().get().to_owned();
        builder = if auth.mode() == OpenAiAuthMode::ChatGpt && self.gateway.is_none() {
            let encoded = tokio::task::spawn_blocking(move || {
                zstd::stream::encode_all(body.as_bytes(), ZSTD_LEVEL)
            })
            .await
            .map_err(|error| encode_error(&error))?
            .map_err(|error| encode_error(&error))?;
            tracing::Span::current().record("request.zstd_bytes", encoded.len());
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
            .map_err(map_http_error)?;
        let status = response.status();
        if !status.is_success() {
            let retry_after = RetryAfter::from_headers(response.headers());
            let should_retry = retry::should_retry(response.headers());

            tracing::debug!(
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
            ResponsesHttpStream {
                response: Some(response),
                chat: None,
                host: None,
                layered: None,
                decoder: SseDecoder::default(),
                ended: false,
                idle: super::idle::IdleTimer::default(),
            },
            metadata,
        ))
    }

    async fn send_layered(
        &self,
        layer: &dyn layer::Layer,
        request: reqwest::Request,
    ) -> Result<(ResponsesHttpStream, HttpMetadata), ResponsesError> {
        let request = layer::Request {
            method: request.method().clone(),
            url: request.url().to_string(),
            headers: request.headers().clone(),
            body: request
                .body()
                .and_then(reqwest::Body::as_bytes)
                .map(<[u8]>::to_vec)
                .unwrap_or_default(),
        };
        let sender = nanocode_http::ReqwestSender::new(&self.client);
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
            ResponsesHttpStream {
                response: None,
                chat: None,
                host: None,
                layered: Some(LayeredBody {
                    body,
                    events,
                    pending: None,
                }),
                decoder,
                ended: false,
                idle: super::idle::IdleTimer::default(),
            },
            metadata,
        ))
    }
}

#[cfg(not(target_family = "wasm"))]
async fn collect_layered(mut body: layer::Body) -> Result<String, layer::Error> {
    use futures_util::StreamExt;

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
        Self {
            response: None,
            chat: Some(Box::new(stream)),
            #[cfg(not(target_family = "wasm"))]
            host: None,
            #[cfg(not(target_family = "wasm"))]
            layered: None,
            decoder: SseDecoder::default(),
            ended: true,
            #[cfg(not(target_family = "wasm"))]
            idle: super::idle::IdleTimer::default(),
        }
    }

    fn host(body: Box<dyn HostHttpBody>) -> Self {
        #[cfg(not(target_family = "wasm"))]
        return Self {
            response: None,
            chat: None,
            host: Some(body),
            layered: None,
            decoder: SseDecoder::default(),
            ended: false,
            idle: super::idle::IdleTimer::default(),
        };
        #[cfg(target_family = "wasm")]
        Self {
            response: Some(body),
            chat: None,
            decoder: SseDecoder::default(),
            ended: false,
        }
    }

    pub(crate) async fn next_text(&mut self) -> Result<ReceivedText, ResponsesError> {
        if let Some(chat) = self.chat.as_mut() {
            return chat.next_text().await;
        }
        loop {
            #[cfg(not(target_family = "wasm"))]
            if let Some(pending) = self
                .layered
                .as_mut()
                .and_then(|layered| layered.pending.take())
            {
                idle_within(&mut self.idle, pending)
                    .await?
                    .map_err(map_layer_error)?;
            }
            if let Some(text) = self.buffered_text()? {
                return Ok(text);
            }
            #[cfg(not(target_family = "wasm"))]
            if self
                .layered
                .as_ref()
                .is_some_and(|layered| layered.pending.is_some())
            {
                continue;
            }
            if self.ended {
                return Err(ResponsesError::UnexpectedEnd);
            }
            #[cfg(not(target_family = "wasm"))]
            let pushed = if let Some(host) = self.host.as_mut() {
                idle_within(&mut self.idle, host.next())
                    .await?
                    .map_err(map_host_error)?
                    .map(|chunk| self.decoder.push(&chunk))
            } else if let Some(layered) = self.layered.as_mut() {
                use futures_util::StreamExt;

                idle_within(&mut self.idle, layered.body.next())
                    .await?
                    .transpose()
                    .map_err(map_layer_error)?
                    .map(|chunk| self.decoder.push(&chunk))
            } else if let Some(response) = self.response.as_mut() {
                idle_within(&mut self.idle, response.chunk())
                    .await?
                    .map_err(map_http_error)?
                    .map(|chunk| self.decoder.push(&chunk))
            } else {
                None
            };
            #[cfg(target_family = "wasm")]
            let pushed = match self.response.as_mut() {
                Some(response) => response
                    .next()
                    .await
                    .map_err(map_host_error)?
                    .map(|chunk| self.decoder.push(&chunk)),
                None => None,
            };
            if pushed.is_none() {
                self.ended = true;
                self.decoder.finish();
            }
        }
    }

    pub(crate) fn try_next_text(&mut self) -> Result<Option<ReceivedText>, ResponsesError> {
        if let Some(chat) = self.chat.as_mut() {
            return Ok(chat.try_next_text());
        }
        self.buffered_text()
    }

    fn buffered_text(&mut self) -> Result<Option<ReceivedText>, ResponsesError> {
        let Some(text) = self.decoder.next()? else {
            return Ok(None);
        };
        #[cfg(not(target_family = "wasm"))]
        if let Some(layered) = self.layered.as_mut()
            && let Some(events) = layered.events.as_mut()
            && let Some(pending) = events.event(layer::Event {
                name: self.decoder.event_name(),
                data: &text,
            })
        {
            layered.pending = Some(pending);
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
fn map_http_error(error: reqwest::Error) -> ResponsesError {
    ResponsesError::HttpRequest {
        retryable: error.is_connect() || error.is_body() || error.is_request(),
        timeout: error.is_timeout(),
        detail: error.to_string(),
    }
}

#[cfg(target_family = "wasm")]
impl ResponsesHttp {
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
                    request.raw().get(),
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
                request.raw().get(),
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
