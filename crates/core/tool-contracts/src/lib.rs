use nanocode_types_core::responses::ResponseItem;
pub use nanocode_types_core::tools::*;
use serde::de::DeserializeOwned;
use serde_json::value::RawValue;
use std::{
    future::Future,
    pin::Pin,
    sync::{Arc, OnceLock},
    time::Duration,
};

pub trait HistorySource: Send + Sync {
    fn items(&self) -> Vec<ResponseItem>;
}

#[derive(Default)]
pub struct SharedHistory {
    items: OnceLock<Arc<Vec<ResponseItem>>>,
    source: Option<Box<dyn HistorySource>>,
}

impl SharedHistory {
    #[must_use]
    pub fn new(items: Arc<Vec<ResponseItem>>) -> Self {
        Self {
            items: OnceLock::from(items),
            source: None,
        }
    }

    #[must_use]
    pub fn from_source(source: impl HistorySource + 'static) -> Self {
        Self {
            items: OnceLock::new(),
            source: Some(Box::new(source)),
        }
    }

    pub fn items(&self) -> &Arc<Vec<ResponseItem>> {
        self.items.get_or_init(|| {
            Arc::new(
                self.source
                    .as_ref()
                    .map_or_else(Vec::new, |source| source.items()),
            )
        })
    }
}

#[derive(Clone, Copy)]
enum History<'a> {
    Borrowed(&'a [ResponseItem]),
    Shared(&'a Arc<SharedHistory>),
}

#[derive(Clone, Copy)]
pub struct ToolContext<'a> {
    model: &'a str,
    session_id: &'a str,
    call_id: &'a str,
    turn_id: Option<&'a str>,
    history: History<'a>,
    output_token_budget: usize,
    host_context: Option<&'a str>,
    instruction_revision: Option<u64>,
}

impl<'a> ToolContext<'a> {
    #[must_use]
    pub const fn new(
        model: &'a str,
        session_id: &'a str,
        call_id: &'a str,
        history: &'a [ResponseItem],
        output_token_budget: usize,
    ) -> Self {
        Self {
            model,
            session_id,
            call_id,
            turn_id: None,
            history: History::Borrowed(history),
            output_token_budget,
            host_context: None,
            instruction_revision: None,
        }
    }

    #[must_use]
    pub const fn with_shared_history(mut self, history: &'a Arc<SharedHistory>) -> Self {
        self.history = History::Shared(history);
        self
    }

    #[must_use]
    pub const fn shared_history(self) -> Option<&'a Arc<SharedHistory>> {
        match self.history {
            History::Shared(history) => Some(history),
            History::Borrowed(_) => None,
        }
    }

    #[must_use]
    pub const fn with_instruction_revision(mut self, revision: Option<u64>) -> Self {
        self.instruction_revision = revision;
        self
    }

    #[must_use]
    pub const fn instruction_revision(self) -> Option<u64> {
        self.instruction_revision
    }

    #[doc(hidden)]
    #[must_use]
    pub const fn with_host_context(mut self, host_context: Option<&'a str>) -> Self {
        self.host_context = host_context;
        self
    }

    #[doc(hidden)]
    #[must_use]
    pub const fn host_context(self) -> Option<&'a str> {
        self.host_context
    }

    #[must_use]
    pub const fn model(self) -> &'a str {
        self.model
    }

    #[must_use]
    pub const fn session_id(self) -> &'a str {
        self.session_id
    }

    #[must_use]
    pub const fn call_id(self) -> &'a str {
        self.call_id
    }

    #[must_use]
    pub const fn with_session_id(mut self, session_id: &'a str) -> Self {
        self.session_id = session_id;
        self
    }

    #[must_use]
    pub const fn with_turn_id(mut self, turn_id: Option<&'a str>) -> Self {
        self.turn_id = turn_id;
        self
    }

    #[must_use]
    pub const fn turn_id(self) -> Option<&'a str> {
        self.turn_id
    }

    #[must_use]
    pub fn history(self) -> &'a [ResponseItem] {
        match self.history {
            History::Borrowed(history) => history,
            History::Shared(history) => history.items(),
        }
    }

    #[must_use]
    pub const fn output_token_budget(self) -> usize {
        self.output_token_budget
    }
}

pub enum ToolInput {
    Function(Box<RawValue>),

    Freeform(String),
}

impl ToolInput {
    pub fn function_json(&self) -> Result<&RawValue, ToolInputError> {
        match self {
            Self::Function(input) => Ok(input),
            Self::Freeform(_) => Err(ToolInputError::ExpectedFunction),
        }
    }

    pub fn decode_json<T: DeserializeOwned>(&self) -> Result<T, ToolInputError> {
        serde_json::from_str(self.function_json()?.get()).map_err(ToolInputError::Decode)
    }

    pub fn into_freeform(self) -> Result<String, ToolInputError> {
        match self {
            Self::Freeform(input) => Ok(input),
            Self::Function(_) => Err(ToolInputError::ExpectedFreeform),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ToolInputError {
    #[error("expected JSON function arguments")]
    ExpectedFunction,

    #[error("expected freeform tool input")]
    ExpectedFreeform,

    #[error("failed to parse function arguments: {0}")]
    Decode(#[source] serde_json::Error),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunningTask {
    pub id: String,
    pub label: String,
    pub silent_for: Duration,
}

pub trait Tool: Send + Sync + 'static {
    fn definition(&self) -> ToolDefinition;

    fn supports_parallel_tool_calls(&self) -> bool {
        false
    }

    fn running_tasks(&self) -> Vec<RunningTask> {
        Vec::new()
    }

    #[cfg(not(target_family = "wasm"))]
    fn end_turn<'life0, 'life1, 'life2, 'life3, 'async_trait>(
        &'life0 self,
        _session_id: &'life1 str,
        _turn_id: &'life2 str,
        _hook_event_name: &'life3 str,
    ) -> Pin<Box<dyn Future<Output = Result<(), ToolError>> + Send + 'async_trait>>
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        'life2: 'async_trait,
        'life3: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async { Ok(()) })
    }

    #[cfg(target_family = "wasm")]
    #[must_use]
    fn end_turn<'life0, 'life1, 'life2, 'life3, 'async_trait>(
        &'life0 self,
        _session_id: &'life1 str,
        _turn_id: &'life2 str,
        _hook_event_name: &'life3 str,
    ) -> Pin<Box<dyn Future<Output = Result<(), ToolError>> + 'async_trait>>
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        'life2: 'async_trait,
        'life3: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async { Ok(()) })
    }

    #[cfg(not(target_family = "wasm"))]
    fn execute<'life0, 'life1, 'async_trait>(
        &'life0 self,
        input: ToolInput,
        context: ToolContext<'life1>,
    ) -> Pin<Box<dyn Future<Output = ToolResult> + Send + 'async_trait>>
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        Self: 'async_trait;

    #[cfg(target_family = "wasm")]
    #[must_use]
    fn execute<'life0, 'life1, 'async_trait>(
        &'life0 self,
        input: ToolInput,
        context: ToolContext<'life1>,
    ) -> Pin<Box<dyn Future<Output = ToolResult> + 'async_trait>>
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        Self: 'async_trait;
}

pub trait ToolCollection: Sized {
    type Error;

    fn add_shared_tool(self, tool: std::sync::Arc<dyn Tool>) -> Result<Self, Self::Error>;
}

#[derive(Debug, thiserror::Error)]
pub enum ToolsBuildError {
    #[error("host tool initialization failed: {0}")]
    HostInitialization(String),

    #[error("tool name must not be empty")]
    EmptyName,

    #[error("working directory override must not be empty")]
    EmptyWorkingDirectory,

    #[error("default shell override must not be empty")]
    EmptyDefaultShell,

    #[error("tool name `{0}` is registered more than once")]
    DuplicateName(Box<str>),

    #[error("tool source `{0}` is configured more than once")]
    DuplicateSource(&'static str),

    #[error("tool name `{0}` conflicts with an enabled built-in tool")]
    BuiltInName(Box<str>),

    #[error("tool name `{0}` is reserved by the Code Mode host")]
    ReservedName(Box<str>),

    #[error(
        "invalid public tool name `{0}`; expected 1-128 ASCII bytes beginning with an alphanumeric and containing only alphanumerics, `.`, `_`, `:`, or `-`"
    )]
    InvalidPublicName(Box<str>),

    #[error("tool names `{first}` and `{second}` both normalize to Code Mode name `{normalized}`")]
    NormalizedNameCollision {
        first: Box<str>,

        second: Box<str>,

        normalized: Box<str>,
    },
}
