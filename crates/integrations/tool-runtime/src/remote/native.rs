use super::*;
use crate::{
    ToolInput, ToolOutput,
    contract::{RunningTask, ToolError, ToolResult},
    embedded::{CodeModeUpdate, HandCalls},
    runtime::ToolRuntime,
};
use futures_util::{Sink, SinkExt, Stream, StreamExt};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::{Path, PathBuf},
    sync::{
        Arc, Weak,
        atomic::{AtomicBool, AtomicU16, AtomicU64, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use nanocode_process::LastActivity;
use nanocode_sync::{Atom, Owned, Signal, Slot};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::{TcpListener, TcpStream},
    sync::{mpsc, oneshot, watch},
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

#[derive(Clone, Default)]
pub struct ExecutorOptions {
    pub name: Option<String>,
    pub runtime_id: Option<String>,
    pub callback_timeout: Option<Duration>,
    pub callback_hold: Option<Duration>,
}

const DEFAULT_CALLBACK_HOLD: Duration = Duration::from_secs(60);
const MAX_BUFFERED_FRAMES: usize = 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BusyKind {
    Cell,
    Call,
    Task,
    Request,
    Reply,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BusyItem {
    pub kind: BusyKind,
    pub id: String,
    pub label: String,
    pub session_id: Option<String>,
    pub silent_for: Duration,
}

impl BusyItem {
    fn task(task: RunningTask, session_id: Option<&str>) -> Self {
        Self {
            kind: BusyKind::Task,
            id: task.id,
            label: task.label,
            session_id: session_id.map(str::to_owned),
            silent_for: task.silent_for,
        }
    }
}

#[derive(Clone)]
pub struct HandActivity {
    shared: Arc<ActivityShared>,
}

struct ActivityShared {
    draining: AtomicBool,
    busy: watch::Sender<usize>,
    tools: Vec<Weak<dyn Tool>>,
    boards: Atom<Vec<Arc<Board>>>,
}

impl Default for HandActivity {
    fn default() -> Self {
        Self::new(Vec::new())
    }
}

impl HandActivity {
    pub(super) fn serving(tools: &crate::Tools) -> Self {
        Self::new(tools.registered_tools().map(Arc::downgrade).collect())
    }

    fn new(tools: Vec<Weak<dyn Tool>>) -> Self {
        Self {
            shared: Arc::new(ActivityShared {
                draining: AtomicBool::new(false),
                busy: watch::channel(0).0,
                tools,
                boards: Atom::default(),
            }),
        }
    }

    #[must_use]
    pub fn busy(&self) -> bool {
        *self.shared.busy.borrow() > 0
    }

    #[must_use]
    pub fn items(&self) -> Vec<BusyItem> {
        let mut items = Vec::new();
        for board in self.shared.boards.get().iter() {
            board.items(&mut items);
        }
        for tool in self.shared.tools.iter().filter_map(Weak::upgrade) {
            items.extend(
                tool.running_tasks()
                    .into_iter()
                    .map(|task| BusyItem::task(task, None)),
            );
        }
        items
    }

    pub async fn idle(&self) {
        let _ = self
            .shared
            .busy
            .subscribe()
            .wait_for(|busy| *busy == 0)
            .await;
    }

    pub fn drain(&self) {
        self.shared.draining.store(true, Ordering::Release);
    }

    pub fn undrain(&self) {
        self.shared.draining.store(false, Ordering::Release);
    }

    fn draining(&self) -> bool {
        self.shared.draining.load(Ordering::Acquire)
    }

    fn attach(&self) -> Arc<Board> {
        let board = Arc::<Board>::default();
        self.shared
            .boards
            .update(|boards| boards.push(Arc::clone(&board)));
        board
    }

    fn detach(&self, board: &Arc<Board>) {
        self.shared
            .boards
            .update(|boards| boards.retain(|other| !Arc::ptr_eq(other, board)));
    }

    fn report(&self, was: bool, now: bool) {
        if was != now {
            self.shared.busy.send_modify(|busy| {
                if now {
                    *busy += 1;
                } else {
                    *busy -= 1;
                }
            });
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

fn config(policy: SocketPolicy) -> tungstenite::protocol::WebSocketConfig {
    tungstenite::protocol::WebSocketConfig::default()
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
    tokio::time::timeout(
        policy.request_timeout,
        tokio_tungstenite::connect_async_with_config(request, Some(config(policy)), true),
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
    let authenticate = |request: &tungstenite::handshake::server::Request,
                        response: tungstenite::handshake::server::Response| {
        if request
            .headers()
            .get(http::header::AUTHORIZATION)
            .and_then(|h| h.to_str().ok())
            .and_then(|h| h.strip_prefix("Bearer "))
            == Some(bearer)
        {
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
                    Some(frame) => write(&mut sink, frame, policy).await?,
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
    fn send(
        &self,
        frames: Vec<Frame>,
        _delivery: RemoteDelivery,
    ) -> HostFuture<'_, Result<(), CodeModeHostError>> {
        let sent = frames
            .iter()
            .map(|frame| frame.encode(self.max_frame_bytes))
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

const BRAIN_DISCONNECTED: &str = "the hand is not connected to its brain";

const CALL_CANCELLED: &str = "the call or its session was cancelled while it ran; its outcome is unknown and it was not retried";

const CALLBACK_DROPPED: &str = "the hand's connection to its brain dropped while the callback ran; its outcome is unknown and it was not retried";

#[derive(Clone)]
struct Outgoing {
    wire: Arc<Atom<Option<mpsc::UnboundedSender<String>>>>,
    limit: Arc<AtomicUsize>,
    backlog: Arc<Atom<VecDeque<Arc<str>>>>,
    flushing: Arc<AtomicBool>,
    resumed: Arc<AtomicBool>,
}

impl Outgoing {
    fn connect(&self, wire: mpsc::UnboundedSender<String>) {
        self.resumed.store(false, Ordering::SeqCst);
        self.wire.set(Some(wire));
    }
    fn clear_backlog(&self) {
        self.backlog.set(VecDeque::new());
    }
    fn queue(&self, text: String) {
        let text = if self.backlog.read(VecDeque::is_empty) && !self.flushing.load(Ordering::SeqCst)
        {
            match self.offer(text) {
                Ok(()) => return,
                Err((text, _)) => text,
            }
        } else {
            text
        };
        let text = Arc::<str>::from(text);
        self.backlog.update(|backlog| {
            if backlog.len() >= MAX_BUFFERED_FRAMES {
                backlog.pop_front();
            }
            backlog.push_back(Arc::clone(&text));
        });
        if self.resumed.load(Ordering::SeqCst) && self.claim() {
            let _ = self.drain();
        }
    }
    fn claim(&self) -> bool {
        self.flushing
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    }
    fn flush(&self) -> Result<(), CodeModeHostError> {
        self.resumed.store(true, Ordering::SeqCst);
        if self.claim() { self.drain() } else { Ok(()) }
    }
    fn drain(&self) -> Result<(), CodeModeHostError> {
        loop {
            let mut frames = self.backlog.update(std::mem::take).into_iter();
            while let Some(text) = frames.next() {
                if let Err((_, issue)) = self.offer(text.to_string()) {
                    let unsent = std::iter::once(text).chain(frames).collect::<VecDeque<_>>();
                    self.backlog.update(|backlog| {
                        let mut held = unsent.clone();
                        held.extend(backlog.iter().cloned());
                        while held.len() > MAX_BUFFERED_FRAMES {
                            held.pop_front();
                        }
                        *backlog = held;
                    });
                    self.flushing.store(false, Ordering::SeqCst);
                    return Err(issue);
                }
            }
            self.flushing.store(false, Ordering::SeqCst);
            if self.backlog.read(VecDeque::is_empty) || !self.claim() {
                return Ok(());
            }
        }
    }
    fn encode(&self, frame: &Frame) -> Result<String, CodeModeHostError> {
        frame.encode(self.limit.load(Ordering::Relaxed))
    }
    fn connected(&self) -> bool {
        self.wire.get().is_some()
    }
    fn push(&self, text: String) -> Result<(), CodeModeHostError> {
        self.offer(text).map_err(|(_, issue)| issue)
    }
    fn offer(&self, text: String) -> Result<(), (String, CodeModeHostError)> {
        match Option::clone(&self.wire.get()) {
            Some(wire) => wire.send(text).map_err(|issue| {
                let reason = error(&issue);
                (issue.0, reason)
            }),
            None => Err((text, error(BRAIN_DISCONNECTED))),
        }
    }
    fn send(&self, frame: &Frame) -> Result<(), CodeModeHostError> {
        self.push(self.encode(frame)?)
    }
    fn send_or(
        &self,
        frame: &Frame,
        fallback: impl FnOnce(&CodeModeHostError) -> Frame,
    ) -> Result<bool, CodeModeHostError> {
        match self.encode(frame) {
            Ok(text) => self.push(text).map(|()| true),
            Err(issue) => self.send(&fallback(&issue)).map(|()| false),
        }
    }
}

fn unsent_update(update: &CodeModeUpdate<'_>, issue: &CodeModeHostError) -> Value {
    match update {
        CodeModeUpdate::NestedCallStarted { call_id, name, .. } => {
            json!({"call_id":call_id,"name":name,"input":Value::Null})
        }
        CodeModeUpdate::NestedCallCompleted(call) => json!({"completed":{
            "call_id":call.call_id,
            "name":call.name,
            "input":Value::Null,
            "output":format!("{issue}; the call's input and output were not sent"),
            "structured_result":Value::Null,
            "success":call.success,
            "started_after_ns":call.started_after_ns,
            "duration_ns":call.duration_ns,
            "metadata":Value::Null,
        }}),
        CodeModeUpdate::Notification(notification) => {
            json!({"notification":{"call_id":notification.call_id,"text":format!("{issue}; the notification was not sent")}})
        }
    }
}

fn status_line(execution: &CodeModeExecution) -> String {
    let header = match &execution.output {
        crate::contract::ToolOutputBody::Text(text) => Some(text.as_str()),
        crate::contract::ToolOutputBody::Content(content) => {
            content.first().and_then(|item| match item {
                crate::contract::ToolOutputContent::InputText { text } => Some(text.as_str()),
                _ => None,
            })
        }
    };
    header
        .and_then(|header| header.lines().next())
        .unwrap_or("Script failed")
        .to_owned()
}

fn reply_text(
    outgoing: &Outgoing,
    id: u64,
    result: Result<Option<Value>, String>,
    status: Option<&str>,
) -> Result<String, CodeModeHostError> {
    let mut frame = Frame::Reply { id, result };
    let mut issue = match outgoing.encode(&frame) {
        Ok(text) => return Ok(text),
        Err(issue) => issue,
    };
    let execution = match &mut frame {
        Frame::Reply {
            result: Ok(Some(Value::Object(execution))),
            ..
        } if status.is_some() => Some(execution),
        _ => None,
    };
    let Some(execution) = execution else {
        return outgoing.encode(&Frame::Reply {
            id,
            result: Err(format!("{issue}; the result was not sent")),
        });
    };
    if let Some(Value::Array(calls)) = execution.get_mut("nested_calls") {
        for call in calls
            .iter_mut()
            .filter_map(Value::as_object_mut)
            .filter(|call| !call.contains_key("delivered"))
        {
            call.insert("input".into(), Value::Null);
            call.insert(
                "output".into(),
                Value::String(format!(
                    "{issue}; the call's input and output were not sent"
                )),
            );
            call.insert("structured_result".into(), Value::Null);
            call.insert("metadata".into(), Value::Null);
        }
    }
    match outgoing.encode(&frame) {
        Ok(text) => return Ok(text),
        Err(next) => issue = next,
    }
    if let Frame::Reply {
        result: Ok(Some(Value::Object(execution))),
        ..
    } = &mut frame
    {
        let status = status.unwrap_or_default();
        execution.insert(
            "output".into(),
            Value::String(format!(
                "{status}\nOutput:\n{issue}; the script's output was not sent"
            )),
        );
        execution.insert("success".into(), Value::Bool(false));
        execution.insert("notifications".into(), json!([]));
    }
    outgoing.encode(&frame)
}

fn origin(call_id: &str) -> &str {
    call_id
        .rsplit_once("/code-")
        .map_or(call_id, |(origin, _)| origin)
}

fn update_call_id<'a>(update: &CodeModeUpdate<'a>) -> &'a str {
    match update {
        CodeModeUpdate::NestedCallStarted { call_id, .. } => call_id,
        CodeModeUpdate::NestedCallCompleted(call) => &call.call_id,
        CodeModeUpdate::Notification(notification) => &notification.call_id,
    }
}

struct Observer {
    id: u64,
    outgoing: Outgoing,
    catalog: CallbackCatalog,
    calls: SentCalls,
}
impl CodeModeObserver for Observer {
    fn update(&mut self, update: CodeModeUpdate<'_>) {
        self.catalog.touch(update_call_id(&update));
        let id = self.id;
        let sent = self.outgoing.send_or(
            &Frame::Update {
                id,
                update: self.calls.update(&update),
            },
            |issue| Frame::Update {
                id,
                update: unsent_update(&update, issue),
            },
        );
        if matches!(sent, Ok(true)) {
            self.calls.sent(&update);
        }
    }
}

struct DetachedObserver {
    session_id: String,
    origin_call_id: String,
    outgoing: Outgoing,
    catalog: CallbackCatalog,
    calls: SentCalls,
}
impl CodeModeObserver for DetachedObserver {
    fn update(&mut self, update: CodeModeUpdate<'_>) {
        self.catalog.touch(&self.origin_call_id);
        let frame = |update| Frame::DetachedUpdate {
            session_id: self.session_id.clone(),
            origin_call_id: self.origin_call_id.clone(),
            update,
        };
        let text = self
            .outgoing
            .encode(&frame(self.calls.update(&update)))
            .or_else(|issue| self.outgoing.encode(&frame(unsent_update(&update, &issue))));
        if let Ok(text) = text {
            self.outgoing.queue(text);
        }
    }
    fn completed(&mut self) {
        self.catalog.finish(&self.origin_call_id);
    }
}

type CallbackReply = Arc<Slot<oneshot::Sender<Result<Value, String>>>>;

#[derive(Clone)]
struct PendingCallback {
    reply: CallbackReply,
    frame: String,
    held: bool,
}

struct CallbackBus {
    outgoing: Outgoing,
    pending: Atom<HashMap<u64, PendingCallback>>,
    next: AtomicU64,
    timeout: Option<Duration>,
    holding: AtomicBool,
}

impl CallbackBus {
    fn hold(&self) {
        self.pending.update(|pending| {
            for entry in pending.values_mut() {
                entry.held = true;
            }
        });
        self.holding.store(true, Ordering::SeqCst);
    }
    fn take_held(&self) -> Vec<String> {
        self.holding.store(false, Ordering::SeqCst);
        self.pending.update(|pending| {
            let mut held = pending
                .iter_mut()
                .filter(|(_, entry)| entry.held)
                .map(|(id, entry)| {
                    entry.held = false;
                    (*id, entry.frame.clone())
                })
                .collect::<Vec<_>>();
            held.sort_unstable_by_key(|(id, _)| *id);
            held.into_iter().map(|(_, frame)| frame).collect()
        })
    }
    fn fail_pending(&self) {
        self.holding.store(false, Ordering::SeqCst);
        for (_, entry) in self.pending.update(std::mem::take) {
            if let Some(reply) = entry.reply.take() {
                let _ = reply.send(Err(CALLBACK_DROPPED.into()));
            }
        }
    }
}
struct CallbackPending {
    bus: Arc<CallbackBus>,
    id: u64,
}
impl Drop for CallbackPending {
    fn drop(&mut self) {
        if self
            .bus
            .pending
            .update(|pending| pending.remove(&self.id))
            .is_some()
        {
            let _ = self
                .bus
                .outgoing
                .send(&Frame::CallbackCancel { id: self.id });
        }
    }
}

async fn callback(bus: &Arc<CallbackBus>, frame: impl FnOnce(u64) -> Frame) -> ToolResult {
    let id = bus.next.fetch_add(1, Ordering::Relaxed);
    let text = bus
        .outgoing
        .encode(&frame(id))
        .map_err(|issue| -> ToolError { format!("{issue}; the call was not dispatched").into() })?;
    let (tx, rx) = oneshot::channel();
    let tx = Arc::new(Slot::new(Some(tx)));
    let held = !bus.outgoing.connected();
    bus.pending.update(|pending| {
        pending.insert(
            id,
            PendingCallback {
                reply: Arc::clone(&tx),
                frame: text.clone(),
                held,
            },
        );
    });
    let _pending = CallbackPending {
        bus: Arc::clone(bus),
        id,
    };
    if held {
        if !bus.holding.load(Ordering::SeqCst) {
            return Err(BRAIN_DISCONNECTED.into());
        }
    } else {
        bus.outgoing
            .push(text)
            .map_err(|e| -> ToolError { e.to_string().into() })?;
    }
    let result = match bus.timeout {
        Some(deadline) => tokio::time::timeout(deadline, rx)
            .await
            .map_err(|_| -> ToolError {
                "brain callback deadline elapsed; outcome unknown".into()
            })?,
        None => rx.await,
    };
    let value = result.map_err(|e| -> ToolError { e.to_string().into() })??;
    output_from_wire(value).map_err(Into::into)
}

struct CallbackTool {
    definition: ToolDefinition,
    bus: Arc<CallbackBus>,
}
#[async_trait::async_trait]
impl Tool for CallbackTool {
    fn definition(&self) -> ToolDefinition {
        self.definition.clone()
    }
    async fn execute(&self, input: ToolInput, context: ToolContext<'_>) -> ToolResult {
        let (input, freeform) = match input {
            ToolInput::Function(input) => (serde_json::from_str(input.get())?, false),
            ToolInput::Freeform(input) => (Value::String(input), true),
        };
        let context = RemoteContext::from_context(context);
        callback(&self.bus, |id| Frame::Callback {
            id,
            name: self.definition.name().into(),
            input,
            freeform,
            context,
            hand: None,
            compact: true,
            output_prefix: true,
        })
        .await
    }
}

struct HandCallbacks {
    bus: Arc<CallbackBus>,
    name: Atom<Option<String>>,
}

impl HandCalls for HandCallbacks {
    fn here(&self, hand: &str) -> bool {
        self.name.read(|name| name.as_deref() == Some(hand))
    }

    fn call<'a>(
        &'a self,
        hand: &'a str,
        name: &'a str,
        input: Value,
        context: ToolContext<'a>,
    ) -> HostFuture<'a, ToolOutput> {
        let context = RemoteContext::from_context(context);
        Box::pin(async move {
            callback(&self.bus, |id| Frame::Callback {
                id,
                name: name.into(),
                input,
                freeform: false,
                context,
                hand: Some(hand.into()),
                compact: true,
                output_prefix: true,
            })
            .await
            .unwrap_or_else(|error| ToolOutput::error(error.to_string()))
        })
    }
}

#[derive(Clone)]
struct CallbackOrigin {
    id: String,
    label: String,
    activity: Arc<LastActivity>,
    turn: u64,
    definitions: Vec<ToolDefinition>,
}

#[derive(Clone, Default)]
struct CallbackCatalogState {
    turn: u64,
    origins: Vec<CallbackOrigin>,
    selected_tools: Option<Vec<String>>,
}

#[derive(Clone)]
struct CallbackCatalog {
    session_id: Arc<str>,
    state: Arc<Atom<CallbackCatalogState>>,
    bus: Arc<CallbackBus>,
}

impl CallbackCatalog {
    fn register(
        &self,
        origin: &str,
        label: &str,
        definitions: Vec<ToolDefinition>,
    ) -> Result<(), CodeModeHostError> {
        let activity = Arc::new(LastActivity::new());
        self.state.modify(|state| {
            if state.origins.iter().any(|entry| entry.id == origin) {
                return Err(error("callback origin is already active"));
            }
            let turn = state.turn;
            state.origins.push(CallbackOrigin {
                id: origin.into(),
                label: label.into(),
                activity: Arc::clone(&activity),
                turn,
                definitions: definitions.clone(),
            });
            Ok(())
        })
    }

    fn touch(&self, call_id: &str) {
        let origin = origin(call_id);
        self.state.read(|state| {
            if let Some(entry) = state.origins.iter().find(|entry| entry.id == origin) {
                entry.activity.touch();
            }
        });
    }

    fn turn(&self) -> u64 {
        self.state.read(|state| state.turn)
    }

    fn begin_turn(&self) {
        self.state.update(|state| state.turn += 1);
    }

    fn adopt(&self, origin: &str, turn: u64) {
        self.state.update(|state| {
            if let Some(entry) = state.origins.iter_mut().find(|entry| entry.id == origin) {
                entry.turn = turn;
            }
        });
    }

    fn finish(&self, origin: &str) {
        let removed = self.state.try_update(|state| {
            let index = state.origins.iter().position(|entry| entry.id == origin)?;
            state.origins.remove(index);
            Some(())
        });
        if removed.is_some()
            && let Ok(text) = self.bus.outgoing.encode(&Frame::DetachedComplete {
                session_id: self.session_id.to_string(),
                origin_call_id: origin.into(),
            })
        {
            self.bus.outgoing.queue(text);
        }
    }

    fn finish_turn(&self, turn: Option<u64>) {
        let origins = self.state.read(|state| {
            state
                .origins
                .iter()
                .filter(|entry| turn.is_none_or(|turn| entry.turn == turn))
                .map(|entry| entry.id.clone())
                .collect::<Vec<_>>()
        });
        for origin in origins {
            self.finish(&origin);
        }
    }
}

struct CallbackOriginGuard {
    catalog: CallbackCatalog,
    origin: String,
    retained: bool,
}

impl Drop for CallbackOriginGuard {
    fn drop(&mut self) {
        if !self.retained {
            self.catalog.finish(&self.origin);
        }
    }
}

#[async_trait::async_trait]
impl crate::runtime::DynamicToolProvider for CallbackCatalog {
    fn start(&self) {}
    fn direct_tools(&self) -> Vec<Arc<dyn Tool>> {
        Vec::new()
    }
    fn available_definitions(&self) -> Vec<ToolDefinition> {
        let state = self.state.get();
        let mut names = HashSet::new();
        state
            .origins
            .iter()
            .rev()
            .flat_map(|entry| entry.definitions.iter())
            .filter(|definition| names.insert(definition.name().to_owned()))
            .cloned()
            .collect()
    }
    async fn execute(
        &self,
        name: &str,
        input: Value,
        context: ToolContext<'_>,
    ) -> Option<ToolOutput> {
        let origin = origin(context.call_id());
        let definition = self.state.read(|state| {
            state
                .origins
                .iter()
                .find(|entry| entry.id == origin)?
                .definitions
                .iter()
                .find(|definition| definition.name() == name)
                .cloned()
        })?;
        let input = match &definition {
            ToolDefinition::Custom { .. } => match input.as_str() {
                Some(source) => ToolInput::Freeform(source.into()),
                None => return Some(ToolOutput::error("custom callback input must be a string")),
            },
            _ => match serde_json::value::to_raw_value(&input) {
                Ok(raw) => ToolInput::Function(raw),
                Err(error) => return Some(ToolOutput::error(error.to_string())),
            },
        };
        let proxy = CallbackTool {
            definition,
            bus: Arc::clone(&self.bus),
        };
        Some(match proxy.execute(input, context).await {
            Ok(output) => output,
            Err(error) => ToolOutput::error(error.to_string()),
        })
    }
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
    let mut executor = Executor::new(workspace.to_owned(), tools, options, activity);
    let served = serve(socket, &mut executor, policy, || {}).await;
    executor.reset().await;
    served.result
}

struct Sessions(HashMap<String, (Arc<ToolRuntime>, CallbackCatalog)>);
impl std::ops::Deref for Sessions {
    type Target = HashMap<String, (Arc<ToolRuntime>, CallbackCatalog)>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl std::ops::DerefMut for Sessions {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}
impl Drop for Sessions {
    fn drop(&mut self) {
        for (_, (runtime, _)) in self.0.drain() {
            tokio::spawn(async move {
                runtime.control().cancel().await;
            });
        }
    }
}

type ReplyResult = Result<Option<Value>, String>;

struct Done {
    id: u64,
    live: ReplyResult,
    full: Option<Value>,
    status: Option<String>,
}

struct Running {
    resumable: bool,
    connection: u64,
    work: Arc<Pending>,
    ended: Arc<AtomicBool>,
    cancel: Option<CancellationToken>,
}

struct Retained {
    text: String,
    work: Arc<Pending>,
    ended: Arc<AtomicBool>,
}

#[derive(Clone)]
struct Pending {
    kind: BusyKind,
    id: String,
    label: String,
    session_id: String,
    since: Instant,
}

impl Pending {
    fn new(id: u64, operation: &RemoteOperation) -> Self {
        let request = |label: &str, session_id: &str| {
            (
                BusyKind::Request,
                id.to_string(),
                label.to_owned(),
                session_id.to_owned(),
            )
        };
        let (kind, id, label, session_id) = match operation {
            RemoteOperation::Execute {
                source, context, ..
            } => (
                BusyKind::Cell,
                context.call_id.clone(),
                source
                    .lines()
                    .map(str::trim)
                    .find(|line| !line.is_empty() && !line.starts_with("// @exec:"))
                    .unwrap_or_default()
                    .to_owned(),
                context.session_id.clone(),
            ),
            RemoteOperation::Call { name, context, .. } => (
                BusyKind::Call,
                context.call_id.clone(),
                name.clone(),
                context.session_id.clone(),
            ),
            RemoteOperation::Wait { context, .. } => request("wait", &context.session_id),
            RemoteOperation::DetachTurn { session_id } => request("detach turn", session_id),
            RemoteOperation::CancelTurn { session_id, .. } => request("cancel turn", session_id),
            RemoteOperation::Cancel { session_id } => request("cancel", session_id),
            RemoteOperation::CancelCall { session_id, .. } => request("cancel call", session_id),
            RemoteOperation::BeginTurn { session_id } => request("begin turn", session_id),
            RemoteOperation::Preempt { session_id } => request("preempt", session_id),
            RemoteOperation::SettlePreempt { session_id } => request("settle preempt", session_id),
            RemoteOperation::EndTurn { session_id, .. } => request("end turn", session_id),
        };
        Self {
            kind,
            id,
            label,
            session_id,
            since: Instant::now(),
        }
    }

    fn item(&self) -> BusyItem {
        BusyItem {
            kind: self.kind,
            id: self.id.clone(),
            label: self.label.clone(),
            session_id: Some(self.session_id.clone()),
            silent_for: self.since.elapsed(),
        }
    }
}

struct Listed {
    work: Arc<Pending>,
    ended: Arc<AtomicBool>,
    next: Option<Arc<Self>>,
}

impl Drop for Listed {
    fn drop(&mut self) {
        let mut next = self.next.take();
        while let Some(node) = next {
            next = Arc::try_unwrap(node)
                .ok()
                .and_then(|mut node| node.next.take());
        }
    }
}

#[derive(Default)]
struct Board {
    sessions: Atom<Vec<(String, Arc<ToolRuntime>, CallbackCatalog)>>,
    work: Atom<Option<Arc<Listed>>>,
}

impl Board {
    fn items(&self, items: &mut Vec<BusyItem>) {
        for (session_id, runtime, catalog) in self.sessions.get().iter() {
            catalog.state.read(|state| {
                items.extend(state.origins.iter().map(|origin| BusyItem {
                    kind: BusyKind::Cell,
                    id: origin.id.clone(),
                    label: origin.label.clone(),
                    session_id: Some(session_id.clone()),
                    silent_for: origin.activity.silent_for(),
                }));
            });
            items.extend(
                runtime
                    .running_tasks()
                    .into_iter()
                    .map(|task| BusyItem::task(task, Some(session_id))),
            );
        }
        let mut next = Option::clone(&self.work.get());
        while let Some(node) = next {
            next = node.next.clone();
            if node.ended.load(Ordering::Acquire) {
                continue;
            }
            let work = &node.work;
            let listed = work.kind == BusyKind::Cell
                && items.iter().any(|item| {
                    item.kind == BusyKind::Cell
                        && item.id == work.id
                        && item.session_id.as_deref() == Some(work.session_id.as_str())
                });
            if !listed {
                items.push(work.item());
            }
        }
    }
}

pub(super) struct Executor {
    workspace: PathBuf,
    tools: crate::Tools,
    options: ExecutorOptions,
    definitions: Option<Vec<ToolDefinition>>,
    outgoing: Outgoing,
    bus: Arc<CallbackBus>,
    callbacks: Arc<HandCallbacks>,
    runtimes: Sessions,
    tasks: tokio::task::JoinSet<()>,
    finished: mpsc::UnboundedSender<Done>,
    results: mpsc::UnboundedReceiver<Done>,
    running: HashMap<u64, Running>,
    calls: HashMap<String, CancellationToken>,
    turn_calls: HashMap<(String, String), CancellationToken>,
    retained: HashMap<u64, Retained>,
    connection: u64,
    resumable: bool,
    activity: HandActivity,
    board: Arc<Board>,
    listed: usize,
    unlisted: usize,
    busy: bool,
    hold: Option<tokio::task::AbortHandle>,
}

impl Executor {
    pub(super) fn new(
        workspace: PathBuf,
        tools: crate::Tools,
        options: ExecutorOptions,
        activity: HandActivity,
    ) -> Self {
        let outgoing = Outgoing {
            wire: Arc::default(),
            limit: Arc::new(AtomicUsize::new(usize::MAX)),
            backlog: Arc::default(),
            flushing: Arc::default(),
            resumed: Arc::default(),
        };
        let bus = Arc::new(CallbackBus {
            outgoing: outgoing.clone(),
            pending: Atom::default(),
            next: AtomicU64::new(0),
            timeout: options.callback_timeout,
            holding: AtomicBool::new(false),
        });
        let callbacks = Arc::new(HandCallbacks {
            bus: Arc::clone(&bus),
            name: Atom::new(options.name.clone()),
        });
        let (finished, results) = mpsc::unbounded_channel();
        let board = activity.attach();
        Self {
            workspace,
            tools,
            options,
            definitions: None,
            outgoing,
            bus,
            callbacks,
            runtimes: Sessions(HashMap::new()),
            tasks: tokio::task::JoinSet::new(),
            finished,
            results,
            running: HashMap::new(),
            calls: HashMap::new(),
            turn_calls: HashMap::new(),
            retained: HashMap::new(),
            connection: 0,
            resumable: false,
            activity,
            board,
            listed: 0,
            unlisted: 0,
            busy: false,
            hold: None,
        }
    }

    fn list(&mut self, work: &Arc<Pending>) -> Arc<AtomicBool> {
        let ended = Arc::new(AtomicBool::new(false));
        let next = Option::clone(&self.board.work.get());
        self.board.work.set(Some(Arc::new(Listed {
            work: Arc::clone(work),
            ended: Arc::clone(&ended),
            next,
        })));
        self.listed += 1;
        ended
    }

    fn unlist(&mut self, ended: &AtomicBool) {
        ended.store(true, Ordering::Release);
        self.unlisted += 1;
        if self.unlisted * 2 <= self.listed {
            return;
        }
        let mut live = Vec::new();
        let mut next = Option::clone(&self.board.work.get());
        while let Some(node) = next {
            if !node.ended.load(Ordering::Acquire) {
                live.push((Arc::clone(&node.work), Arc::clone(&node.ended)));
            }
            next = node.next.clone();
        }
        self.listed = live.len();
        self.unlisted = 0;
        let mut head = None;
        for (work, ended) in live.into_iter().rev() {
            head = Some(Arc::new(Listed {
                work,
                ended,
                next: head,
            }));
        }
        self.board.work.set(head);
    }

    fn publish_sessions(&self) {
        self.board.sessions.set(
            self.runtimes
                .iter()
                .map(|(session_id, (runtime, catalog))| {
                    (session_id.clone(), Arc::clone(runtime), catalog.clone())
                })
                .collect(),
        );
    }

    fn publish(&mut self) {
        let busy = !self.running.is_empty()
            || !self.retained.is_empty()
            || !self.bus.pending.read(|pending| pending.is_empty())
            || self
                .runtimes
                .0
                .values()
                .any(|(_, catalog)| catalog.state.read(|state| !state.origins.is_empty()));
        self.activity.report(self.busy, busy);
        self.busy = busy;
    }

    async fn definitions(&mut self) -> Vec<ToolDefinition> {
        if let Some(definitions) = &self.definitions {
            return definitions.clone();
        }
        let runtime = ToolRuntime::new_with_tools(self.workspace.clone(), Vec::new(), &self.tools);
        let definitions = runtime.registered_code_mode_definitions();
        runtime.control().cancel().await;
        self.definitions = Some(definitions.clone());
        definitions
    }

    fn disconnected(&mut self) {
        self.outgoing.wire.set(None);
        self.bus.hold();
        let bus = Arc::clone(&self.bus);
        let hold = self.options.callback_hold.unwrap_or(DEFAULT_CALLBACK_HOLD);
        let timer = tokio::spawn(async move {
            tokio::time::sleep(hold).await;
            bus.fail_pending();
        });
        if let Some(previous) = self.hold.replace(timer.abort_handle()) {
            previous.abort();
        }
    }

    pub(super) async fn reset(&mut self) {
        self.outgoing.wire.set(None);
        if let Some(timer) = self.hold.take() {
            timer.abort();
        }
        self.bus.fail_pending();
        self.outgoing.clear_backlog();
        self.tasks.abort_all();
        while self.tasks.join_next().await.is_some() {}
        for (_, (runtime, _)) in self.runtimes.0.drain() {
            runtime.control().cancel().await;
        }
        while self.results.try_recv().is_ok() {}
        self.running.clear();
        self.calls.clear();
        self.turn_calls.clear();
        self.retained.clear();
        self.board.work.set(None);
        self.listed = 0;
        self.unlisted = 0;
        self.publish_sessions();
        self.resumable = false;
        self.publish();
    }

    fn end_turn(&mut self, id: u64, session_id: String, turn_id: String, hook_event_name: String) {
        self.turn_calls
            .remove(&(session_id.clone(), turn_id.clone()));
        let tools: Vec<Arc<dyn crate::Tool>> = self.tools.registered_tools().cloned().collect();
        let finished = self.finished.clone();
        self.tasks.spawn(async move {
            for tool in tools {
                if let Err(issue) = tool.end_turn(&session_id, &turn_id, &hook_event_name).await {
                    tracing::warn!(%issue, tool = tool.definition().name(), "hand tool turn cleanup failed; not retried");
                }
            }
            let _ = finished.send(Done { id, live: Ok(None), full: None, status: None });
        });
    }

    fn finish(&self, id: u64, live: ReplyResult) {
        let _ = self.finished.send(Done {
            id,
            live,
            full: None,
            status: None,
        });
    }

    fn deliver(&mut self, done: Done) -> Result<(), CodeModeHostError> {
        let Done {
            id,
            live,
            full,
            status,
        } = done;
        let running = self.running.remove(&id);
        let reconnected = running
            .as_ref()
            .is_some_and(|running| running.connection != self.connection);
        let status = status.as_deref();
        let refused = matches!(&live, Err(message) if message == NOT_RECEIVED);
        let text = match running {
            Some(running) if running.resumable && !refused => {
                let retained = match full {
                    Some(full) => reply_text(&self.outgoing, id, Ok(Some(full)), status)?,
                    None => reply_text(&self.outgoing, id, live.clone(), status)?,
                };
                let text = if reconnected {
                    retained.clone()
                } else {
                    reply_text(&self.outgoing, id, live, status)?
                };
                let work = Arc::new(Pending {
                    kind: BusyKind::Reply,
                    since: Instant::now(),
                    ..Pending::clone(&running.work)
                });
                self.unlist(&running.ended);
                let ended = self.list(&work);
                self.retained.insert(
                    id,
                    Retained {
                        text: retained,
                        work,
                        ended,
                    },
                );
                text
            }
            Some(running) => {
                self.unlist(&running.ended);
                reply_text(&self.outgoing, id, live, status)?
            }
            None => reply_text(&self.outgoing, id, live, status)?,
        };
        let _ = self.outgoing.push(text);
        Ok(())
    }

    fn resume(&mut self, pending: &[u64]) -> Result<(), CodeModeHostError> {
        if let Some(timer) = self.hold.take() {
            timer.abort();
        }
        for text in self.bus.take_held() {
            self.outgoing.push(text)?;
        }
        self.outgoing.flush()?;
        let wanted = pending.iter().copied().collect::<HashSet<_>>();
        let forgotten = self
            .retained
            .keys()
            .filter(|id| !wanted.contains(id))
            .copied()
            .collect::<Vec<_>>();
        for id in forgotten {
            if let Some(retained) = self.retained.remove(&id) {
                self.unlist(&retained.ended);
            }
        }
        for id in pending {
            if let Some(retained) = self.retained.get(id) {
                self.outgoing.push(retained.text.clone())?;
            } else if !self.running.contains_key(id) {
                self.outgoing.send(&Frame::Reply {
                    id: *id,
                    result: Err(NOT_RECEIVED.into()),
                })?;
            }
        }
        Ok(())
    }
}

impl Drop for Executor {
    fn drop(&mut self) {
        self.activity.report(self.busy, false);
        self.activity.detach(&self.board);
    }
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
    executor: &mut Executor,
    policy: SocketPolicy,
    heard: impl FnOnce() + Send,
) -> Served {
    let liveness = Liveness::new();
    let result = serve_link(socket, executor, policy, &liveness, heard).await;
    Served {
        result,
        close_code: liveness.close_code(),
    }
}

async fn serve_link<S: AsyncRead + AsyncWrite + Unpin + Send + 'static>(
    mut socket: WebSocketStream<S>,
    executor: &mut Executor,
    policy: SocketPolicy,
    liveness: &Arc<Liveness>,
    heard: impl FnOnce() + Send,
) -> Result<(), CodeModeHostError> {
    let mut heard = Some(heard);
    executor.connection += 1;
    let connection = executor.connection;
    let catalog = Frame::Catalog {
        definitions: executor.definitions().await,
        name: executor.options.name.clone(),
        workspace: Some(executor.workspace.display().to_string()),
        runtime_id: executor.options.runtime_id.clone(),
        max_frame_bytes: Some(policy.max_frame_bytes),
        resumable: executor.options.runtime_id.is_some(),
        tool_selection: !executor.tools.has_dynamic_providers(),
        cancels_calls: true,
    };
    write(&mut socket, catalog.encode(policy.max_frame_bytes)?, policy).await?;
    let (sink, mut socket) = socket.split();
    let watchdog = liveness.watch(policy.request_timeout / 2);
    tokio::pin!(watchdog);
    let (wire, mut writer) = writer(sink, policy, Arc::clone(liveness), Arc::default());
    executor
        .outgoing
        .limit
        .store(policy.max_frame_bytes, Ordering::Relaxed);
    executor.outgoing.connect(wire);
    let outgoing = executor.outgoing.clone();
    let bus = Arc::clone(&executor.bus);
    let hands: Arc<dyn HandCalls> = executor.callbacks.clone();
    let result = async {
        loop {
            executor.publish();
            tokio::select! {
                result = &mut writer.0 => return result.map_err(error)?,
                () = liveness.heard.wait(), if heard.is_some() => {
                    if let Some(heard) = heard.take() {
                        heard();
                    }
                },
                Some(done) = executor.results.recv() => {
                    if let Err(issue) = executor.deliver(done) {
                        tracing::error!(%issue, "remote reply could not be encoded");
                    }
                },
                frame = read_live(&mut socket, liveness, watchdog.as_mut()) => {
                    let Some(frame) = frame? else { return Ok(()); };
                    match frame {
                        Frame::CallbackResult { id, result } => { if let Some(tx) = bus.pending.update(|pending| pending.remove(&id)).and_then(|entry| entry.reply.take()) { let _ = tx.send(result); } },
                        Frame::Ack { ids } => { for id in ids { if let Some(retained) = executor.retained.remove(&id) { executor.unlist(&retained.ended); } } },
                        Frame::Resume { pending } => executor.resume(&pending)?,
                        Frame::Request { id, operation, max_frame_bytes, compact, output_prefix, resumable } => {
                            if let Some(bytes) = max_frame_bytes { outgoing.limit.fetch_min(bytes, Ordering::Relaxed); }
                            executor.resumable |= resumable;
                            let work = Arc::new(Pending::new(id, &operation));
                            let known = executor.running.get(&id).map(|running| Arc::clone(&running.work)).or_else(|| executor.retained.get(&id).map(|retained| Arc::clone(&retained.work)));
                            if let Some(known) = known {
                                if known.id != work.id {
                                    outgoing.send(&Frame::Reply { id, result: Err(REQUEST_IN_USE.into()) })?;
                                } else if let Some(retained) = executor.retained.get(&id) {
                                    outgoing.push(retained.text.clone())?;
                                }
                                continue;
                            }
                            let session_id = work.session_id.clone();
                            let ended = executor.list(&work);
                            executor.running.insert(id, Running { resumable, connection, work: Arc::clone(&work), ended, cancel: None });
                            executor.publish();
                            if executor.activity.draining() && matches!(&operation, RemoteOperation::Execute { .. } | RemoteOperation::Call { .. }) {
                                executor.finish(id, Err(NOT_RECEIVED.into()));
                                continue;
                            }
                            if let RemoteOperation::EndTurn { session_id, turn_id, hook_event_name } = operation {
                                executor.end_turn(id, session_id, turn_id, hook_event_name);
                                continue;
                            }
                            if matches!(&operation, RemoteOperation::Execute { selected_tools: Some(_), .. } | RemoteOperation::Call { selected_tools: Some(_), .. }) && executor.tools.has_dynamic_providers() {
                                executor.finish(id, Err("hand tool selection is unavailable with dynamic hand providers; nothing was dispatched".into()));
                                continue;
                            }
                            if matches!(&operation, RemoteOperation::Execute { .. } | RemoteOperation::Call { .. }) && !executor.runtimes.contains_key(&session_id) {
                                let catalog = CallbackCatalog {
                                    session_id: Arc::from(session_id.as_str()),
                                    state: Arc::new(Atom::new(CallbackCatalogState { selected_tools: match &operation { RemoteOperation::Execute { selected_tools, .. } | RemoteOperation::Call { selected_tools, .. } => selected_tools.clone(), _ => None }, ..Default::default() })),
                                    bus: Arc::clone(&bus),
                                };
                                let selected = catalog.state.read(|state| state.selected_tools.clone());
                                let excluded = executor.definitions().await.into_iter().filter(|definition| selected.as_ref().is_some_and(|names| !names.iter().any(|name| name == definition.name()))).map(|definition| definition.name().to_owned()).collect::<Vec<_>>();
                                match executor.tools.clone().excluding_code_tools(excluded).into_builder().provider(catalog.clone()).hand_calls(Arc::clone(&hands)).build() {
                                    Ok(selected) => { executor.runtimes.insert(session_id.clone(), (Arc::new(ToolRuntime::new_with_tools(executor.workspace.clone(), Vec::new(), &selected)), catalog)); executor.publish_sessions(); },
                                    Err(issue) => {
                                        executor.finish(id, Err(issue.to_string()));
                                        continue;
                                    }
                                }
                            }
                            if let RemoteOperation::Execute { selected_tools, .. } | RemoteOperation::Call { selected_tools, .. } = &operation
                                && let Some((_, catalog)) = executor.runtimes.get(&session_id)
                                && catalog.state.read(|state| &state.selected_tools != selected_tools)
                            {
                                executor.finish(id, Err("hand tool selection cannot change within a session".into()));
                                continue;
                            }
                            if matches!(&operation, RemoteOperation::Cancel { .. } | RemoteOperation::CancelTurn { turn_id: None, .. }) {
                                if let Some(calls) = executor.calls.remove(&session_id) {
                                    calls.cancel();
                                }
                                executor.turn_calls.retain(|(session, _), _| session != &session_id);
                            }
                            if let RemoteOperation::CancelTurn { turn_id: Some(turn_id), .. } = &operation {
                                if let Some(calls) = executor.turn_calls.remove(&(session_id.clone(), turn_id.clone())) {
                                    calls.cancel();
                                }
                                executor.finish(id, Ok(None));
                                continue;
                            }
                            if let RemoteOperation::CancelCall { call_id, .. } = &operation {
                                for running in executor.running.values().filter(|running| running.work.session_id == session_id && running.work.id == *call_id) {
                                    if let Some(cancel) = &running.cancel { cancel.cancel(); }
                                }
                                executor.finish(id, Ok(None));
                                continue;
                            }
                            let session_calls = executor.calls.entry(session_id.clone()).or_default().clone();
                            let cancelled = match &operation {
                                RemoteOperation::Call { context: RemoteContext { turn_id: Some(turn_id), .. }, .. } => executor.turn_calls.entry((session_id.clone(), turn_id.clone())).or_insert_with(|| session_calls.child_token()).clone(),
                                _ => session_calls,
                            };
                            let cancelled = match &operation {
                                RemoteOperation::Call { .. } => {
                                    let call = cancelled.child_token();
                                    if let Some(running) = executor.running.get_mut(&id) { running.cancel = Some(call.clone()); }
                                    call
                                }
                                _ => cancelled,
                            };
                            if let RemoteOperation::Execute { hand: Some(hand), .. } = &operation {
                                executor.callbacks.name.set(Some(hand.clone()));
                            }
                            if let RemoteOperation::Execute { local_tools, context, .. } = &operation
                                && let Some((_, catalog)) = executor.runtimes.get(&session_id)
                                && let Err(issue) = catalog.register(&context.call_id, &work.label, local_tools.clone())
                            {
                                executor.finish(id, Err(issue.to_string()));
                                continue;
                            }
                            let runtime = if matches!(&operation, RemoteOperation::Cancel { .. }) { let removed = executor.runtimes.0.remove(&session_id); executor.publish_sessions(); removed } else { executor.runtimes.get(&session_id).map(|(runtime, catalog)| (Arc::clone(runtime), catalog.clone())) };
                            let runtime = runtime.map(|(runtime, catalog)| {
                                let turn = catalog.turn();
                                (runtime, catalog, turn)
                            });
                            if let Some((runtime, catalog, turn)) = &runtime {
                                match &operation {
                                    RemoteOperation::CancelTurn { .. } => { runtime.control().cancel_turn_with_updates(&mut Observer { id, outgoing:outgoing.clone(), catalog:catalog.clone(), calls:SentCalls::new(compact) }).await; catalog.finish_turn(Some(*turn)); executor.finish(id, Ok(None)); continue; },
                                    RemoteOperation::BeginTurn { .. } => { runtime.control().begin_turn(); catalog.begin_turn(); executor.finish(id, Ok(None)); continue; },
                                    RemoteOperation::SettlePreempt { .. } => { runtime.control().settle_preempt(); executor.finish(id, Ok(None)); continue; },
                                    RemoteOperation::DetachTurn { session_id } => { runtime.control().detach_turn_with_updates(&mut |origin| Box::new(DetachedObserver { session_id:session_id.clone(), origin_call_id:origin.into(), outgoing:outgoing.clone(), catalog:catalog.clone(), calls:SentCalls::new(compact) })).await; executor.finish(id, Ok(None)); continue; },
                                    _ => {},
                                }
                            }
                            let outgoing = outgoing.clone();
                            let finished = executor.finished.clone();
                            executor.tasks.spawn(async move {
                                let mut status = None;
                                let mut full = None;
                                let result = async {
                                    let Some((runtime, catalog, turn)) = runtime else {
                                        return match operation { RemoteOperation::Execute { .. } | RemoteOperation::Wait { .. } | RemoteOperation::Call { .. } => Err(error("this hand has no cells for the session: it restarted or reconnected since the cell started, so the cell's outcome is unknown")), _ => Ok(None) };
                                    };
                                    let mut observer = Observer { id, outgoing:outgoing.clone(), catalog:catalog.clone(), calls:SentCalls::new(compact).output_prefix(output_prefix) };
                                    match operation {
                                        RemoteOperation::Execute { source, context, .. } => {
                                            let mut origin = CallbackOriginGuard { catalog:catalog.clone(), origin:context.call_id.clone(), retained:false };
                                            let result = runtime.execute_code_owned_with_updates(&source, context.owned(), &mut observer).await;
                                            origin.retained = result.as_ref().is_ok_and(|execution| execution.cell.as_ref().is_some_and(|cell| cell.running));
                                            result.map(|value| {
                                                status = Some(status_line(&value));
                                                full = resumable.then(|| SentCalls::new(compact).encode(&value));
                                                Some(observer.calls.execution(value))
                                            })
                                        },
                                        RemoteOperation::Wait { input, context } => {
                                            let execution = runtime.wait_for_code_with_updates(&input, context.owned().as_context(), &mut observer).await?;
                                            if let Some(cell) = &execution.cell {
                                                if cell.running { catalog.adopt(&cell.origin_call_id, turn); }
                                                else { catalog.finish(&cell.origin_call_id); }
                                            }
                                            status = Some(status_line(&execution));
                                            full = resumable.then(|| SentCalls::new(compact).encode(&execution));
                                            Ok(Some(observer.calls.execution(execution)))
                                        },
                                        RemoteOperation::Call { name, input, context, .. } => {
                                            let owned = context.owned();
                                            tokio::select! {
                                                output = runtime.execute_nested_tool(&name, input, owned.as_context()) => encode_output(output, compact, output_prefix).and_then(|value| {
                                                    if resumable && value.get("output_prefix").is_some() {
                                                        full = Some(output_to_wire(output_from_wire(value.clone())?, compact)?);
                                                    }
                                                    Ok(Some(value))
                                                }),
                                                () = cancelled.cancelled() => Err(error(CALL_CANCELLED)),
                                            }
                                        },
                                        RemoteOperation::Cancel { .. } => { runtime.control().cancel_with_updates(&mut observer).await; catalog.finish_turn(None); Ok(None) },
                                        RemoteOperation::CancelTurn { .. } => { runtime.control().cancel_turn_with_updates(&mut observer).await; catalog.finish_turn(Some(turn)); Ok(None) },
                                        RemoteOperation::DetachTurn { session_id } => { runtime.control().detach_turn_with_updates(&mut |origin| Box::new(DetachedObserver { session_id:session_id.clone(), origin_call_id:origin.into(), outgoing:outgoing.clone(), catalog:catalog.clone(), calls:SentCalls::new(compact) })).await; Ok(None) },
                                        RemoteOperation::BeginTurn { .. } => { runtime.control().begin_turn(); catalog.begin_turn(); Ok(None) },
                                        RemoteOperation::Preempt { .. } => { runtime.control().preempt_turn().await; Ok(None) },
                                        RemoteOperation::SettlePreempt { .. } => { runtime.control().settle_preempt(); Ok(None) },
                                        RemoteOperation::EndTurn { .. } | RemoteOperation::CancelCall { .. } => Ok(None),
                                    }
                                }.await.map_err(|e| e.to_string());
                                let _ = finished.send(Done { id, live: result, full, status });
                            });
                        },
                        _ => return Err(error("unexpected brain frame")),
                    }
                },
                Some(result) = executor.tasks.join_next(), if !executor.tasks.is_empty() => { result.map_err(error)?; },
            }
        }
    }.await;
    executor.disconnected();
    if !executor.resumable {
        executor.reset().await;
    }
    executor.publish();
    result
}
