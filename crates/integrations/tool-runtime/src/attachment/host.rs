use std::{
    collections::HashMap,
    fmt,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use async_trait::async_trait;
use futures_util::{Sink, SinkExt, Stream, StreamExt, future};
use nanocode_sync::{Atom, Slot};
use serde::Deserialize;
use serde_json::{Value, json, value::RawValue};
use tokio::{
    net::{TcpListener, TcpStream, ToSocketAddrs},
    sync::{mpsc, oneshot, watch},
};
use tokio_tungstenite::tungstenite::{
    self, Message,
    handshake::server::{ErrorResponse, Request, Response},
    protocol::{CloseFrame, frame::coding::CloseCode},
};

use super::AttachmentError;
use crate::{
    Tool, ToolContext, ToolDefinition, ToolInput, ToolOutput, ToolResult,
    contract::{ToolError, ToolOutputWire},
};

use super::protocol::JSON_SAFE_INTEGER_MAX;
const RECOVER_BATCH_LIMIT: usize = 100;
const PUBLIC_TOOL_NAME_LIMIT: usize = 128;
const PROTOCOL_IDENTIFIER_LIMIT: usize = 128;
const TURN_ID_LIMIT: usize = 256;
const WEBSOCKET_CLOSE_REASON_LIMIT: usize = 123;
const NAMESPACE_SEPARATOR: &str = "__";

#[derive(Clone)]
pub struct RemoteRunner {
    inner: Arc<Inner>,
}

struct Inner {
    namespace: Box<str>,
    catalog: watch::Sender<Option<Arc<[Arc<RemoteSpec>]>>>,
    failure: watch::Sender<Option<AttachmentError>>,
    connected: watch::Sender<bool>,
    state: Atom<State>,
}

#[derive(Clone, Default)]
struct State {
    revoked: bool,
    generation: u64,
    connection: Option<Connection>,
    runtime_id: Option<Box<str>>,
    catalog: Option<Arc<Value>>,
    pending: HashMap<Box<str>, Pending>,
    last_turn_ended: Option<[Box<str>; 3]>,
    deferred_frames: Vec<String>,
    call_timeout_ms: Option<u64>,
    workspace: Option<PathBuf>,
}

#[derive(Clone)]
struct Connection {
    generation: u64,
    outbound: mpsc::UnboundedSender<Outbound>,
}

type Reply = oneshot::Sender<(Value, Option<Box<str>>)>;

#[derive(Clone)]
struct Pending {
    frame: Arc<str>,
    sent: bool,
    result: Arc<Slot<Reply>>,
}

impl Pending {
    fn resolve(&self, outcome: Value, runtime_id: Option<Box<str>>) {
        if let Some(result) = self.result.take() {
            let _ = result.send((outcome, runtime_id));
        }
    }
}

enum Outbound {
    Call(Box<str>),
    Frame(String),
}

struct RemoteSpec {
    remote_name: Box<str>,
    definition: ToolDefinition,
    parallel_safe: bool,
    timeout_ms: u64,
    routing: Option<nanocode_tool_host::WorkspaceToolRouting>,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum RunnerFrame {
    Catalog {
        runtime_id: String,
        tools: Value,
        #[serde(default)]
        machines: Vec<RunnerMachine>,
    },
    Diagnostic {},
    Result {
        call_id: String,
        outcome: Value,
    },
    Status {
        call_id: String,
        state: String,
    },
    Drain {},
}

#[derive(Deserialize)]
struct RunnerMachine {
    workspace: PathBuf,
}

#[derive(Deserialize)]
struct CatalogEntry {
    definition: ToolDefinition,
    parallel_safe: bool,
    timeout_ms: u64,
    #[serde(default)]
    routing: Option<Box<str>>,
}

#[derive(Deserialize)]
struct Outcome {
    status: OutcomeStatus,
    #[serde(default)]
    output: Option<Box<RawValue>>,
    #[serde(default)]
    message: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum OutcomeStatus {
    Completed,
    Unavailable,
    Ambiguous,
}

impl fmt::Debug for RemoteRunner {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RemoteRunner")
            .field("namespace", &self.inner.namespace)
            .field("connected", &self.is_connected())
            .finish_non_exhaustive()
    }
}

impl RemoteRunner {
    pub fn new(namespace: impl Into<String>) -> Result<Self, AttachmentError> {
        let namespace = namespace.into();
        if !valid_namespace(&namespace) {
            return Err(AttachmentError::Catalog(
                format!(
                    "runner namespace `{namespace}` must start with an ASCII letter, contain only ASCII alphanumerics, `_`, or `-`, and must not contain `__` or end with `_`"
                )
                .into(),
            ));
        }
        Ok(Self {
            inner: Arc::new(Inner {
                namespace: namespace.into(),
                catalog: watch::channel(None).0,
                failure: watch::channel(None).0,
                connected: watch::channel(false).0,
                state: Atom::default(),
            }),
        })
    }

    pub fn with_call_timeout(self, timeout: Duration) -> Result<Self, AttachmentError> {
        let timeout_ms = u64::try_from(timeout.as_millis()).map_err(transport_error)?;
        if timeout_ms == 0
            || timeout_ms > JSON_SAFE_INTEGER_MAX
            || Duration::from_millis(timeout_ms) != timeout
        {
            return Err(AttachmentError::Transport("call timeout must be positive whole milliseconds within the protocol integer range".into()));
        }
        self.inner
            .state
            .update(|state| state.call_timeout_ms = Some(timeout_ms));
        Ok(self)
    }

    #[must_use]
    pub fn namespace(&self) -> &str {
        &self.inner.namespace
    }

    #[must_use]
    pub fn is_connected(&self) -> bool {
        self.inner.state.read(|state| state.connection.is_some())
    }

    pub(crate) fn runtime_id(&self) -> Option<Box<str>> {
        self.inner.state.read(|state| state.runtime_id.clone())
    }

    pub(crate) fn connection_status(&self) -> watch::Receiver<bool> {
        self.inner.connected.subscribe()
    }

    pub(crate) fn failure_status(&self) -> watch::Receiver<Option<AttachmentError>> {
        self.inner.failure.subscribe()
    }

    pub(crate) fn revoke(&self) {
        let pending = self.inner.state.update(|state| {
            state.revoked = true;
            std::mem::take(&mut state.pending)
        });
        for (_, pending) in pending {
            let (status, message) = if pending.sent {
                (
                    "ambiguous",
                    "runner was disconnected before returning a receipt",
                )
            } else {
                (
                    "unavailable",
                    "runner was disconnected before the call was dispatched",
                )
            };
            pending.resolve(json!({"status": status, "message": message}), None);
        }
    }

    pub(crate) fn tools_snapshot(&self) -> Option<RemoteTools> {
        let specs = self.inner.catalog.borrow().clone()?;
        Some(RemoteTools {
            namespace: self.inner.namespace.clone(),
            workspace: self.inner.state.read(|state| state.workspace.clone()),
            tools: specs
                .iter()
                .map(|spec| RemoteTool {
                    runner: Arc::clone(&self.inner),
                    spec: Arc::clone(spec),
                })
                .collect(),
        })
    }

    pub fn tools<'call>(
        &'call self,
    ) -> futures_util::future::BoxFuture<'call, Result<RemoteTools, AttachmentError>> {
        Box::pin(async move {
            let mut catalog = self.inner.catalog.subscribe();
            let mut failure = self.inner.failure.subscribe();
            let timeout_ms = self.inner.state.read(|state| state.call_timeout_ms);
            let wait = async {
                tokio::select! {
                    result = catalog.wait_for(Option::is_some) => result.map_err(|_| AttachmentError::Closed)?.clone().ok_or(AttachmentError::Closed),
                    result = failure.wait_for(Option::is_some) => Err(result.map_err(|_| AttachmentError::Closed)?.clone().unwrap_or(AttachmentError::Closed)),
                }
            };
            let specs = match timeout_ms {
            Some(ms) => tokio::time::timeout(Duration::from_millis(ms), wait)
                .await
                .map_err(|_| AttachmentError::Transport(format!("runner `{}` unavailable: no catalog before the configured connection deadline", self.namespace()).into()))??,
            None => wait.await?,
        };
            Ok(RemoteTools {
                namespace: self.inner.namespace.clone(),
                workspace: self.inner.state.read(|state| state.workspace.clone()),
                tools: specs
                    .iter()
                    .map(|spec| RemoteTool {
                        runner: Arc::clone(&self.inner),
                        spec: Arc::clone(spec),
                    })
                    .collect(),
            })
        })
    }

    pub async fn serve<T, E>(&self, mut transport: T) -> Result<(), AttachmentError>
    where
        T: Stream<Item = Result<String, E>> + Sink<String, Error = E> + Unpin,
        E: fmt::Display,
    {
        let first = match tokio::time::timeout(super::driver::HANDSHAKE_TIMEOUT, transport.next())
            .await
            .map_err(transport_error)?
        {
            Some(Ok(text)) => text,
            Some(Err(error)) => return Err(transport_error(error)),
            None => {
                return Err(AttachmentError::Transport(
                    "runner closed before publishing its catalog".into(),
                ));
            }
        };
        self.serve_catalog(first, transport).await
    }

    pub(crate) async fn serve_catalog<T, E>(
        &self,
        first: String,
        mut transport: T,
    ) -> Result<(), AttachmentError>
    where
        T: Stream<Item = Result<String, E>> + Sink<String, Error = E> + Unpin,
        E: fmt::Display,
    {
        let RunnerFrame::Catalog {
            runtime_id,
            tools,
            machines,
        } = parse_frame(&first)?
        else {
            return Err(AttachmentError::Fenced(
                "runner must publish its catalog first".into(),
            ));
        };
        if !valid_identifier(&runtime_id) {
            return Err(AttachmentError::Catalog(
                "invalid runner runtime identity".into(),
            ));
        }
        let workspace = match machines.as_slice() {
            [] => None,
            [machine] if machine.workspace.is_absolute() => Some(machine.workspace.clone()),
            _ => {
                return Err(AttachmentError::Catalog(
                    "runner must advertise one absolute workspace for automatic routing".into(),
                ));
            }
        };
        let (generation, mut outbound, backlog) =
            self.inner.attach(&runtime_id, tools, workspace)?;
        let _lease = ConnectionLease {
            inner: Arc::clone(&self.inner),
            generation,
        };
        let result = self
            .connection(&mut transport, generation, &mut outbound, backlog)
            .await;
        self.inner.detach(generation);
        result
    }

    async fn connection<T, E>(
        &self,
        transport: &mut T,
        generation: u64,
        outbound: &mut mpsc::UnboundedReceiver<Outbound>,
        backlog: Vec<String>,
    ) -> Result<(), AttachmentError>
    where
        T: Stream<Item = Result<String, E>> + Sink<String, Error = E> + Unpin,
        E: fmt::Display,
    {
        send(transport, json!({"type": "ready"}).to_string()).await?;
        for frame in backlog {
            send(transport, frame).await?;
        }
        let mut draining = false;
        loop {
            tokio::select! {
                command = outbound.recv(), if !draining => {
                    let Some(command) = command else { return Ok(()) };
                    let frame = match command {
                        Outbound::Call(call_id) => match self.inner.claim(generation, &call_id) {
                            Some(frame) => frame,
                            None => continue,
                        },
                        Outbound::Frame(frame) => frame,
                    };
                    send(transport, frame).await?;
                }
                incoming = transport.next() => {
                    let text = match incoming {
                        Some(Ok(text)) => text,
                        Some(Err(error)) => return Err(transport_error(error)),
                        None => return Ok(()),
                    };
                    match parse_frame(&text)? {
                        RunnerFrame::Result { call_id, outcome } => {
                            self.inner.deliver(generation, &call_id, outcome);
                            send(transport, json!({"type": "ack", "call_id": call_id}).to_string()).await?;
                        }
                        RunnerFrame::Status { call_id, state } => match state.as_str() {
                            "running" => {}
                            "missing" => {
                                if let Some(frame) = self.inner.resend(generation, &call_id) {
                                    send(transport, frame).await?;
                                }
                            }
                            state => {
                                return Err(AttachmentError::Fenced(
                                    format!("runner reported unknown call state `{state}`").into(),
                                ));
                            }
                        },
                        RunnerFrame::Diagnostic {} => {}
                        RunnerFrame::Drain {} => {
                            if draining {
                                return Err(AttachmentError::Fenced("runner drained twice".into()));
                            }
                            draining = true;
                            self.inner.detach(generation);
                            while let Ok(command) = outbound.try_recv() {
                                if let Outbound::Frame(frame) = command {
                                    send(transport, frame).await?;
                                }
                            }
                            send(transport, json!({"type": "draining"}).to_string()).await?;
                        }
                        RunnerFrame::Catalog { .. } => {
                            return Err(AttachmentError::Fenced(
                                "runner published a catalog after it was ready".into(),
                            ));
                        }
                    }
                }
            }
        }
    }
}

impl Inner {
    fn attach(
        &self,
        runtime_id: &str,
        tools: Value,
        workspace: Option<PathBuf>,
    ) -> Result<(u64, mpsc::UnboundedReceiver<Outbound>, Vec<String>), AttachmentError> {
        let specs = parse_catalog(&self.namespace, &tools)?;
        let tools = Arc::new(tools);
        let (generation, receiver, backlog, first, lost) = self.state.modify(|state| {
            if state.revoked {
                return Err(AttachmentError::Closed);
            }
            if state.connection.is_some() {
                return Err(AttachmentError::Catalog(
                    "runner already has a live connection".into(),
                ));
            }
            if state.catalog.is_some() && state.workspace != workspace {
                return Err(AttachmentError::Catalog(
                    "runner workspace differs from its pinned catalog".into(),
                ));
            }
            if state
                .catalog
                .as_ref()
                .is_some_and(|pinned| **pinned != *tools)
            {
                return Err(AttachmentError::Catalog(
                    format!(
                        "runner `{}` published a catalog that differs from the pinned catalog",
                        self.namespace
                    )
                    .into(),
                ));
            }
            let mut lost = Vec::new();
            if state
                .runtime_id
                .as_deref()
                .is_some_and(|previous| previous != runtime_id)
            {
                let sent: Vec<_> = state
                    .pending
                    .iter()
                    .filter(|(_, pending)| pending.sent)
                    .map(|(call_id, _)| call_id.clone())
                    .collect();
                lost.extend(
                    sent.into_iter()
                        .filter_map(|call_id| state.pending.remove(&call_id)),
                );
            }
            state.runtime_id = Some(runtime_id.into());
            state.generation += 1;
            let generation = state.generation;
            let (outbound, receiver) = mpsc::unbounded_channel();
            state.connection = Some(Connection {
                generation,
                outbound,
            });
            let mut backlog = std::mem::take(&mut state.deferred_frames);
            let mut recover = Vec::new();
            for (call_id, pending) in &mut state.pending {
                if pending.sent {
                    recover.push(call_id.clone());
                } else {
                    pending.sent = true;
                    backlog.push(pending.frame.to_string());
                }
            }
            for batch in recover.chunks(RECOVER_BATCH_LIMIT) {
                backlog.push(json!({"type": "recover", "call_ids": batch}).to_string());
            }
            let first = state.catalog.is_none();
            if first {
                state.catalog = Some(Arc::clone(&tools));
                state.workspace.clone_from(&workspace);
            }
            Ok((generation, receiver, backlog, first, lost))
        })?;
        for pending in lost {
            pending.resolve(
                json!({
                    "status": "ambiguous",
                    "message": "runner restarted before returning a receipt",
                }),
                None,
            );
        }
        if first {
            self.catalog.send_replace(Some(specs));
        }
        self.failure.send_replace(None);
        self.publish_connection();
        Ok((generation, receiver, backlog))
    }

    fn detach(&self, generation: u64) {
        let detached = self.state.try_update(|state| {
            state
                .connection
                .as_ref()
                .is_some_and(|connection| connection.generation == generation)
                .then(|| state.connection = None)
        });
        if detached.is_some() {
            self.publish_connection();
        }
    }

    fn publish_connection(&self) {
        self.connected.send_if_modified(|connected| {
            let current = self.state.read(|state| state.connection.is_some());
            std::mem::replace(connected, current) != current
        });
    }

    fn claim(&self, generation: u64, call_id: &str) -> Option<String> {
        self.state.try_update(|state| {
            if state
                .connection
                .as_ref()
                .is_none_or(|connection| connection.generation != generation)
            {
                return None;
            }
            let pending = state
                .pending
                .get_mut(call_id)
                .filter(|pending| !pending.sent)?;
            pending.sent = true;
            Some(pending.frame.to_string())
        })
    }

    fn resend(&self, generation: u64, call_id: &str) -> Option<String> {
        self.state.try_update(|state| {
            if state
                .connection
                .as_ref()
                .is_none_or(|connection| connection.generation != generation)
            {
                return None;
            }
            let pending = state.pending.get_mut(call_id)?;
            pending.sent = true;
            Some(pending.frame.to_string())
        })
    }

    fn deliver(&self, generation: u64, call_id: &str, outcome: Value) {
        let delivered = self.state.try_update(|state| {
            if state.generation != generation {
                return None;
            }
            let runtime_id = state.runtime_id.clone();
            state
                .pending
                .remove(call_id)
                .map(|pending| (pending, runtime_id))
        });
        if let Some((pending, runtime_id)) = delivered {
            pending.resolve(outcome, runtime_id);
        }
    }

    fn send_or_defer(&self, frame: String, change: impl Fn(&mut State) -> bool) {
        let connection = self.state.try_update(|state| {
            if !change(state) {
                return None;
            }
            match &state.connection {
                Some(connection) => Some(Some(connection.outbound.clone())),
                None => {
                    state.deferred_frames.push(frame.clone());
                    Some(None)
                }
            }
        });
        if let Some(Some(outbound)) = connection {
            let _ = outbound.send(Outbound::Frame(frame));
        }
    }

    fn cancel(&self, call_id: &str) {
        let frame = json!({"type": "cancel", "call_id": call_id}).to_string();
        self.send_or_defer(frame, |state| {
            state
                .pending
                .remove(call_id)
                .is_some_and(|pending| pending.sent)
        });
    }

    async fn call(
        self: &Arc<Self>,
        spec: &RemoteSpec,
        input: Value,
        context: ToolContext<'_>,
    ) -> Result<(Value, Option<Box<str>>), AttachmentError> {
        let session_id = context.session_id();
        if !valid_identifier(session_id) {
            return Err(AttachmentError::Transport(
                format!("session id `{session_id}` is not a valid attachment identifier").into(),
            ));
        }
        if context
            .turn_id()
            .is_some_and(|turn_id| turn_id.is_empty() || turn_id.len() > TURN_ID_LIMIT)
        {
            return Err(AttachmentError::Transport(
                "turn id is not a valid attachment turn identifier".into(),
            ));
        }
        let output_token_budget = if context.output_token_budget()
            == crate::contract::DEFAULT_TOOL_OUTPUT_TOKENS
        {
            JSON_SAFE_INTEGER_MAX
        } else {
            let budget = u64::try_from(context.output_token_budget()).map_err(transport_error)?;
            if budget > JSON_SAFE_INTEGER_MAX {
                return Err(AttachmentError::Transport(
                    "output token budget exceeds the protocol integer range".into(),
                ));
            }
            budget
        };
        let call_id: Box<str> = uuid::Uuid::new_v4().to_string().into();
        let now = now_ms();
        let timeout_ms = self
            .state
            .read(|state| state.call_timeout_ms)
            .map_or(spec.timeout_ms, |policy| policy.min(spec.timeout_ms));
        let deadline_at = if timeout_ms == JSON_SAFE_INTEGER_MAX {
            JSON_SAFE_INTEGER_MAX
        } else {
            now.checked_add(timeout_ms)
                .filter(|value| *value <= JSON_SAFE_INTEGER_MAX)
                .ok_or_else(|| {
                    AttachmentError::Transport(
                        "call deadline exceeds the protocol integer range".into(),
                    )
                })?
        };
        let mut frame = json!({
            "type": "call",
            "session_id": session_id,
            "call_id": call_id,
            "model": context.model(),
            "name": spec.remote_name,
            "input": input,
            "output_token_budget": output_token_budget,
            "output_byte_budget": JSON_SAFE_INTEGER_MAX,
            "deadline_at": deadline_at,
        });
        if let Some(turn_id) = context.turn_id() {
            frame["turn_id"] = turn_id.into();
        }
        let (result, receipt) = oneshot::channel();
        let pending = Pending {
            frame: frame.to_string().into(),
            sent: false,
            result: Arc::new(Slot::new(Some(result))),
        };
        let outbound = self.state.modify(|state| {
            if state.revoked {
                return Err(AttachmentError::Closed);
            }
            state.pending.insert(call_id.clone(), pending.clone());
            Ok(state
                .connection
                .as_ref()
                .map(|connection| connection.outbound.clone()))
        })?;
        if let Some(outbound) = outbound {
            let _ = outbound.send(Outbound::Call(call_id.clone()));
        }
        let guard = CancelOnDrop {
            inner: Arc::clone(self),
            call_id,
        };
        let waited = tokio::time::timeout(
            Duration::from_millis(deadline_at.saturating_sub(now)),
            receipt,
        )
        .await;
        let dispatched = self.state.read(|state| {
            state
                .pending
                .get(&guard.call_id)
                .is_some_and(|pending| pending.sent)
        });
        let call_id = guard.call_id.clone();
        drop(guard);
        match waited {
            Ok(Ok(outcome)) => {
                if matches!(
                    outcome.0.get("message").and_then(Value::as_str),
                    Some("tool deadline elapsed" | "tool deadline elapsed before execution")
                ) {
                    return Err(AttachmentError::Timeout {
                        runner: self.namespace.clone(),
                        call_id,
                        outcome: if outcome.0.get("status").and_then(Value::as_str)
                            == Some("ambiguous")
                        {
                            super::AttachmentCallOutcome::Ambiguous
                        } else {
                            super::AttachmentCallOutcome::Unavailable
                        },
                    });
                }
                Ok(outcome)
            }
            Ok(Err(_)) => Err(AttachmentError::Closed),
            Err(_) => Err(AttachmentError::Timeout {
                runner: self.namespace.clone(),
                call_id,
                outcome: if dispatched {
                    super::AttachmentCallOutcome::Ambiguous
                } else {
                    super::AttachmentCallOutcome::Unavailable
                },
            }),
        }
    }

    fn turn_ended(
        &self,
        session_id: &str,
        turn_id: &str,
        hook_event_name: &str,
    ) -> Result<(), AttachmentError> {
        if !valid_identifier(session_id)
            || turn_id.is_empty()
            || turn_id.len() > TURN_ID_LIMIT
            || !matches!(hook_event_name, "Stop" | "Interrupt" | "SubagentStop")
        {
            return Err(AttachmentError::Transport(
                "turn end is not expressible in the attachment protocol".into(),
            ));
        }
        let key: [Box<str>; 3] = [session_id.into(), turn_id.into(), hook_event_name.into()];
        let frame = json!({"type": "turn_ended", "session_id": session_id, "turn_id": turn_id, "hook_event_name": hook_event_name}).to_string();
        self.send_or_defer(frame, |state| {
            if state.last_turn_ended.as_ref() == Some(&key) {
                return false;
            }
            state.last_turn_ended = Some(key.clone());
            true
        });
        Ok(())
    }
}

struct CancelOnDrop {
    inner: Arc<Inner>,
    call_id: Box<str>,
}

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.inner.cancel(&self.call_id);
    }
}

#[derive(Clone)]
pub struct RemoteTools {
    namespace: Box<str>,
    tools: Vec<RemoteTool>,
    workspace: Option<PathBuf>,
}

impl fmt::Debug for RemoteTools {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_list()
            .entries(self.tools.iter().map(RemoteTool::name))
            .finish()
    }
}

impl RemoteTools {
    pub fn namespace(&self) -> &str {
        &self.namespace
    }

    #[must_use]
    pub fn workspace(&self) -> Option<&Path> {
        self.workspace.as_deref()
    }

    pub fn iter(&self) -> impl Iterator<Item = &RemoteTool> {
        self.tools.iter()
    }

    #[must_use]
    pub fn get(&self, name: &str) -> Option<&RemoteTool> {
        self.tools.iter().find(|tool| tool.name() == name)
    }
}

impl IntoIterator for RemoteTools {
    type Item = RemoteTool;
    type IntoIter = std::vec::IntoIter<RemoteTool>;

    fn into_iter(self) -> Self::IntoIter {
        self.tools.into_iter()
    }
}

#[derive(Clone)]
pub struct RemoteTool {
    runner: Arc<Inner>,
    spec: Arc<RemoteSpec>,
}

impl fmt::Debug for RemoteTool {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RemoteTool")
            .field("name", &self.name())
            .field("remote_name", &self.remote_name())
            .finish()
    }
}

impl RemoteTool {
    pub(crate) fn runtime_id(&self) -> Option<Box<str>> {
        self.runner.state.read(|state| state.runtime_id.clone())
    }
    pub(crate) fn native_definition(&self) -> ToolDefinition {
        let mut definition = self.spec.definition.clone();
        match &mut definition {
            ToolDefinition::Function { name, .. } | ToolDefinition::Custom { name, .. } => {
                *name = self.spec.remote_name.clone()
            }
            _ => {}
        }
        definition
    }

    #[must_use]
    pub fn name(&self) -> &str {
        self.spec.definition.name()
    }

    #[must_use]
    pub fn remote_name(&self) -> &str {
        &self.spec.remote_name
    }

    pub(crate) fn routing(&self) -> Option<nanocode_tool_host::WorkspaceToolRouting> {
        self.spec.routing
    }

    #[must_use]
    pub fn namespace(&self) -> &str {
        &self.runner.namespace
    }
}

#[async_trait]
impl Tool for RemoteTool {
    fn definition(&self) -> ToolDefinition {
        self.spec.definition.clone()
    }

    fn supports_parallel_tool_calls(&self) -> bool {
        self.spec.parallel_safe
    }

    async fn end_turn(
        &self,
        session_id: &str,
        turn_id: &str,
        hook_event_name: &str,
    ) -> Result<(), ToolError> {
        Ok(self
            .runner
            .turn_ended(session_id, turn_id, hook_event_name)?)
    }

    async fn execute(&self, input: ToolInput, context: ToolContext<'_>) -> ToolResult {
        self.execute_with_runtime(input, context)
            .await
            .map(|(output, _)| output)
    }
}

impl RemoteTool {
    pub(crate) async fn execute_with_runtime(
        &self,
        input: ToolInput,
        context: ToolContext<'_>,
    ) -> Result<(ToolOutput, Option<Box<str>>), ToolError> {
        let input = match input {
            ToolInput::Function(input) => {
                let input: Value = serde_json::from_str(input.get())?;
                if !input.is_object() {
                    return Err(format!("`{}` requires an object input", self.name()).into());
                }
                input
            }
            ToolInput::Freeform(input) => Value::String(input),
        };
        let (outcome, runtime_id) = self.runner.call(&self.spec, input, context).await?;
        let Outcome {
            status,
            output,
            message,
        } = serde_json::from_str(&outcome.to_string())?;
        match (status, output) {
            (OutcomeStatus::Completed, Some(output)) => {
                Ok(ToolOutput::from_wire(serde_json::from_str::<
                    ToolOutputWire,
                >(output.get())?)?)
            }
            (OutcomeStatus::Completed, None) => Err(format!(
                "runner `{}` completed `{}` without an output",
                self.runner.namespace,
                self.remote_name()
            )
            .into()),
            (OutcomeStatus::Unavailable, _) => Err(format!(
                "runner `{}` could not run `{}`: {message}",
                self.runner.namespace,
                self.remote_name()
            )
            .into()),
            (OutcomeStatus::Ambiguous, _) => Err(format!(
                "runner `{}` call to `{}` has an unknown outcome: {message}",
                self.runner.namespace,
                self.remote_name()
            )
            .into()),
        }
        .map(|output| (output, runtime_id))
    }
}

type Runners = Arc<Atom<HashMap<Box<str>, RemoteRunner>>>;

pub struct RunnerHost {
    endpoint: Box<str>,
    runners: Runners,
    accept: tokio::task::JoinHandle<()>,
}

impl fmt::Debug for RunnerHost {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RunnerHost")
            .field("endpoint", &self.endpoint)
            .finish_non_exhaustive()
    }
}

impl Drop for RunnerHost {
    fn drop(&mut self) {
        self.accept.abort();
    }
}

impl RunnerHost {
    pub async fn bind(address: impl ToSocketAddrs) -> Result<Self, AttachmentError> {
        let listener = TcpListener::bind(address).await.map_err(transport_error)?;
        let address = listener.local_addr().map_err(transport_error)?;
        let runners = Runners::default();
        let accept = tokio::spawn(accept_runners(listener, Arc::clone(&runners)));
        Ok(Self {
            endpoint: format!("ws://{address}/").into(),
            runners,
            accept,
        })
    }

    #[must_use]
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    pub fn runner(
        &self,
        namespace: impl Into<String>,
        bearer: impl Into<String>,
    ) -> Result<RemoteRunner, AttachmentError> {
        let runner = RemoteRunner::new(namespace)?;
        let bearer = bearer.into();
        if bearer.trim().is_empty() {
            return Err(AttachmentError::Authentication(
                "bearer credential must not be empty".into(),
            ));
        }
        self.runners.modify(|runners| {
            if runners
                .values()
                .any(|existing| existing.namespace() == runner.namespace())
            {
                return Err(AttachmentError::Catalog(
                    format!(
                        "runner namespace `{}` is already registered",
                        runner.namespace()
                    )
                    .into(),
                ));
            }
            if runners.contains_key(bearer.as_str()) {
                return Err(AttachmentError::Authentication(
                    "bearer credential is already assigned to another runner".into(),
                ));
            }
            runners.insert(bearer.as_str().into(), runner.clone());
            Ok(())
        })?;
        Ok(runner)
    }
}

async fn accept_runners(listener: TcpListener, runners: Runners) {
    let mut connections = tokio::task::JoinSet::new();
    loop {
        tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok((stream, _)) => {
                    connections.spawn(serve_socket(stream, Arc::clone(&runners)));
                }
                Err(error) => {
                    tracing::warn!(target: "nanocode_code_mode::attachment", %error, "runner host accept failed");
                }
            },
            Some(_) = connections.join_next(), if !connections.is_empty() => {}
        }
    }
}

async fn serve_socket(stream: TcpStream, runners: Runners) {
    if let Err(error) = stream.set_nodelay(true) {
        tracing::warn!(target: "nanocode_code_mode::attachment", %error, "runner socket nodelay failed");
    }
    let mut runner = None;
    let authenticate = |request: &Request, response: Response| {
        let found = request
            .headers()
            .get(http::header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))
            .and_then(|bearer| runners.read(|runners| runners.get(bearer).cloned()));
        if let Some(found) = found {
            runner = Some(found);
            Ok(response)
        } else {
            let mut rejection = ErrorResponse::new(Some("unknown runner credential".into()));
            *rejection.status_mut() = http::StatusCode::UNAUTHORIZED;
            Err(rejection)
        }
    };
    let socket = match tokio::time::timeout(
        super::driver::HANDSHAKE_TIMEOUT,
        tokio_tungstenite::accept_hdr_async(stream, authenticate),
    )
    .await
    .map_err(transport_error)
    .and_then(|result| result.map_err(transport_error))
    {
        Ok(socket) => socket,
        Err(error) => {
            tracing::info!(target: "nanocode_code_mode::attachment", %error, "runner handshake rejected");
            return;
        }
    };
    let Some(runner) = runner else { return };
    let mut frames = socket
        .with(|text: String| future::ready(Ok::<_, tungstenite::Error>(Message::text(text))))
        .filter_map(|message| {
            future::ready(match message {
                Ok(Message::Text(text)) => Some(Ok(text.as_str().to_owned())),
                Ok(Message::Binary(_)) => Some(Err(tungstenite::Error::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "runner sent a binary frame",
                )))),
                Ok(_) => None,
                Err(error) => Some(Err(error)),
            })
        });
    let result = runner.serve(&mut frames).await;
    let close = match &result {
        Ok(()) => None,
        Err(error) => {
            tracing::warn!(target: "nanocode_code_mode::attachment", namespace = runner.namespace(), %error, "runner connection ended");
            let code = if matches!(
                error,
                AttachmentError::Catalog(_) | AttachmentError::Fenced(_)
            ) {
                CloseCode::Policy
            } else {
                CloseCode::Error
            };
            Some(CloseFrame {
                code,
                reason: close_reason(&error.to_string()).to_owned().into(),
            })
        }
    };
    let _ = frames.get_mut().get_mut().close(close).await;
}

fn close_reason(reason: &str) -> &str {
    &reason[..reason.floor_char_boundary(WEBSOCKET_CLOSE_REASON_LIMIT)]
}

fn parse_catalog(
    namespace: &str,
    tools: &Value,
) -> Result<Arc<[Arc<RemoteSpec>]>, AttachmentError> {
    let entries: Vec<CatalogEntry> = serde_json::from_value(tools.clone())
        .map_err(|error| AttachmentError::Catalog(error.to_string().into()))?;
    crate::host::validate_public_tool_catalog_names(
        entries.iter().map(|entry| entry.definition.name()),
    )
    .map_err(|error| AttachmentError::Catalog(error.to_string().into()))?;
    if entries
        .iter()
        .any(|entry| entry.timeout_ms == 0 || entry.timeout_ms > JSON_SAFE_INTEGER_MAX)
    {
        return Err(AttachmentError::Catalog("invalid tool timeout".into()));
    }
    entries
        .into_iter()
        .map(|entry| {
            let remote_name: Box<str> = entry.definition.name().into();
            let name = format!("{namespace}{NAMESPACE_SEPARATOR}{remote_name}");
            if name.len() > PUBLIC_TOOL_NAME_LIMIT {
                return Err(AttachmentError::Catalog(
                    format!("namespaced tool name `{name}` exceeds {PUBLIC_TOOL_NAME_LIMIT} bytes")
                        .into(),
                ));
            }
            let definition = match entry.definition {
                ToolDefinition::Function {
                    description,
                    strict,
                    parameters,
                    output_schema,
                    ..
                } => {
                    let mut definition = ToolDefinition::function(name, description, parameters);
                    if strict {
                        definition = definition.with_strict_parameters();
                    }
                    if let Some(output_schema) = output_schema {
                        definition = definition.with_output_schema(output_schema);
                    }
                    definition
                }
                ToolDefinition::Custom {
                    description,
                    format,
                    ..
                } => ToolDefinition::custom(name, description, format),
                definition => {
                    return Err(AttachmentError::Catalog(
                        format!(
                            "attached tool `{}` has an unsupported definition",
                            definition.name()
                        )
                        .into(),
                    ));
                }
            };
            let routing = entry
                .routing
                .map(|routing| {
                    nanocode_tool_host::WorkspaceToolRouting::from_wire(&routing).ok_or_else(|| {
                        AttachmentError::Catalog(
                            format!(
                                "attached tool `{remote_name}` has unknown routing `{routing}`"
                            )
                            .into(),
                        )
                    })
                })
                .transpose()?;
            Ok(Arc::new(RemoteSpec {
                remote_name,
                definition,
                parallel_safe: entry.parallel_safe,
                timeout_ms: entry.timeout_ms,
                routing,
            }))
        })
        .collect()
}

fn parse_frame(text: &str) -> Result<RunnerFrame, AttachmentError> {
    serde_json::from_str(text)
        .map_err(|error| AttachmentError::Fenced(format!("invalid runner frame: {error}").into()))
}

async fn send<T, E>(transport: &mut T, frame: String) -> Result<(), AttachmentError>
where
    T: Sink<String, Error = E> + Unpin,
    E: fmt::Display,
{
    transport.send(frame).await.map_err(transport_error)
}

fn transport_error(error: impl fmt::Display) -> AttachmentError {
    AttachmentError::Transport(error.to_string().into())
}

fn valid_namespace(namespace: &str) -> bool {
    namespace
        .as_bytes()
        .first()
        .is_some_and(u8::is_ascii_alphabetic)
        && namespace
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        && !namespace.contains(NAMESPACE_SEPARATOR)
        && !namespace.ends_with('_')
}

fn valid_identifier(value: &str) -> bool {
    value.len() <= PROTOCOL_IDENTIFIER_LIMIT
        && value
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-'))
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
        })
}

pub struct RunnerConnection(tokio::task::JoinHandle<()>);

impl Drop for RunnerConnection {
    fn drop(&mut self) {
        self.0.abort();
    }
}

pub(crate) type DialedSocket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

impl RemoteRunner {
    pub fn connect(
        &self,
        target: super::AttachmentTarget,
    ) -> Result<RunnerConnection, AttachmentError> {
        self.connect_established(target, None)
    }

    pub(crate) fn connect_established(
        &self,
        target: super::AttachmentTarget,
        mut initial: Option<(DialedSocket, String)>,
    ) -> Result<RunnerConnection, AttachmentError> {
        use tungstenite::client::IntoClientRequest;
        let mut request = target
            .endpoint()
            .as_str()
            .into_client_request()
            .map_err(transport_error)?;
        let mut authorization = http::HeaderValue::from_str(&format!("Bearer {}", target.bearer()))
            .map_err(transport_error)?;
        authorization.set_sensitive(true);
        request
            .headers_mut()
            .insert(http::header::AUTHORIZATION, authorization);
        let runner = self.clone();
        Ok(RunnerConnection(tokio::spawn(async move {
            let mut backoff = super::driver::INITIAL_TRANSPORT_BACKOFF;
            loop {
                let connected = match initial.take() {
                    Some((socket, first)) => Ok((socket, Some(first))),
                    None => tokio::time::timeout(super::driver::HANDSHAKE_TIMEOUT, async {
                        let connector = super::driver::tls_connector(&request).await?;
                        tokio_tungstenite::connect_async_tls_with_config(
                            request.clone(),
                            None,
                            false,
                            connector,
                        )
                        .await
                    })
                    .await
                    .map_err(|error| {
                        tungstenite::Error::Io(std::io::Error::new(
                            std::io::ErrorKind::TimedOut,
                            error,
                        ))
                    })
                    .and_then(|result| result)
                    .map(|(socket, _)| (socket, None)),
                };
                match connected {
                    Ok((socket, first)) => {
                        let mut frames = socket.with(|text: String| future::ready(Ok::<_, tungstenite::Error>(Message::text(text)))).filter_map(|message| future::ready(match message {
                            Ok(Message::Text(text)) => Some(Ok(text.to_string())),
                            Ok(Message::Close(_)) => None,
                            Ok(Message::Binary(_)) => Some(Err(tungstenite::Error::Protocol(tungstenite::error::ProtocolError::ResetWithoutClosingHandshake))),
                            Ok(_) => None,
                            Err(error) => Some(Err(error)),
                        }));
                        let served = match first {
                            Some(first) => runner.serve_catalog(first, &mut frames).await,
                            None => runner.serve(&mut frames).await,
                        };
                        if let Err(
                            error @ (AttachmentError::Catalog(_) | AttachmentError::Fenced(_)),
                        ) = served
                        {
                            runner.inner.failure.send_replace(Some(error));
                            return;
                        }
                    }
                    Err(tungstenite::Error::Http(response))
                        if matches!(response.status().as_u16(), 401 | 403) =>
                    {
                        runner
                            .inner
                            .failure
                            .send_replace(Some(AttachmentError::Authentication(
                                "runner rejected the bearer credential".into(),
                            )));
                        return;
                    }
                    Err(error) => {
                        tracing::warn!(target: "nanocode_code_mode::attachment", %error, "runner connection failed")
                    }
                }
                tokio::time::sleep(super::driver::Failure::Transport.delay(&mut backoff, None))
                    .await;
            }
        })))
    }
}

struct ConnectionLease {
    inner: Arc<Inner>,
    generation: u64,
}
impl Drop for ConnectionLease {
    fn drop(&mut self) {
        self.inner.detach(self.generation);
    }
}
