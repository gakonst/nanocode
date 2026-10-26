use std::{future::Future, sync::Arc, time::Duration};

use arc_swap::ArcSwap;
use futures_util::StreamExt;
use http::{
    HeaderMap, HeaderName, HeaderValue, Method, StatusCode,
    header::{ACCEPT, AUTHORIZATION, CONTENT_LENGTH, CONTENT_TYPE, WWW_AUTHENTICATE},
};
use nanocode_http::layer::{Body, Request, Sender};
use nanocode_mcp::{
    Error as SessionError, Inbound, Message, Outgoing, RequestId, TransportError,
    protocol::{INITIALIZE, INITIALIZED},
    sse,
};
use nanocode_platform::{sleep, timeout};
use nanocode_sync::{SingleFlight, Slot};

#[cfg(not(target_family = "wasm"))]
use crate::oauth::OAuthRuntime;

const HEADER_SESSION_ID: HeaderName = HeaderName::from_static("mcp-session-id");
const HEADER_PROTOCOL_VERSION: HeaderName = HeaderName::from_static("mcp-protocol-version");
const HEADER_LAST_EVENT_ID: HeaderName = HeaderName::from_static("last-event-id");
const EVENT_STREAM_MIME_TYPE: &str = "text/event-stream";
const JSON_MIME_TYPE: &str = "application/json";
const ACCEPT_MCP: &str = "text/event-stream, application/json";
const RESERVED_HEADERS: [&str; 3] = ["accept", "mcp-session-id", "last-event-id"];
const MAX_SSE_EVENT_BYTES: usize = 16 * 1024 * 1024;
const SESSION_CLEANUP_TIMEOUT: Duration = Duration::from_secs(5);
const RESPONSE_DRAIN_TIMEOUT: Duration = Duration::from_millis(50);
const SSE_RETRY_BASE: Duration = Duration::from_secs(1);
const CLOSED_BEFORE_RESPONSE: &str =
    "streamable HTTP response stream closed before its final response";

pub enum HttpAuth {
    None,
    Bearer(String),
    #[cfg(not(target_family = "wasm"))]
    OAuth(Arc<OAuthRuntime>),
    Source(Arc<dyn BearerSource>),
}

#[cfg_attr(not(target_family = "wasm"), async_trait::async_trait)]
#[cfg_attr(target_family = "wasm", async_trait::async_trait(?Send))]
pub trait BearerSource: Send + Sync {
    async fn token(&self) -> Result<String, String>;

    async fn rejected(&self, token: &str) -> Result<String, String>;
}

pub fn validate_custom_headers<'a>(
    names: impl IntoIterator<Item = &'a HeaderName>,
) -> Result<(), String> {
    for name in names {
        if RESERVED_HEADERS.contains(&name.as_str()) {
            return Err(format!(
                "Header name '{name}' is reserved and conflicts with default headers"
            ));
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Debug)]
pub struct HttpOptions {
    pub max_response_bytes: Option<usize>,
    pub standalone_stream: bool,
    pub reuse_connections: bool,
}

impl Default for HttpOptions {
    fn default() -> Self {
        Self {
            max_response_bytes: None,
            standalone_stream: true,
            reuse_connections: false,
        }
    }
}

fn exceeded(limit: usize) -> TransportError {
    TransportError::new(format!(
        "MCP response exceeded its max_response_bytes of {limit} bytes; its outcome is unknown"
    ))
}

#[derive(Clone, Default)]
struct SessionState {
    id: Option<Arc<str>>,
    protocol_version: Option<HeaderValue>,
    initialize: Option<Message>,
}

enum Client {
    #[cfg(not(target_family = "wasm"))]
    Reqwest(reqwest::Client),
    Sender {
        sender: Arc<dyn Sender>,
        headers: HeaderMap,
    },
}

struct Reply {
    status: StatusCode,
    headers: HeaderMap,
    body: Body,
}

struct Shared {
    url: String,
    client: Client,
    auth: HttpAuth,
    inbound: Inbound,
    session: ArcSwap<SessionState>,
    reinitialize: SingleFlight<Result<(), String>>,
    listener: Slot<Background>,
    options: HttpOptions,
}

enum PostError {
    SessionExpired,
    Failed(TransportError),
}

const SESSION_EXPIRED: &str = "Session expired (HTTP 404)";

impl From<PostError> for TransportError {
    fn from(error: PostError) -> Self {
        match error {
            PostError::SessionExpired => Self::with_status(SESSION_EXPIRED, 404),
            PostError::Failed(error) => error,
        }
    }
}

enum StreamEnd {
    Response,
    Closed,
    Failed(String),
    Fatal(String),
}

#[derive(Default)]
struct Cursor {
    last_event_id: Option<String>,
    retry: Option<Duration>,
}

#[cfg(not(target_family = "wasm"))]
type Background = tokio::task::JoinHandle<()>;
#[cfg(target_family = "wasm")]
type Background = futures_util::future::AbortHandle;

#[cfg(not(target_family = "wasm"))]
type Tasks = tokio::task::JoinSet<()>;
#[cfg(target_family = "wasm")]
type Tasks =
    futures_util::stream::FuturesUnordered<futures_util::future::LocalBoxFuture<'static, ()>>;

#[cfg(not(target_family = "wasm"))]
fn spawn_background(future: impl Future<Output = ()> + Send + 'static) -> Background {
    tokio::spawn(future)
}

#[cfg(target_family = "wasm")]
fn spawn_background(future: impl Future<Output = ()> + 'static) -> Background {
    let (handle, registration) = futures_util::future::AbortHandle::new_pair();
    nanocode_platform::spawn_local(async move {
        let _ = futures_util::future::Abortable::new(future, registration).await;
    });
    handle
}

#[cfg(not(target_family = "wasm"))]
fn run_task(tasks: &mut Tasks, task: impl Future<Output = ()> + Send + 'static) {
    tasks.spawn(task);
}

#[cfg(target_family = "wasm")]
fn run_task(tasks: &mut Tasks, task: impl Future<Output = ()> + 'static) {
    tasks.push(Box::pin(task));
}

#[cfg(not(target_family = "wasm"))]
async fn next_message(outgoing: &mut Outgoing, tasks: &mut Tasks) -> Option<Message> {
    loop {
        tokio::select! {
            message = outgoing.next() => return message,
            Some(_) = tasks.join_next(), if !tasks.is_empty() => {}
        }
    }
}

#[cfg(target_family = "wasm")]
async fn next_message(outgoing: &mut Outgoing, tasks: &mut Tasks) -> Option<Message> {
    std::future::poll_fn(|context| {
        while let std::task::Poll::Ready(Some(())) = tasks.poll_next_unpin(context) {}
        outgoing.poll_next_unpin(context)
    })
    .await
}

#[cfg(not(target_family = "wasm"))]
fn stop_tasks(mut tasks: Tasks) {
    tasks.abort_all();
}

#[cfg(target_family = "wasm")]
fn stop_tasks(tasks: Tasks) {
    drop(tasks);
}

#[cfg(not(target_family = "wasm"))]
pub fn spawn(
    url: String,
    client: reqwest::Client,
    auth: HttpAuth,
    outgoing: Outgoing,
    inbound: Inbound,
) {
    spawn_with_options(url, client, auth, HttpOptions::default(), outgoing, inbound);
}

#[cfg(not(target_family = "wasm"))]
pub fn spawn_with_options(
    url: String,
    client: reqwest::Client,
    auth: HttpAuth,
    options: HttpOptions,
    outgoing: Outgoing,
    inbound: Inbound,
) {
    start(
        url,
        Client::Reqwest(client),
        auth,
        options,
        outgoing,
        inbound,
    );
}

pub fn spawn_with_sender(
    url: String,
    sender: Arc<dyn Sender>,
    headers: HeaderMap,
    auth: HttpAuth,
    outgoing: Outgoing,
    inbound: Inbound,
) {
    spawn_with_sender_options(
        url,
        sender,
        headers,
        auth,
        HttpOptions::default(),
        outgoing,
        inbound,
    );
}

pub fn spawn_with_sender_options(
    url: String,
    sender: Arc<dyn Sender>,
    headers: HeaderMap,
    auth: HttpAuth,
    options: HttpOptions,
    outgoing: Outgoing,
    inbound: Inbound,
) {
    start(
        url,
        Client::Sender { sender, headers },
        auth,
        options,
        outgoing,
        inbound,
    );
}

fn start(
    url: String,
    client: Client,
    auth: HttpAuth,
    options: HttpOptions,
    outgoing: Outgoing,
    inbound: Inbound,
) {
    let shared = Arc::new(Shared {
        url,
        client,
        auth,
        inbound,
        session: ArcSwap::default(),
        reinitialize: SingleFlight::default(),
        listener: Slot::default(),
        options,
    });
    drop(spawn_background(drive(shared, outgoing)));
}

async fn drive(shared: Arc<Shared>, mut outgoing: Outgoing) {
    let mut tasks = Tasks::default();
    let mut initialized = false;
    loop {
        let Some(message) = next_message(&mut outgoing, &mut tasks).await else {
            break;
        };
        if !initialized {
            match &message {
                Message::Request { id, method, .. }
                    if method == INITIALIZE && shared.session.load().initialize.is_none() =>
                {
                    match shared.post_initialize(&message).await {
                        Ok((response, session_id)) => {
                            shared.session.store(Arc::new(SessionState {
                                id: session_id,
                                protocol_version: negotiated_version(&response),
                                initialize: Some(message.clone()),
                            }));
                            shared.inbound.deliver(response);
                        }
                        Err(error) => shared.inbound.fail(id, SessionError::Transport(error)),
                    }
                    continue;
                }
                Message::Notification { method, .. } if method == INITIALIZED => {
                    initialized = true;
                    let state = shared.state();
                    if let Err(error) = shared.post(&message, &state).await {
                        tracing::error!(
                            error = %TransportError::from(error),
                            "failed to send MCP initialized notification"
                        );
                        shared.inbound.close();
                        break;
                    }
                    shared.restart_listener();
                    continue;
                }
                _ => {}
            }
        }
        let shared = Arc::clone(&shared);
        run_task(&mut tasks, async move { shared.dispatch(message).await });
    }
    if let Some(listener) = shared.listener.take() {
        listener.abort();
    }
    stop_tasks(tasks);
    shared.delete_session().await;
}

fn negotiated_version(message: &Message) -> Option<HeaderValue> {
    let Message::Response { result, .. } = message else {
        return None;
    };
    result
        .get("protocolVersion")
        .and_then(serde_json::Value::as_str)
        .and_then(|version| HeaderValue::from_str(version).ok())
}

fn content_type(response: &Reply) -> Option<String> {
    response
        .headers
        .get(CONTENT_TYPE)
        .map(|value| String::from_utf8_lossy(value.as_bytes()).into_owned())
}

fn is_mime(content_type: Option<&str>, mime: &str) -> bool {
    content_type.is_some_and(|content_type| content_type.as_bytes().starts_with(mime.as_bytes()))
}

#[cfg(not(target_family = "wasm"))]
fn client_error(error: &reqwest::Error) -> TransportError {
    let message = format!("Client error: {}", crate::error_chain(error));
    if error.is_connect() || error.is_builder() {
        TransportError::unsent(message)
    } else {
        TransportError::new(message)
    }
}

fn sender_error(error: &nanocode_http::layer::Error) -> TransportError {
    let message = format!("Client error: {}", error.detail());
    if error.is_unsent() {
        TransportError::unsent(message)
    } else {
        TransportError::new(message)
    }
}

/// Whether the server rejected the token sent. Any 401 counts, with or
/// without the `WWW-Authenticate` header the spec requires, since some
/// servers omit it.
fn challenged(response: &Reply) -> bool {
    response.status == StatusCode::UNAUTHORIZED
}

fn challenge_error(response: &Reply) -> Option<TransportError> {
    let status = response.status;
    let header = response.headers.get(WWW_AUTHENTICATE)?;
    let Ok(header) = header.to_str() else {
        return Some(TransportError::with_status(
            "unexpected server response: invalid www-authenticate header value",
            status.as_u16(),
        ));
    };
    let message = match status {
        StatusCode::UNAUTHORIZED => format!("Auth required: authorization required: {header}"),
        StatusCode::FORBIDDEN => format!("Insufficient scope: insufficient scope: {header}"),
        _ => return None,
    };
    Some(TransportError::with_status(message, status.as_u16()))
}

impl Reply {
    #[cfg(not(target_family = "wasm"))]
    fn from_reqwest(response: reqwest::Response) -> Self {
        let status = response.status();
        let headers = response.headers().clone();
        let body = futures_util::stream::unfold(response, |mut response| async move {
            match response.chunk().await {
                Ok(Some(chunk)) => Some((Ok(chunk.to_vec()), response)),
                Ok(None) => None,
                Err(error) => Some((
                    Err(nanocode_http::layer::Error::new(crate::error_chain(&error))),
                    response,
                )),
            }
        });
        Self {
            status,
            headers,
            body: Box::pin(body),
        }
    }

    async fn chunk(&mut self) -> Result<Option<Vec<u8>>, TransportError> {
        self.body
            .next()
            .await
            .transpose()
            .map_err(|error| sender_error(&error))
    }

    async fn bytes(mut self, limit: Option<usize>) -> Result<Vec<u8>, TransportError> {
        let mut bytes = Vec::new();
        while let Some(chunk) = self.chunk().await? {
            bytes.extend_from_slice(&chunk);
            if let Some(limit) = limit.filter(|limit| bytes.len() > *limit) {
                return Err(exceeded(limit));
            }
        }
        Ok(bytes)
    }

    async fn text(self, limit: Option<usize>) -> Result<String, TransportError> {
        self.bytes(limit)
            .await
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
    }
}

impl Client {
    async fn send(
        &self,
        method: &Method,
        url: &str,
        headers: HeaderMap,
        body: &[u8],
    ) -> Result<Reply, TransportError> {
        match self {
            #[cfg(not(target_family = "wasm"))]
            Self::Reqwest(client) => {
                let mut request = client.request(method.clone(), url).headers(headers);
                if !body.is_empty() {
                    request = request.body(body.to_vec());
                }
                let response = request
                    .send()
                    .await
                    .map(nanocode_http::decode_response)
                    .map_err(|error| client_error(&error))?;
                Ok(Reply::from_reqwest(response))
            }
            Self::Sender {
                sender,
                headers: defaults,
            } => {
                let mut merged = defaults.clone();
                merged.extend(headers);
                let response = sender
                    .send(Request {
                        method: method.clone(),
                        url: url.to_owned(),
                        headers: merged,
                        body: body.to_vec(),
                    })
                    .await
                    .map_err(|error| sender_error(&error))?;
                let (status, headers, body, _) = response.into_parts();
                Ok(Reply {
                    status,
                    headers,
                    body,
                })
            }
        }
    }
}

impl Shared {
    fn state(&self) -> SessionState {
        SessionState::clone(&self.session.load())
    }

    async fn attempt(
        &self,
        method: &Method,
        headers: &HeaderMap,
        body: &[u8],
        token: Option<&str>,
    ) -> Result<Reply, TransportError> {
        let mut headers = headers.clone();
        if let Some(token) = token {
            let mut value = HeaderValue::try_from(format!("Bearer {token}"))
                .map_err(|error| TransportError::unsent(format!("Client error: {error}")))?;
            value.set_sensitive(true);
            headers.insert(AUTHORIZATION, value);
        }
        self.client.send(method, &self.url, headers, body).await
    }

    async fn send(
        &self,
        method: &Method,
        headers: HeaderMap,
        body: &[u8],
    ) -> Result<Reply, TransportError> {
        let token = match &self.auth {
            HttpAuth::None => None,
            HttpAuth::Bearer(token) => Some(token.clone()),
            #[cfg(not(target_family = "wasm"))]
            HttpAuth::OAuth(runtime) => Some(
                runtime
                    .access_token()
                    .await
                    .map_err(TransportError::unsent)?,
            ),
            HttpAuth::Source(source) => Some(source.token().await.map_err(TransportError::unsent)?),
        };
        let response = self
            .attempt(method, &headers, body, token.as_deref())
            .await?;
        let Some(sent) = token.as_deref().filter(|_| challenged(&response)) else {
            return Ok(response);
        };
        let fresh = match &self.auth {
            #[cfg(not(target_family = "wasm"))]
            HttpAuth::OAuth(runtime) => runtime.refresh_after_rejection(sent).await,
            HttpAuth::Source(source) => source.rejected(sent).await,
            HttpAuth::None | HttpAuth::Bearer(_) => return Ok(response),
        };
        match fresh {
            Ok(fresh) if fresh != sent => {
                let retried = self.attempt(method, &headers, body, Some(&fresh)).await?;
                if challenged(&retried) {
                    match &self.auth {
                        HttpAuth::Source(source) => {
                            if let Err(error) = source.rejected(&fresh).await {
                                tracing::debug!(
                                    "bearer source refused a rejected refreshed token: {error}"
                                );
                            }
                        }
                        #[cfg(not(target_family = "wasm"))]
                        HttpAuth::OAuth(runtime) => runtime.refused_after_refresh(&fresh).await,
                        HttpAuth::None | HttpAuth::Bearer(_) => {}
                    }
                }
                return Ok(retried);
            }
            Ok(_) => {}
            Err(error) => {
                tracing::debug!("token refresh after server rejection failed: {error}");
            }
        }
        Ok(response)
    }

    async fn post_initialize(
        &self,
        initialize: &Message,
    ) -> Result<(Message, Option<Arc<str>>), TransportError> {
        let body = initialize
            .to_vec()
            .map_err(|error| TransportError::unsent(format!("Deserialize error: {error}")))?;
        let mut response = self
            .send(&Method::POST, post_headers(HeaderMap::new()), &body)
            .await?;
        if let Some(error) = challenge_error(&response) {
            return Err(error);
        }
        let status = response.status;
        if matches!(status, StatusCode::ACCEPTED | StatusCode::NO_CONTENT) {
            return Err("unexpected server response: expect initialized, accepted".into());
        }
        let session_id = response
            .headers
            .get(&HEADER_SESSION_ID)
            .and_then(|value| value.to_str().ok())
            .map(Arc::from);
        let content_type = content_type(&response);
        if !status.is_success() {
            let body = response
                .text(self.options.max_response_bytes)
                .await
                .unwrap_or_default();
            if !status.is_server_error()
                && is_mime(content_type.as_deref(), JSON_MIME_TYPE)
                && let Ok(message @ Message::Error { .. }) = Message::from_slice(body.as_bytes())
            {
                return Ok((
                    with_request_id(message, initialize.request_id()),
                    session_id,
                ));
            }
            return Err(TransportError::with_status(
                format!("unexpected server response: HTTP {status}: {body}"),
                status.as_u16(),
            ));
        }
        if is_mime(content_type.as_deref(), EVENT_STREAM_MIME_TYPE) {
            let mut parser = sse::Parser::new(self.event_limit());
            let mut events = Vec::new();
            let mut read = 0_usize;
            while let Some(chunk) = response
                .chunk()
                .await
                .map_err(|error| TransportError::new(format!("SSE error: {error}")))?
            {
                read = read.saturating_add(chunk.len());
                if let Some(limit) = self
                    .options
                    .max_response_bytes
                    .filter(|limit| read > *limit)
                {
                    return Err(exceeded(limit));
                }
                parser
                    .feed(&chunk, &mut events)
                    .map_err(|error| TransportError::new(format!("SSE error: {error}")))?;
                for event in events.drain(..) {
                    let Some(data) = event.data.filter(|data| !data.trim().is_empty()) else {
                        continue;
                    };
                    let message = Message::from_slice(data.as_bytes()).map_err(|error| {
                        TransportError::new(format!("Deserialize error: {error}"))
                    })?;
                    if message.is_response() {
                        return Ok((message, session_id));
                    }
                    tracing::debug!(
                        ?message,
                        "received message before initialize response; continuing to drain stream"
                    );
                }
            }
            return Err("unexpected server response: empty sse stream".into());
        }
        if is_mime(content_type.as_deref(), JSON_MIME_TYPE) {
            let body = response.bytes(self.options.max_response_bytes).await?;
            let message = Message::from_slice(&body)
                .map_err(|error| TransportError::new(format!("Deserialize error: {error}")))?;
            return Ok((message, session_id));
        }
        Err(TransportError::new(format!(
            "Unexpected content type: {content_type:?}"
        )))
    }

    async fn dispatch(self: Arc<Self>, message: Message) {
        let request_id = message.request_id().cloned();
        if let Err(error) = self.post_with_recovery(&message).await {
            match request_id {
                Some(id) => self.inbound.fail(&id, SessionError::Transport(error)),
                None => tracing::warn!(%error, "failed to send MCP message"),
            }
        }
    }

    async fn post_with_recovery(self: &Arc<Self>, message: &Message) -> Result<(), TransportError> {
        let state = self.state();
        match self.post(message, &state).await {
            Err(PostError::SessionExpired) if state.initialize.is_some() => {
                tracing::info!(
                    "session expired (HTTP 404), attempting transparent re-initialization"
                );
                self.reinitialize(state.id.clone()).await.map_err(|error| {
                    TransportError::with_status(format!("{SESSION_EXPIRED}: {error}"), 404)
                })?;
                let state = self.state();
                self.post(message, &state)
                    .await
                    .map_err(TransportError::from)
            }
            result => result.map_err(TransportError::from),
        }
    }

    async fn reinitialize(self: &Arc<Self>, stale: Option<Arc<str>>) -> Result<(), String> {
        let shared = Arc::clone(self);
        self.reinitialize
            .run(|| async move { shared.renew(stale).await })
            .await
    }

    async fn renew(self: Arc<Self>, stale: Option<Arc<str>>) -> Result<(), String> {
        let state = self.state();
        if state.id != stale {
            return Ok(());
        }
        let Some(initialize) = state.initialize else {
            return Err(SESSION_EXPIRED.to_owned());
        };
        let (response, session_id) = self
            .post_initialize(&initialize)
            .await
            .map_err(|error| error.to_string())?;
        if let Message::Error { error, .. } = &response {
            return Err(format!("MCP re-initialization failed: {error}"));
        }
        let state = SessionState {
            id: session_id,
            protocol_version: negotiated_version(&response),
            initialize: Some(initialize),
        };
        self.post(&Message::notification(INITIALIZED, None), &state)
            .await
            .map_err(|error| TransportError::from(error).to_string())?;
        self.session.store(Arc::new(state));
        self.restart_listener();
        Ok(())
    }

    async fn post(
        self: &Arc<Self>,
        message: &Message,
        state: &SessionState,
    ) -> Result<(), PostError> {
        let body = message.to_vec().map_err(|error| {
            PostError::Failed(TransportError::unsent(format!(
                "Deserialize error: {error}"
            )))
        })?;
        let response = self
            .send(&Method::POST, post_headers(session_headers(state)), &body)
            .await
            .map_err(PostError::Failed)?;
        if let Some(error) = challenge_error(&response) {
            return Err(PostError::Failed(error));
        }
        let status = response.status;
        if matches!(status, StatusCode::ACCEPTED | StatusCode::NO_CONTENT) {
            return Ok(());
        }
        if status == StatusCode::NOT_FOUND && state.id.is_some() {
            return Err(PostError::SessionExpired);
        }
        let content_type = content_type(&response);
        let empty = response
            .headers
            .get(CONTENT_LENGTH)
            .is_some_and(|length| length.as_bytes() == b"0");
        if status.is_success() && empty && !message.is_request() {
            return Ok(());
        }
        if !status.is_success() {
            let body = response
                .text(self.options.max_response_bytes)
                .await
                .unwrap_or_else(|error| format!("<failed to read response body: {error}>"));
            if !status.is_server_error() && is_mime(content_type.as_deref(), JSON_MIME_TYPE) {
                match Message::from_slice(body.as_bytes()) {
                    Ok(Message::Error { id, error }) => {
                        let error = Message::Error {
                            id,
                            error: error.with_status(status.as_u16()),
                        };
                        self.inbound
                            .deliver(with_request_id(error, message.request_id()));
                        return Ok(());
                    }
                    _ => tracing::warn!(
                        "HTTP {status}: could not parse JSON body as a JSON-RPC error"
                    ),
                }
            }
            return Err(PostError::Failed(TransportError::with_status(
                format!("unexpected server response: HTTP {status}: {body}"),
                status.as_u16(),
            )));
        }
        if is_mime(content_type.as_deref(), EVENT_STREAM_MIME_TYPE) {
            return self
                .follow_response_stream(response, message.request_id())
                .await;
        }
        if is_mime(content_type.as_deref(), JSON_MIME_TYPE) {
            let body = response
                .bytes(self.options.max_response_bytes)
                .await
                .map_err(PostError::Failed)?;
            return match Message::from_slice(&body) {
                Ok(reply) => {
                    self.inbound
                        .deliver(with_request_id(reply, message.request_id()));
                    Ok(())
                }
                Err(error) if message.is_request() => Err(PostError::Failed(TransportError::new(
                    format!("Deserialize error: {error}"),
                ))),
                Err(error) => {
                    tracing::warn!(
                        "could not parse JSON response as a JSON-RPC message, treating as accepted: {error}"
                    );
                    Ok(())
                }
            };
        }
        tracing::error!("unexpected content type: {content_type:?}");
        Err(PostError::Failed(TransportError::new(format!(
            "Unexpected content type: {content_type:?}"
        ))))
    }

    async fn follow_response_stream(
        self: &Arc<Self>,
        response: Reply,
        request_id: Option<&RequestId>,
    ) -> Result<(), PostError> {
        let mut cursor = Cursor::default();
        let mut response = response;
        loop {
            let end = self.consume(response, &mut cursor, true).await;
            let Some(id) = request_id else {
                return Ok(());
            };
            let wait = match end {
                StreamEnd::Response => return Ok(()),
                StreamEnd::Fatal(error) => {
                    tracing::warn!("fatal sse stream error: {error}");
                    return Err(PostError::Failed(TransportError::new(format!(
                        "{CLOSED_BEFORE_RESPONSE}: {error}"
                    ))));
                }
                _ if cursor.last_event_id.is_none() => {
                    tracing::debug!(
                        "sse response ended before an event ID was received; cannot resume"
                    );
                    return Err(PostError::Failed(CLOSED_BEFORE_RESPONSE.into()));
                }
                StreamEnd::Closed => cursor.retry.take().unwrap_or(SSE_RETRY_BASE),
                StreamEnd::Failed(error) => {
                    tracing::warn!("sse stream error: {error}");
                    Duration::ZERO
                }
            };
            response = self
                .reconnect(&mut cursor, wait, Some(id))
                .await
                .ok_or_else(|| PostError::Failed(CLOSED_BEFORE_RESPONSE.into()))?;
        }
    }

    async fn consume(
        &self,
        mut response: Reply,
        cursor: &mut Cursor,
        close_on_response: bool,
    ) -> StreamEnd {
        let mut parser = sse::Parser::new(self.event_limit());
        let mut events = Vec::new();
        let mut read = 0_usize;
        loop {
            let chunk = match response.chunk().await {
                Ok(Some(chunk)) => chunk,
                Ok(None) => return StreamEnd::Closed,
                Err(error) => return StreamEnd::Failed(error.to_string()),
            };
            read = read.saturating_add(chunk.len());
            if close_on_response
                && let Some(limit) = self
                    .options
                    .max_response_bytes
                    .filter(|limit| read > *limit)
            {
                return StreamEnd::Fatal(exceeded(limit).to_string());
            }
            if let Err(error) = parser.feed(&chunk, &mut events) {
                return StreamEnd::Fatal(error.to_string());
            }
            for event in events.drain(..) {
                if let Some(retry) = event.retry {
                    cursor.retry = Some(Duration::from_millis(retry));
                }
                if let Some(id) = &event.id {
                    cursor.last_event_id = Some(id.clone());
                }
                if !event.is_message() {
                    continue;
                }
                let Some(data) = event.data else {
                    continue;
                };
                let message = match Message::from_slice(data.as_bytes()) {
                    Ok(message) => message,
                    Err(error) => {
                        tracing::debug!(
                            last_event_id = cursor.last_event_id.as_deref().unwrap_or(""),
                            "failed to deserialize server message: {error}"
                        );
                        continue;
                    }
                };
                let is_response = message.is_response();
                if !self.inbound.deliver(message) {
                    return StreamEnd::Closed;
                }
                if close_on_response && is_response {
                    let _ = timeout(RESPONSE_DRAIN_TIMEOUT, async {
                        while let Ok(Some(_)) = response.chunk().await {}
                    })
                    .await;
                    return StreamEnd::Response;
                }
            }
        }
    }

    async fn reconnect(
        &self,
        cursor: &mut Cursor,
        initial_wait: Duration,
        pending: Option<&RequestId>,
    ) -> Option<Reply> {
        let mut wait = initial_wait;
        let mut attempt = 0_u32;
        loop {
            if !wait.is_zero() {
                sleep(wait).await;
            }
            if !self.inbound.is_alive() || pending.is_some_and(|id| !self.inbound.is_pending(id)) {
                return None;
            }
            match self.get(cursor.last_event_id.as_deref()).await {
                Ok(Some(response)) => return Some(response),
                Ok(None) => tracing::debug!("retry sse stream error: server does not support SSE"),
                Err(error) => tracing::debug!("retry sse stream error: {error}"),
            }
            attempt = attempt.saturating_add(1);
            let backoff = SSE_RETRY_BASE.saturating_mul(2_u32.saturating_pow(attempt));
            wait = cursor.retry.map_or(backoff, |retry| retry.max(backoff));
        }
    }

    async fn get(&self, last_event_id: Option<&str>) -> Result<Option<Reply>, String> {
        let state = self.state();
        let mut headers = session_headers(&state);
        headers.insert(ACCEPT, HeaderValue::from_static(ACCEPT_MCP));
        if let Some(last_event_id) = last_event_id {
            let value = HeaderValue::from_str(last_event_id)
                .map_err(|error| format!("Client error: {error}"))?;
            headers.insert(HEADER_LAST_EVENT_ID, value);
        }
        let response = self
            .send(&Method::GET, headers, &[])
            .await
            .map_err(|error| error.to_string())?;
        if response.status == StatusCode::METHOD_NOT_ALLOWED {
            return Ok(None);
        }
        if !response.status.is_success() {
            return Err(format!(
                "Client error: HTTP status {} for url ({})",
                response.status, self.url
            ));
        }
        let content_type = content_type(&response);
        if !is_mime(content_type.as_deref(), EVENT_STREAM_MIME_TYPE)
            && !is_mime(content_type.as_deref(), JSON_MIME_TYPE)
        {
            return Err(format!("Unexpected content type: {content_type:?}"));
        }
        Ok(Some(response))
    }

    fn event_limit(&self) -> usize {
        self.options
            .max_response_bytes
            .map_or(MAX_SSE_EVENT_BYTES, |limit| limit.min(MAX_SSE_EVENT_BYTES))
    }

    fn restart_listener(self: &Arc<Self>) {
        if let Some(previous) = self.listener.take() {
            previous.abort();
        }
        if self.options.standalone_stream
            && self.session.load().id.is_some()
            && let Some(raced) = self
                .listener
                .put(spawn_background(Arc::clone(self).listen()))
        {
            raced.abort();
        }
    }

    async fn listen(self: Arc<Self>) {
        let mut cursor = Cursor::default();
        let mut response = match self.get(None).await {
            Ok(Some(response)) => response,
            Ok(None) => {
                tracing::debug!("server doesn't support sse, skip common stream");
                return;
            }
            Err(error) => {
                tracing::error!("fail to get common stream: {error}");
                return;
            }
        };
        loop {
            let wait = match self.consume(response, &mut cursor, false).await {
                StreamEnd::Fatal(error) => {
                    tracing::warn!("sse client event stream terminated with error: {error}");
                    return;
                }
                StreamEnd::Closed | StreamEnd::Response => {
                    cursor.retry.take().unwrap_or(SSE_RETRY_BASE)
                }
                StreamEnd::Failed(error) => {
                    tracing::warn!("sse stream error: {error}");
                    Duration::ZERO
                }
            };
            match self.reconnect(&mut cursor, wait, None).await {
                Some(next) => response = next,
                None => return,
            }
        }
    }

    async fn delete_session(&self) {
        let state = self.state();
        let Some(session_id) = state.id.clone() else {
            return;
        };
        match timeout(
            SESSION_CLEANUP_TIMEOUT,
            self.send(&Method::DELETE, session_headers(&state), &[]),
        )
        .await
        {
            Ok(Ok(response)) if response.status == StatusCode::METHOD_NOT_ALLOWED => {
                tracing::debug!("this server doesn't support deleting session");
            }
            Ok(Ok(response)) if response.status.is_success() => {
                tracing::info!(session_id = session_id.as_ref(), "delete session success");
            }
            Ok(Ok(response)) => tracing::error!(
                session_id = session_id.as_ref(),
                "fail to delete session: HTTP status {}",
                response.status
            ),
            Ok(Err(error)) => tracing::error!(
                session_id = session_id.as_ref(),
                "fail to delete session: {error}"
            ),
            Err(_) => tracing::warn!(
                session_id = session_id.as_ref(),
                "session cleanup timed out after {SESSION_CLEANUP_TIMEOUT:?}"
            ),
        }
    }
}

fn session_headers(state: &SessionState) -> HeaderMap {
    let mut headers = HeaderMap::new();
    if let Some(id) = &state.id
        && let Ok(value) = HeaderValue::from_str(id)
    {
        headers.insert(HEADER_SESSION_ID, value);
    }
    if let Some(version) = &state.protocol_version {
        headers.insert(HEADER_PROTOCOL_VERSION, version.clone());
    }
    headers
}

fn post_headers(mut headers: HeaderMap) -> HeaderMap {
    headers.insert(ACCEPT, HeaderValue::from_static(ACCEPT_MCP));
    headers.insert(CONTENT_TYPE, HeaderValue::from_static(JSON_MIME_TYPE));
    headers
}

fn with_request_id(message: Message, request_id: Option<&RequestId>) -> Message {
    match (message, request_id) {
        (Message::Error { id: None, error }, Some(id)) => Message::Error {
            id: Some(id.clone()),
            error,
        },
        (message, _) => message,
    }
}
