use std::{
    collections::HashMap,
    fmt,
    sync::{
        Arc, Weak,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
    time::Duration,
};

use nanocode_sync::{Slot, Swap, SwapOption};

use nanocode_sync::unbounded;
use tokio::sync::oneshot;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};

use crate::{
    jsonrpc::{METHOD_NOT_FOUND, Message, RequestId, RpcError},
    protocol::{
        CANCELLED, CallToolParams, CallToolResult, ELICITATION_CREATE, INITIALIZE, INITIALIZED,
        Implementation, InitializeParams, InitializeResult, ListResourceTemplatesResult,
        ListResourcesResult, ListToolsResult, PING, PaginatedParams, RESOURCE_TEMPLATES_LIST,
        RESOURCES_LIST, RESOURCES_READ, ROOTS_LIST, ReadResourceParams, ReadResourceResult,
        TOOLS_CALL, TOOLS_LIST,
    },
};

#[derive(Clone, Debug, PartialEq)]
pub enum Error {
    Rpc(RpcError),
    Transport(TransportError),
    Closed,
    Cancelled(Option<String>),
    InvalidResult { method: String, error: String },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Rpc(error) => write!(f, "Mcp error: {error}"),
            Self::Transport(error) => write!(f, "Transport send error: {error}"),
            Self::Closed => f.write_str("Transport closed"),
            Self::Cancelled(reason) => write!(
                f,
                "task cancelled for reason {}",
                reason.as_deref().unwrap_or("<unknown>")
            ),
            Self::InvalidResult { method, error } => {
                write!(f, "Unexpected response type for {method}: {error}")
            }
        }
    }
}

impl std::error::Error for Error {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransportError {
    message: String,
    unsent: bool,
    status: Option<u16>,
    retry_after: Option<Duration>,
}

impl TransportError {
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            unsent: false,
            status: None,
            retry_after: None,
        }
    }

    #[must_use]
    pub fn unsent(message: impl Into<String>) -> Self {
        Self {
            unsent: true,
            ..Self::new(message)
        }
    }

    #[must_use]
    pub fn with_status(message: impl Into<String>, status: u16) -> Self {
        Self {
            status: Some(status),
            ..Self::new(message)
        }
    }

    #[must_use]
    pub const fn with_retry_after(mut self, retry_after: Option<Duration>) -> Self {
        self.retry_after = retry_after;
        self
    }

    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }

    #[must_use]
    pub const fn is_unsent(&self) -> bool {
        self.unsent
    }

    #[must_use]
    pub const fn status(&self) -> Option<u16> {
        self.status
    }

    #[must_use]
    pub const fn retry_after(&self) -> Option<Duration> {
        self.retry_after
    }
}

impl fmt::Display for TransportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for TransportError {}

impl From<String> for TransportError {
    fn from(message: String) -> Self {
        Self::new(message)
    }
}

impl From<&str> for TransportError {
    fn from(message: &str) -> Self {
        Self::new(message)
    }
}

fn unsent_closed() -> Error {
    Error::Transport(TransportError::unsent("Transport closed"))
}

const CLIENT_CANCELLED_REASON: &str = "request cancelled by client";

type Reply = oneshot::Sender<Result<Value, Error>>;

pub type Outgoing = unbounded::Receiver<Message>;

type Pending = HashMap<RequestId, Arc<Slot<Reply>>>;

struct Shared {
    outbound: unbounded::Sender<Message>,
    next_id: AtomicU32,
    pending: Swap<Pending>,
    closed: AtomicBool,
    server: SwapOption<InitializeResult>,
}

impl Shared {
    fn take_pending(&self, id: &RequestId) -> Option<Reply> {
        let numeric = id.numeric_equivalent();
        let mut taken = None;
        self.pending.rcu(|pending| {
            let mut pending = Pending::clone(pending);
            taken = pending
                .remove(id)
                .or_else(|| numeric.as_ref().and_then(|id| pending.remove(id)));
            pending
        });
        taken.and_then(|slot| slot.take())
    }

    fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    fn close(&self) {
        self.closed.store(true, Ordering::Release);
        let pending = self.pending.swap(Arc::default());
        for slot in pending.values() {
            if let Some(reply) = slot.take() {
                let _ = reply.send(Err(Error::Closed));
            }
        }
        self.outbound.close();
    }
}

#[derive(Clone)]
pub struct Session {
    shared: Arc<Shared>,
}

#[derive(Clone)]
pub struct Inbound {
    shared: Weak<Shared>,
}

struct PendingGuard<'a> {
    shared: &'a Shared,
    id: Option<RequestId>,
    cancellable: bool,
}

impl Drop for PendingGuard<'_> {
    fn drop(&mut self) {
        let Some(id) = self.id.take() else {
            return;
        };
        let pending = self.shared.take_pending(&id).is_some() && !self.shared.is_closed();
        if pending && self.cancellable {
            let _ = self.shared.outbound.send(Message::notification(
                CANCELLED,
                Some(json!({"requestId": id, "reason": CLIENT_CANCELLED_REASON})),
            ));
        }
    }
}

impl Session {
    #[must_use]
    pub fn new() -> (Self, Outgoing, Inbound) {
        let (outbound, receiver) = unbounded::channel();
        let shared = Arc::new(Shared {
            outbound,
            next_id: AtomicU32::new(0),
            pending: Swap::default(),
            closed: AtomicBool::new(false),
            server: SwapOption::empty(),
        });
        let inbound = Inbound {
            shared: Arc::downgrade(&shared),
        };
        (Self { shared }, receiver, inbound)
    }

    #[must_use]
    pub fn inbound(&self) -> Inbound {
        Inbound {
            shared: Arc::downgrade(&self.shared),
        }
    }

    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.shared.is_closed()
    }

    pub fn close(&self) {
        self.shared.close();
    }

    #[must_use]
    pub fn server(&self) -> Option<InitializeResult> {
        self.shared.server.load().as_deref().cloned()
    }

    #[must_use]
    pub fn supports_tools(&self) -> bool {
        self.shared
            .server
            .load()
            .as_ref()
            .is_some_and(|server| server.capabilities.supports_tools())
    }

    #[must_use]
    pub fn supports_resources(&self) -> bool {
        self.shared
            .server
            .load()
            .as_ref()
            .is_some_and(|server| server.capabilities.supports_resources())
    }

    pub async fn request_value(&self, method: &str, params: Option<Value>) -> Result<Value, Error> {
        let (reply, response) = oneshot::channel();
        if self.shared.is_closed() {
            return Err(unsent_closed());
        }
        let id = RequestId::Number(i64::from(
            self.shared.next_id.fetch_add(1, Ordering::Relaxed),
        ));
        let slot = Arc::new(Slot::new(Some(reply)));
        self.shared.pending.rcu(|pending| {
            let mut pending = Pending::clone(pending);
            pending.insert(id.clone(), Arc::clone(&slot));
            pending
        });
        if self.shared.is_closed() {
            drop(self.shared.take_pending(&id));
            return Err(unsent_closed());
        }
        let mut guard = PendingGuard {
            shared: &self.shared,
            id: Some(id.clone()),
            cancellable: method != INITIALIZE,
        };
        self.shared
            .outbound
            .send(Message::request(id, method, params))
            .map_err(|_| unsent_closed())?;
        let result = response.await.map_err(|_| Error::Closed)?;
        guard.id = None;
        result
    }

    pub async fn request<P, R>(&self, method: &str, params: &P) -> Result<R, Error>
    where
        P: Serialize + ?Sized,
        R: DeserializeOwned,
    {
        let params = serde_json::to_value(params).map_err(|error| {
            Error::Transport(TransportError::unsent(format!(
                "Serialize error for {method}: {error}"
            )))
        })?;
        let result = self.request_value(method, Some(params)).await?;
        serde_json::from_value(result).map_err(|error| Error::InvalidResult {
            method: method.to_owned(),
            error: error.to_string(),
        })
    }

    pub fn notify(&self, method: &str, params: Option<Value>) -> Result<(), Error> {
        if self.shared.is_closed() {
            return Err(Error::Closed);
        }
        self.shared
            .outbound
            .send(Message::notification(method, params))
            .map_err(|_| Error::Closed)
    }

    pub async fn initialize(&self, client_info: Implementation) -> Result<InitializeResult, Error> {
        let result: InitializeResult = self
            .request(INITIALIZE, &InitializeParams::new(client_info))
            .await?;
        self.shared.server.store(Some(Arc::new(result.clone())));
        self.notify(INITIALIZED, None)?;
        Ok(result)
    }

    pub async fn list_tools(&self, cursor: Option<String>) -> Result<ListToolsResult, Error> {
        self.paginated(TOOLS_LIST, cursor).await
    }

    pub async fn list_resources(
        &self,
        cursor: Option<String>,
    ) -> Result<ListResourcesResult, Error> {
        self.paginated(RESOURCES_LIST, cursor).await
    }

    pub async fn list_resource_templates(
        &self,
        cursor: Option<String>,
    ) -> Result<ListResourceTemplatesResult, Error> {
        self.paginated(RESOURCE_TEMPLATES_LIST, cursor).await
    }

    pub async fn read_resource(&self, uri: &str) -> Result<ReadResourceResult, Error> {
        self.request(
            RESOURCES_READ,
            &ReadResourceParams {
                uri: uri.to_owned(),
            },
        )
        .await
    }

    pub async fn call_tool(&self, params: &CallToolParams) -> Result<CallToolResult, Error> {
        self.request(TOOLS_CALL, params).await
    }

    async fn paginated<R: DeserializeOwned>(
        &self,
        method: &str,
        cursor: Option<String>,
    ) -> Result<R, Error> {
        match cursor {
            Some(cursor) => {
                self.request(
                    method,
                    &PaginatedParams {
                        cursor: Some(cursor),
                    },
                )
                .await
            }
            None => {
                let result = self.request_value(method, None).await?;
                serde_json::from_value(result).map_err(|error| Error::InvalidResult {
                    method: method.to_owned(),
                    error: error.to_string(),
                })
            }
        }
    }
}

impl Inbound {
    pub fn deliver(&self, message: Message) -> bool {
        let Some(shared) = self.shared.upgrade() else {
            return false;
        };
        match message {
            Message::Response { id, result } => complete(&shared, &id, Ok(result)),
            Message::Error {
                id: Some(id),
                error,
            } => complete(&shared, &id, Err(Error::Rpc(error))),
            Message::Error { id: None, error } => {
                nanocode_trace::debug!(%error, "MCP server sent an error without a request id");
            }
            Message::Request { id, method, .. } => {
                let reply = server_request_reply(id, &method);
                let _ = shared.outbound.send(reply);
            }
            Message::Notification { method, params } if method == CANCELLED => {
                let params = params.unwrap_or_default();
                let reason = params
                    .get("reason")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                if let Some(id) = params
                    .get("requestId")
                    .cloned()
                    .and_then(|id| serde_json::from_value::<RequestId>(id).ok())
                {
                    nanocode_trace::info!(%id, reason, "MCP server cancelled request");
                    complete(&shared, &id, Err(Error::Cancelled(reason)));
                }
            }
            Message::Notification { method, .. } => {
                nanocode_trace::trace!(%method, "MCP server notification");
            }
        }
        true
    }

    pub fn send(&self, message: Message) -> bool {
        self.shared
            .upgrade()
            .is_some_and(|shared| shared.outbound.send(message).is_ok())
    }

    #[must_use]
    pub fn is_pending(&self, id: &RequestId) -> bool {
        self.shared.upgrade().is_some_and(|shared| {
            let pending = shared.pending.load();
            pending.contains_key(id)
                || id
                    .numeric_equivalent()
                    .is_some_and(|id| pending.contains_key(&id))
        })
    }

    pub fn fail(&self, id: &RequestId, error: Error) {
        if let Some(shared) = self.shared.upgrade() {
            complete(&shared, id, Err(error));
        }
    }

    pub fn close(&self) {
        if let Some(shared) = self.shared.upgrade() {
            shared.close();
        }
    }

    #[must_use]
    pub fn is_alive(&self) -> bool {
        self.shared.strong_count() > 0
    }
}

fn complete(shared: &Shared, id: &RequestId, result: Result<Value, Error>) {
    match shared.take_pending(id) {
        Some(reply) => {
            let _ = reply.send(result);
        }
        None => nanocode_trace::debug!(%id, "MCP response did not match a pending request"),
    }
}

#[must_use]
pub fn server_request_reply(id: RequestId, method: &str) -> Message {
    let result = match method {
        PING => json!({}),
        ROOTS_LIST => json!({"roots": []}),
        ELICITATION_CREATE => json!({"action": "decline"}),
        _ => {
            return Message::Error {
                id: Some(id),
                error: RpcError::new(METHOD_NOT_FOUND, method),
            };
        }
    };
    Message::Response { id, result }
}
