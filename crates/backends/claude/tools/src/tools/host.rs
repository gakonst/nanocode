use nanocode_tools::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug)]
pub struct HostContext<'a> {
    model: &'a str,
    session_id: &'a str,
    turn_id: Option<&'a str>,
    call_id: &'a str,
    max_output_tokens: usize,
}
impl<'a> HostContext<'a> {
    #[must_use]
    pub const fn new(
        model: &'a str,
        session_id: &'a str,
        call_id: &'a str,
        max_output_tokens: usize,
    ) -> Self {
        Self {
            model,
            session_id,
            turn_id: None,
            call_id,
            max_output_tokens,
        }
    }

    #[must_use]
    pub const fn with_turn_id(mut self, turn_id: Option<&'a str>) -> Self {
        self.turn_id = turn_id;
        self
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
    pub const fn turn_id(self) -> Option<&'a str> {
        self.turn_id
    }

    #[must_use]
    pub const fn call_id(self) -> &'a str {
        self.call_id
    }

    #[must_use]
    pub const fn max_output_tokens(self) -> usize {
        self.max_output_tokens
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ImageSource {
    Base64 { media_type: String, data: String },

    Url { url: String },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolResultBlock {
    Text { text: String },

    Image { source: ImageSource },

    Document { file_data: String },

    UnsupportedMedia { media_type: String },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ToolContent {
    Text(String),

    Blocks(Vec<ToolResultBlock>),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolOutput {
    pub content: ToolContent,

    pub is_error: bool,

    pub structured_result: Option<Value>,

    pub metadata: Option<Value>,
}
impl ToolOutput {
    #[must_use]
    pub fn text(text: impl Into<String>) -> Self {
        let text = text.into();
        Self {
            structured_result: Some(Value::String(text.clone())),
            content: ToolContent::Text(text),
            is_error: false,
            metadata: None,
        }
    }

    #[must_use]
    pub fn error(text: impl Into<String>) -> Self {
        Self {
            is_error: true,
            ..Self::text(text)
        }
    }

    #[must_use]
    pub const fn content(blocks: Vec<ToolResultBlock>) -> Self {
        Self {
            content: ToolContent::Blocks(blocks),
            is_error: false,
            structured_result: None,
            metadata: None,
        }
    }

    #[must_use]
    pub fn with_structured_result(mut self, result: Value) -> Self {
        self.structured_result = Some(result);
        self
    }

    #[must_use]
    pub fn with_metadata(mut self, metadata: Value) -> Self {
        self.metadata = Some(metadata);
        self
    }
}

pub const MAX_HOST_INPUT_BYTES: usize = 64 * 1024;

pub const MAX_TASK_OUTPUT_TIMEOUT_MS: u64 = 300_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum HostTool {
    TaskOutput,

    TaskStop,
}
impl HostTool {
    pub const ALL: [Self; 2] = [Self::TaskOutput, Self::TaskStop];

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::TaskOutput => "TaskOutput",
            Self::TaskStop => "TaskStop",
        }
    }

    #[must_use]
    pub fn definition(self) -> Value {
        definition(self)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostRequest {
    TaskOutput(TaskOutputRequest),

    TaskStop(TaskStopRequest),
}

pub trait ClaudeHost: Send + Sync {
    fn execute(
        &self,
        request: HostRequest,
        context: HostContext<'_>,
    ) -> impl std::future::Future<Output = Result<ToolOutput, String>> + Send;
}

pub struct ClaudeHostTools<H: ClaudeHost> {
    host: H,
    enabled: BTreeSet<HostTool>,
}
impl<H: ClaudeHost> ClaudeHostTools<H> {
    pub fn new(host: H, enabled: impl IntoIterator<Item = HostTool>) -> Self {
        Self {
            host,
            enabled: enabled.into_iter().collect(),
        }
    }

    #[must_use]
    pub fn definitions(&self) -> Vec<Value> {
        self.enabled.iter().map(|tool| definition(*tool)).collect()
    }

    pub async fn execute(
        &self,
        name: &str,
        input: Value,
        context: HostContext<'_>,
    ) -> Result<ToolOutput, String> {
        let tool = self
            .enabled
            .iter()
            .find(|tool| tool.name() == name)
            .copied()
            .ok_or_else(|| format!("Claude host tool is unavailable: {name}"))?;
        if context.session_id().is_empty()
            || context.call_id().is_empty()
            || context.turn_id().is_none_or(str::is_empty)
        {
            return Err("host tools require real session, turn and call identities".into());
        }
        let request = match tool {
            HostTool::TaskOutput => {
                let request: TaskOutputRequest = decode(input)?;
                if request.timeout > MAX_TASK_OUTPUT_TIMEOUT_MS {
                    return Err(format!(
                        "timeout must be 0..{MAX_TASK_OUTPUT_TIMEOUT_MS} milliseconds"
                    ));
                }
                HostRequest::TaskOutput(request)
            }
            HostTool::TaskStop => {
                let request: TaskStopRequest = decode(input)?;
                HostRequest::TaskStop(request)
            }
        };
        self.host.execute(request, context).await
    }
}
fn decode<T: serde::de::DeserializeOwned>(input: Value) -> Result<T, String> {
    serde_json::from_value(input).map_err(|e| format!("invalid Claude host input: {e}"))
}
fn definition(tool: HostTool) -> Value {
    match tool {
        HostTool::TaskOutput => json!({
            "name": tool.name(),
            "description": "SDK extension: read a host-owned background task's status and output, optionally waiting up to timeout milliseconds.",
            "input_schema": {"type":"object","properties":{"task_id":{"type":"string"},"block":{"type":"boolean","default":default_block()},"timeout":{"type":"integer","minimum":0,"maximum":MAX_TASK_OUTPUT_TIMEOUT_MS,"default":default_timeout(),"description":format!("Milliseconds to wait when block is true: 0 to {MAX_TASK_OUTPUT_TIMEOUT_MS}, default {}", default_timeout())}},"required":["task_id"],"additionalProperties":false}
        }),
        _ => crate::upstream::tool_value(tool.name()),
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TaskOutputRequest {
    pub task_id: String,

    #[serde(default = "default_block")]
    pub block: bool,

    #[serde(default = "default_timeout")]
    pub timeout: u64,
}
const fn default_block() -> bool {
    true
}
const fn default_timeout() -> u64 {
    30_000
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(try_from = "TaskStopInput")]
pub struct TaskStopRequest {
    pub task_id: String,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct TaskStopInput {
    task_id: Option<String>,
    shell_id: Option<String>,
}

impl TryFrom<TaskStopInput> for TaskStopRequest {
    type Error = &'static str;

    fn try_from(input: TaskStopInput) -> Result<Self, Self::Error> {
        input
            .task_id
            .or(input.shell_id)
            .map(|task_id| Self { task_id })
            .ok_or("missing field `task_id`")
    }
}

pub fn mcp_tool_output(result: Value) -> Result<ToolOutput, String> {
    let content = result
        .get("content")
        .and_then(Value::as_array)
        .ok_or("MCP result omitted its content array")?;
    let mut blocks = Vec::new();
    for item in content {
        match item.get("type").and_then(Value::as_str) {
            Some("text") => blocks.push(ToolResultBlock::Text {
                text: item
                    .get("text")
                    .and_then(Value::as_str)
                    .ok_or("MCP text content omitted text")?
                    .to_owned(),
            }),
            Some("image") => {
                let media_type = item
                    .get("mimeType")
                    .and_then(Value::as_str)
                    .ok_or("MCP image omitted mimeType")?;
                if !matches!(
                    media_type,
                    "image/png" | "image/jpeg" | "image/gif" | "image/webp"
                ) {
                    return Err(format!(
                        "MCP image type is unsupported by Claude: {media_type}"
                    ));
                }
                blocks.push(ToolResultBlock::Image {
                    source: ImageSource::Base64 {
                        media_type: media_type.to_owned(),
                        data: item
                            .get("data")
                            .and_then(Value::as_str)
                            .ok_or("MCP image omitted data")?
                            .to_owned(),
                    },
                });
            }
            Some("resource") => {
                let resource = item
                    .get("resource")
                    .ok_or("MCP resource omitted resource body")?;

                if let Some(blob) = resource.get("blob") {
                    let data = blob.as_str().ok_or("MCP resource blob must be a string")?;
                    let media_type = resource
                        .get("mimeType")
                        .and_then(Value::as_str)
                        .ok_or("MCP binary resource omitted mimeType")?;
                    match media_type {
                        "application/pdf" | "text/plain" => {
                            blocks.push(ToolResultBlock::Document {
                                file_data: format!("data:{media_type};base64,{data}"),
                            })
                        }
                        "image/png" | "image/jpeg" | "image/gif" | "image/webp" => {
                            blocks.push(ToolResultBlock::Image {
                                source: ImageSource::Base64 {
                                    media_type: media_type.to_owned(),
                                    data: data.to_owned(),
                                },
                            })
                        }
                        _ => {
                            return Err(format!(
                                "MCP binary resource type is unsupported by Claude: {media_type}"
                            ));
                        }
                    }
                } else if resource.get("text").and_then(Value::as_str).is_some() {
                    blocks.push(ToolResultBlock::Text {
                        text: serde_json::to_string(resource).map_err(|e| e.to_string())?,
                    });
                } else {
                    return Err("MCP resource omitted text or blob".into());
                }
            }
            Some("resource_link") => blocks.push(ToolResultBlock::Text {
                text: serde_json::to_string(item).map_err(|e| e.to_string())?,
            }),
            kind => {
                return Err(format!(
                    "MCP content cannot be represented in Claude Messages: {}",
                    kind.unwrap_or("missing type")
                ));
            }
        }
    }
    let mut output = ToolOutput::content(blocks);
    output.is_error = result
        .get("isError")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    output.metadata = result.get("_meta").cloned();
    output.structured_result = Some(result);
    Ok(output)
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct McpToolDefinition {
    pub name: String,

    pub description: String,

    pub input_schema: Value,
}
impl McpToolDefinition {
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        input_schema: Value,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            input_schema,
        }
    }
}

pub trait ClaudeMcpProvider: Send + Sync {
    fn start(&self);

    fn available_definitions(&self) -> Vec<McpToolDefinition>;

    fn execute(
        &self,
        name: &str,
        input: Value,
        context: HostContext<'_>,
    ) -> impl std::future::Future<Output = Result<Option<ToolOutput>, String>> + Send;
}

pub struct ClaudeMcp<P: ClaudeMcpProvider + ?Sized> {
    provider: std::sync::Arc<P>,
}
impl<P: ClaudeMcpProvider + ?Sized> ClaudeMcp<P> {
    #[must_use]
    pub const fn new(provider: std::sync::Arc<P>) -> Self {
        Self { provider }
    }

    pub fn start(&self) {
        self.provider.start();
    }

    pub fn definitions(&self) -> Result<Vec<Value>, String> {
        let mut names = BTreeSet::new();
        self.provider.available_definitions().into_iter().map(|definition| {
            let name = &definition.name;
            if !name.starts_with("mcp__") || !names.insert(name.clone()) {
                return Err(format!("invalid or duplicate MCP tool name: {name}"));
            }
            if !definition.input_schema.is_object() { return Err("MCP input schema must be an object".into()); }
            Ok(json!({"name":name,"description":definition.description,"input_schema":definition.input_schema}))
        }).collect()
    }

    pub async fn execute(
        &self,
        name: &str,
        input: Value,
        context: HostContext<'_>,
    ) -> Result<ToolOutput, String> {
        if !self
            .definitions()?
            .iter()
            .any(|definition| definition["name"].as_str() == Some(name))
        {
            return Err(format!("MCP tool is no longer available: {name}"));
        }
        if !input.is_object() {
            return Err("MCP input must be an object".into());
        }
        self.provider
            .execute(name, input, context)
            .await?
            .ok_or_else(|| format!("MCP tool became unavailable: {name}"))
    }
}
