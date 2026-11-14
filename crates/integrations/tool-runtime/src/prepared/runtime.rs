use std::{panic::AssertUnwindSafe, sync::Arc};

use futures_util::FutureExt;
use nanocode_tools::ToolOutputWire;
use serde::Serialize;
use serde_json::{Value, value::to_raw_value};
use nanocode_trace::Instrument;

use super::PreparedTools;
use crate::{Tool, ToolContext, ToolDefinition, ToolInput, ToolOutput};

const DEFAULT_TOOL_TIMEOUT_MS: u64 = 9_007_199_254_740_991;

const MCP_DISCOVERY_TIMEOUT_MS: u64 = 120_000;

enum PreparedToolHandler {
    Fixed(Arc<dyn Tool>),

    Mcp(crate::mcp::PreparedMcpTool),
}

enum PreparedToolInput {
    Contract(ToolInput),

    Mcp(Value),
}

pub(crate) struct PreparedToolEntry {
    provider: Box<str>,
    remote_name: Box<str>,
    definition: ToolDefinition,
    supports_parallel_tool_calls: bool,
    timeout_ms: u64,
    routing: Option<nanocode_tool_host::WorkspaceToolRouting>,
    handler: PreparedToolHandler,
}

impl PreparedToolEntry {
    pub(crate) fn fixed(
        definition: ToolDefinition,
        supports_parallel_tool_calls: bool,
        tool: Arc<dyn Tool>,
    ) -> Self {
        let remote_name = definition.name().into();
        Self {
            provider: "fixed".into(),
            remote_name,
            definition,
            supports_parallel_tool_calls,
            timeout_ms: DEFAULT_TOOL_TIMEOUT_MS,
            routing: None,
            handler: PreparedToolHandler::Fixed(tool),
        }
    }

    pub(crate) fn workspace(
        definition: ToolDefinition,
        timeout_ms: Option<u64>,
        tool: Arc<dyn Tool>,
    ) -> Self {
        Self {
            provider: "workspace".into(),
            remote_name: definition.name().into(),
            definition,
            supports_parallel_tool_calls: tool.supports_parallel_tool_calls(),
            timeout_ms: timeout_ms.unwrap_or(DEFAULT_TOOL_TIMEOUT_MS),
            routing: None,
            handler: PreparedToolHandler::Fixed(tool),
        }
    }

    pub(crate) fn mcp(tool: crate::mcp::PreparedMcpTool) -> Self {
        Self {
            provider: tool.provider().into(),
            remote_name: tool.remote_name().into(),
            definition: tool.definition().clone(),
            supports_parallel_tool_calls: tool.supports_parallel_tool_calls(),
            timeout_ms: u64::try_from(tool.timeout().as_millis()).unwrap_or(u64::MAX),
            routing: None,
            handler: PreparedToolHandler::Mcp(tool),
        }
    }

    pub(crate) const fn definition(&self) -> &ToolDefinition {
        &self.definition
    }

    pub(crate) fn classify(
        &mut self,
        policies: &[Arc<dyn nanocode_tool_host::CodeModeToolPolicy>],
    ) {
        self.routing = policies
            .iter()
            .find_map(|policy| policy.workspace_routing(&self.remote_name));
    }
}

#[doc(hidden)]
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum PreparedToolDefinition {
    Function {
        name: Box<str>,
        description: Box<str>,
        strict: bool,
        parameters: Value,
        #[serde(skip_serializing_if = "Option::is_none")]
        output_schema: Option<Value>,
    },

    Custom {
        name: Box<str>,
        description: Box<str>,
        format: PreparedCustomToolFormat,
    },
}

#[doc(hidden)]
#[derive(Clone, Debug, Serialize)]
pub(crate) struct PreparedCustomToolFormat {
    #[serde(rename = "type")]
    kind: &'static str,
    syntax: Box<str>,
    definition: Box<str>,
}

#[doc(hidden)]
#[derive(Clone, Debug, Serialize)]
pub(crate) struct PreparedToolCatalogEntry {
    provider: Box<str>,
    remote_name: Box<str>,
    definition: PreparedToolDefinition,
    parallel_safe: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    summary: Option<Box<str>>,
    timeout_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    routing: Option<&'static str>,
}

#[doc(hidden)]
pub(crate) struct PreparedToolCall {
    model: String,
    session_id: String,
    turn_id: Option<String>,
    call_id: String,
    name: String,
    input: Value,
    output_token_budget: usize,
}

impl PreparedToolCall {
    #[must_use]
    pub fn new(
        model: impl Into<String>,
        session_id: impl Into<String>,
        call_id: impl Into<String>,
        name: impl Into<String>,
        input: Value,
        output_token_budget: usize,
    ) -> Self {
        Self {
            model: model.into(),
            session_id: session_id.into(),
            turn_id: None,
            call_id: call_id.into(),
            name: name.into(),
            input,
            output_token_budget,
        }
    }

    pub(crate) fn with_turn_id(mut self, turn_id: Option<String>) -> Self {
        self.turn_id = turn_id;
        self
    }
}

#[doc(hidden)]
pub(crate) struct PreparedToolRuntime {
    entries: Vec<PreparedToolEntry>,

    workspaces: Vec<Arc<dyn crate::workspace_runtime::WorkspaceExecutor>>,
}

impl PreparedToolRuntime {
    pub async fn initialize(tools: PreparedTools) -> Result<Self, PreparedToolError> {
        let PreparedTools {
            mut entries,
            mcps,
            workspaces,
        } = tools;

        for mcp in mcps {
            let tools = mcp
                .prepared_snapshot(std::time::Duration::from_millis(MCP_DISCOVERY_TIMEOUT_MS))
                .await
                .map_err(PreparedToolError::McpInitialization)?;
            entries.extend(tools.into_iter().map(PreparedToolEntry::mcp));
        }
        entries.sort_by(|left, right| left.definition().name().cmp(right.definition().name()));
        for pair in entries.windows(2) {
            if pair[0].definition().name() == pair[1].definition().name() {
                return Err(PreparedToolError::DuplicateTool(
                    pair[0].definition().name().into(),
                ));
            }
        }
        Ok(Self {
            entries,

            workspaces,
        })
    }

    pub(crate) async fn end_turn(&self, session_id: &str, turn_id: &str, hook_event_name: &str) {
        for entry in &self.entries {
            match &entry.handler {
                PreparedToolHandler::Fixed(tool) => {
                    if let Err(error) = tool.end_turn(session_id, turn_id, hook_event_name).await {
                        nanocode_trace::warn!(target: "nanocode_code_mode::attachment",
                            %error, "attached tool turn cleanup failed; not retried");
                    }
                }

                PreparedToolHandler::Mcp(_) => {}
            }
        }
    }

    pub(crate) fn timeout_ms(&self, name: &str) -> Option<u64> {
        self.entries
            .iter()
            .find(|entry| entry.definition.name() == name)
            .map(|entry| entry.timeout_ms)
    }

    pub fn catalog(&self) -> Result<Vec<PreparedToolCatalogEntry>, PreparedToolError> {
        self.entries.iter().map(catalog_entry).collect()
    }

    pub async fn execute(
        &self,
        call: PreparedToolCall,
    ) -> Result<ToolOutputWire, PreparedToolError> {
        let entry = self
            .entries
            .iter()
            .find(|entry| entry.definition.name() == call.name)
            .ok_or_else(|| PreparedToolError::UnknownTool(call.name.clone().into()))?;
        let input_content = nanocode_trace::enabled!(
            target: "nanocode_code_mode",
            nanocode_trace::Level::INFO
        )
        .then(|| serde_json::to_string(&call.input))
        .transpose()
        .map_err(PreparedToolError::InvalidOutput)?;
        let input = match &entry.handler {
            PreparedToolHandler::Mcp(_) => {
                if !call.input.is_object() {
                    return Err(PreparedToolError::InvalidInput {
                        name: call.name.into(),
                        expected: "an object",
                    });
                }
                PreparedToolInput::Mcp(call.input)
            }
            _ => PreparedToolInput::Contract(input_for_definition(&entry.definition, call.input)?),
        };
        let context = ToolContext::new(
            &call.model,
            &call.session_id,
            &call.call_id,
            &[],
            call.output_token_budget,
        )
        .with_turn_id(call.turn_id.as_deref());
        let started_at = std::time::Instant::now();
        let span = nanocode_trace::info_span!(
            target: "nanocode_code_mode",
            "attached_tool.execute",
            tool.name = call.name,
            session.id = call.session_id,
            tool.call_id = call.call_id,
            status = nanocode_trace::field::Empty,
            duration_ns = nanocode_trace::field::Empty,
        );
        if let Some(input_content) = &input_content {
            nanocode_trace::event!(
                target: "nanocode_code_mode",
                parent: &span,
                nanocode_trace::Level::INFO,
                name = "tool.arguments",
                content = %input_content,
                "tool content"
            );
        }
        let dispatch = async {
            match (&entry.handler, input) {
                (PreparedToolHandler::Fixed(tool), PreparedToolInput::Contract(input)) => tool
                    .execute(input, context)
                    .await
                    .unwrap_or_else(|error| ToolOutput::error(error.to_string())),

                (PreparedToolHandler::Mcp(tool), PreparedToolInput::Mcp(input)) => {
                    tool.execute(input, context).await
                }

                _ => ToolOutput::error("invalid attached tool input routing"),
            }
        }
        .instrument(span.clone());
        let output = match AssertUnwindSafe(dispatch).catch_unwind().await {
            Ok(output) => output,
            Err(payload) => {
                let message = nanocode_tool_host::panic_message(payload);
                nanocode_trace::event!(
                    target: "nanocode_code_mode",
                    parent: &span,
                    nanocode_trace::Level::INFO,
                    name = "tool.panic",
                    content = %message,
                    "tool content"
                );
                ToolOutput::error("aborted")
            }
        };
        let wire = output
            .into_wire()
            .map_err(PreparedToolError::InvalidOutput)?;
        span.record("status", if wire.success { "completed" } else { "failed" });
        span.record(
            "duration_ns",
            u64::try_from(started_at.elapsed().as_nanos()).unwrap_or(u64::MAX),
        );
        if nanocode_trace::enabled!(target: "nanocode_code_mode", nanocode_trace::Level::INFO) {
            let output_content =
                serde_json::to_string(&wire).map_err(PreparedToolError::InvalidOutput)?;
            nanocode_trace::event!(
                target: "nanocode_code_mode",
                parent: &span,
                nanocode_trace::Level::INFO,
                name = "tool.output",
                content = %output_content,
                "tool content"
            );
        }
        Ok(wire)
    }

    pub async fn shutdown(&self) {
        for workspace in &self.workspaces {
            workspace.cancel().await;
        }
    }
}

fn catalog_entry(entry: &PreparedToolEntry) -> Result<PreparedToolCatalogEntry, PreparedToolError> {
    let definition = match &entry.definition {
        ToolDefinition::Function {
            name,
            description,
            strict,
            parameters,
            output_schema,
            ..
        } => PreparedToolDefinition::Function {
            name: name.clone(),
            description: description.as_ref().into(),
            strict: *strict,
            parameters: parameters.as_value().clone(),
            output_schema: output_schema
                .as_ref()
                .map(|schema| schema.as_value().clone()),
        },
        ToolDefinition::Custom {
            name,
            description,
            format,
            ..
        } => PreparedToolDefinition::Custom {
            name: name.clone(),
            description: description.as_ref().into(),
            format: PreparedCustomToolFormat {
                kind: "grammar",
                syntax: format.syntax.clone(),
                definition: format.definition.clone(),
            },
        },
        definition => {
            return Err(PreparedToolError::UnsupportedDefinition(
                definition.name().into(),
            ));
        }
    };
    Ok(PreparedToolCatalogEntry {
        provider: entry.provider.clone(),
        remote_name: entry.remote_name.clone(),
        definition,
        parallel_safe: entry.supports_parallel_tool_calls,
        summary: None,
        timeout_ms: entry.timeout_ms,
        routing: entry
            .routing
            .map(nanocode_tool_host::WorkspaceToolRouting::as_str),
    })
}

fn input_for_definition(
    definition: &ToolDefinition,
    input: Value,
) -> Result<ToolInput, PreparedToolError> {
    match definition {
        ToolDefinition::Function { name, .. } if !input.is_object() => {
            Err(PreparedToolError::InvalidInput {
                name: name.clone(),
                expected: "an object",
            })
        }
        ToolDefinition::Function { .. } => to_raw_value(&input)
            .map(ToolInput::Function)
            .map_err(PreparedToolError::InvalidOutput),
        ToolDefinition::Custom { name, .. } => input
            .as_str()
            .map(|input| ToolInput::Freeform(input.to_owned()))
            .ok_or_else(|| PreparedToolError::InvalidInput {
                name: name.clone(),
                expected: "a string",
            }),
        ToolDefinition::Namespace { name, .. } => {
            Err(PreparedToolError::UnsupportedDefinition(name.clone()))
        }
        ToolDefinition::ToolSearch { .. } => Err(PreparedToolError::UnsupportedDefinition(
            "tool_search".into(),
        )),
    }
}

#[doc(hidden)]
#[derive(Debug)]
pub(crate) enum PreparedToolError {
    NonAttachable(Box<str>),

    DuplicateTool(Box<str>),

    McpInitialization(String),

    UnknownTool(Box<str>),

    InvalidInput {
        name: Box<str>,

        expected: &'static str,
    },

    UnsupportedDefinition(Box<str>),

    InvalidOutput(serde_json::Error),
}

impl std::fmt::Display for PreparedToolError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NonAttachable(value) => {
                write!(formatter, "tool selection is not attachable: {value}")
            }
            Self::DuplicateTool(value) => write!(
                formatter,
                "attached tool name `{value}` is present more than once after discovery"
            ),
            Self::McpInitialization(value) => {
                write!(formatter, "failed to initialize attached MCP: {value}")
            }
            Self::UnknownTool(value) => write!(formatter, "unknown attached tool `{value}`"),
            Self::InvalidInput { name, expected } => write!(
                formatter,
                "attached tool `{name}` requires {expected} input"
            ),
            Self::UnsupportedDefinition(value) => write!(
                formatter,
                "attached tool `{value}` has a non-executable definition"
            ),
            Self::InvalidOutput(value) => {
                write!(formatter, "failed to encode attached tool output: {value}")
            }
        }
    }
}

impl std::error::Error for PreparedToolError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidOutput(value) => Some(value),
            _ => None,
        }
    }
}
