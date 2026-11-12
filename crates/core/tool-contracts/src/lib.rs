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

#[derive(Debug)]
pub enum ToolInputError {
    ExpectedFunction,

    ExpectedFreeform,

    Decode(serde_json::Error),
}

impl std::fmt::Display for ToolInputError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ExpectedFunction => formatter.write_str("expected JSON function arguments"),
            Self::ExpectedFreeform => formatter.write_str("expected freeform tool input"),
            Self::Decode(value) => write!(formatter, "failed to parse function arguments: {value}"),
        }
    }
}

impl std::error::Error for ToolInputError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Decode(value) => Some(value),
            _ => None,
        }
    }
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

    #[cfg(not(target_family = "wasm"))]
    fn recover<'life0, 'life1, 'async_trait>(
        &'life0 self,
        _input: ToolInput,
        _context: ToolContext<'life1>,
    ) -> Option<Pin<Box<dyn Future<Output = Option<ToolOutput>> + Send + 'async_trait>>>
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        Self: 'async_trait,
    {
        None
    }

    #[cfg(target_family = "wasm")]
    fn recover<'life0, 'life1, 'async_trait>(
        &'life0 self,
        _input: ToolInput,
        _context: ToolContext<'life1>,
    ) -> Option<Pin<Box<dyn Future<Output = Option<ToolOutput>> + 'async_trait>>>
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        Self: 'async_trait,
    {
        None
    }
}

pub trait ToolCollection: Sized {
    type Error;

    fn add_shared_tool(self, tool: std::sync::Arc<dyn Tool>) -> Result<Self, Self::Error>;

    fn add_shared_tools(
        self,
        tools: impl IntoIterator<Item = std::sync::Arc<dyn Tool>>,
    ) -> Result<Self, Self::Error> {
        tools
            .into_iter()
            .try_fold(self, |collection, tool| collection.add_shared_tool(tool))
    }
}

#[derive(Debug)]
pub enum ToolsBuildError {
    HostInitialization(String),

    EmptyName,

    EmptyWorkingDirectory,

    EmptyDefaultShell,

    DuplicateName(Box<str>),

    DuplicateSource(&'static str),

    BuiltInName(Box<str>),

    ReservedName(Box<str>),

    InvalidPublicName(Box<str>),

    NormalizedNameCollision {
        first: Box<str>,

        second: Box<str>,

        normalized: Box<str>,
    },
}

impl std::fmt::Display for ToolsBuildError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::HostInitialization(value) => {
                write!(formatter, "host tool initialization failed: {value}")
            }
            Self::EmptyName => formatter.write_str("tool name must not be empty"),
            Self::EmptyWorkingDirectory => {
                formatter.write_str("working directory override must not be empty")
            }
            Self::EmptyDefaultShell => {
                formatter.write_str("default shell override must not be empty")
            }
            Self::DuplicateName(value) => write!(
                formatter,
                "tool name `{value}` is registered more than once"
            ),
            Self::DuplicateSource(value) => write!(
                formatter,
                "tool source `{value}` is configured more than once"
            ),
            Self::BuiltInName(value) => write!(
                formatter,
                "tool name `{value}` conflicts with an enabled built-in tool"
            ),
            Self::ReservedName(value) => write!(
                formatter,
                "tool name `{value}` is reserved by the Code Mode host"
            ),
            Self::InvalidPublicName(value) => write!(
                formatter,
                "invalid public tool name `{value}`; expected 1-128 ASCII bytes beginning with an alphanumeric and containing only alphanumerics, `.`, `_`, `:`, or `-`"
            ),
            Self::NormalizedNameCollision {
                first,
                second,
                normalized,
            } => write!(
                formatter,
                "tool names `{first}` and `{second}` both normalize to Code Mode name `{normalized}`"
            ),
        }
    }
}

impl std::error::Error for ToolsBuildError {}
