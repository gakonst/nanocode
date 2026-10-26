mod admission;
mod client;
mod compact;
mod executors;
mod message;
#[cfg(not(target_family = "wasm"))]
mod queue;

use std::{collections::HashMap, sync::Arc};

use nanocode_sync::Atom;

pub use admission::{Admission, CatalogError, admit_runtime, read_catalog};
pub use client::{
    CallReply, LOCAL_CLOSED, PEER_CLOSED, RemoteActivity, RemoteClient, RemoteDelivery,
    RemoteLink, ResumedReply,
};
pub use compact::{ReceivedCalls, SentCalls, encode_output, output_from_wire, output_to_wire};
pub use executors::{
    CodeModeExecutors, ExecutorSelection, HandRoute, HandsTool, PLACEMENT_INSTRUCTIONS, Placement,
};
#[cfg(not(target_family = "wasm"))]
pub use message::websocket;
pub use message::{MessageLink, MessageWriter, SocketMessage};

#[cfg(not(target_family = "wasm"))]
pub use queue::{
    Admit, CommandQueue, Polled, QueueOptions, QueuedCall, Settlement, request_id, serve_queue,
};

use crate::embedded::{CodeModeNotification, CodeModeUpdate, NestedToolCall};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    Tool, ToolContext, ToolDefinition, ToolOutput,
    embedded::{
        CodeModeExecution, CodeModeHost, CodeModeHostError, CodeModeObserver, EmbeddedToolMode,
        HandCalls, HostFuture, OwnedToolContext,
    },
};

#[derive(Clone, Serialize, Deserialize)]
pub struct RemoteContext {
    pub model: String,
    pub session_id: String,
    pub call_id: String,
    pub turn_id: Option<String>,
    #[serde(skip)]
    pub history: Arc<nanocode_tool_contracts::SharedHistory>,
    #[serde(
        serialize_with = "serialize_output_budget",
        deserialize_with = "deserialize_output_budget"
    )]
    pub output_token_budget: usize,
    pub instruction_revision: Option<u64>,
    pub host_context: Option<String>,
}

fn serialize_output_budget<S: serde::Serializer>(
    value: &usize,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.serialize_u64(if *value == usize::MAX {
        u64::MAX
    } else {
        *value as u64
    })
}

fn deserialize_output_budget<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<usize, D::Error> {
    let value = u64::deserialize(deserializer)?;
    if value == u64::MAX {
        Ok(usize::MAX)
    } else {
        usize::try_from(value).map_err(serde::de::Error::custom)
    }
}

impl RemoteContext {
    #[must_use]
    pub fn new(session_id: String, call_id: String, turn_id: Option<String>) -> Self {
        Self {
            model: String::new(),
            session_id,
            call_id,
            turn_id,
            history: Arc::default(),
            output_token_budget: usize::try_from(9_007_199_254_740_991_u64).unwrap_or(usize::MAX),
            instruction_revision: None,
            host_context: None,
        }
    }

    pub fn from_context(context: ToolContext<'_>) -> Self {
        Self {
            model: context.model().into(),
            session_id: context.session_id().into(),
            call_id: context.call_id().into(),
            turn_id: context.turn_id().map(Into::into),
            history: crate::embedded::shared_history(context),
            output_token_budget: context.output_token_budget(),
            instruction_revision: context.instruction_revision(),
            host_context: context.host_context().map(Into::into),
        }
    }

    pub fn owned(&self) -> OwnedToolContext {
        OwnedToolContext::new(
            &self.model,
            &self.session_id,
            &self.call_id,
            Arc::default(),
            self.output_token_budget,
        )
        .with_history(Arc::clone(&self.history))
        .with_turn_id(self.turn_id.as_deref().map(Arc::from))
        .with_instruction_revision(self.instruction_revision)
        .with_host_context(self.host_context.as_deref().map(Arc::from))
    }
}

#[derive(Serialize, Deserialize)]
pub enum RemoteOperation {
    Execute {
        source: String,
        context: RemoteContext,
        local_tools: Vec<ToolDefinition>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        hand: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        selected_tools: Option<Vec<String>>,
    },
    Wait {
        input: String,
        context: RemoteContext,
    },
    Call {
        name: String,
        input: Value,
        context: RemoteContext,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        selected_tools: Option<Vec<String>>,
    },
    DetachTurn {
        session_id: String,
    },
    CancelTurn {
        session_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        turn_id: Option<String>,
    },
    Cancel {
        session_id: String,
    },
    CancelCall {
        session_id: String,
        call_id: String,
    },
    BeginTurn {
        session_id: String,
    },
    Preempt {
        session_id: String,
    },
    SettlePreempt {
        session_id: String,
    },
    EndTurn {
        session_id: String,
        turn_id: String,
        hook_event_name: String,
    },
}

pub trait RemoteTransport: Send + Sync + 'static {
    fn supports_tool_selection(&self) -> bool {
        false
    }

    fn request<'a>(
        &'a self,
        operation: RemoteOperation,
        local_tools: Vec<Arc<dyn Tool>>,
        observer: Option<&'a mut dyn CodeModeObserver>,
    ) -> HostFuture<'a, Result<Option<CodeModeExecution>, CodeModeHostError>>;
    fn call(&self, operation: RemoteOperation) -> HostFuture<'_, Result<Value, CodeModeHostError>>;
    fn recover<'a>(
        &'a self,
        _context: &'a RemoteContext,
    ) -> Option<HostFuture<'a, Result<Option<Value>, CodeModeHostError>>> {
        None
    }
    fn route_hands(&self, hands: Arc<dyn HandCalls>);
    fn notify(&self, operation: RemoteOperation) -> Result<(), CodeModeHostError>;
    fn detach_turn(
        &self,
        session_id: &str,
        observer: &mut (dyn FnMut(&str) -> Box<dyn CodeModeObserver> + Send),
    ) -> Result<(), CodeModeHostError>;
}

pub trait HandToolSelection: Send + Sync + 'static {
    fn allows(&self, session_id: &str, backend: &str, name: &str) -> bool;
}

pub struct RemoteCodeModeHost {
    transport: Arc<dyn RemoteTransport>,
    definitions: Vec<ToolDefinition>,
    hand: Atom<Option<String>>,
    selection: Option<Arc<dyn HandToolSelection>>,
    backends: Atom<HashMap<String, String>>,
}

impl RemoteCodeModeHost {
    pub fn new(transport: impl RemoteTransport, definitions: Vec<ToolDefinition>) -> Self {
        Self {
            transport: Arc::new(transport),
            definitions,
            hand: Atom::default(),
            selection: None,
            backends: Atom::default(),
        }
    }

    pub fn with_tool_selection(mut self, selection: Arc<dyn HandToolSelection>) -> Self {
        self.selection = Some(selection);
        self
    }

    pub fn recover_call<'a>(
        &'a self,
        context: ToolContext<'a>,
    ) -> HostFuture<'a, Result<Option<ToolOutput>, CodeModeHostError>> {
        Box::pin(async move {
            self.recovered(context)
                .await?
                .map(output_from_wire)
                .transpose()
        })
    }

    async fn recovered(
        &self,
        context: ToolContext<'_>,
    ) -> Result<Option<Value>, CodeModeHostError> {
        let context = RemoteContext::from_context(context);
        match self.transport.recover(&context) {
            Some(reply) => reply.await,
            None => Ok(None),
        }
    }

    fn selected_tools(&self, session_id: &str) -> Result<Option<Vec<String>>, CodeModeHostError> {
        let Some(selection) = &self.selection else {
            return Ok(None);
        };
        if !self.transport.supports_tool_selection() {
            return Err(CodeModeHostError::unsent(
                "this hand does not support tool selection; nothing was dispatched",
            ));
        }
        let backend = self
            .backends
            .read(|backends| backends.get(session_id).cloned())
            .ok_or_else(|| {
                CodeModeHostError::unsent(
                    "the session has no registered backend; nothing was dispatched",
                )
            })?;
        Ok(Some(
            self.definitions
                .iter()
                .filter(|definition| selection.allows(session_id, &backend, definition.name()))
                .map(|definition| definition.name().to_owned())
                .collect(),
        ))
    }

    async fn run<'a>(
        &'a self,
        operation: RemoteOperation,
        tools: Vec<Arc<dyn Tool>>,
        observer: Option<&'a mut dyn CodeModeObserver>,
    ) -> Result<CodeModeExecution, CodeModeHostError> {
        self.transport
            .request(operation, tools, observer)
            .await?
            .ok_or_else(|| CodeModeHostError::new("remote runner omitted execution result"))
    }
}

impl CodeModeHost for RemoteCodeModeHost {
    fn tool_mode(&self) -> EmbeddedToolMode {
        EmbeddedToolMode::CodeOnly
    }
    fn supports_cells(&self) -> bool {
        true
    }
    fn bind_session(&self, session_id: &str, backend: &str) {
        self.backends.update(|backends| {
            backends.insert(session_id.into(), backend.into());
        });
    }
    fn tool_definitions(&self, session_id: &str) -> Result<Vec<ToolDefinition>, CodeModeHostError> {
        let selected = self.selected_tools(session_id)?;
        Ok(self
            .definitions
            .iter()
            .filter(|definition| {
                selected
                    .as_ref()
                    .is_none_or(|names| names.iter().any(|name| name == definition.name()))
            })
            .cloned()
            .collect())
    }
    fn execute<'a>(
        &'a self,
        source: &'a str,
        context: ToolContext<'a>,
    ) -> HostFuture<'a, Result<CodeModeExecution, CodeModeHostError>> {
        self.execute_with_local_tools(source, context, Vec::new(), None)
    }
    fn execute_with_updates<'a>(
        &'a self,
        source: &'a str,
        context: ToolContext<'a>,
        observer: &'a mut dyn CodeModeObserver,
    ) -> HostFuture<'a, Result<CodeModeExecution, CodeModeHostError>> {
        self.execute_with_local_tools(source, context, Vec::new(), Some(observer))
    }
    fn execute_with_local_tools<'a>(
        &'a self,
        source: &'a str,
        context: ToolContext<'a>,
        tools: Vec<Arc<dyn Tool>>,
        observer: Option<&'a mut dyn CodeModeObserver>,
    ) -> HostFuture<'a, Result<CodeModeExecution, CodeModeHostError>> {
        Box::pin(async move {
            let selected_tools = self.selected_tools(context.session_id())?;
            let operation = RemoteOperation::Execute {
                source: source.into(),
                context: RemoteContext::from_context(context),
                local_tools: tools.iter().map(|tool| tool.definition()).collect(),
                hand: Option::clone(&self.hand.get()),
                selected_tools,
            };
            self.run(operation, tools, observer).await
        })
    }
    fn wait_with_updates<'a>(
        &'a self,
        input: &'a str,
        context: ToolContext<'a>,
        observer: &'a mut dyn CodeModeObserver,
    ) -> HostFuture<'a, Result<CodeModeExecution, CodeModeHostError>> {
        Box::pin(async move {
            self.run(
                RemoteOperation::Wait {
                    input: input.into(),
                    context: RemoteContext::from_context(context),
                },
                Vec::new(),
                Some(observer),
            )
            .await
        })
    }
    fn call<'a>(
        &'a self,
        name: &'a str,
        input: Value,
        context: ToolContext<'a>,
    ) -> HostFuture<'a, Result<ToolOutput, CodeModeHostError>> {
        Box::pin(async move {
            let selected_tools = self.selected_tools(context.session_id())?;
            if selected_tools
                .as_ref()
                .is_some_and(|names| !names.iter().any(|allowed| allowed == name))
            {
                return Err(CodeModeHostError::unsent(format!(
                    "hand tool `{name}` is not permitted for this session; nothing was dispatched"
                )));
            }
            let operation = RemoteOperation::Call {
                name: name.into(),
                input,
                context: RemoteContext::from_context(context),
                selected_tools,
            };
            output_from_wire(self.transport.call(operation).await?)
        })
    }
    fn recover<'a>(
        &'a self,
        context: ToolContext<'a>,
    ) -> HostFuture<'a, Result<Option<CodeModeExecution>, CodeModeHostError>> {
        Box::pin(async move {
            let Some(mut value) = self.recovered(context).await? else {
                return Ok(None);
            };
            ReceivedCalls::default().execution(&mut value)?;
            serde_json::from_value(value)
                .map(Some)
                .map_err(|error| CodeModeHostError::new(error.to_string()))
        })
    }
    fn route_hands(&self, name: &str, hands: Arc<dyn HandCalls>) {
        self.hand.set(Some(name.into()));
        self.transport.route_hands(hands);
    }
    fn begin_turn(&self, session_id: &str) {
        if let Err(error) = self.transport.notify(RemoteOperation::BeginTurn {
            session_id: session_id.into(),
        }) {
            tracing::error!(%error, "remote begin turn failed");
        }
    }
    fn settle_preempt(&self, session_id: &str) {
        if let Err(error) = self.transport.notify(RemoteOperation::SettlePreempt {
            session_id: session_id.into(),
        }) {
            tracing::error!(%error, "remote settle preempt failed");
        }
    }
    fn preempt_turn<'a>(
        &'a self,
        session_id: &'a str,
    ) -> HostFuture<'a, Result<(), CodeModeHostError>> {
        Box::pin(async move {
            self.transport
                .request(
                    RemoteOperation::Preempt {
                        session_id: session_id.into(),
                    },
                    Vec::new(),
                    None,
                )
                .await
                .map(|_| ())
        })
    }
    fn detach_turn(
        &self,
        session_id: &str,
        observer: &mut (dyn FnMut(&str) -> Box<dyn CodeModeObserver> + Send),
    ) {
        if let Err(error) = self.transport.detach_turn(session_id, observer) {
            tracing::error!(%error, "remote detach turn failed");
        }
    }
    fn cancel_turn<'a>(
        &'a self,
        session_id: &'a str,
    ) -> HostFuture<'a, Result<(), CodeModeHostError>> {
        Box::pin(async move {
            self.transport
                .request(
                    RemoteOperation::CancelTurn {
                        session_id: session_id.into(),
                        turn_id: None,
                    },
                    Vec::new(),
                    None,
                )
                .await
                .map(|_| ())
        })
    }
    fn cancel_with_updates<'a>(
        &'a self,
        session_id: &'a str,
        observer: &'a mut dyn CodeModeObserver,
    ) -> HostFuture<'a, Result<(), CodeModeHostError>> {
        Box::pin(async move {
            self.transport
                .request(
                    RemoteOperation::Cancel {
                        session_id: session_id.into(),
                    },
                    Vec::new(),
                    Some(observer),
                )
                .await
                .map(|_| ())
        })
    }
    fn cancel_turn_with_updates<'a>(
        &'a self,
        session_id: &'a str,
        observer: &'a mut dyn CodeModeObserver,
    ) -> HostFuture<'a, Result<(), CodeModeHostError>> {
        Box::pin(async move {
            self.transport
                .request(
                    RemoteOperation::CancelTurn {
                        session_id: session_id.into(),
                        turn_id: None,
                    },
                    Vec::new(),
                    Some(observer),
                )
                .await
                .map(|_| ())
        })
    }
    fn cancel<'a>(&'a self, session_id: &'a str) -> HostFuture<'a, Result<(), CodeModeHostError>> {
        Box::pin(async move {
            self.transport
                .request(
                    RemoteOperation::Cancel {
                        session_id: session_id.into(),
                    },
                    Vec::new(),
                    None,
                )
                .await
                .map(|_| ())
        })
    }
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RemoteFrame {
    Catalog {
        definitions: Vec<ToolDefinition>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        workspace: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        runtime_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_frame_bytes: Option<usize>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        resumable: bool,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        tool_selection: bool,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        cancels_calls: bool,
    },
    Request {
        id: u64,
        operation: RemoteOperation,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_frame_bytes: Option<usize>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        compact: bool,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        output_prefix: bool,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        resumable: bool,
    },
    Reply {
        id: u64,
        result: Result<Option<Value>, String>,
    },
    Update {
        id: u64,
        update: Value,
    },
    Callback {
        id: u64,
        name: String,
        input: Value,
        freeform: bool,
        context: RemoteContext,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        hand: Option<String>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        compact: bool,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        output_prefix: bool,
    },
    CallbackCancel {
        id: u64,
    },
    DetachedUpdate {
        session_id: String,
        origin_call_id: String,
        update: Value,
    },
    DetachedComplete {
        session_id: String,
        origin_call_id: String,
    },
    CallbackResult {
        id: u64,
        result: Result<Value, String>,
    },
    Ack {
        ids: Vec<u64>,
    },
    Resume {
        pending: Vec<u64>,
    },
}

impl RemoteFrame {
    #[must_use]
    pub const fn dispatches(&self) -> bool {
        matches!(
            self,
            Self::Request {
                operation: RemoteOperation::Execute { .. }
                    | RemoteOperation::Wait { .. }
                    | RemoteOperation::Call { .. },
                ..
            }
        )
    }
    pub fn encode(&self, max_frame_bytes: usize) -> Result<String, CodeModeHostError> {
        let text = serde_json::to_string(self)
            .map_err(|error| CodeModeHostError::new(error.to_string()))?;
        if text.len() > max_frame_bytes {
            return Err(oversized(text.len(), max_frame_bytes));
        }
        Ok(text)
    }
    pub fn decode(text: &str, max_frame_bytes: usize) -> Result<Self, CodeModeHostError> {
        if text.len() > max_frame_bytes {
            return Err(CodeModeHostError::new(
                "remote frame exceeds caller max_frame_bytes policy",
            ));
        }
        serde_json::from_str(text).map_err(|error| CodeModeHostError::new(error.to_string()))
    }
}

#[must_use]
pub fn oversized(bytes: usize, max_frame_bytes: usize) -> CodeModeHostError {
    CodeModeHostError::new(format!(
        "remote frame of {bytes} bytes exceeds the connection's max_frame_bytes policy of {max_frame_bytes} bytes"
    ))
}

pub const NO_CATALOG: &str = "runner did not publish a code mode catalog";

pub const HAND_REPLACED: &str = "another hand attached under this name";

pub const HAND_RESTARTED: &str = "the hand reconnected as a new runtime, so it no longer has this connection's cells; dispatched operations have unknown outcomes and were not retried";

pub const NOT_RECEIVED: &str =
    "the hand never received this operation before its connection dropped; it was not dispatched";

pub const REQUEST_IN_USE: &str =
    "the hand is already running or retaining another operation under this request id; it was not dispatched";

pub struct RemoteCatalog {
    pub definitions: Vec<ToolDefinition>,
    pub name: Option<String>,
    pub workspace: Option<String>,
    pub runtime_id: Option<String>,
    pub max_frame_bytes: Option<usize>,
    pub resumable: bool,
    pub tool_selection: bool,
    pub cancels_calls: bool,
}

impl RemoteCatalog {
    pub fn decode(text: &str, max_frame_bytes: usize) -> Result<Self, CodeModeHostError> {
        Self::try_from(RemoteFrame::decode(text, max_frame_bytes)?)
    }

    #[must_use]
    pub fn resumes(&self) -> Option<&str> {
        self.runtime_id.as_deref().filter(|_| self.resumable)
    }

    #[must_use]
    pub fn publishes(&self, name: &str) -> bool {
        self.definitions
            .iter()
            .any(|definition| definition.name() == name)
    }

    #[must_use]
    pub fn frame_limit(&self, limit: usize) -> usize {
        self.max_frame_bytes.map_or(limit, |bytes| bytes.min(limit))
    }
}

impl TryFrom<RemoteFrame> for RemoteCatalog {
    type Error = CodeModeHostError;

    fn try_from(frame: RemoteFrame) -> Result<Self, Self::Error> {
        match frame {
            RemoteFrame::Catalog {
                definitions,
                name,
                workspace,
                runtime_id,
                max_frame_bytes,
                resumable,
                tool_selection,
                cancels_calls,
            } => Ok(Self {
                definitions,
                name,
                workspace,
                runtime_id,
                max_frame_bytes,
                resumable,
                tool_selection,
                cancels_calls,
            }),
            _ => Err(CodeModeHostError::new(NO_CATALOG)),
        }
    }
}

pub fn nested(call: &NestedToolCall) -> Value {
    json!({"call_id":call.call_id,"name":call.name,"input":call.input,"output":call.output,"structured_result":call.structured_result,"success":call.success,"started_after_ns":call.started_after_ns,"duration_ns":call.duration_ns,"metadata":call.metadata})
}
pub fn execution_to_wire(value: CodeModeExecution) -> Value {
    json!({"cell":value.cell,"output":value.output,"success":value.success,"nested_calls":value.nested_calls.iter().map(nested).collect::<Vec<_>>(),"notifications":value.notifications.iter().map(|n| json!({"call_id":n.call_id,"text":n.text})).collect::<Vec<_>>()})
}
pub fn deliver_update(
    observer: &mut dyn CodeModeObserver,
    value: Value,
) -> Result<(), CodeModeHostError> {
    if let Some(call) = value.get("completed") {
        let call: NestedToolCall = serde_json::from_value(call.clone())
            .map_err(|error| CodeModeHostError::new(error.to_string()))?;
        observer.update(CodeModeUpdate::NestedCallCompleted(&call));
    } else if let Some(notification) = value.get("notification") {
        let notification: CodeModeNotification = serde_json::from_value(notification.clone())
            .map_err(|error| CodeModeHostError::new(error.to_string()))?;
        observer.update(CodeModeUpdate::Notification(&notification));
    } else {
        let call_id = value["call_id"]
            .as_str()
            .ok_or_else(|| CodeModeHostError::new("missing update call_id"))?;
        let name = value["name"]
            .as_str()
            .ok_or_else(|| CodeModeHostError::new("missing update name"))?;
        observer.update(CodeModeUpdate::NestedCallStarted {
            call_id,
            name,
            input: &value["input"],
        });
    }
    Ok(())
}
