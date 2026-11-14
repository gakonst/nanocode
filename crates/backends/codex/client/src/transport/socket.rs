use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use http::{HeaderValue, header};
use nanocode_http::layer::retry;
use nanocode_platform::timeout;
use nanocode_sync::{FutureExt, SinkExt, StreamExt};
use nanocode_websocket::{
    Error as WebSocketError, Frame, IntoClientRequest, MaybeTlsStream, Message, OpCode,
    ProtocolError, Utf8Bytes, WebSocketStream,
};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, oneshot};

use crate::client::{
    EncodedRequest, OpenAiAuthSnapshot, ResponsesError,
    connector::connect_deflate,
    transport::{
        SendOutcome,
        deflate::{self, InflateStream},
    },
};
use crate::client::{monotonic_now_ns, transport::wire::turn_state_from_event};

pub(crate) use crate::client::transport::wire::decode_event;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const SEND_TIMEOUT: Duration = Duration::from_secs(30);
const SOCKET_BATCH_CAPACITY: usize = 32;
const RESPONSES_WEBSOCKETS_BETA: &str = "responses_websockets=2026-02-06";
const RESPONSES_LITE_HEADER: &str = "x-openai-internal-codex-responses-lite";
const TURN_STATE_HEADER: &str = "x-codex-turn-state";

type Socket = WebSocketStream<InflateStream<MaybeTlsStream<TcpStream>>>;

pub(crate) struct ConnectionMetadata {
    pub status: u16,
    pub request_id: Option<String>,
    pub server_model: Option<String>,
    pub reasoning_included: bool,
    pub turn_state: Option<String>,
}

pub(crate) struct ResponsesSocket {
    pump: SocketPump,
    turn_state: Option<String>,
    deflate: bool,
}

pub(crate) struct ReceivedText {
    pub text: Utf8Bytes,
    pub received_ns: u64,
    pub retry_receipt: super::RetryReceipt,
}

struct SocketPump {
    commands: mpsc::Sender<SocketCommand>,
    messages: mpsc::Receiver<PumpBatch>,
    batch: Option<ReceivedBatch>,
    task: tokio::task::JoinHandle<()>,
    stopped: Arc<AtomicBool>,
}

struct PumpBatch {
    messages: Vec<std::result::Result<Message, WebSocketError>>,
    received_ns: u64,
    retry_receipt: super::RetryReceipt,
}

struct ReceivedBatch {
    messages: std::vec::IntoIter<std::result::Result<Message, WebSocketError>>,
    received_ns: u64,
    retry_receipt: super::RetryReceipt,
}

struct PumpMessage {
    message: std::result::Result<Message, WebSocketError>,
    received_ns: u64,
    retry_receipt: super::RetryReceipt,
}

enum SocketCommand {
    Send {
        message: Message,
        result: oneshot::Sender<std::result::Result<SendOutcome, WebSocketError>>,
    },
}

impl ResponsesSocket {
    pub(crate) async fn connect(
        endpoint: &str,
        auth: &OpenAiAuthSnapshot,
        session_id: &str,
        thread_id: &str,
        turn_state: Option<&str>,
    ) -> Result<(Self, ConnectionMetadata), ResponsesError> {
        let mut request =
            endpoint
                .into_client_request()
                .map_err(|error| ResponsesError::InvalidUrl {
                    detail: error.to_string(),
                })?;
        let authorization =
            HeaderValue::from_str(&format!("Bearer {}", auth.bearer())).map_err(|error| {
                ResponsesError::InvalidAuthorization {
                    detail: error.to_string(),
                }
            })?;
        request
            .headers_mut()
            .insert(header::AUTHORIZATION, authorization);
        if let Some(account_id) = auth.account_id() {
            request.headers_mut().insert(
                "ChatGPT-Account-ID",
                HeaderValue::from_str(account_id).map_err(|error| {
                    ResponsesError::InvalidAuthorization {
                        detail: error.to_string(),
                    }
                })?,
            );
        }
        if auth.is_fedramp() {
            request
                .headers_mut()
                .insert("X-OpenAI-Fedramp", HeaderValue::from_static("true"));
        }
        request.headers_mut().insert(
            "OpenAI-Beta",
            HeaderValue::from_static(RESPONSES_WEBSOCKETS_BETA),
        );
        request
            .headers_mut()
            .insert(RESPONSES_LITE_HEADER, HeaderValue::from_static("true"));
        request.headers_mut().insert(
            "session-id",
            HeaderValue::from_str(session_id).map_err(|error| {
                ResponsesError::InvalidSessionId {
                    detail: error.to_string(),
                }
            })?,
        );
        for name in ["thread-id", "x-client-request-id"] {
            request.headers_mut().insert(
                name,
                HeaderValue::from_str(thread_id).map_err(|error| {
                    ResponsesError::InvalidSessionId {
                        detail: error.to_string(),
                    }
                })?,
            );
        }
        if let Some(turn_state) = turn_state.and_then(|state| HeaderValue::from_str(state).ok()) {
            request.headers_mut().insert(TURN_STATE_HEADER, turn_state);
        }
        request.headers_mut().insert(
            "x-responsesapi-include-timing-metrics",
            HeaderValue::from_static("true"),
        );
        request.headers_mut().insert(
            header::USER_AGENT,
            HeaderValue::from_static(concat!("nanocode/", env!("CARGO_PKG_VERSION"))),
        );
        request.headers_mut().insert(
            header::SEC_WEBSOCKET_EXTENSIONS,
            HeaderValue::from_static(deflate::OFFER),
        );
        let (socket, response) = timeout(CONNECT_TIMEOUT, connect_deflate(request))
            .await
            .map_err(|_| ResponsesError::HandshakeTimeout {
                seconds: CONNECT_TIMEOUT.as_secs(),
            })?
            .map_err(map_handshake_error)?;
        let turn_state = header_string(response.headers(), TURN_STATE_HEADER);
        let metadata = ConnectionMetadata {
            status: response.status().as_u16(),
            request_id: header_string(response.headers(), "x-request-id"),
            server_model: header_string(response.headers(), "openai-model"),
            reasoning_included: response.headers().contains_key("x-reasoning-included"),
            turn_state: turn_state.clone(),
        };
        Ok((
            Self {
                pump: SocketPump::new(socket),
                turn_state,
                deflate: deflate::negotiated(response.headers()),
            },
            metadata,
        ))
    }

    pub(crate) async fn send(
        &self,
        request: EncodedRequest,
    ) -> Result<SendOutcome, ResponsesError> {
        let text = request.into_string();
        let message = if self.deflate {
            let payload =
                nanocode_platform::spawn_blocking(move || deflate::deflate(text.as_bytes()))
                    .await
                    .map_err(|error| deflate_error(&error))?
                    .map_err(|error| deflate_error(&error))?;
            nanocode_trace::Span::current().record("request.deflate_bytes", payload.len());
            let mut frame = Frame::message(payload, OpCode::Text, true);
            frame.header_mut().rsv1 = true;
            Message::Frame(frame)
        } else {
            Message::Text(text.into())
        };
        timeout(SEND_TIMEOUT, self.pump.send(message))
            .await
            .map_err(|_| ResponsesError::SendTimeout {
                seconds: SEND_TIMEOUT.as_secs(),
            })?
            .map_err(map_send_error)
    }

    pub(crate) fn is_closed(&self) -> bool {
        self.pump.commands.is_closed()
    }

    pub(crate) async fn next_text(&mut self) -> Result<ReceivedText, ResponsesError> {
        loop {
            let received = self
                .pump
                .next()
                .await
                .ok_or(ResponsesError::UnexpectedEnd)?;
            if let Some(text) = self.received_text(received)? {
                return Ok(text);
            }
        }
    }

    pub(crate) fn try_next_text(&mut self) -> Result<Option<ReceivedText>, ResponsesError> {
        while let Some(received) = self.pump.try_next() {
            if let Some(text) = self.received_text(received)? {
                return Ok(Some(text));
            }
        }
        Ok(None)
    }

    fn received_text(
        &mut self,
        received: PumpMessage,
    ) -> Result<Option<ReceivedText>, ResponsesError> {
        match received.message.map_err(map_receive_error)? {
            Message::Text(text) => {
                self.capture_turn_state(text.as_str());
                Ok(Some(ReceivedText {
                    text,
                    received_ns: received.received_ns,
                    retry_receipt: received.retry_receipt,
                }))
            }
            Message::Binary(_) => Err(ResponsesError::UnexpectedBinary),
            Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => Ok(None),
            Message::Close(frame) => {
                let detail = frame.map_or_else(
                    || "without a reason".to_owned(),
                    |frame| format!("with code {}: {}", frame.code, frame.reason),
                );
                Err(ResponsesError::Closed { detail })
            }
        }
    }

    #[must_use]
    pub(crate) fn turn_state(&self) -> Option<&str> {
        self.turn_state.as_deref()
    }

    pub(crate) fn reset_turn_state(&mut self) {
        self.turn_state = None;
    }

    fn capture_turn_state(&mut self, text: &str) {
        if self.turn_state.is_some() {
            return;
        }
        self.turn_state = turn_state_from_event(text);
    }
}

impl SocketPump {
    fn new(mut socket: Socket) -> Self {
        let (commands, mut command_receiver) = mpsc::channel(32);
        let (message_sender, messages) = mpsc::channel(SOCKET_BATCH_CAPACITY);
        let stopped = Arc::new(AtomicBool::new(false));
        let completion = Arc::clone(&stopped);
        let task = nanocode_platform::spawn_task(async move {
            loop {
                nanocode_sync::select! {
                    command = command_receiver.recv() => {
                        let Some(command) = command else {
                            break;
                        };
                        match command {
                            SocketCommand::Send { message, result } => {
                                let send_result = socket.send(message).await;
                                let should_stop = send_result.is_err();
                                let send_result = match send_result {
                                    Ok(()) => Ok(SendOutcome::Sent),
                                    Err(WebSocketError::AlreadyClosed | WebSocketError::Protocol(ProtocolError::SendAfterClosing)) => Ok(SendOutcome::Disconnected),
                                    Err(error) => Err(error),
                                };
                                drop(result.send(send_result));
                                if should_stop {
                                    break;
                                }
                            }
                        }
                    }
                    message = socket.next() => {
                        let Some(message) = message else {
                            break;
                        };
                        let received_ns = monotonic_now_ns();
                        let retry_receipt = super::RetryReceipt::now();
                        let mut messages = Vec::new();
                        let mut next = Some(message);
                        let mut should_stop = false;
                        while let Some(message) = next.take() {
                            match message {
                                Ok(Message::Ping(payload)) => {
                                    if let Err(error) = socket.send(Message::Pong(payload)).await {
                                        messages.push(Err(error));
                                        should_stop = true;
                                        break;
                                    }
                                }
                                Ok(Message::Pong(_)) => {}
                                Ok(message) => {
                                    should_stop = matches!(message, Message::Close(_));
                                    messages.push(Ok(message));
                                    if should_stop {
                                        break;
                                    }
                                }
                                Err(error) => {
                                    messages.push(Err(error));
                                    should_stop = true;
                                    break;
                                }
                            }
                            match socket.next().now_or_never() {
                                Some(Some(message)) => next = Some(message),
                                Some(None) => should_stop = true,
                                None => {}
                            }
                        }
                        let sent = messages.is_empty()
                            || message_sender
                                .send(PumpBatch {
                                    messages,
                                    received_ns,
                                    retry_receipt,
                                })
                                .await
                                .is_ok();
                        if !sent || should_stop {
                            break;
                        }
                    }
                }
            }
            completion.store(true, Ordering::Release);
        });
        Self {
            commands,
            messages,
            batch: None,
            task,
            stopped,
        }
    }

    async fn send(&self, message: Message) -> std::result::Result<SendOutcome, WebSocketError> {
        let (result, receiver) = oneshot::channel();
        if self
            .commands
            .send(SocketCommand::Send { message, result })
            .await
            .is_err()
        {
            return Ok(SendOutcome::Disconnected);
        }
        match receiver.await {
            Ok(result) => result,
            Err(_) if self.stopped.load(Ordering::Acquire) => Ok(SendOutcome::Disconnected),
            Err(_) => Err(WebSocketError::ConnectionClosed),
        }
    }

    async fn next(&mut self) -> Option<PumpMessage> {
        if let Some(message) = self.buffered() {
            return Some(message);
        }
        let batch = self.messages.recv().await?;
        self.install(batch);
        self.buffered()
    }

    fn try_next(&mut self) -> Option<PumpMessage> {
        if let Some(message) = self.buffered() {
            return Some(message);
        }
        let batch = self.messages.try_recv().ok()?;
        self.install(batch);
        self.buffered()
    }

    fn install(&mut self, batch: PumpBatch) {
        self.batch = Some(ReceivedBatch {
            messages: batch.messages.into_iter(),
            received_ns: batch.received_ns,
            retry_receipt: batch.retry_receipt,
        });
    }

    fn buffered(&mut self) -> Option<PumpMessage> {
        let batch = self.batch.as_mut()?;
        let message = batch.messages.next()?;
        Some(PumpMessage {
            message,
            received_ns: batch.received_ns,
            retry_receipt: batch.retry_receipt,
        })
    }
}

impl Drop for SocketPump {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn map_handshake_error(error: WebSocketError) -> ResponsesError {
    let WebSocketError::Http(response) = error else {
        return ResponsesError::Handshake {
            reconnectable: is_transient_websocket(&error),
            detail: error.to_string(),
        };
    };
    ResponsesError::HandshakeRejected {
        status: response.status().as_u16(),
        body: handshake_rejection_detail(response.body().as_deref(), response.headers()),
        retry_after: crate::client::RetryAfter::from_headers(response.headers()),
        should_retry: retry::should_retry(response.headers()),
    }
}

fn handshake_rejection_detail(body: Option<&[u8]>, headers: &http::HeaderMap) -> String {
    let body = body.map_or_else(String::new, |body| {
        String::from_utf8_lossy(body).into_owned()
    });
    if !body.trim().is_empty() {
        return body;
    }

    let request_id = header_string(headers, "x-request-id")
        .or_else(|| header_string(headers, "x-oai-request-id"));
    let diagnostics = [
        header_string(headers, "cf-ray").map(|value| format!("cf-ray={value}")),
        request_id.map(|value| format!("request-id={value}")),
        header_string(headers, "x-openai-authorization-error")
            .map(|value| format!("authorization-error={value}")),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();
    if diagnostics.is_empty() {
        "empty response body".to_owned()
    } else {
        format!("empty response body ({})", diagnostics.join(", "))
    }
}

fn deflate_error(error: &dyn std::error::Error) -> ResponsesError {
    ResponsesError::Send {
        reconnectable: false,
        detail: format!("deflate request: {error}"),
    }
}

fn map_send_error(error: WebSocketError) -> ResponsesError {
    ResponsesError::Send {
        reconnectable: is_transient_websocket(&error),
        detail: error.to_string(),
    }
}

fn map_receive_error(error: WebSocketError) -> ResponsesError {
    ResponsesError::Receive {
        reconnectable: is_transient_websocket(&error),
        detail: error.to_string(),
    }
}

const fn is_transient_websocket(error: &WebSocketError) -> bool {
    matches!(
        error,
        WebSocketError::ConnectionClosed
            | WebSocketError::AlreadyClosed
            | WebSocketError::Io(_)
            | WebSocketError::Protocol(
                ProtocolError::HandshakeIncomplete
                    | ProtocolError::ResetWithoutClosingHandshake
                    | ProtocolError::SendAfterClosing
            )
    )
}

fn header_string(headers: &http::HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
}
