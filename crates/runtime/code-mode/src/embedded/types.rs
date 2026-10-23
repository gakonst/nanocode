use std::sync::Arc;

use nanocode_tool_contracts::{SharedHistory, ToolContext, ToolOutputBody};
use nanocode_types_core::responses::ResponseItem;
use serde::Deserialize;
use serde_json::{Value, value::RawValue};

pub struct OwnedToolContext {
    pub instruction_revision: Option<u64>,
    pub model: String,
    pub session_id: String,
    pub call_id: String,
    pub turn_id: Option<Arc<str>>,
    pub history: Arc<SharedHistory>,
    pub output_token_budget: usize,
    pub host_context: Option<Arc<str>>,
}

pub(crate) fn shared_history(context: ToolContext<'_>) -> Arc<SharedHistory> {
    context.shared_history().map_or_else(
        || Arc::new(SharedHistory::new(Arc::new(context.history().to_vec()))),
        Arc::clone,
    )
}

impl OwnedToolContext {
    #[must_use]
    pub fn new(
        model: impl Into<String>,
        session_id: impl Into<String>,
        call_id: impl Into<String>,
        history: Arc<Vec<ResponseItem>>,
        output_token_budget: usize,
    ) -> Self {
        Self {
            instruction_revision: None,
            model: model.into(),
            session_id: session_id.into(),
            call_id: call_id.into(),
            turn_id: None,
            history: Arc::new(SharedHistory::new(history)),
            output_token_budget,
            host_context: None,
        }
    }

    #[must_use]
    pub fn with_history(mut self, history: Arc<SharedHistory>) -> Self {
        self.history = history;
        self
    }

    #[must_use]
    pub fn from_context(context: ToolContext<'_>) -> Self {
        let history = shared_history(context);
        Self::new(
            context.model(),
            context.session_id(),
            context.call_id(),
            Arc::default(),
            context.output_token_budget(),
        )
        .with_history(history)
        .with_instruction_revision(context.instruction_revision())
        .with_host_context(context.host_context().map(Arc::from))
        .with_turn_id(context.turn_id().map(Arc::from))
    }

    #[must_use]
    pub fn as_context(&self) -> ToolContext<'_> {
        ToolContext::new(
            &self.model,
            &self.session_id,
            &self.call_id,
            &[],
            self.output_token_budget,
        )
        .with_shared_history(&self.history)
        .with_instruction_revision(self.instruction_revision)
        .with_host_context(self.host_context.as_deref())
        .with_turn_id(self.turn_id.as_deref())
    }

    #[must_use]
    pub const fn with_instruction_revision(mut self, revision: Option<u64>) -> Self {
        self.instruction_revision = revision;
        self
    }

    #[must_use]
    pub fn with_turn_id(mut self, turn_id: Option<Arc<str>>) -> Self {
        self.turn_id = turn_id;
        self
    }

    #[doc(hidden)]
    #[must_use]
    pub fn with_host_context(mut self, host_context: Option<Arc<str>>) -> Self {
        self.host_context = host_context;
        self
    }

    pub const fn with_output_token_budget(mut self, output_token_budget: usize) -> Self {
        self.output_token_budget = output_token_budget;
        self
    }
}

#[derive(Clone, Debug, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct CodeModeCell {
    pub origin_call_id: String,

    pub running: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodeModeExecution {
    pub cell: Option<CodeModeCell>,

    pub output: ToolOutputBody,

    pub success: bool,

    #[serde(default)]
    pub nested_calls: Vec<NestedToolCall>,

    #[serde(default)]
    pub notifications: Vec<CodeModeNotification>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodeModeNotification {
    pub call_id: String,

    pub text: String,
}

impl CodeModeNotification {
    pub fn new(call_id: &str, text: String) -> Self {
        Self {
            call_id: call_id.to_owned(),
            text,
        }
    }
}

pub enum CodeModeUpdate<'a> {
    NestedCallStarted {
        call_id: &'a str,

        name: &'a str,

        input: &'a Value,
    },

    NestedCallCompleted(&'a NestedToolCall),

    Notification(&'a CodeModeNotification),
}

pub trait CodeModeObserver: Send {
    fn update(&mut self, update: CodeModeUpdate<'_>);

    fn completed(&mut self) {}
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NestedToolCall {
    pub call_id: String,

    pub name: String,

    pub input: Value,

    pub output: ToolOutputBody,

    pub structured_result: Value,

    pub success: bool,

    pub started_after_ns: u64,

    pub duration_ns: u64,

    #[serde(default)]
    pub metadata: Option<Box<RawValue>>,
}
