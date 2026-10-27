use super::*;
use crate::{
    ToolOutput,
    contract::RunningTask,
    embedded::{HandCalls, OwnedToolContext},
    runtime::ToolRuntime,
};
use futures_util::{Sink, SinkExt, Stream, StreamExt};
use serde_json::Value;
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU16, AtomicU64, Ordering},
    },
    time::Duration,
};

use nanocode_sync::{Atom, Owned, Signal};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::{TcpListener, TcpStream},
    sync::{mpsc, watch},
};
use tokio_tungstenite::{
    WebSocketStream,
    tungstenite::{self, Message, client::IntoClientRequest},
};
use tokio_util::sync::{CancellationToken, DropGuard};

#[derive(Clone, Copy)]
pub struct SocketPolicy {
    pub max_frame_bytes: usize,
    pub request_timeout: Duration,
}

impl SocketPolicy {
    #[must_use]
    pub fn runner() -> Self {
        Self {
            max_frame_bytes: tungstenite::protocol::WebSocketConfig::default()
                .max_message_size
                .unwrap_or(usize::MAX),
            request_timeout: crate::attachment::HEARTBEAT_INTERVAL * 2,
        }
    }
}

#[derive(Clone)]
pub(crate) struct SocketShutdown {
    token: CancellationToken,
    reason: Arc<Atom<Option<&'static str>>>,
}

impl SocketShutdown {
    pub(crate) fn close(&self) {
        self.token.cancel();
    }

    pub(crate) fn reject(&self, reason: &'static str) {
        self.reason.set(Some(reason));
        self.token.cancel();
    }

    pub(crate) fn revoke(&self) {
        self.reject(nanocode_tool_host::attachment::ATTACHMENT_REVOKED_REASON);
    }
}

use super::RemoteFrame as Frame;

pub struct SocketTransport {
    client: RemoteClient,
    closed: Arc<watch::Sender<Option<String>>>,
    definitions: Vec<ToolDefinition>,
    name: Option<String>,
    workspace: Option<String>,
    runtime_id: Option<String>,
    shutdown: SocketShutdown,
    _shutdown: DropGuard,
}

pub(super) fn error(value: impl std::fmt::Display) -> CodeModeHostError {
    CodeModeHostError::new(value.to_string())
}

pub(super) fn validate_policy(policy: SocketPolicy) -> Result<(), CodeModeHostError> {
    if policy.max_frame_bytes == 0 || policy.request_timeout.is_zero() {
        return Err(error(
            "max_frame_bytes and request_timeout must be positive",
        ));
    }
    if tokio::time::Instant::now()
        .checked_add(policy.request_timeout)
        .is_none()
    {
        return Err(error("request_timeout exceeds the clock range"));
    }
    if (policy.request_timeout / 2).is_zero() {
        return Err(error(
            "request_timeout must be at least 2ns so the heartbeat can ping every half request_timeout",
        ));
    }
    Ok(())
}

struct Liveness {
    origin: tokio::time::Instant,
    last_inbound_us: AtomicU64,
    probe_us: AtomicU64,
    /// Fired by the first text frame or ping from the peer: a pong may answer
    /// our own ping before the peer took the connection, but a peer only
    /// sends frames or pings once it serves the socket.
    heard: Signal,
    /// The code of the peer's close frame, 0 before one arrives.
    close_code: AtomicU16,
}

impl Liveness {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            origin: tokio::time::Instant::now(),
            last_inbound_us: AtomicU64::new(0),
            probe_us: AtomicU64::new(0),
            heard: Signal::new(),
            close_code: AtomicU16::new(0),
        })
    }
    fn close_code(&self) -> Option<u16> {
        Some(self.close_code.load(Ordering::Relaxed)).filter(|code| *code != 0)
    }
    fn elapsed_us(&self) -> u64 {
        u64::try_from(self.origin.elapsed().as_micros()).unwrap_or(u64::MAX)
    }
    fn touch(&self) {
        self.last_inbound_us
            .store(self.elapsed_us(), Ordering::Relaxed);
    }
    fn probe_sent(&self, started_us: u64) {
        if self.probe_us.load(Ordering::Relaxed) <= self.last_inbound_us.load(Ordering::Relaxed) {
            self.probe_us.store(started_us, Ordering::Relaxed);
        }
    }
    fn unanswered_probe_age(&self) -> Option<Duration> {
        let probe = self.probe_us.load(Ordering::Relaxed);
        (probe > self.last_inbound_us.load(Ordering::Relaxed))
            .then(|| Duration::from_micros(self.elapsed_us().saturating_sub(probe)))
    }
    async fn watch(&self, window: Duration) {
        loop {
            match self.unanswered_probe_age() {
                Some(age) if age >= window => return,
                Some(age) => tokio::time::sleep(window - age).await,
                None => tokio::time::sleep(window).await,
            }
        }
    }
}

const UNRESPONSIVE: &str =
    "remote peer did not answer a WebSocket heartbeat ping within half of request_timeout";

async fn read_live<S: Stream<Item = Result<Message, tungstenite::Error>> + Unpin>(
    socket: &mut S,
    liveness: &Liveness,
    watchdog: std::pin::Pin<&mut impl std::future::Future<Output = ()>>,
) -> Result<Option<Frame>, CodeModeHostError> {
    tokio::select! {
        biased;
        frame = read(socket, liveness) => frame,
        () = watchdog => Err(error(UNRESPONSIVE)),
    }
}

const READ_BUFFER_BYTES: usize = 16 * 1024;

fn config(policy: SocketPolicy) -> tungstenite::protocol::WebSocketConfig {
    tungstenite::protocol::WebSocketConfig::default()
        .read_buffer_size(READ_BUFFER_BYTES)
        .max_message_size(Some(policy.max_frame_bytes))
        .max_frame_size(Some(policy.max_frame_bytes))
}

type DialedSocket = WebSocketStream<tokio_tungstenite::MaybeTlsStream<TcpStream>>;

pub(super) enum Dial {
    Rejected(CodeModeHostError, Option<u16>),
    Failed(CodeModeHostError),
}

async fn connect(
    target: crate::attachment::AttachmentTarget,
    policy: SocketPolicy,
) -> Result<DialedSocket, CodeModeHostError> {
    dial(&target, &http::HeaderMap::new(), policy)
        .await
        .map_err(|failure| match failure {
            Dial::Rejected(error, _) | Dial::Failed(error) => error,
        })
}

pub(super) fn request_headers(
    pairs: &[(String, String)],
) -> Result<http::HeaderMap, CodeModeHostError> {
    let mut headers = http::HeaderMap::new();
    for (name, value) in pairs {
        let header = http::HeaderName::from_bytes(name.as_bytes())
            .map_err(|issue| error(format!("invalid hand header name {name:?}: {issue}")))?;
        let reserved = matches!(
            header,
            http::header::AUTHORIZATION
                | http::header::CONNECTION
                | http::header::HOST
                | http::header::UPGRADE
        ) || header.as_str().starts_with("sec-websocket-");
        if reserved {
            return Err(error(format!(
                "hand header {name:?} is set by the WebSocket upgrade or the runner credential"
            )));
        }
        let value = http::HeaderValue::from_str(value)
            .map_err(|issue| error(format!("invalid value for hand header {name:?}: {issue}")))?;
        headers.append(header, value);
    }
    Ok(headers)
}

pub(super) async fn dial(
    target: &crate::attachment::AttachmentTarget,
    headers: &http::HeaderMap,
    policy: SocketPolicy,
) -> Result<DialedSocket, Dial> {
    validate_policy(policy).map_err(|issue| Dial::Rejected(issue, None))?;
    nanocode_tls::install_default_rustls_crypto_provider();
    let mut request = target
        .endpoint()
        .as_str()
        .into_client_request()
        .map_err(|issue| Dial::Rejected(error(issue), None))?;
    let mut bearer = http::HeaderValue::from_str(&format!("Bearer {}", target.bearer()))
        .map_err(|issue| Dial::Rejected(error(issue), None))?;
    bearer.set_sensitive(true);
    for (name, value) in headers {
        request.headers_mut().append(name, value.clone());
    }
    request
        .headers_mut()
        .insert(http::header::AUTHORIZATION, bearer);
    let connector = if request.uri().scheme_str() == Some("wss") {
        Some(tokio_tungstenite::Connector::Rustls(
            nanocode_tls::native_client_config()
                .await
                .map_err(|issue| Dial::Failed(error(issue)))?,
        ))
    } else {
        None
    };
    tokio::time::timeout(
        policy.request_timeout,
        tokio_tungstenite::connect_async_tls_with_config(
            request,
            Some(config(policy)),
            true,
            connector,
        ),
    )
    .await
    .map_err(|issue| Dial::Failed(error(issue)))?
    .map(|(socket, _)| socket)
    .map_err(|issue| match issue {
        tungstenite::Error::Http(response)
            if matches!(response.status().as_u16(), 401 | 403 | 410 | 426) =>
        {
            Dial::Rejected(
                error(format!(
                    "brain rejected the runner credential with HTTP {}",
                    response.status()
                )),
                Some(response.status().as_u16()),
            )
        }
        issue => Dial::Failed(error(issue)),
    })
}

async fn accept(
    listener: &TcpListener,
    bearer: &str,
    policy: SocketPolicy,
) -> Result<WebSocketStream<TcpStream>, CodeModeHostError> {
    validate_policy(policy)?;
    if !listener.local_addr().map_err(error)?.ip().is_loopback() {
        return Err(error(
            "plaintext runner listeners require loopback; use a TLS proxy for remote access",
        ));
    }
    let (stream, _) = listener.accept().await.map_err(error)?;
    handshake(stream, bearer, policy).await
}

async fn handshake(
    stream: TcpStream,
    bearer: &str,
    policy: SocketPolicy,
) -> Result<WebSocketStream<TcpStream>, CodeModeHostError> {
    validate_policy(policy)?;
    if bearer.is_empty() {
        return Err(error("runner bearer must not be empty"));
    }
    stream.set_nodelay(true).map_err(error)?;
    let authenticate =
        |request: &tungstenite::handshake::server::Request,
         mut response: tungstenite::handshake::server::Response| {
            let header = request
                .headers()
                .get(http::header::AUTHORIZATION)
                .and_then(|h| h.to_str().ok())
                .and_then(|h| h.strip_prefix("Bearer "));
            let protocol = request
                .headers()
                .get(http::header::SEC_WEBSOCKET_PROTOCOL)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| {
                    let mut values = value.split(',').map(str::trim);
                    (values.next() == Some("bearer"))
                        .then(|| values.next())
                        .flatten()
                });
            if header.or(protocol) == Some(bearer) {
                if protocol.is_some() {
                    response.headers_mut().insert(
                        http::header::SEC_WEBSOCKET_PROTOCOL,
                        http::HeaderValue::from_static("bearer"),
                    );
                }
                Ok(response)
            } else {
                let mut rejection = tungstenite::handshake::server::ErrorResponse::new(Some(
                    "invalid runner credential".into(),
                ));
                *rejection.status_mut() = http::StatusCode::UNAUTHORIZED;
                Err(rejection)
            }
        };
    tokio::time::timeout(
        policy.request_timeout,
        tokio_tungstenite::accept_hdr_async_with_config(stream, authenticate, Some(config(policy))),
    )
    .await
    .map_err(error)?
    .map_err(error)
}

async fn read<S: Stream<Item = Result<Message, tungstenite::Error>> + Unpin>(
    socket: &mut S,
    liveness: &Liveness,
) -> Result<Option<Frame>, CodeModeHostError> {
    while let Some(message) = socket.next().await {
        let message = message.map_err(error)?;
        liveness.touch();
        match message {
            Message::Text(text) => {
                liveness.heard.fire();
                return serde_json::from_str(&text).map(Some).map_err(error);
            }
            Message::Close(frame) => {
                if let Some(frame) = &frame {
                    liveness
                        .close_code
                        .store(u16::from(frame.code), Ordering::Relaxed);
                }
                return match frame.filter(|frame| !frame.reason.is_empty()) {
                    Some(frame) => Err(error(frame.reason.as_str())),
                    None => Ok(None),
                };
            }
            Message::Ping(_) => liveness.heard.fire(),
            Message::Pong(_) => {}
            _ => return Err(error("remote code mode requires JSON text frames")),
        }
    }
    Ok(None)
}

async fn write<S: Sink<Message, Error = tungstenite::Error> + Unpin>(
    socket: &mut S,
    text: String,
    policy: SocketPolicy,
) -> Result<(), CodeModeHostError> {
    tokio::time::timeout(policy.request_timeout, socket.send(Message::text(text)))
        .await
        .map_err(|_| error("remote write deadline elapsed"))?
        .map_err(error)
}

async fn write_all<S: Sink<Message, Error = tungstenite::Error> + Unpin>(
    socket: &mut S,
    frames: Vec<String>,
    policy: SocketPolicy,
) -> Result<(), CodeModeHostError> {
    tokio::time::timeout(policy.request_timeout, async {
        for frame in frames {
            socket.feed(Message::text(frame)).await?;
        }
        socket.flush().await
    })
    .await
    .map_err(|_| error("remote write deadline elapsed"))?
    .map_err(error)
}

struct Writer(tokio::task::JoinHandle<Result<(), CodeModeHostError>>);
impl Drop for Writer {
    fn drop(&mut self) {
        self.0.abort();
    }
}
fn writer<S: Sink<Message, Error = tungstenite::Error> + Unpin + Send + 'static>(
    mut sink: S,
    policy: SocketPolicy,
    liveness: Arc<Liveness>,
    reason: Arc<Atom<Option<&'static str>>>,
) -> (mpsc::UnboundedSender<String>, Writer) {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let task = tokio::spawn(async move {
        let period = policy.request_timeout / 2;
        let mut heartbeat = tokio::time::interval_at(tokio::time::Instant::now() + period, period);
        heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                frame = rx.recv() => match frame {
                    Some(frame) => {
                        let mut frames = vec![frame];
                        while let Ok(frame) = rx.try_recv() {
                            frames.push(frame);
                        }
                        write_all(&mut sink, frames, policy).await?;
                    }
                    None => {
                        let reason = *reason.get();
                        let close = reason.map(|reason| tungstenite::protocol::CloseFrame {
                            code: tungstenite::protocol::frame::coding::CloseCode::Policy,
                            reason: reason.into(),
                        });
                        tokio::time::timeout(policy.request_timeout, sink.send(Message::Close(close)))
                            .await
                            .map_err(|_| error("remote write deadline elapsed"))?
                            .map_err(error)?;
                        return Ok(());
                    }
                },
                _ = heartbeat.tick() => {
                    let started_us = liveness.elapsed_us();
                    tokio::time::timeout(policy.request_timeout, sink.send(Message::Ping(Default::default())))
                        .await
                        .map_err(|_| error("remote write deadline elapsed"))?
                        .map_err(error)?;
                    liveness.probe_sent(started_us);
                }
            }
        }
    });
    (tx, Writer(task))
}

impl SocketTransport {
    pub async fn connect(
        target: crate::attachment::AttachmentTarget,
        policy: SocketPolicy,
    ) -> Result<Self, CodeModeHostError> {
        Self::start(
            connect(target, policy).await?,
            None,
            policy,
            Some(policy.request_timeout),
        )
        .await
    }
    pub async fn accept(
        listener: &TcpListener,
        bearer: &str,
        policy: SocketPolicy,
    ) -> Result<Self, CodeModeHostError> {
        Self::start(
            accept(listener, bearer, policy).await?,
            None,
            policy,
            Some(policy.request_timeout),
        )
        .await
    }
    pub(crate) async fn established<S: AsyncRead + AsyncWrite + Unpin + Send + 'static>(
        socket: WebSocketStream<S>,
        catalog: String,
        policy: SocketPolicy,
        operation_timeout: Option<Duration>,
    ) -> Result<Self, CodeModeHostError> {
        validate_policy(policy)?;
        Self::start(socket, Some(catalog), policy, operation_timeout).await
    }
    pub fn definitions(&self) -> Vec<ToolDefinition> {
        self.definitions.clone()
    }
    pub(crate) fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }
    pub(crate) fn workspace(&self) -> Option<&str> {
        self.workspace.as_deref()
    }
    pub(crate) fn closed(&self) -> watch::Receiver<Option<String>> {
        self.closed.subscribe()
    }
    pub(crate) fn runtime_id(&self) -> Option<&str> {
        self.runtime_id.as_deref()
    }
    pub(crate) fn shutdown_handle(&self) -> SocketShutdown {
        self.shutdown.clone()
    }
    pub fn into_host(self) -> RemoteCodeModeHost {
        let definitions = self.definitions();
        RemoteCodeModeHost::new(self, definitions)
    }

    async fn start<S: AsyncRead + AsyncWrite + Unpin + Send + 'static>(
        mut socket: WebSocketStream<S>,
        catalog: Option<String>,
        policy: SocketPolicy,
        operation_timeout: Option<Duration>,
    ) -> Result<Self, CodeModeHostError> {
        let liveness = Liveness::new();
        let RemoteCatalog {
            definitions,
            name,
            workspace,
            runtime_id,
            max_frame_bytes,
            resumable: _,
            tool_selection,
            cancels_calls: _,
            caches_local_tools,
        } = match catalog {
            Some(text) => RemoteCatalog::decode(&text, policy.max_frame_bytes)?,
            None => tokio::time::timeout(policy.request_timeout, read(&mut socket, &liveness))
                .await
                .map_err(error)??
                .ok_or_else(|| error(NO_CATALOG))?
                .try_into()?,
        };
        crate::host::validate_public_tool_catalog_names(
            definitions.iter().map(ToolDefinition::name),
        )
        .map_err(|issue| {
            error(format!(
                "runner published an invalid code mode catalog: {issue}"
            ))
        })?;
        let (sink, socket) = socket.split();
        let shutdown = SocketShutdown {
            token: CancellationToken::new(),
            reason: Arc::default(),
        };
        let (wire, writer) = writer(
            sink,
            policy,
            Arc::clone(&liveness),
            Arc::clone(&shutdown.reason),
        );
        let (client, driver) = RemoteClient::new(
            SocketLink {
                reader: Owned::new(SocketReader {
                    socket,
                    writer: Some(writer),
                }),
                wire: Atom::new(Some(wire)),
                liveness,
                window: policy.request_timeout / 2,
                stopped: shutdown.token.clone(),
                tool_selection,
                caches_local_tools,
                max_frame_bytes: max_frame_bytes.map_or(policy.max_frame_bytes, |peer| {
                    peer.min(policy.max_frame_bytes)
                }),
            },
            operation_timeout,
        );
        let closed = Arc::new(watch::channel(None).0);
        let published = Arc::clone(&closed);
        tokio::spawn(async move {
            let message = driver.await;
            published.send_replace(Some(message));
        });
        Ok(Self {
            client,
            closed,
            definitions,
            name,
            workspace,
            runtime_id,
            _shutdown: shutdown.token.clone().drop_guard(),
            shutdown,
        })
    }
}

struct SocketReader<S> {
    socket: futures_util::stream::SplitStream<WebSocketStream<S>>,
    writer: Option<Writer>,
}

struct SocketLink<S> {
    reader: Owned<SocketReader<S>>,
    wire: Atom<Option<mpsc::UnboundedSender<String>>>,
    liveness: Arc<Liveness>,
    window: Duration,
    stopped: CancellationToken,
    max_frame_bytes: usize,
    tool_selection: bool,
    caches_local_tools: bool,
}

enum Received {
    Stopped,
    Written(Result<Result<(), CodeModeHostError>, tokio::task::JoinError>),
    Frame(Result<Option<Frame>, CodeModeHostError>),
    Unresponsive,
}

impl<S: AsyncRead + AsyncWrite + Unpin + Send + 'static> RemoteLink for SocketLink<S> {
    fn supports_tool_selection(&self) -> bool {
        self.tool_selection
    }
    fn caches_local_tools(&self) -> bool {
        self.caches_local_tools
    }
    fn send(
        &self,
        frames: Vec<Frame>,
        delivery: RemoteDelivery,
    ) -> HostFuture<'_, Result<(), CodeModeHostError>> {
        self.send_encoded(
            frames.into_iter().map(|frame| (frame, None)).collect(),
            delivery,
        )
    }

    fn send_encoded(
        &self,
        frames: Vec<(Frame, Option<String>)>,
        _delivery: RemoteDelivery,
    ) -> HostFuture<'_, Result<(), CodeModeHostError>> {
        let sent = frames
            .into_iter()
            .map(|(frame, text)| text.map_or_else(|| frame.encode(self.max_frame_bytes), Ok))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|issue| CodeModeHostError::unsent(issue.message()))
            .and_then(|texts| match &*self.wire.get() {
                Some(wire) => texts.into_iter().enumerate().try_for_each(|(index, text)| {
                    wire.send(text).map_err(|issue| {
                        if index == 0 {
                            CodeModeHostError::unsent(issue.to_string())
                        } else {
                            error(issue)
                        }
                    })
                }),
                None => Err(CodeModeHostError::unsent(LOCAL_CLOSED)),
            });
        Box::pin(std::future::ready(sent))
    }

    fn max_frame_bytes(&self) -> Option<usize> {
        Some(self.max_frame_bytes)
    }

    fn receive(&self) -> HostFuture<'_, Result<Vec<Frame>, CodeModeHostError>> {
        Box::pin(async move {
            let mut reader = self.reader.acquire().await;
            let SocketReader { socket, writer } = &mut *reader;
            let Some(task) = writer.as_mut() else {
                return Err(error(LOCAL_CLOSED));
            };
            let received = tokio::select! {
                biased;
                () = self.stopped.cancelled() => Received::Stopped,
                result = &mut task.0 => Received::Written(result),
                frame = read(socket, &self.liveness) => Received::Frame(frame),
                () = self.liveness.watch(self.window) => Received::Unresponsive,
            };
            match received {
                Received::Stopped => Err(error(LOCAL_CLOSED)),
                Received::Written(result) => {
                    *writer = None;
                    Err(match result {
                        Ok(Ok(())) => error(LOCAL_CLOSED),
                        Ok(Err(issue)) => issue,
                        Err(issue) => error(issue),
                    })
                }
                Received::Frame(Ok(Some(frame))) => Ok(vec![frame]),
                Received::Frame(Ok(None)) => Err(error(PEER_CLOSED)),
                Received::Frame(Err(issue)) => {
                    Err(error(format!("runner connection failed: {issue}")))
                }
                Received::Unresponsive => {
                    Err(error(format!("runner connection failed: {UNRESPONSIVE}")))
                }
            }
        })
    }

    fn close<'a>(&'a self, _reason: &'a str) -> HostFuture<'a, ()> {
        Box::pin(async move {
            self.wire.set(None);
            if let Some(mut writer) = self.reader.acquire().await.writer.take() {
                let _ = (&mut writer.0).await;
            }
        })
    }
}

impl RemoteTransport for SocketTransport {
    fn supports_tool_selection(&self) -> bool {
        self.client.supports_tool_selection()
    }
    fn request<'a>(
        &'a self,
        operation: RemoteOperation,
        tools: Vec<Arc<dyn Tool>>,
        observer: Option<&'a mut dyn CodeModeObserver>,
    ) -> HostFuture<'a, Result<Option<CodeModeExecution>, CodeModeHostError>> {
        self.client.request(operation, tools, observer)
    }
    fn call(&self, operation: RemoteOperation) -> HostFuture<'_, Result<Value, CodeModeHostError>> {
        self.client.call(operation)
    }
    fn route_hands(&self, hands: Arc<dyn HandCalls>) {
        self.client.route_hands(hands);
    }
    fn notify(&self, operation: RemoteOperation) -> Result<(), CodeModeHostError> {
        self.client.notify(operation)
    }
    fn detach_turn(
        &self,
        session_id: &str,
        factory: &mut (dyn FnMut(&str) -> Box<dyn CodeModeObserver> + Send),
    ) -> Result<(), CodeModeHostError> {
        self.client.detach_turn(session_id, factory)
    }
}

struct NativeHost {
    workspace: PathBuf,
    tools: crate::Tools,
}

impl HandHost for NativeHost {
    fn definitions(&self) -> HostFuture<'_, Vec<ToolDefinition>> {
        Box::pin(async move {
            let runtime =
                ToolRuntime::new_with_tools(self.workspace.clone(), Vec::new(), &self.tools);
            let definitions = runtime.registered_code_mode_definitions();
            runtime.control().cancel().await;
            definitions
        })
    }

    fn workspace(&self) -> Option<String> {
        Some(self.workspace.display().to_string())
    }

    fn selects_tools(&self) -> bool {
        !self.tools.has_dynamic_providers()
    }

    fn open(
        &self,
        _session_id: &str,
        excluded: Vec<String>,
        brain: HandBrain,
    ) -> Result<Arc<dyn HandSession>, CodeModeHostError> {
        let hands = brain.hand_calls();
        let tools = self
            .tools
            .clone()
            .excluding_code_tools(excluded)
            .into_builder()
            .provider(BrainProvider(brain))
            .hand_calls(hands)
            .build()
            .map_err(error)?;
        Ok(Arc::new(NativeSession(Arc::new(
            ToolRuntime::new_with_tools(self.workspace.clone(), Vec::new(), &tools),
        ))))
    }

    fn end_turn<'a>(
        &'a self,
        session_id: &'a str,
        turn_id: &'a str,
        hook_event_name: &'a str,
    ) -> HostFuture<'a, ()> {
        let tools: Vec<Arc<dyn crate::Tool>> = self.tools.registered_tools().cloned().collect();
        Box::pin(async move {
            for tool in tools {
                if let Err(issue) = tool.end_turn(session_id, turn_id, hook_event_name).await {
                    tracing::warn!(%issue, tool = tool.definition().name(), "hand tool turn cleanup failed; not retried");
                }
            }
        })
    }

    fn spawn(&self, task: HostFuture<'static, ()>) {
        tokio::spawn(task);
    }
}

struct NativeSession(Arc<ToolRuntime>);

impl HandSession for NativeSession {
    fn execute<'a>(
        &'a self,
        source: &'a str,
        context: OwnedToolContext,
        observer: &'a mut dyn CodeModeObserver,
    ) -> HostFuture<'a, Result<CodeModeExecution, CodeModeHostError>> {
        Box::pin(
            self.0
                .execute_code_owned_with_updates(source, context, observer),
        )
    }

    fn wait<'a>(
        &'a self,
        input: &'a str,
        context: OwnedToolContext,
        observer: &'a mut dyn CodeModeObserver,
    ) -> HostFuture<'a, Result<CodeModeExecution, CodeModeHostError>> {
        Box::pin(async move {
            self.0
                .wait_for_code_with_updates(input, context.as_context(), observer)
                .await
        })
    }

    fn call<'a>(
        &'a self,
        name: &'a str,
        input: Value,
        context: OwnedToolContext,
    ) -> HostFuture<'a, ToolOutput> {
        Box::pin(async move {
            self.0
                .execute_nested_tool(name, input, context.as_context())
                .await
        })
    }

    fn cancel<'a>(&'a self, observer: &'a mut dyn CodeModeObserver) -> HostFuture<'a, ()> {
        Box::pin(async move { self.0.control().cancel_with_updates(observer).await })
    }

    fn cancel_turn<'a>(&'a self, observer: &'a mut dyn CodeModeObserver) -> HostFuture<'a, ()> {
        Box::pin(async move { self.0.control().cancel_turn_with_updates(observer).await })
    }

    fn detach_turn<'a>(&'a self, observers: DetachedObservers<'a>) -> HostFuture<'a, ()> {
        Box::pin(async move { self.0.control().detach_turn_with_updates(observers).await })
    }

    fn begin_turn(&self) {
        self.0.control().begin_turn();
    }

    fn preempt(&self) -> HostFuture<'_, ()> {
        Box::pin(async move { self.0.control().preempt_turn().await })
    }

    fn settle_preempt(&self) {
        self.0.control().settle_preempt();
    }

    fn running_tasks(&self) -> Vec<RunningTask> {
        self.0.running_tasks()
    }

    fn close(&self) -> HostFuture<'_, ()> {
        Box::pin(async move { self.0.control().cancel().await })
    }
}

struct BrainProvider(HandBrain);

#[async_trait::async_trait]
impl crate::runtime::DynamicToolProvider for BrainProvider {
    fn start(&self) {}
    fn direct_tools(&self) -> Vec<Arc<dyn Tool>> {
        Vec::new()
    }
    fn available_definitions(&self) -> Vec<ToolDefinition> {
        self.0.definitions()
    }
    async fn execute(
        &self,
        name: &str,
        input: Value,
        context: ToolContext<'_>,
    ) -> Option<ToolOutput> {
        self.0.execute(name, input, context).await
    }
}

pub(super) fn hand_core(
    workspace: PathBuf,
    tools: crate::Tools,
    options: ExecutorOptions,
    activity: HandActivity,
) -> HandCore {
    let (core, driver) =
        HandCore::new(Arc::new(NativeHost { workspace, tools }), options, activity);
    tokio::spawn(driver);
    core
}

pub(super) fn serving(tools: &crate::Tools) -> HandActivity {
    HandActivity::new(tools.registered_tools().map(Arc::downgrade).collect())
}

const fn policy_options(policy: SocketPolicy) -> ExecutorOptions {
    ExecutorOptions {
        name: None,
        runtime_id: None,
        callback_timeout: Some(policy.request_timeout),
        callback_hold: None,
    }
}
pub async fn serve_connect(
    target: crate::attachment::AttachmentTarget,
    workspace: &Path,
    tools: crate::Tools,
    policy: SocketPolicy,
) -> Result<(), CodeModeHostError> {
    serve_connect_with(target, workspace, tools, policy, policy_options(policy)).await
}
pub async fn serve_accept(
    listener: &TcpListener,
    bearer: &str,
    workspace: &Path,
    tools: crate::Tools,
    policy: SocketPolicy,
) -> Result<(), CodeModeHostError> {
    let socket = accept(listener, bearer, policy).await?;
    serve_once(
        socket,
        workspace,
        tools,
        policy,
        policy_options(policy),
        HandActivity::default(),
    )
    .await
}
pub async fn serve_connect_with(
    target: crate::attachment::AttachmentTarget,
    workspace: &Path,
    tools: crate::Tools,
    policy: SocketPolicy,
    options: ExecutorOptions,
) -> Result<(), CodeModeHostError> {
    let socket = connect(target, policy).await?;
    serve_once(
        socket,
        workspace,
        tools,
        policy,
        options,
        HandActivity::default(),
    )
    .await
}
pub async fn serve_stream(
    stream: TcpStream,
    bearer: &str,
    workspace: &Path,
    tools: crate::Tools,
    policy: SocketPolicy,
    options: ExecutorOptions,
) -> Result<(), CodeModeHostError> {
    serve_tracked_stream(
        stream,
        bearer,
        workspace,
        tools,
        policy,
        options,
        HandActivity::default(),
    )
    .await
}
pub(super) async fn serve_tracked_stream(
    stream: TcpStream,
    bearer: &str,
    workspace: &Path,
    tools: crate::Tools,
    policy: SocketPolicy,
    options: ExecutorOptions,
    activity: HandActivity,
) -> Result<(), CodeModeHostError> {
    let socket = handshake(stream, bearer, policy).await?;
    serve_once(socket, workspace, tools, policy, options, activity).await
}

async fn serve_once<S: AsyncRead + AsyncWrite + Unpin + Send + 'static>(
    socket: WebSocketStream<S>,
    workspace: &Path,
    tools: crate::Tools,
    policy: SocketPolicy,
    options: ExecutorOptions,
    activity: HandActivity,
) -> Result<(), CodeModeHostError> {
    let core = hand_core(workspace.to_owned(), tools, options, activity);
    let served = serve(socket, &core, policy, || {}).await;
    core.reset().await;
    served.result
}

/// How a served connection ended: its result and the code of the peer's
/// close frame, if one arrived.
pub(super) struct Served {
    pub(super) result: Result<(), CodeModeHostError>,
    pub(super) close_code: Option<u16>,
}

/// Serves one connection. `heard` runs once the peer first sends a frame or
/// a ping, which a brain does only after it took the connection.
pub(super) async fn serve<S: AsyncRead + AsyncWrite + Unpin + Send + 'static>(
    socket: WebSocketStream<S>,
    core: &HandCore,
    policy: SocketPolicy,
    heard: impl FnOnce() + Send,
) -> Served {
    let liveness = Liveness::new();
    let result = serve_link(socket, core, policy, &liveness, heard).await;
    Served {
        result,
        close_code: liveness.close_code(),
    }
}

async fn serve_link<S: AsyncRead + AsyncWrite + Unpin + Send + 'static>(
    mut socket: WebSocketStream<S>,
    core: &HandCore,
    policy: SocketPolicy,
    liveness: &Arc<Liveness>,
    heard: impl FnOnce() + Send,
) -> Result<(), CodeModeHostError> {
    let mut heard = Some(heard);
    write(
        &mut socket,
        core.catalog(policy.max_frame_bytes).await?,
        policy,
    )
    .await?;
    let (sink, mut socket) = socket.split();
    let watchdog = liveness.watch(policy.request_timeout / 2);
    tokio::pin!(watchdog);
    let (wire, mut writer) = writer(sink, policy, Arc::clone(liveness), Arc::default());
    let mut connection = core.connect(wire, policy.max_frame_bytes);
    let mut open = true;
    let result = async {
        loop {
            tokio::select! {
                result = &mut writer.0 => return result.map_err(error)?,
                () = liveness.heard.wait(), if heard.is_some() => {
                    if let Some(heard) = heard.take() {
                        heard();
                    }
                },
                failed = &mut connection.failed, if open => match failed {
                    Ok(issue) => return Err(issue),
                    Err(_) => open = false,
                },
                frame = read_live(&mut socket, liveness, watchdog.as_mut()) => {
                    let Some(frame) = frame? else { return Ok(()); };
                    core.receive(connection.id, frame);
                },
            }
        }
    }
    .await;
    core.closed(connection.id).await;
    result
}
