#[cfg(all(target_family = "wasm", not(target_os = "unknown")))]
compile_error!(
    "nanocode-backend-claude-client supports native targets and hosted wasm*-unknown-unknown targets; WASI is not yet supported"
);

use std::{collections::BTreeMap, future::Future, pin::Pin, sync::Arc, time::Duration};

use futures_util::{Stream, StreamExt, stream};
use nanocode_agent_types::{ClaudeTokens, EstimatedUsdCost, UsdAmount};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

mod auth;
#[cfg(not(target_family = "wasm"))]
mod claude_code;
#[cfg(not(target_family = "wasm"))]
mod federation;
#[cfg(not(target_family = "wasm"))]
mod home;
mod subscription_identity;
#[doc(hidden)]
pub mod subscription_wire;
pub mod transport;
pub use subscription_wire::SubscriptionIdentity;
use transport::{HttpClient, HttpError, HttpResponse, layer::retry};
pub mod subscription;
pub mod thread;
mod wire_cache;
pub use auth::{ClaudeAccessToken, ClaudeTokenSource, RefreshingClaudeAuth};
#[cfg(not(target_family = "wasm"))]
pub use federation::{ClaudeIdentityTokenSource, ClaudeWorkloadIdentity};
#[cfg(not(target_family = "wasm"))]
pub use home::ClaudeHomeLogin;
pub use thread::{
    CreateReason, MESSAGE_THREADS_BETA, MessageThread, THREAD_IDLE_EXPIRY, ThreadErrorCode,
    ThreadMode, ThreadParam, ThreadRecovery, ThreadTurn,
};
pub use wire_cache::WireCache;

pub const ANTHROPIC_MESSAGES_URL: &str = "https://api.anthropic.com/v1/messages";

pub const ANTHROPIC_SUBSCRIPTION_MESSAGES_URL: &str =
    "https://api.anthropic.com/v1/messages?beta=true";
pub const ANTHROPIC_VERSION: &str = "2023-06-01";

const FAST_MODE_BETA: &str = "fast-mode-2026-02-01";
const THINKING_BINDING_BETA: &str = "thinking-binding-controls-2026-08-01";
const EFFORT_UPDATES_BETA: &str = "mid-conversation-output-config-2026-07-01";
const FIRST_PARTY_HOST: &str = "api.anthropic.com";
const GZIP_MIN_BODY_BYTES: usize = 4096;
const INVALID_JSON_MESSAGES: [&str; 2] = [
    "The request body is not valid JSON",
    "Failed to parse request: could not parse request body as JSON",
];

#[derive(Debug, Error)]
pub enum ClaudeError {
    #[error("Messages HTTP {status}: {body}")]
    Http {
        status: u16,
        body: String,

        retry_after: Option<Duration>,

        should_retry: Option<bool>,
    },
    #[error("Messages transport: {0}")]
    Transport(#[from] HttpError),
    #[error("Messages JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Messages stream error {kind}: {message}")]
    StreamError { kind: String, message: String },
    #[error("Messages protocol: {0}")]
    Protocol(String),
    #[error("Messages stream ended before message_stop")]
    IncompleteStream,
    #[error(transparent)]
    AuthUnavailable(#[from] ClaudeAuthUnavailable),
}

impl ClaudeError {
    #[doc(hidden)]
    pub fn is_throttled(&self) -> bool {
        match self {
            Self::Http { status, .. } => matches!(status, 429 | 529),
            Self::StreamError { kind, .. } => {
                matches!(kind.as_str(), "rate_limit_error" | "overloaded_error")
            }
            _ => false,
        }
    }

    #[doc(hidden)]
    pub fn is_transient(&self) -> bool {
        match self {
            Self::Http {
                status,
                should_retry,
                ..
            } => should_retry.unwrap_or_else(|| retry::retryable_status(*status)),
            Self::Transport(error) => transport::is_transient(error),
            Self::StreamError { kind, .. } => matches!(
                kind.as_str(),
                "overloaded_error" | "api_error" | "rate_limit_error" | "timeout_error"
            ),
            Self::IncompleteStream => true,
            Self::Json(_) | Self::Protocol(_) | Self::AuthUnavailable(_) => false,
        }
    }
}

impl nanocode_agent_types::BackendFailure for ClaudeError {
    fn retryable(&self) -> bool {
        self.is_transient()
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

fn retry_after(headers: &http::header::HeaderMap) -> Option<Duration> {
    if let Some(delay) = retry::retry_after_ms(headers) {
        return Some(delay);
    }
    let value = headers
        .get(http::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim();
    if !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Some(Duration::from_secs(value.parse().unwrap_or(u64::MAX)));
    }
    let deadline = httpdate::parse_http_date(value)
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?;
    let now = nanocode_platform::system_time()
        .duration_since(nanocode_platform::UNIX_EPOCH)
        .ok()?;
    Some(deadline.saturating_sub(now))
}

fn rejects_gzip(status: u16, from_anthropic: bool, body: &str) -> bool {
    matches!(status, 400 | 403 | 415) && (!from_anthropic || (status == 400 && invalid_json(body)))
}

fn invalid_json(body: &str) -> bool {
    let Ok(value) = serde_json::from_str::<Value>(body) else {
        return bad_json(body.trim_end());
    };
    if value["type"] == "error"
        && let Some(message) = value["error"]["message"].as_str()
    {
        return INVALID_JSON_MESSAGES
            .iter()
            .any(|prefix| message.starts_with(prefix));
    }
    value["reason"] == "bad_json" && value["error"].as_str().is_some_and(bad_json)
}

fn bad_json(text: &str) -> bool {
    text.strip_prefix("bad json: ").is_some_and(|detail| {
        detail.starts_with("invalid character ")
            || matches!(
                detail,
                "unexpected EOF" | "unexpected end of JSON input" | "EOF"
            )
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
    System,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Image {
        source: Value,
        #[serde(flatten)]
        extra: BTreeMap<String, Value>,
    },
    Document {
        source: Value,
        #[serde(flatten)]
        extra: BTreeMap<String, Value>,
    },
    Text {
        text: String,
        #[serde(flatten)]
        extra: BTreeMap<String, Value>,
    },
    ToolUse {
        id: String,
        name: String,
        input: Value,
        #[serde(flatten)]
        extra: BTreeMap<String, Value>,
    },
    ServerToolUse {
        id: String,
        name: String,
        #[serde(default = "empty_object")]
        input: Value,
        #[serde(flatten)]
        extra: BTreeMap<String, Value>,
    },
    WebSearchToolResult {
        tool_use_id: String,
        content: Value,
        #[serde(flatten)]
        extra: BTreeMap<String, Value>,
    },
    WebFetchToolResult {
        tool_use_id: String,
        content: Value,
        #[serde(flatten)]
        extra: BTreeMap<String, Value>,
    },
    ToolSearchToolResult {
        tool_use_id: String,
        content: Value,
        #[serde(flatten)]
        extra: BTreeMap<String, Value>,
    },
    CodeExecutionToolResult {
        tool_use_id: String,
        content: Value,
        #[serde(flatten)]
        extra: BTreeMap<String, Value>,
    },
    BashCodeExecutionToolResult {
        tool_use_id: String,
        content: Value,
        #[serde(flatten)]
        extra: BTreeMap<String, Value>,
    },
    TextEditorCodeExecutionToolResult {
        tool_use_id: String,
        content: Value,
        #[serde(flatten)]
        extra: BTreeMap<String, Value>,
    },
    McpToolUse {
        id: String,
        name: String,
        server_name: String,
        #[serde(default = "empty_object")]
        input: Value,
        #[serde(flatten)]
        extra: BTreeMap<String, Value>,
    },
    McpToolResult {
        tool_use_id: String,
        content: Value,
        #[serde(flatten)]
        extra: BTreeMap<String, Value>,
    },
    McpToolListing {
        mcp_server_name: String,
        tools: Value,
        #[serde(flatten)]
        extra: BTreeMap<String, Value>,
    },
    ToolResult {
        tool_use_id: String,
        content: ToolResultContent,
        #[serde(default, skip_serializing_if = "is_false")]
        is_error: bool,
        #[serde(flatten)]
        extra: BTreeMap<String, Value>,
    },
    Thinking {
        thinking: String,
        #[serde(default)]
        signature: String,
        #[serde(flatten)]
        extra: BTreeMap<String, Value>,
    },
    RedactedThinking {
        data: String,
        #[serde(flatten)]
        extra: BTreeMap<String, Value>,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ToolResultContent {
    Text(String),
    Blocks(Vec<Value>),
}

fn empty_object() -> Value {
    serde_json::json!({})
}

fn is_false(value: &bool) -> bool {
    !value
}

impl ContentBlock {
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text {
            text: text.into(),
            extra: BTreeMap::new(),
        }
    }

    pub fn tool_use(id: impl Into<String>, name: impl Into<String>, input: Value) -> Self {
        Self::ToolUse {
            id: id.into(),
            name: name.into(),
            input,
            extra: BTreeMap::new(),
        }
    }

    pub fn tool_result(id: impl Into<String>, content: impl Into<String>, is_error: bool) -> Self {
        Self::ToolResult {
            tool_use_id: id.into(),
            content: ToolResultContent::Text(content.into()),
            is_error,
            extra: BTreeMap::new(),
        }
    }

    pub fn tool_result_content(
        id: impl Into<String>,
        content: ToolResultContent,
        is_error: bool,
    ) -> Self {
        Self::ToolResult {
            tool_use_id: id.into(),
            content,
            is_error,
            extra: BTreeMap::new(),
        }
    }

    pub fn tool_result_blocks(id: impl Into<String>, blocks: Vec<Value>, is_error: bool) -> Self {
        Self::ToolResult {
            tool_use_id: id.into(),
            content: ToolResultContent::Blocks(blocks),
            is_error,
            extra: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: Vec<ContentBlock>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_config: Option<OutputConfig>,
}

impl Message {
    pub fn text(role: Role, text: impl Into<String>) -> Self {
        Self {
            role,
            content: vec![ContentBlock::text(text)],
            output_config: None,
        }
    }

    pub const fn tool_results(content: Vec<ContentBlock>) -> Self {
        Self {
            role: Role::User,
            content,
            output_config: None,
        }
    }

    pub const fn effort(effort: Effort) -> Self {
        Self {
            role: Role::System,
            content: Vec::new(),
            output_config: Some(OutputConfig { effort }),
        }
    }

    pub const fn is_effort_update(&self) -> bool {
        matches!(self.role, Role::System) && self.output_config.is_some()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: std::sync::Arc<str>,
    pub input_schema: std::sync::Arc<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub strict: Option<bool>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub defer_loading: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<CacheControl>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ServerToolDefinition {
    #[serde(rename = "type")]
    pub kind: String,
    pub name: String,
    #[serde(flatten)]
    pub options: BTreeMap<String, Value>,
}
impl ServerToolDefinition {
    pub fn tool_search_bm25() -> Self {
        Self {
            kind: "tool_search_tool_bm25_20251119".into(),
            name: "tool_search_tool_bm25".into(),
            options: BTreeMap::new(),
        }
    }
    pub fn tool_search_regex() -> Self {
        Self {
            kind: "tool_search_tool_regex_20251119".into(),
            name: "tool_search_tool_regex".into(),
            options: BTreeMap::new(),
        }
    }
    pub fn web_search_basic(max_uses: u32) -> Self {
        Self {
            kind: "web_search_20250305".into(),
            name: "web_search".into(),
            options: BTreeMap::from([("max_uses".into(), Value::from(max_uses))]),
        }
    }

    pub fn web_search_current(max_uses: u32) -> Self {
        Self {
            kind: "web_search_20260318".into(),
            name: "web_search".into(),
            options: BTreeMap::from([("max_uses".into(), Value::from(max_uses))]),
        }
    }

    pub fn web_fetch_current(max_uses: u32) -> Self {
        Self {
            kind: "web_fetch_20260318".into(),
            name: "web_fetch".into(),
            options: BTreeMap::from([("max_uses".into(), Value::from(max_uses))]),
        }
    }

    pub fn code_execution_current() -> Self {
        Self {
            kind: "code_execution_20260521".into(),
            name: "code_execution".into(),
            options: BTreeMap::new(),
        }
    }
    pub fn web_fetch_basic(max_uses: u32) -> Self {
        Self {
            kind: "web_fetch_20250910".into(),
            name: "web_fetch".into(),
            options: BTreeMap::from([("max_uses".into(), Value::from(max_uses))]),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ClaudeToolSpec {
    Client(ToolDefinition),
    Server(ServerToolDefinition),
}
impl From<ToolDefinition> for ClaudeToolSpec {
    fn from(value: ToolDefinition) -> Self {
        Self::Client(value)
    }
}
impl From<ServerToolDefinition> for ClaudeToolSpec {
    fn from(value: ServerToolDefinition) -> Self {
        Self::Server(value)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CacheControl {
    #[serde(rename = "type")]
    pub kind: CacheType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ttl: Option<CacheTtl>,
}
impl CacheControl {
    pub const fn ephemeral() -> Self {
        Self {
            kind: CacheType::Ephemeral,
            ttl: None,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheType {
    Ephemeral,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CacheTtl {
    #[serde(rename = "5m")]
    FiveMinutes,
    #[serde(rename = "1h")]
    OneHour,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Effort {
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutputConfig {
    pub effort: Effort,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Speed {
    Standard,
    Fast,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MessagesRequest {
    pub model: String,
    pub max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<CacheControl>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_config: Option<OutputConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speed: Option<Speed>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_management: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnostics: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<std::sync::Arc<Value>>,
    pub messages: Vec<Message>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub container: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ClaudeToolSpec>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub effort_updates: bool,
}

impl CacheControl {
    const fn effective_ttl(&self) -> CacheTtl {
        match self.ttl {
            Some(ttl) => ttl,
            None => CacheTtl::FiveMinutes,
        }
    }
}

const CACHE_BREAKPOINTS: usize = 4;

#[derive(Default)]
struct CacheLayout {
    explicit: Vec<CacheTtl>,

    last_eligible: Option<Option<CacheTtl>>,
}

impl CacheLayout {
    fn block(&mut self, control: Option<&Value>, eligible: bool) -> Result<(), ClaudeError> {
        let ttl = control
            .map(|value| {
                if !eligible {
                    return Err(ClaudeError::Protocol(
                        "cache_control cannot target thinking or empty text blocks".into(),
                    ));
                }
                serde_json::from_value::<CacheControl>(value.clone())
                    .map(|control| control.effective_ttl())
                    .map_err(|_| ClaudeError::Protocol("invalid cache_control type or TTL".into()))
            })
            .transpose()?;
        if let Some(ttl) = ttl {
            self.explicit.push(ttl);
        }
        if eligible {
            self.last_eligible = Some(ttl);
        }
        Ok(())
    }

    fn validate(mut self, automatic: Option<&CacheControl>) -> Result<usize, ClaudeError> {
        if let (Some(automatic), Some(last)) = (automatic, self.last_eligible) {
            let ttl = automatic.effective_ttl();
            if let Some(explicit) = last {
                if explicit != ttl {
                    return Err(ClaudeError::Protocol(
                        "automatic cache TTL conflicts with the final cache breakpoint".into(),
                    ));
                }
            } else {
                self.explicit.push(ttl);
            }
        }
        let breakpoints = self.explicit.len();
        if breakpoints > CACHE_BREAKPOINTS {
            return Err(ClaudeError::Protocol(
                "at most four cache breakpoints are allowed".into(),
            ));
        }
        let mut short_ttl = false;
        for ttl in self.explicit {
            match ttl {
                CacheTtl::FiveMinutes => short_ttl = true,
                CacheTtl::OneHour if short_ttl => {
                    return Err(ClaudeError::Protocol(
                        "1h cache breakpoints must precede 5m cache breakpoints".into(),
                    ));
                }
                CacheTtl::OneHour => {}
            }
        }
        Ok(breakpoints)
    }
}

#[doc(hidden)]
pub trait WireMessage {
    fn message(&self) -> &Message;

    fn encoded<'a>(
        &'a self,
        _subscription: bool,
        encode: &dyn Fn(&Message) -> Result<String, ClaudeError>,
    ) -> Result<std::borrow::Cow<'a, str>, ClaudeError> {
        encode(self.message()).map(std::borrow::Cow::Owned)
    }

    fn digests(
        &self,
        encode: &dyn Fn(&Message) -> Result<String, ClaudeError>,
    ) -> Result<MessageDigests, ClaudeError> {
        Ok(MessageDigests::of(&self.encoded(false, encode)?))
    }
}

#[doc(hidden)]
#[derive(Clone, Copy, Debug)]
pub struct MessageDigests {
    sent: blake3::Hash,
    read: blake3::Hash,
}

impl MessageDigests {
    #[must_use]
    pub fn of(json: &str) -> Self {
        let sent = blake3::hash(json.as_bytes());
        let mut read = blake3::Hasher::new();
        let mut rest = json;
        let mut marked = false;
        while let Some((kept, after)) = cache_marker(rest) {
            read.update(kept.as_bytes());
            rest = after;
            marked = true;
        }
        let read = if marked {
            read.update(rest.as_bytes());
            read.finalize()
        } else {
            sent
        };
        Self { sent, read }
    }
}

impl WireMessage for Message {
    fn message(&self) -> &Message {
        self
    }
}

impl<T: WireMessage + ?Sized> WireMessage for &T {
    fn message(&self) -> &Message {
        (**self).message()
    }

    fn encoded<'a>(
        &'a self,
        subscription: bool,
        encode: &dyn Fn(&Message) -> Result<String, ClaudeError>,
    ) -> Result<std::borrow::Cow<'a, str>, ClaudeError> {
        (**self).encoded(subscription, encode)
    }

    fn digests(
        &self,
        encode: &dyn Fn(&Message) -> Result<String, ClaudeError>,
    ) -> Result<MessageDigests, ClaudeError> {
        (**self).digests(encode)
    }
}

impl WireMessage for std::borrow::Cow<'_, Message> {
    fn message(&self) -> &Message {
        self
    }
}

fn validate_cache_layout<'a>(
    tools: &[ClaudeToolSpec],
    system: Option<&Value>,
    messages: impl Iterator<Item = &'a Message>,
    automatic: Option<&CacheControl>,
) -> Result<usize, ClaudeError> {
    let mut layout = CacheLayout::default();
    for tool in tools {
        let client = match tool {
            ClaudeToolSpec::Client(tool) => tool
                .cache_control
                .as_ref()
                .map(serde_json::to_value)
                .transpose()?,
            ClaudeToolSpec::Server(_) => None,
        };
        let control = match tool {
            ClaudeToolSpec::Server(tool) => tool.options.get("cache_control"),
            ClaudeToolSpec::Client(_) => client.as_ref(),
        };
        layout.block(control, true)?;
    }
    match system {
        Some(Value::String(text)) => layout.block(None, !text.is_empty())?,
        Some(Value::Array(blocks)) => {
            for block in blocks {
                layout.block(block.get("cache_control"), cacheable_json_block(block))?;
            }
        }
        _ => {}
    }
    for message in messages {
        for block in &message.content {
            let (control, eligible) = block.cache_slot();
            layout.block(control, eligible)?;
        }
    }
    layout.validate(automatic)
}

pub fn fingerprint(value: &impl Serialize) -> String {
    fingerprint_bytes(&serde_json::to_vec(value).unwrap_or_default())
}

#[doc(hidden)]
pub fn fingerprint_bytes(bytes: &[u8]) -> String {
    format!("{:016x}", xxhash_rust::xxh64::xxh64(bytes, 0))
}

#[doc(hidden)]
pub fn cache_tail<M: WireMessage>(messages: &[M]) -> Option<(usize, usize)> {
    messages
        .iter()
        .enumerate()
        .rev()
        .find_map(|(index, message)| {
            message
                .message()
                .content
                .iter()
                .rposition(|block| block.cache_slot().1)
                .map(|block| (index, block))
        })
}

fn mark_system_tail(system: Option<&mut Value>, marker: Value) {
    if let Some(Value::Array(blocks)) = system
        && let Some(block) = blocks
            .iter_mut()
            .rev()
            .find(|block| cacheable_json_block(block))
        && let Some(block) = block.as_object_mut()
    {
        block.entry("cache_control").or_insert(marker);
    }
}

impl MessagesRequest {
    fn head(&self) -> Self {
        let Self {
            model,
            max_tokens,
            cache_control,
            output_config,
            speed,
            tool_choice,
            thinking,
            context_management,
            diagnostics,
            system,
            messages: _,
            container,
            tools,
            effort_updates,
        } = self;
        Self {
            model: model.clone(),
            max_tokens: *max_tokens,
            cache_control: cache_control.clone(),
            output_config: output_config.clone(),
            speed: *speed,
            tool_choice: tool_choice.clone(),
            thinking: thinking.clone(),
            context_management: context_management.clone(),
            diagnostics: diagnostics.clone(),
            system: system.clone(),
            messages: Vec::new(),
            container: container.clone(),
            tools: tools.clone(),
            effort_updates: *effort_updates,
        }
    }

    pub fn apply_cache_ttl(&mut self) {
        let Some(ttl) = self.cache_control.as_ref().and_then(|control| control.ttl) else {
            return;
        };
        let Some(Value::Array(blocks)) = self.system.as_deref() else {
            return;
        };
        if blocks
            .iter()
            .filter_map(|block| block.get("cache_control"))
            .filter_map(Value::as_object)
            .all(|control| control.contains_key("ttl"))
        {
            return;
        }
        let Some(Value::Array(blocks)) = self.system.as_mut().map(std::sync::Arc::make_mut) else {
            return;
        };
        for control in blocks
            .iter_mut()
            .filter_map(|block| block.get_mut("cache_control"))
            .filter_map(Value::as_object_mut)
        {
            control
                .entry("ttl")
                .or_insert_with(|| serde_json::to_value(ttl).expect("cache TTL serializes"));
        }
    }

    pub fn validate_cache_control(&self) -> Result<(), ClaudeError> {
        self.validate_cache_with(&self.messages)
    }

    fn validate_cache_with<M: WireMessage>(&self, messages: &[M]) -> Result<(), ClaudeError> {
        self.cache_breakpoints_with(messages).map(drop)
    }

    fn cache_breakpoints_with<M: WireMessage>(&self, messages: &[M]) -> Result<usize, ClaudeError> {
        validate_cache_layout(
            &self.tools,
            self.system.as_deref(),
            messages.iter().map(WireMessage::message),
            self.cache_control.as_ref(),
        )
    }

    pub fn cache_system_prefix(&mut self) -> Result<(), ClaudeError> {
        let messages = std::mem::take(&mut self.messages);
        let result = self.cache_system_prefix_with(&messages);
        self.messages = messages;
        result
    }

    #[doc(hidden)]
    pub fn cache_system_prefix_with<M: WireMessage>(
        &mut self,
        messages: &[M],
    ) -> Result<(), ClaudeError> {
        self.cache_system_tail_with(messages)?;
        self.cache_tools_tail_with(messages)
    }

    fn cache_tools_tail_with<M: WireMessage>(&mut self, messages: &[M]) -> Result<(), ClaudeError> {
        let Some(control) = self.cache_control.clone() else {
            return Ok(());
        };
        if self.tools.iter().any(|tool| match tool {
            ClaudeToolSpec::Client(tool) => tool.cache_control.is_some(),
            ClaudeToolSpec::Server(tool) => tool.options.contains_key("cache_control"),
        }) {
            return Ok(());
        }
        let Some(at) = self
            .tools
            .iter()
            .rposition(|tool| !matches!(tool, ClaudeToolSpec::Client(tool) if tool.defer_loading))
        else {
            return Ok(());
        };
        let marker = serde_json::to_value(&control)?;
        match &mut self.tools[at] {
            ClaudeToolSpec::Client(tool) => tool.cache_control = Some(control),
            ClaudeToolSpec::Server(tool) => {
                tool.options.insert("cache_control".to_owned(), marker);
            }
        }
        if !self
            .cache_breakpoints_with(messages)
            .is_ok_and(|count| count <= 3)
        {
            match &mut self.tools[at] {
                ClaudeToolSpec::Client(tool) => tool.cache_control = None,
                ClaudeToolSpec::Server(tool) => {
                    tool.options.remove("cache_control");
                }
            }
        }
        Ok(())
    }

    fn cache_system_tail_with<M: WireMessage>(
        &mut self,
        messages: &[M],
    ) -> Result<(), ClaudeError> {
        self.validate_cache_with(messages)?;
        let Some(control) = self.cache_control.as_ref() else {
            return Ok(());
        };
        let mut blocks = match self.system.as_deref() {
            Some(Value::String(text)) if !text.is_empty() => {
                vec![serde_json::json!({"type":"text","text":text})]
            }
            Some(Value::Array(blocks)) => blocks.clone(),
            _ => return Ok(()),
        };
        if blocks
            .iter()
            .any(|block| block.get("cache_control").is_some())
        {
            return Ok(());
        }
        let Some(block) = blocks.iter_mut().rev().find(|block| {
            block.get("type").and_then(Value::as_str) == Some("text")
                && block
                    .get("text")
                    .and_then(Value::as_str)
                    .is_some_and(|text| !text.is_empty())
        }) else {
            return Ok(());
        };
        block["cache_control"] = serde_json::to_value(control)?;
        let previous = self
            .system
            .replace(std::sync::Arc::new(Value::Array(blocks)));
        if self.validate_cache_with(messages).is_err() {
            self.system = previous;
        }
        Ok(())
    }

    #[doc(hidden)]
    pub fn cache_floors_with<M: WireMessage>(
        &mut self,
        messages: &mut [M],
        floors: [Option<usize>; 2],
        owned: impl Fn(&mut M) -> &mut Message,
    ) -> Result<(), ClaudeError> {
        let Some(control) = self.cache_control.clone() else {
            return Ok(());
        };
        let mut blocks = Vec::new();
        for floor in floors.into_iter().flatten() {
            if let Some(at) = cache_tail(&messages[..floor])
                && !blocks.contains(&at)
                && messages[at.0].message().content[at.1]
                    .cache_slot()
                    .0
                    .is_none()
            {
                blocks.push(at);
            }
        }
        let mut spare = CACHE_BREAKPOINTS.saturating_sub(self.cache_breakpoints_with(messages)?);
        if spare > 0 && blocks.len() > spare && self.release_system_marker() {
            spare += 1;
        }
        for (message, block) in blocks.into_iter().take(spare) {
            owned(&mut messages[message]).content[block].set_cache_control(&control)?;
        }
        Ok(())
    }

    fn release_system_marker(&mut self) -> bool {
        let marked = matches!(
            self.system.as_deref(),
            Some(Value::Array(blocks)) if blocks.iter().any(|block| block.get("cache_control").is_some())
        );
        if let Some(Value::Array(blocks)) = self
            .system
            .as_mut()
            .filter(|_| marked)
            .map(std::sync::Arc::make_mut)
            && let Some(block) = blocks
                .iter_mut()
                .rev()
                .filter_map(Value::as_object_mut)
                .find(|block| block.contains_key("cache_control"))
        {
            block.remove("cache_control");
        }
        marked
    }

    pub fn explicit_cache_tail(&mut self) -> Result<(), ClaudeError> {
        self.validate_cache_control()?;
        let Some(control) = self.cache_control.take() else {
            return Ok(());
        };
        let marker = serde_json::to_value(&control)?;
        match cache_tail(&self.messages) {
            Some((index, block)) => {
                self.messages[index].content[block]
                    .extra_mut()
                    .entry("cache_control".to_owned())
                    .or_insert(marker);
            }
            None => mark_system_tail(self.system.as_mut().map(std::sync::Arc::make_mut), marker),
        }
        Ok(())
    }
}

impl ContentBlock {
    #[doc(hidden)]
    pub fn set_cache_control(&mut self, control: &CacheControl) -> Result<(), ClaudeError> {
        self.extra_mut()
            .insert("cache_control".to_owned(), serde_json::to_value(control)?);
        Ok(())
    }

    fn cache_slot(&self) -> (Option<&Value>, bool) {
        match self {
            Self::Text { text, extra } => (extra.get("cache_control"), !text.is_empty()),
            Self::Thinking { extra, .. } | Self::RedactedThinking { extra, .. } => {
                (extra.get("cache_control"), false)
            }
            Self::Image { extra, .. }
            | Self::Document { extra, .. }
            | Self::ToolResult { extra, .. }
            | Self::ToolUse { extra, .. }
            | Self::ServerToolUse { extra, .. }
            | Self::WebSearchToolResult { extra, .. }
            | Self::WebFetchToolResult { extra, .. }
            | Self::ToolSearchToolResult { extra, .. }
            | Self::CodeExecutionToolResult { extra, .. }
            | Self::BashCodeExecutionToolResult { extra, .. }
            | Self::TextEditorCodeExecutionToolResult { extra, .. }
            | Self::McpToolUse { extra, .. }
            | Self::McpToolResult { extra, .. }
            | Self::McpToolListing { extra, .. } => (extra.get("cache_control"), true),
        }
    }
    const fn extra_mut(&mut self) -> &mut BTreeMap<String, Value> {
        match self {
            Self::Image { extra, .. }
            | Self::Document { extra, .. }
            | Self::Text { extra, .. }
            | Self::ToolUse { extra, .. }
            | Self::ServerToolUse { extra, .. }
            | Self::WebSearchToolResult { extra, .. }
            | Self::WebFetchToolResult { extra, .. }
            | Self::ToolSearchToolResult { extra, .. }
            | Self::CodeExecutionToolResult { extra, .. }
            | Self::BashCodeExecutionToolResult { extra, .. }
            | Self::TextEditorCodeExecutionToolResult { extra, .. }
            | Self::McpToolUse { extra, .. }
            | Self::McpToolResult { extra, .. }
            | Self::McpToolListing { extra, .. }
            | Self::ToolResult { extra, .. }
            | Self::Thinking { extra, .. }
            | Self::RedactedThinking { extra, .. } => extra,
        }
    }
}

fn cacheable_json_block(block: &Value) -> bool {
    match block.get("type").and_then(Value::as_str) {
        Some("thinking" | "redacted_thinking") => false,
        Some("text") => block
            .get("text")
            .and_then(Value::as_str)
            .is_some_and(|text| !text.is_empty()),
        _ => true,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    EndTurn,
    MaxTokens,
    StopSequence,
    ToolUse,
    PauseTurn,
    Refusal,
    ModelContextWindowExceeded,
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub cache_read_input_tokens: u64,
    #[serde(default)]
    pub cache_creation_input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_creation: Option<CacheCreation>,
    #[serde(
        default,
        with = "nanocode_agent_types::usage::usd_number",
        skip_serializing_if = "Option::is_none"
    )]
    pub cost: Option<UsdAmount>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimated_cost: Option<EstimatedUsdCost>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CacheCreation {
    #[serde(default)]
    pub ephemeral_5m_input_tokens: u64,
    #[serde(default)]
    pub ephemeral_1h_input_tokens: u64,
}

impl Usage {
    #[must_use]
    pub const fn tokens(&self, requested_ttl: Option<CacheTtl>) -> ClaudeTokens {
        let (cache_write_5m_input_tokens, cache_write_1h_input_tokens) =
            match (self.cache_creation, requested_ttl) {
                (Some(created), _) => (
                    created.ephemeral_5m_input_tokens,
                    created.ephemeral_1h_input_tokens,
                ),
                (None, Some(CacheTtl::OneHour)) => (0, self.cache_creation_input_tokens),
                (None, _) => (self.cache_creation_input_tokens, 0),
            };
        ClaudeTokens {
            input_tokens: self.input_tokens,
            cache_read_input_tokens: self.cache_read_input_tokens,
            cache_write_5m_input_tokens,
            cache_write_1h_input_tokens,
            output_tokens: self.output_tokens,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MessageResponse {
    pub id: String,
    pub role: Role,
    pub model: String,
    pub content: Vec<ContentBlock>,
    #[serde(default)]
    pub stop_reason: Option<StopReason>,
    pub usage: Usage,
    #[serde(default)]
    pub container: Option<Value>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct UsageDelta {
    pub input_tokens: Option<u64>,
    pub cache_read_input_tokens: Option<u64>,
    pub cache_creation_input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    #[serde(default, with = "nanocode_agent_types::usage::usd_number")]
    pub cost: Option<UsdAmount>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentDelta {
    TextDelta {
        text: String,
    },
    InputJsonDelta {
        partial_json: String,
    },
    ThinkingDelta {
        thinking: String,
    },
    SignatureDelta {
        signature: String,
    },
    CitationsDelta {
        citation: Value,
    },
    #[serde(other)]
    Other,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct ApiError {
    #[serde(rename = "type")]
    pub kind: String,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamEvent {
    MessageStart {
        message: MessageResponse,
    },
    ContentBlockStart {
        index: usize,
        content_block: ContentBlock,
    },
    ContentBlockDelta {
        index: usize,
        delta: ContentDelta,
    },
    ContentBlockStop {
        index: usize,
    },
    MessageDelta {
        delta: MessageChange,
        usage: UsageDelta,
    },
    MessageStop,
    Ping,
    Error {
        error: ApiError,
    },
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
struct DeltaFrame<'a> {
    #[serde(rename = "type")]
    kind: &'a str,
    index: usize,
    #[serde(borrow)]
    delta: DeltaPart<'a>,
}

#[derive(Deserialize)]
struct DeltaPart<'a> {
    #[serde(rename = "type")]
    kind: &'a str,
    text: Option<String>,
    thinking: Option<String>,
    partial_json: Option<String>,
    signature: Option<String>,
}

impl DeltaPart<'_> {
    fn into_delta(self) -> Option<ContentDelta> {
        match self.kind {
            "text_delta" => self.text.map(|text| ContentDelta::TextDelta { text }),
            "thinking_delta" => self
                .thinking
                .map(|thinking| ContentDelta::ThinkingDelta { thinking }),
            "input_json_delta" => self
                .partial_json
                .map(|partial_json| ContentDelta::InputJsonDelta { partial_json }),
            "signature_delta" => self
                .signature
                .map(|signature| ContentDelta::SignatureDelta { signature }),
            _ => None,
        }
    }
}

fn decode_stream_event(data: &str, event_name: Option<&str>) -> Result<StreamEvent, ClaudeError> {
    if event_name == Some("content_block_delta")
        && let Ok(frame) = serde_json::from_str::<DeltaFrame<'_>>(data)
        && frame.kind == "content_block_delta"
        && let Some(delta) = frame.delta.into_delta()
    {
        return Ok(StreamEvent::ContentBlockDelta {
            index: frame.index,
            delta,
        });
    }
    if let Ok(event) = serde_json::from_str::<StreamEvent>(data) {
        let kind = match &event {
            StreamEvent::MessageStart { .. } => Some("message_start"),
            StreamEvent::ContentBlockStart { .. } => Some("content_block_start"),
            StreamEvent::ContentBlockDelta { .. } => Some("content_block_delta"),
            StreamEvent::ContentBlockStop { .. } => Some("content_block_stop"),
            StreamEvent::MessageDelta { .. } => Some("message_delta"),
            StreamEvent::MessageStop => Some("message_stop"),
            StreamEvent::Ping => Some("ping"),
            StreamEvent::Error { .. } => Some("error"),
            StreamEvent::Other => None,
        };
        if let Some(kind) = kind {
            if event_name.is_some_and(|name| name != kind) {
                return Err(ClaudeError::Protocol(
                    "SSE event name disagrees with data type".into(),
                ));
            }
            return Ok(event);
        }
    }
    let value = serde_json::from_str::<Value>(data)?;
    if event_name.is_some_and(|name| value.get("type").and_then(Value::as_str) != Some(name)) {
        return Err(ClaudeError::Protocol(
            "SSE event name disagrees with data type".into(),
        ));
    }
    serde_json::from_value(value).map_err(ClaudeError::from)
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
pub struct MessageChange {
    #[serde(default)]
    pub stop_reason: Option<StopReason>,
    #[serde(default)]
    pub container: Option<Value>,
}

#[cfg(not(target_family = "wasm"))]
pub type ClaudeAuthFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

#[cfg(target_family = "wasm")]
pub type ClaudeAuthFuture<'a, T> = Pin<Box<dyn Future<Output = T> + 'a>>;

pub trait ClaudeAuthProvider: Send + Sync {
    fn headers(
        &self,
    ) -> ClaudeAuthFuture<'_, Result<http::header::HeaderMap, ClaudeAuthUnavailable>>;

    fn recover_unauthorized<'a>(
        &'a self,
        _rejected: &'a http::header::HeaderMap,
    ) -> ClaudeAuthFuture<'a, Result<bool, ClaudeAuthUnavailable>> {
        Box::pin(async { Ok(false) })
    }
}

#[derive(Debug, Error)]
#[error("Claude authentication unavailable: {reason}")]
pub struct ClaudeAuthUnavailable {
    reason: std::borrow::Cow<'static, str>,
}

impl ClaudeAuthUnavailable {
    pub fn new(reason: impl Into<std::borrow::Cow<'static, str>>) -> Self {
        Self {
            reason: reason.into(),
        }
    }

    pub fn reason(&self) -> &str {
        &self.reason
    }
}

#[derive(Clone)]
enum ClientAuth {
    ApiKey(String),
    Headers(http::header::HeaderMap),
    Provider(Arc<dyn ClaudeAuthProvider>),
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[doc(hidden)]
pub struct FrozenWireProfile {
    #[doc(hidden)]
    pub enabled: bool,
    identity: SubscriptionIdentity,
    session: String,

    #[serde(default, skip_serializing_if = "is_false")]
    cache_tail: bool,
}

#[derive(Clone)]
enum HttpConnection {
    Client(HttpClient),
    Transport(Arc<dyn transport::HttpTransport>),
}

#[derive(Clone)]
pub struct ClaudeClient {
    http: HttpConnection,
    endpoint: String,
    auth: ClientAuth,
    subscription_compatibility: bool,
    subscription_identity: SubscriptionIdentity,
    subscription_session: String,

    subscription_cache_tail: bool,
    gzip_rejected: Arc<std::sync::atomic::AtomicBool>,
    http_layer: Option<Arc<dyn transport::Layer>>,
}

impl ClaudeClient {
    pub fn new(http: HttpClient, endpoint: impl Into<String>, api_key: impl Into<String>) -> Self {
        Self {
            http: HttpConnection::Client(http),
            endpoint: endpoint.into(),
            auth: ClientAuth::ApiKey(api_key.into()),
            subscription_compatibility: false,
            subscription_identity: SubscriptionIdentity::default(),
            subscription_session: nanocode_platform::uuid_v4().to_string(),
            subscription_cache_tail: true,
            gzip_rejected: Arc::default(),
            http_layer: None,
        }
    }

    #[cfg(not(target_family = "wasm"))]
    pub fn messages_api(
        base_url: &str,
        api_key: impl Into<String>,
        connect_timeout: Duration,
        idle_timeout: Duration,
    ) -> Result<Self, reqwest::Error> {
        nanocode_tls::install_default_rustls_crypto_provider();
        let http = reqwest::Client::builder()
            .connect_timeout(connect_timeout)
            .read_timeout(idle_timeout)
            .build()?;
        let endpoint = format!("{}/messages", base_url.trim_end_matches('/'));
        Ok(Self::new(http, endpoint, api_key))
    }

    pub fn with_auth_headers(
        http: HttpClient,
        endpoint: impl Into<String>,
        headers: http::header::HeaderMap,
    ) -> Self {
        Self {
            http: HttpConnection::Client(http),
            endpoint: endpoint.into(),
            auth: ClientAuth::Headers(headers),
            subscription_compatibility: false,
            subscription_identity: SubscriptionIdentity::default(),
            subscription_session: nanocode_platform::uuid_v4().to_string(),
            subscription_cache_tail: true,
            gzip_rejected: Arc::default(),
            http_layer: None,
        }
    }

    pub fn with_auth_provider(
        http: HttpClient,
        endpoint: impl Into<String>,
        provider: Arc<dyn ClaudeAuthProvider>,
    ) -> Self {
        Self {
            http: HttpConnection::Client(http),
            endpoint: endpoint.into(),
            auth: ClientAuth::Provider(provider),
            subscription_compatibility: false,
            subscription_identity: SubscriptionIdentity::default(),
            subscription_session: nanocode_platform::uuid_v4().to_string(),
            subscription_cache_tail: true,
            gzip_rejected: Arc::default(),
            http_layer: None,
        }
    }

    #[must_use]
    pub fn with_transport(mut self, transport: impl transport::HttpTransport) -> Self {
        self.http = HttpConnection::Transport(Arc::new(transport));
        self
    }

    #[must_use]
    pub fn with_http_layer(mut self, layer: impl transport::Layer) -> Self {
        self.http_layer = Some(Arc::new(layer));
        self
    }

    pub fn official(http: HttpClient, api_key: impl Into<String>) -> Self {
        Self::new(http, ANTHROPIC_MESSAGES_URL, api_key)
    }

    pub fn subscription(http: HttpClient, provider: Arc<dyn ClaudeAuthProvider>) -> Self {
        Self::with_auth_provider(http, ANTHROPIC_SUBSCRIPTION_MESSAGES_URL, provider)
            .subscription_compatibility()
    }

    #[must_use]
    pub const fn uses_subscription(&self) -> bool {
        self.subscription_compatibility
    }

    #[cfg_attr(target_family = "wasm", allow(clippy::missing_const_for_fn))]
    pub fn subscription_compatibility(mut self) -> Self {
        self.subscription_compatibility = true;
        #[cfg(not(target_family = "wasm"))]
        if let Ok(version) = std::env::var("PI_AI_CLAUDE_CODE_VERSION")
            && !version.is_empty()
        {
            self.subscription_identity.version = Some(version);
        }
        self
    }

    pub fn with_subscription_identity(mut self, identity: SubscriptionIdentity) -> Self {
        self.subscription_identity = identity;
        self
    }
    #[doc(hidden)]
    pub fn bind_subscription_session(mut self, session: &str) -> Self {
        self.subscription_session = session.to_owned();
        self
    }

    #[doc(hidden)]
    pub fn freeze_wire_profile(&self, automatic_cache: bool) -> FrozenWireProfile {
        let mut identity = self.subscription_identity.clone();
        identity.version = Some(identity.version().to_owned());
        identity
            .install_id
            .get_or_insert_with(|| self.subscription_session.clone());
        if identity.platform.is_none() {
            identity.platform = Some(
                if cfg!(target_os = "macos") {
                    "darwin"
                } else if cfg!(target_os = "windows") {
                    "win32"
                } else {
                    "linux"
                }
                .into(),
            );
        }
        if identity.arch.is_none() {
            identity.arch = Some(
                if cfg!(target_arch = "aarch64") {
                    "arm64"
                } else if cfg!(target_arch = "x86") {
                    "ia32"
                } else {
                    "x64"
                }
                .into(),
            );
        }
        FrozenWireProfile {
            enabled: self.subscription_compatibility,
            identity,
            session: self.subscription_session.clone(),

            cache_tail: self.subscription_compatibility
                && self.subscription_cache_tail
                && automatic_cache,
        }
    }
    #[doc(hidden)]
    pub fn restore_wire_profile(&self, profile: Option<&FrozenWireProfile>) -> Self {
        let mut client = self.clone();

        client.subscription_compatibility = profile.is_some_and(|p| p.enabled);
        client.subscription_cache_tail = profile.is_some_and(|p| p.cache_tail);
        if let Some(profile) = profile {
            client.subscription_identity = profile.identity.clone();
            client.subscription_session = profile.session.clone();
        }
        client
    }

    pub fn prepare_request(&self, request: &mut MessagesRequest) {
        if self.subscription_compatibility {
            subscription_wire::prepare(request, &self.subscription_identity);
        }
    }

    pub fn request_body(
        &self,
        request: &MessagesRequest,
        streaming: bool,
    ) -> Result<String, ClaudeError> {
        self.wire_body(request, &request.messages, streaming, None)
            .map(|wire| wire.body)
    }

    fn wire_body<M: WireMessage>(
        &self,
        head: &MessagesRequest,
        messages: &[M],
        streaming: bool,
        cache: Option<&WireCache>,
    ) -> Result<WireBody, ClaudeError> {
        if self.subscription_compatibility {
            let (body, messages) = subscription_wire::wire(
                head,
                messages,
                streaming,
                &self.subscription_identity,
                &self.subscription_session,
                self.subscription_cache_tail,
            )?;
            Ok(WireBody {
                body,
                messages,
                system: None,
                tools: None,
                digests: None,
            })
        } else {
            api_body(head, messages, streaming, None, cache)
        }
    }

    #[doc(hidden)]
    pub fn encode_message(&self, message: &Message) -> Result<String, ClaudeError> {
        if self.subscription_compatibility {
            subscription_wire::encode_message(message)
        } else {
            Ok(serde_json::to_string(message)?)
        }
    }

    #[doc(hidden)]
    pub fn prepare_wire<M: WireMessage>(
        &self,
        head: &MessagesRequest,
        messages: &[M],
        streaming: bool,
    ) -> Result<PreparedWire, ClaudeError> {
        self.prepare_cached_wire(head, messages, streaming, None)
    }

    #[doc(hidden)]
    pub fn prepare_cached_wire<M: WireMessage>(
        &self,
        head: &MessagesRequest,
        messages: &[M],
        streaming: bool,
        cache: Option<&WireCache>,
    ) -> Result<PreparedWire, ClaudeError> {
        if !self.subscription_compatibility {
            head.validate_cache_with(messages)?;
        }
        let wire = self.wire_body(head, messages, streaming, cache)?;
        Ok(PreparedWire {
            body: Arc::new(wire.body),
            messages: wire.messages,
            system: wire.system,
            tools: wire.tools,
            digests: wire.digests,
            thread: None,
            subscription: self.subscription_compatibility,
            streaming,
            agent: !head.tools.is_empty() || head.thinking.is_some(),
            thinking: head.thinking.is_some(),
            thinking_updates: head
                .thinking
                .as_ref()
                .and_then(|thinking| thinking.get("display"))
                .and_then(Value::as_str)
                == Some("updates"),
            thinking_binding: head
                .thinking
                .as_ref()
                .is_some_and(|thinking| thinking.get("block_binding").is_some()),
            context_management: head.context_management.is_some(),
            speed: head.speed.is_some(),
            effort_updates: head.effort_updates,
        })
    }

    #[doc(hidden)]
    pub fn durable_request(
        &self,
        request: &MessagesRequest,
    ) -> Result<Box<serde_json::value::RawValue>, ClaudeError> {
        let wire = self.wire_body(request, &request.messages, true, None)?;
        PreparedWire {
            body: Arc::new(wire.body),
            messages: wire.messages,
            system: wire.system,
            tools: wire.tools,
            digests: wire.digests,
            thread: None,
            subscription: self.subscription_compatibility,
            streaming: true,
            agent: false,
            thinking: false,
            thinking_updates: false,
            thinking_binding: false,
            context_management: false,
            speed: false,
            effort_updates: false,
        }
        .legacy_durable_input()
    }

    pub fn attach_thread<M: WireMessage>(
        &self,
        wire: &mut PreparedWire,
        head: &MessagesRequest,
        messages: &[M],
        turn: &ThreadTurn,
    ) -> Result<(), ClaudeError> {
        let Some(param) = turn.param() else {
            wire.thread = None;
            return Ok(());
        };
        if self.subscription_compatibility {
            return Err(ClaudeError::Protocol(
                "message threads need the Messages API wire, not the subscription wire".into(),
            ));
        }
        wire.thread = Some(
            api_body(
                head,
                turn.new_messages(messages),
                wire.streaming,
                Some(param),
                None,
            )?
            .body
            .into(),
        );
        Ok(())
    }

    const fn decode_block(&self, block: &mut ContentBlock) {
        if self.subscription_compatibility
            && let ContentBlock::ToolUse { name, .. } = block
        {
            subscription_wire::strip(name);
        }
    }

    async fn post(&self, wire: &PreparedWire) -> Result<(HttpResponse, Vec<String>), ClaudeError> {
        let mut retried = false;

        let mut credentials = Vec::<String>::new();
        loop {
            let mut provider_headers = None;
            let mut headers = match &self.auth {
                ClientAuth::ApiKey(key) => {
                    let mut headers = http::header::HeaderMap::new();
                    let mut value = http::header::HeaderValue::from_str(key).map_err(|_| {
                        ClaudeAuthUnavailable::new(
                            "the Claude API key is not a valid HTTP header value",
                        )
                    })?;
                    value.set_sensitive(true);
                    headers.insert("x-api-key", value);
                    headers
                }
                ClientAuth::Headers(headers) => headers.clone(),
                ClientAuth::Provider(provider) => {
                    let headers = provider.headers().await?;
                    provider_headers = Some(headers.clone());
                    headers
                }
            };

            let mut betas = Vec::new();
            for value in headers.get_all("anthropic-beta") {
                for beta in value
                    .to_str()
                    .map_err(|_| {
                        ClaudeAuthUnavailable::new(
                            "the Claude authentication anthropic-beta header is not visible ASCII",
                        )
                    })?
                    .split(',')
                    .map(str::trim)
                    .filter(|beta| !beta.is_empty())
                {
                    if !betas.contains(&beta) {
                        betas.push(beta);
                    }
                }
            }
            if self.subscription_compatibility {
                let agent = wire.agent;
                let defaults: &[&str] = if agent {
                    &[
                        "claude-code-20250219",
                        "oauth-2025-04-20",
                        "interleaved-thinking-2025-05-14",
                        "thinking-token-count-2026-05-13",
                        "context-management-2025-06-27",
                        "prompt-caching-scope-2026-01-05",
                        "mid-conversation-system-2026-04-07",
                    ]
                } else {
                    &[
                        "oauth-2025-04-20",
                        "interleaved-thinking-2025-05-14",
                        "thinking-token-count-2026-05-13",
                        "context-management-2025-06-27",
                        "prompt-caching-scope-2026-01-05",
                        "structured-outputs-2025-12-15",
                    ]
                };
                let extras = std::mem::take(&mut betas);
                betas.extend_from_slice(defaults);
                if agent && wire.thinking && !betas.contains(&"effort-2025-11-24") {
                    betas.push("effort-2025-11-24");
                }
                if agent && !betas.contains(&"fallback-credit-2026-06-01") {
                    betas.push("fallback-credit-2026-06-01");
                }
                for beta in extras {
                    if !betas.contains(&beta) {
                        betas.push(beta);
                    }
                }
            }
            if wire.context_management && !betas.contains(&"context-management-2025-06-27") {
                betas.push("context-management-2025-06-27");
            }

            const THINKING_UPDATES_BETA: &str = "thinking-display-updates-2026-08-18";
            if wire.thinking_updates && !betas.contains(&THINKING_UPDATES_BETA) {
                betas.push(THINKING_UPDATES_BETA);
            }
            if wire.thinking_binding && !betas.contains(&THINKING_BINDING_BETA) {
                betas.push(THINKING_BINDING_BETA);
            }
            if wire.speed && !betas.contains(&FAST_MODE_BETA) {
                betas.push(FAST_MODE_BETA);
            }
            if wire.effort_updates && !betas.contains(&EFFORT_UPDATES_BETA) {
                betas.push(EFFORT_UPDATES_BETA);
            }
            if wire.thread.is_some() && !betas.contains(&MESSAGE_THREADS_BETA) {
                betas.push(MESSAGE_THREADS_BETA);
            }
            if !betas.is_empty() {
                let value =
                    http::header::HeaderValue::from_str(&betas.join(",")).map_err(|_| {
                        ClaudeAuthUnavailable::new("the combined anthropic-beta header is invalid")
                    })?;
                headers.insert("anthropic-beta", value);
            }
            if self.subscription_compatibility {
                let identity = &self.subscription_identity;
                for (name, value) in [
                    (
                        "user-agent",
                        format!("claude-cli/{} (external, cli)", identity.version()),
                    ),
                    ("x-app", "cli".into()),
                    ("x-stainless-lang", "js".into()),
                    ("x-stainless-runtime", "node".into()),
                    ("x-stainless-runtime-version", "v26.3.0".into()),
                    (
                        "x-stainless-package-version",
                        subscription_wire::CLAUDE_CODE_SDK_VERSION.into(),
                    ),
                    ("x-stainless-retry-count", "0".into()),
                    ("x-stainless-timeout", "600".into()),
                    ("x-stainless-os", identity.os()),
                    ("x-stainless-arch", identity.arch()),
                    (
                        "x-claude-code-session-id",
                        self.subscription_session.clone(),
                    ),
                    ("accept", "application/json".into()),
                    ("accept-encoding", "gzip, deflate, br, zstd".into()),
                    ("connection", "keep-alive".into()),
                    ("anthropic-dangerous-direct-browser-access", "true".into()),
                ] {
                    headers.insert(
                        name,
                        http::header::HeaderValue::from_str(&value).map_err(|_| {
                            ClaudeError::Protocol("invalid subscription wire header".into())
                        })?,
                    );
                }
                headers.remove("x-claude-code-request-class");
            }
            for (name, value) in &headers {
                if name == http::header::AUTHORIZATION
                    || name == "x-api-key"
                    || value.is_sensitive()
                {
                    let value = String::from_utf8_lossy(value.as_bytes());
                    if !value.is_empty() && !credentials.iter().any(|item| item == &value) {
                        credentials.push(value.to_string());
                    }
                    if name == http::header::AUTHORIZATION
                        && let Some((_, token)) = value.split_once(' ')
                        && !token.trim().is_empty()
                        && !credentials.iter().any(|item| item == token.trim())
                    {
                        credentials.push(token.trim().to_owned());
                    }
                }
            }
            let body = wire.sent_shared();
            let gzip = self.gzip_request_body(body.len());
            let layer = self.http_layer.as_deref();
            let response = match &self.http {
                HttpConnection::Client(client) => {
                    transport::post(client, &self.endpoint, headers, body, gzip, layer).await?
                }
                HttpConnection::Transport(transport) => {
                    transport::post_with(
                        transport.as_ref(),
                        &self.endpoint,
                        headers,
                        body.to_string(),
                        layer,
                    )
                    .await?
                }
            };

            if response.status() == http::StatusCode::UNAUTHORIZED
                && !retried
                && let (ClientAuth::Provider(provider), Some(headers)) =
                    (&self.auth, provider_headers.as_ref())
                && provider.recover_unauthorized(headers).await?
            {
                retried = true;
                continue;
            }
            if !response.status().is_success() {
                let status = response.status().as_u16();
                let retry_after = retry_after(response.headers());
                let should_retry = retry::should_retry(response.headers());
                let from_anthropic = response
                    .headers()
                    .get("request-id")
                    .and_then(|value| value.to_str().ok())
                    .is_some_and(|id| id.starts_with("req_"))
                    || response.headers().contains_key("cf-ray");
                let mut body = response.text().await?;
                if gzip && rejects_gzip(status, from_anthropic, &body) {
                    self.gzip_rejected
                        .store(true, std::sync::atomic::Ordering::Relaxed);
                    tracing::warn!(
                        target: "nanocode_claude",
                        status,
                        from_anthropic,
                        "Claude Messages rejected a gzip request body; resending it and later requests uncompressed"
                    );
                    continue;
                }
                credentials.sort_unstable_by_key(|value| std::cmp::Reverse(value.len()));
                for credential in &credentials {
                    body = body.replace(credential, "[redacted]");
                }
                return Err(ClaudeError::Http {
                    status,
                    body,
                    retry_after,
                    should_retry,
                });
            }
            credentials.sort_unstable_by_key(|value| std::cmp::Reverse(value.len()));
            return Ok((response, credentials));
        }
    }

    fn gzip_request_body(&self, bytes: usize) -> bool {
        cfg!(not(target_family = "wasm"))
            && matches!(self.http, HttpConnection::Client(_))
            && bytes >= GZIP_MIN_BODY_BYTES
            && !self
                .gzip_rejected
                .load(std::sync::atomic::Ordering::Relaxed)
            && url::Url::parse(&self.endpoint)
                .is_ok_and(|endpoint| endpoint.host_str() == Some(FIRST_PARTY_HOST))
    }

    pub async fn create(&self, request: &MessagesRequest) -> Result<MessageResponse, ClaudeError> {
        let wire = self.prepare_wire(request, &request.messages, false)?;
        let mut response: MessageResponse = self.post(&wire).await?.0.json().await?;
        for block in &mut response.content {
            self.decode_block(block);
        }
        Ok(response)
    }

    pub async fn stream(&self, request: &MessagesRequest) -> Result<ClaudeStream, ClaudeError> {
        let wire = self.prepare_wire(request, &request.messages, true)?;
        self.stream_prepared(&wire).await
    }

    #[doc(hidden)]
    pub async fn stream_prepared(&self, wire: &PreparedWire) -> Result<ClaudeStream, ClaudeError> {
        let (mut response, credentials) = self.post(wire).await?;
        let events = response.take_events();

        let is_sse = response
            .headers()
            .get(http::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.split(';').next())
            .is_some_and(|value| value.trim().eq_ignore_ascii_case("text/event-stream"));
        if !is_sse {
            return Err(ClaudeError::Protocol(
                "expected text/event-stream response".into(),
            ));
        }
        let response = response.bytes_stream();
        let decode_subscription = self.subscription_compatibility;
        Ok(Box::pin(stream::unfold(
            Some(SseState::new(response, credentials, events)),
            move |state| async move {
                let mut state = state?;
                loop {
                    let frame = match state.pop_frame() {
                        Ok(frame) => frame,
                        Err(error) => {
                            return Some((Err(redact_error(error, &state.credentials)), None));
                        }
                    };
                    if let Some(frame) = frame {
                        let handled = match state.handled_event(&frame) {
                            Ok(handled) => handled,
                            Err(error) => {
                                return Some((Err(redact_error(error, &state.credentials)), None));
                            }
                        };
                        if let Some(handled) = handled {
                            if let Err(error) = handled.await {
                                return Some((Err(ClaudeError::Transport(error.into())), None));
                            }
                            continue;
                        }
                        let parsed = state.decode(&frame);
                        let event = match parsed {
                            Ok(StreamEvent::Error { error }) => Err(ClaudeError::StreamError {
                                kind: error.kind,
                                message: error.message,
                            }),
                            other => other,
                        };
                        match event {
                            Ok(StreamEvent::Other | StreamEvent::Ping) => continue,
                            Ok(StreamEvent::MessageStop) => {
                                return Some((Ok(StreamEvent::MessageStop), None));
                            }
                            Err(error) => {
                                return Some((Err(redact_error(error, &state.credentials)), None));
                            }
                            Ok(mut event) => {
                                if decode_subscription {
                                    match &mut event {
                                        StreamEvent::ContentBlockStart {
                                            content_block: ContentBlock::ToolUse { name, .. },
                                            ..
                                        } => subscription_wire::strip(name),
                                        StreamEvent::MessageStart { message } => {
                                            for block in &mut message.content {
                                                if let ContentBlock::ToolUse { name, .. } = block {
                                                    subscription_wire::strip(name);
                                                }
                                            }
                                        }
                                        _ => {}
                                    }
                                }
                                return Some((Ok(event), Some(state)));
                            }
                        }
                    }
                    match state.response.next().await {
                        Some(Ok(chunk)) => {
                            state.push(&chunk);
                        }
                        None => {
                            return Some((Err(ClaudeError::IncompleteStream), None));
                        }
                        Some(Err(error)) => {
                            return Some((Err(ClaudeError::Transport(error)), None));
                        }
                    }
                }
            },
        )))
    }
}

#[doc(hidden)]
#[derive(Clone, Debug)]
pub struct PreparedWire {
    body: Arc<String>,
    messages: std::ops::Range<usize>,
    system: Option<Encoding>,
    tools: Option<Encoding>,
    digests: Option<(blake3::Hash, blake3::Hash)>,
    thread: Option<Arc<String>>,
    subscription: bool,
    streaming: bool,
    agent: bool,
    thinking: bool,
    thinking_updates: bool,
    thinking_binding: bool,
    context_management: bool,
    speed: bool,
    effort_updates: bool,
}

impl PreparedWire {
    pub fn body(&self) -> &str {
        &self.body
    }

    pub fn sent_body(&self) -> &str {
        self.thread.as_deref().unwrap_or(&self.body)
    }

    fn sent_shared(&self) -> Arc<String> {
        Arc::clone(self.thread.as_ref().unwrap_or(&self.body))
    }

    pub fn durable_input(&self) -> Result<Box<serde_json::value::RawValue>, ClaudeError> {
        #[derive(Serialize)]
        struct Digest<'a> {
            wire: &'a str,
            digest: String,
            bytes: usize,
            conversation: String,
            prefix: String,
        }
        let (conversation, sent) = self.digests.unwrap_or_else(|| self.stream_digests());
        let prefix = self.prefix_digest();
        let mut whole = blake3::Hasher::new();
        whole
            .update(prefix.as_bytes())
            .update(conversation.as_bytes())
            .update(sent.as_bytes());
        Ok(serde_json::value::to_raw_value(&Digest {
            wire: self.wire_name(),
            digest: whole.finalize().to_hex().to_string(),
            bytes: self.body.len(),
            conversation: conversation.to_hex().to_string(),
            prefix: prefix.to_hex().to_string(),
        })?)
    }

    fn prefix_digest(&self) -> blake3::Hash {
        let mut cuts = vec![(self.messages.clone(), None)];
        cuts.extend(
            [&self.system, &self.tools]
                .into_iter()
                .flatten()
                .map(|span| (span.range.clone(), Some(span))),
        );
        cuts.sort_by_key(|(range, _)| range.start);
        let bytes = self.body.as_bytes();
        let mut literal = blake3::Hasher::new();
        let mut parts = blake3::Hasher::new();
        let (mut from, mut at) = (0, 0_u64);
        for (range, span) in cuts {
            literal.update(&bytes[from..range.start]);
            at += (range.start - from) as u64;
            if let Some(span) = span {
                parts
                    .update(&at.to_le_bytes())
                    .update(span.digest(&self.body).as_bytes());
            }
            from = range.end;
        }
        literal.update(&bytes[from..]);
        let mut prefix = blake3::Hasher::new();
        prefix
            .update(literal.finalize().as_bytes())
            .update(parts.finalize().as_bytes());
        prefix.finalize()
    }

    pub fn stream_durable_input(&self) -> Result<Box<serde_json::value::RawValue>, ClaudeError> {
        #[derive(Serialize)]
        struct Digest<'a> {
            wire: &'a str,
            digest: String,
            bytes: usize,
            conversation: String,
            prefix: String,
        }
        let (conversation, markers) = self.stream_digests();
        let prefix = self.prefix_digest();
        let mut whole = blake3::Hasher::new();
        whole
            .update(prefix.as_bytes())
            .update(conversation.as_bytes())
            .update(markers.as_bytes());
        Ok(serde_json::value::to_raw_value(&Digest {
            wire: self.wire_name(),
            digest: whole.finalize().to_hex().to_string(),
            bytes: self.body.len(),
            conversation: conversation.to_hex().to_string(),
            prefix: prefix.to_hex().to_string(),
        })?)
    }

    pub fn body_durable_input(&self) -> Result<Box<serde_json::value::RawValue>, ClaudeError> {
        #[derive(Serialize)]
        struct Digest<'a> {
            wire: &'a str,
            blake3: String,
            bytes: usize,
            conversation: String,
            prefix: String,
        }
        let (before, after) = (
            &self.body[..self.messages.start],
            &self.body[self.messages.end..],
        );
        let mut prefix = blake3::Hasher::new();
        prefix.update(before.as_bytes()).update(after.as_bytes());
        Ok(serde_json::value::to_raw_value(&Digest {
            wire: self.wire_name(),
            blake3: blake3::hash(self.body.as_bytes()).to_hex().to_string(),
            bytes: self.body.len(),
            conversation: self.stream_digests().0.to_hex().to_string(),
            prefix: prefix.finalize().to_hex().to_string(),
        })?)
    }

    pub fn blake3_durable_input(&self) -> Result<Box<serde_json::value::RawValue>, ClaudeError> {
        #[derive(Serialize)]
        struct Digest<'a> {
            wire: &'a str,
            blake3: String,
            bytes: usize,
        }
        Ok(serde_json::value::to_raw_value(&Digest {
            wire: self.wire_name(),
            blake3: blake3::hash(self.body.as_bytes()).to_hex().to_string(),
            bytes: self.body.len(),
        })?)
    }

    #[must_use]
    pub fn same_conversation(&self, recorded: &str) -> bool {
        #[derive(Deserialize)]
        struct Digest {
            wire: String,
            conversation: Option<String>,
        }
        serde_json::from_str::<Digest>(recorded).is_ok_and(|digest| {
            digest.wire == self.wire_name()
                && digest.conversation.is_some_and(|recorded| {
                    self.digests
                        .is_some_and(|(conversation, _)| recorded == conversation.to_hex().as_str())
                        || recorded == self.stream_digests().0.to_hex().as_str()
                })
        })
    }

    fn stream_digests(&self) -> (blake3::Hash, blake3::Hash) {
        let mut hasher = blake3::Hasher::new();
        let mut markers = blake3::Hasher::new();
        let mut kept_bytes = 0_u64;
        let mut rest = &self.body[self.messages.clone()];
        while let Some((kept, after)) = cache_marker(rest) {
            hasher.update(kept.as_bytes());
            kept_bytes += kept.len() as u64;
            let marker = &rest[kept.len()..rest.len() - after.len()];
            markers
                .update(&kept_bytes.to_le_bytes())
                .update(&(marker.len() as u64).to_le_bytes())
                .update(marker.as_bytes());
            rest = after;
        }
        hasher.update(rest.as_bytes());
        (hasher.finalize(), markers.finalize())
    }

    pub fn sha256_durable_input(&self) -> Result<Box<serde_json::value::RawValue>, ClaudeError> {
        #[derive(Serialize)]
        struct Digest<'a> {
            wire: &'a str,
            sha256: String,
            bytes: usize,
        }
        Ok(serde_json::value::to_raw_value(&Digest {
            wire: self.wire_name(),
            sha256: subscription_wire::hex_digest(self.body.as_bytes()),
            bytes: self.body.len(),
        })?)
    }

    const fn wire_name(&self) -> &'static str {
        if self.subscription {
            "subscription_wire_v1"
        } else {
            "messages_v1"
        }
    }

    pub fn legacy_durable_input(&self) -> Result<Box<serde_json::value::RawValue>, ClaudeError> {
        if !self.streaming {
            return Err(ClaudeError::Protocol(
                "durable request inputs record streaming bodies".into(),
            ));
        }
        if self.subscription {
            #[derive(Serialize)]
            struct Wire<'a> {
                subscription_wire_v1: &'a str,
            }
            return Ok(serde_json::value::to_raw_value(&Wire {
                subscription_wire_v1: &self.body,
            })?);
        }
        let request = self
            .body
            .strip_suffix(",\"stream\":true}")
            .ok_or_else(|| ClaudeError::Protocol("unexpected Messages request body".into()))?;
        Ok(serde_json::value::RawValue::from_string(format!(
            "{request}}}"
        ))?)
    }
}

struct WireBody {
    body: String,
    messages: std::ops::Range<usize>,
    system: Option<Encoding>,
    tools: Option<Encoding>,
    digests: Option<(blake3::Hash, blake3::Hash)>,
}

#[derive(Clone, Debug)]
struct Encoding {
    range: std::ops::Range<usize>,
    digest: Option<blake3::Hash>,
}

impl Encoding {
    fn digest(&self, body: &str) -> blake3::Hash {
        self.digest
            .unwrap_or_else(|| blake3::hash(&body.as_bytes()[self.range.clone()]))
    }
}

fn api_body<M: WireMessage>(
    head: &MessagesRequest,
    messages: &[M],
    streaming: bool,
    thread: Option<&ThreadParam>,
    cache: Option<&WireCache>,
) -> Result<WireBody, ClaudeError> {
    let continuing = matches!(thread, Some(ThreadParam::Continue { .. }));
    let MessagesRequest {
        model,
        max_tokens,
        cache_control,
        output_config,
        speed,
        tool_choice,
        thinking,
        context_management,
        diagnostics,
        system,
        messages: _,
        container,
        tools,
        effort_updates: _,
    } = head;
    #[derive(Serialize)]
    struct Head<'a> {
        model: &'a str,
        max_tokens: u32,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<&'a CacheControl>,
        #[serde(skip_serializing_if = "Option::is_none")]
        output_config: Option<&'a OutputConfig>,
        #[serde(skip_serializing_if = "Option::is_none")]
        speed: Option<Speed>,
        #[serde(skip_serializing_if = "Option::is_none")]
        tool_choice: Option<&'a Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        thinking: Option<&'a Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        context_management: Option<&'a Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        diagnostics: Option<&'a Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        system: Option<&'a serde_json::value::RawValue>,
    }
    #[derive(Serialize)]
    struct Tail<'a> {
        #[serde(skip_serializing_if = "Option::is_none")]
        container: Option<&'a str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        tools: Option<&'a serde_json::value::RawValue>,
        #[serde(skip_serializing_if = "Option::is_none")]
        thread: Option<&'a ThreadParam>,
        stream: bool,
    }
    let system = system.as_deref().filter(|_| !continuing);
    let tools: &[ClaudeToolSpec] = if continuing { &[] } else { tools };
    let cached_system = match (cache, system) {
        (Some(cache), Some(system)) => Some(cache.system(system)?),
        _ => None,
    };
    let cached_tools = match cache {
        Some(cache) if !tools.is_empty() => Some(cache.tools(tools)?),
        _ => None,
    };
    let owned_system = match (&cached_system, system) {
        (None, Some(system)) => Some(serde_json::value::to_raw_value(system)?),
        _ => None,
    };
    let owned_tools = match &cached_tools {
        None if !tools.is_empty() => Some(serde_json::value::to_raw_value(tools)?),
        _ => None,
    };
    let system_raw = cached_system
        .as_ref()
        .map(|encoded| &*encoded.raw)
        .or(owned_system.as_deref());
    let tools_raw = cached_tools
        .as_ref()
        .map(|encoded| &*encoded.raw)
        .or(owned_tools.as_deref());
    let encode = |message: &Message| serde_json::to_string(message).map_err(ClaudeError::from);
    let encoded = messages
        .iter()
        .map(|message| message.encoded(false, &encode))
        .collect::<Result<Vec<_>, _>>()?;
    let mut read = Vec::with_capacity(messages.len() * blake3::OUT_LEN);
    let mut sent = Vec::with_capacity(messages.len() * blake3::OUT_LEN);
    for message in messages {
        let digests = message.digests(&encode)?;
        read.extend_from_slice(digests.read.as_bytes());
        sent.extend_from_slice(digests.sent.as_bytes());
    }
    let head = serde_json::to_string(&Head {
        model,
        max_tokens: *max_tokens,
        cache_control: cache_control.as_ref(),
        output_config: output_config.as_ref(),
        speed: *speed,
        tool_choice: tool_choice.as_ref(),
        thinking: thinking.as_ref(),
        context_management: context_management.as_ref(),
        diagnostics: diagnostics.as_ref(),
        system: system_raw,
    })?;
    let tail = serde_json::to_string(&Tail {
        container: container.as_deref(),
        tools: tools_raw,
        thread,
        stream: streaming,
    })?;
    let mut body = String::with_capacity(
        head.len()
            + tail.len()
            + "\"messages\":[],".len()
            + encoded
                .iter()
                .map(|message| message.len() + 1)
                .sum::<usize>(),
    );
    body.push_str(&head[..head.len() - 1]);
    body.push_str(",\"messages\":[");
    let start = body.len();
    for (index, message) in encoded.iter().enumerate() {
        if index > 0 {
            body.push(',');
        }
        body.push_str(message);
    }
    let end = body.len();
    body.push_str("],");
    let base = body.len() - 1;
    body.push_str(&tail[1..]);
    let system = system_raw.map(|raw| {
        let at = head.len() - 1 - raw.get().len();
        Encoding {
            range: at..at + raw.get().len(),
            digest: cached_system.as_ref().map(|encoded| encoded.digest),
        }
    });
    let tools = tools_raw.and_then(|raw| {
        const KEY: &str = "\"tools\":";
        let at = base + tail.find(KEY)? + KEY.len();
        Some(Encoding {
            range: at..at + raw.get().len(),
            digest: cached_tools.as_ref().map(|encoded| encoded.digest),
        })
    });
    Ok(WireBody {
        body,
        messages: start..end,
        system,
        tools,
        digests: Some((blake3::hash(&read), blake3::hash(&sent))),
    })
}

fn cache_marker(json: &str) -> Option<(&str, &str)> {
    const KEY: &str = "\"cache_control\":{";
    let mut from = 0;
    loop {
        let at = from + memchr::memmem::find(&json.as_bytes()[from..], KEY.as_bytes())?;
        let value = at + KEY.len();
        let end = value + json[value..].find('}')? + 1;
        match json.as_bytes()[..at].last() {
            Some(b',') => return Some((&json[..at - 1], &json[end..])),
            Some(b'{') => {
                let after = &json[end..];
                return Some((&json[..at], after.strip_prefix(',').unwrap_or(after)));
            }
            _ => from = value,
        }
    }
}

#[cfg(not(target_family = "wasm"))]
pub type ClaudeStream = Pin<Box<dyn Stream<Item = Result<StreamEvent, ClaudeError>> + Send>>;
#[cfg(target_family = "wasm")]
pub type ClaudeStream = Pin<Box<dyn Stream<Item = Result<StreamEvent, ClaudeError>>>>;

#[doc(hidden)]
pub trait ToolFuture: Future + nanocode_platform::MaybeSend {}
impl<T: Future + nanocode_platform::MaybeSend> ToolFuture for T {}

const MAX_SSE_FRAME_BYTES: usize = 32 * 1024 * 1024;

type ResponseBytes = transport::HttpBody;

fn redact_error(error: ClaudeError, credentials: &[String]) -> ClaudeError {
    let scrub = |mut text: String| {
        for credential in credentials {
            text = text.replace(credential, "[redacted]");
        }
        text
    };
    match error {
        ClaudeError::StreamError { kind, message } => ClaudeError::StreamError {
            kind: scrub(kind),
            message: scrub(message),
        },
        ClaudeError::Protocol(message) => ClaudeError::Protocol(scrub(message)),
        ClaudeError::Json(error) => {
            let original = error.to_string();
            let redacted = scrub(original.clone());
            if redacted == original {
                ClaudeError::Json(error)
            } else {
                ClaudeError::Protocol(format!("invalid Messages JSON: {redacted}"))
            }
        }
        other => other,
    }
}

struct SseState {
    credentials: Vec<String>,
    response: ResponseBytes,
    events: Option<Box<dyn transport::layer::EventHook>>,
    bytes: Vec<u8>,
    data: Option<FrameData>,
    frame_bytes: usize,
    frame_start: usize,
    event_name: Option<std::ops::Range<usize>>,
    first_line: bool,
    skip_lf: bool,
    scan_start: usize,
    cursor: usize,
}

enum FrameData {
    Line(std::ops::Range<usize>),
    Joined(String),
}

struct Frame {
    event_name: Option<std::ops::Range<usize>>,
    data: FrameData,
}

impl SseState {
    fn new(
        response: ResponseBytes,
        credentials: Vec<String>,
        events: Option<Box<dyn transport::layer::EventHook>>,
    ) -> Self {
        Self {
            credentials,
            response,
            events,
            bytes: Vec::new(),
            data: None,
            frame_bytes: 0,
            frame_start: 0,
            event_name: None,
            first_line: true,
            skip_lf: false,
            scan_start: 0,
            cursor: 0,
        }
    }

    fn text(&self, range: std::ops::Range<usize>) -> Result<&str, ClaudeError> {
        std::str::from_utf8(&self.bytes[range])
            .map_err(|_| ClaudeError::Protocol("invalid UTF-8 SSE line".into()))
    }

    fn handled_event(
        &mut self,
        frame: &Frame,
    ) -> Result<
        Option<transport::layer::BoxFuture<'static, Result<(), transport::layer::Error>>>,
        ClaudeError,
    > {
        let Some(events) = self.events.as_mut() else {
            return Ok(None);
        };
        let text = |range: std::ops::Range<usize>| {
            std::str::from_utf8(&self.bytes[range])
                .map_err(|_| ClaudeError::Protocol("invalid UTF-8 SSE line".into()))
        };
        let name = frame.event_name.clone().map(text).transpose()?;
        let data = match &frame.data {
            FrameData::Line(range) => text(range.clone())?,
            FrameData::Joined(data) => data,
        };
        Ok(events.event(transport::layer::Event { name, data }))
    }

    fn decode(&self, frame: &Frame) -> Result<StreamEvent, ClaudeError> {
        let event_name = frame
            .event_name
            .clone()
            .map(|range| self.text(range))
            .transpose()?
            .filter(|name| *name != "message");
        match &frame.data {
            FrameData::Line(range) => decode_stream_event(self.text(range.clone())?, event_name),
            FrameData::Joined(data) => decode_stream_event(data, event_name),
        }
    }

    fn pop_frame(&mut self) -> Result<Option<Frame>, ClaudeError> {
        loop {
            if self.skip_lf {
                if self.cursor == self.bytes.len() {
                    return Ok(None);
                }
                if self.bytes[self.cursor] == b'\n' {
                    self.cursor += 1;
                    self.scan_start = self.cursor;
                }
                self.skip_lf = false;
            }
            let delimiter = memchr::memchr2(b'\n', b'\r', &self.bytes[self.scan_start..])
                .map(|offset| self.scan_start + offset);
            let pending = delimiter.map_or(self.bytes.len(), |index| index + 1) - self.cursor;
            if pending + self.frame_bytes > MAX_SSE_FRAME_BYTES {
                return Err(ClaudeError::Protocol("SSE frame exceeds 32 MiB".into()));
            }
            let Some(index) = delimiter else {
                self.scan_start = self.bytes.len();
                return Ok(None);
            };
            self.frame_bytes += index + 1 - self.cursor;
            self.skip_lf = self.bytes[index] == b'\r';
            let start = self.cursor;
            self.cursor = index + 1;
            self.scan_start = self.cursor;

            let mut line_start = start;
            let line = std::str::from_utf8(&self.bytes[start..index])
                .map_err(|_| ClaudeError::Protocol("invalid UTF-8 SSE line".into()))?;
            if self.first_line {
                self.first_line = false;
                if line.starts_with('\u{feff}') {
                    line_start += '\u{feff}'.len_utf8();
                }
            }
            let line = &self.bytes[line_start..index];
            if line.is_empty() {
                self.frame_bytes = 0;
                self.frame_start = self.cursor;
                let event_name = self.event_name.take();
                let Some(data) = self.data.take() else {
                    continue;
                };
                return Ok(Some(Frame { event_name, data }));
            }
            let (field, value) = match memchr::memchr(b':', line) {
                Some(colon) => {
                    let value = line_start + colon + 1;
                    let value = if self.bytes.get(value) == Some(&b' ') && value < index {
                        value + 1
                    } else {
                        value
                    };
                    (&line[..colon], value..index)
                }
                None => (line, index..index),
            };
            match field {
                b"event" => self.event_name = (!value.is_empty()).then_some(value),
                b"data" => {
                    let text = |range: std::ops::Range<usize>| {
                        std::str::from_utf8(&self.bytes[range])
                            .map_err(|_| ClaudeError::Protocol("invalid UTF-8 SSE line".into()))
                    };
                    match &mut self.data {
                        None => self.data = Some(FrameData::Line(value)),
                        Some(FrameData::Line(first)) => {
                            let joined = format!("{}\n{}", text(first.clone())?, text(value)?);
                            self.data = Some(FrameData::Joined(joined));
                        }
                        Some(FrameData::Joined(joined)) => {
                            let value = text(value)?;
                            joined.push('\n');
                            joined.push_str(value);
                        }
                    }
                }
                _ => {}
            }
        }
    }

    fn push(&mut self, chunk: &[u8]) {
        let shift = self.frame_start;
        if shift > 0 {
            self.bytes.copy_within(shift.., 0);
            self.bytes.truncate(self.bytes.len() - shift);
            self.scan_start -= shift;
            self.cursor -= shift;
            self.frame_start = 0;
            let line = match &mut self.data {
                Some(FrameData::Line(range)) => Some(range),
                Some(FrameData::Joined(_)) | None => None,
            };
            for range in [line, self.event_name.as_mut()].into_iter().flatten() {
                *range = range.start - shift..range.end - shift;
            }
        }
        self.bytes.extend_from_slice(chunk);
    }
}

#[derive(Debug)]
enum BlockAccumulator {
    Text {
        text: String,
        extra: BTreeMap<String, Value>,
    },
    ToolUse {
        id: String,
        name: String,
        initial: Value,
        fragments: String,
        extra: BTreeMap<String, Value>,
    },
    Thinking {
        thinking: String,
        signature: String,
        extra: BTreeMap<String, Value>,
    },
    ServerToolUse {
        id: String,
        name: String,
        initial: Value,
        fragments: String,
        extra: BTreeMap<String, Value>,
    },
    McpToolUse {
        id: String,
        name: String,
        server_name: String,
        initial: Value,
        fragments: String,
        extra: BTreeMap<String, Value>,
    },
    Other(ContentBlock),
}

impl BlockAccumulator {
    const fn diagnostic_kind(&self) -> &'static str {
        match self {
            Self::Text { .. } => "text",
            Self::ToolUse { .. } => "tool_use",
            Self::Thinking { .. } => "thinking",
            Self::ServerToolUse { .. } => "server_tool_use",
            Self::McpToolUse { .. } => "mcp_tool_use",
            Self::Other(block) => match block {
                ContentBlock::Image { .. } => "image",
                ContentBlock::Document { .. } => "document",
                ContentBlock::Text { .. } => "text",
                ContentBlock::ToolUse { .. } => "tool_use",
                ContentBlock::ServerToolUse { .. } => "server_tool_use",
                ContentBlock::WebSearchToolResult { .. } => "web_search_tool_result",
                ContentBlock::WebFetchToolResult { .. } => "web_fetch_tool_result",
                ContentBlock::ToolSearchToolResult { .. } => "tool_search_tool_result",
                ContentBlock::CodeExecutionToolResult { .. } => "code_execution_tool_result",
                ContentBlock::BashCodeExecutionToolResult { .. } => {
                    "bash_code_execution_tool_result"
                }
                ContentBlock::TextEditorCodeExecutionToolResult { .. } => {
                    "text_editor_code_execution_tool_result"
                }
                ContentBlock::McpToolUse { .. } => "mcp_tool_use",
                ContentBlock::McpToolResult { .. } => "mcp_tool_result",
                ContentBlock::McpToolListing { .. } => "mcp_tool_listing",
                ContentBlock::ToolResult { .. } => "tool_result",
                ContentBlock::Thinking { .. } => "thinking",
                ContentBlock::RedactedThinking { .. } => "redacted_thinking",
            },
        }
    }

    fn finish(self, truncated: bool) -> Result<ContentBlock, ClaudeError> {
        let block = match self {
            Self::Text { text, extra } => ContentBlock::Text { text, extra },
            Self::ToolUse {
                id,
                name,
                initial,
                fragments,
                extra,
            } => {
                let input = if fragments.is_empty() {
                    Ok(initial)
                } else {
                    serde_json::from_str::<Value>(&fragments)
                };
                let input = match input {
                    Ok(input) if input.is_object() => input,
                    _ if truncated => {
                        return Ok(ContentBlock::text(format!(
                            "Incomplete tool input for {name} ({id}) was not executed because the output token limit was reached. Issue a fresh complete call if needed."
                        )));
                    }
                    Err(error) => return Err(error.into()),
                    _ => {
                        return Err(ClaudeError::Protocol(
                            "tool input must be a JSON object".into(),
                        ));
                    }
                };
                ContentBlock::ToolUse {
                    id,
                    name,
                    input,
                    extra,
                }
            }
            Self::ServerToolUse {
                id,
                name,
                initial,
                fragments,
                extra,
            } => {
                let input = if fragments.is_empty() {
                    initial
                } else {
                    serde_json::from_str(&fragments)?
                };
                if !input.is_object() {
                    return Err(ClaudeError::Protocol(
                        "server tool input must be a JSON object".into(),
                    ));
                }
                ContentBlock::ServerToolUse {
                    id,
                    name,
                    input,
                    extra,
                }
            }
            Self::Thinking {
                thinking,
                signature,
                extra,
            } => ContentBlock::Thinking {
                thinking,
                signature,
                extra,
            },
            Self::McpToolUse {
                id,
                name,
                server_name,
                initial,
                fragments,
                extra,
            } => {
                let input = if fragments.is_empty() {
                    initial
                } else {
                    serde_json::from_str(&fragments)?
                };
                if !input.is_object() {
                    return Err(ClaudeError::Protocol(
                        "MCP tool input must be a JSON object".into(),
                    ));
                }
                ContentBlock::McpToolUse {
                    id,
                    name,
                    server_name,
                    input,
                    extra,
                }
            }
            Self::Other(block) => block,
        };
        Ok(block)
    }
}

pub async fn collect_stream<S>(
    first: StreamEvent,
    mut events: S,
) -> Result<MessageResponse, ClaudeError>
where
    S: Stream<Item = Result<StreamEvent, ClaudeError>> + Unpin,
{
    let mut accumulator = MessageAccumulator::new(first)?;
    while let Some(event) = events.next().await {
        if accumulator.push(event?)? {
            return accumulator.finish();
        }
    }
    Err(ClaudeError::IncompleteStream)
}

pub struct MessageAccumulator {
    message: MessageResponse,
    active: BTreeMap<usize, BlockAccumulator>,
    completed: BTreeMap<usize, BlockAccumulator>,
    stopped: bool,
}

impl MessageAccumulator {
    pub fn new(first: StreamEvent) -> Result<Self, ClaudeError> {
        let StreamEvent::MessageStart { message } = first else {
            return Err(ClaudeError::Protocol("expected message_start".into()));
        };
        Ok(Self {
            message,
            active: BTreeMap::new(),
            completed: BTreeMap::new(),
            stopped: false,
        })
    }

    pub fn push(&mut self, event: StreamEvent) -> Result<bool, ClaudeError> {
        match event {
            StreamEvent::ContentBlockStart {
                index,
                content_block,
            } => {
                if self.active.contains_key(&index) || self.completed.contains_key(&index) {
                    return Err(ClaudeError::Protocol(format!(
                        "duplicate content block {index}"
                    )));
                }
                let block = match content_block {
                    ContentBlock::Text { text, extra } => BlockAccumulator::Text { text, extra },
                    ContentBlock::ToolUse {
                        id,
                        name,
                        input,
                        extra,
                    } => BlockAccumulator::ToolUse {
                        id,
                        name,
                        initial: input,
                        fragments: String::new(),
                        extra,
                    },
                    ContentBlock::ServerToolUse {
                        id,
                        name,
                        input,
                        extra,
                    } => BlockAccumulator::ServerToolUse {
                        id,
                        name,
                        initial: input,
                        fragments: String::new(),
                        extra,
                    },
                    ContentBlock::Thinking {
                        thinking,
                        signature,
                        extra,
                    } => BlockAccumulator::Thinking {
                        thinking,
                        signature,
                        extra,
                    },
                    ContentBlock::McpToolUse {
                        id,
                        name,
                        server_name,
                        input,
                        extra,
                    } => BlockAccumulator::McpToolUse {
                        id,
                        name,
                        server_name,
                        initial: input,
                        fragments: String::new(),
                        extra,
                    },
                    other => BlockAccumulator::Other(other),
                };
                self.active.insert(index, block);
            }
            StreamEvent::ContentBlockDelta { index, delta } => {
                match (self.active.get_mut(&index), delta) {
                    (
                        Some(BlockAccumulator::Text { text, .. }),
                        ContentDelta::TextDelta { text: chunk },
                    ) => text.push_str(&chunk),
                    (
                        Some(BlockAccumulator::Text { extra, .. }),
                        ContentDelta::CitationsDelta { citation },
                    ) => match extra
                        .entry("citations".to_owned())
                        .or_insert_with(|| Value::Array(Vec::new()))
                    {
                        Value::Array(citations) => citations.push(citation),
                        _ => {
                            return Err(ClaudeError::Protocol(
                                "text citations must be an array".into(),
                            ));
                        }
                    },
                    (
                        Some(
                            BlockAccumulator::ToolUse { fragments, .. }
                            | BlockAccumulator::ServerToolUse { fragments, .. }
                            | BlockAccumulator::McpToolUse { fragments, .. },
                        ),
                        ContentDelta::InputJsonDelta { partial_json },
                    ) => fragments.push_str(&partial_json),
                    (
                        Some(BlockAccumulator::Thinking { thinking, .. }),
                        ContentDelta::ThinkingDelta { thinking: chunk },
                    ) => thinking.push_str(&chunk),
                    (
                        Some(BlockAccumulator::Thinking { signature, .. }),
                        ContentDelta::SignatureDelta { signature: chunk },
                    ) => signature.push_str(&chunk),
                    (Some(_), ContentDelta::Other) => {
                        return Err(ClaudeError::Protocol(format!(
                            "unsupported delta for content block {index}"
                        )));
                    }
                    _ => {
                        return Err(ClaudeError::Protocol(format!(
                            "unexpected delta for content block {index}"
                        )));
                    }
                }
            }
            StreamEvent::ContentBlockStop { index } => {
                let block = self.active.remove(&index).ok_or_else(|| {
                    ClaudeError::Protocol(format!("unknown content block {index}"))
                })?;
                self.completed.insert(index, block);
            }
            StreamEvent::MessageDelta { delta, usage } => {
                if let Some(container) = delta.container {
                    self.message.container = Some(container);
                }
                if let Some(reason) = delta.stop_reason {
                    self.message.stop_reason = Some(reason);
                }
                if let Some(input) = usage.input_tokens {
                    self.message.usage.input_tokens = input;
                }
                if let Some(cached) = usage.cache_read_input_tokens {
                    self.message.usage.cache_read_input_tokens = cached;
                }
                if let Some(created) = usage.cache_creation_input_tokens {
                    self.message.usage.cache_creation_input_tokens = created;
                }
                if let Some(output) = usage.output_tokens {
                    self.message.usage.output_tokens = output;
                }
                if let Some(cost) = usage.cost {
                    self.message.usage.cost = Some(cost);
                }
            }
            StreamEvent::MessageStop => {
                self.stopped = true;
                return Ok(true);
            }
            StreamEvent::Error { error } => {
                return Err(ClaudeError::StreamError {
                    kind: error.kind,
                    message: error.message,
                });
            }
            StreamEvent::MessageStart { .. } => {
                return Err(ClaudeError::Protocol("duplicate message_start".into()));
            }
            StreamEvent::Ping | StreamEvent::Other => {}
        }
        Ok(false)
    }

    pub fn finish(self) -> Result<MessageResponse, ClaudeError> {
        if !self.stopped {
            return Err(ClaudeError::IncompleteStream);
        }
        let Self {
            mut message,
            active,
            mut completed,
            ..
        } = self;
        if message.stop_reason.is_none() {
            return Err(ClaudeError::Protocol("missing final stop_reason".into()));
        }
        let truncated = message.stop_reason == Some(StopReason::MaxTokens);
        for (index, block) in active {
            let block = match block {
                BlockAccumulator::Text { .. } if truncated => block,
                BlockAccumulator::ToolUse { id, name, .. } if truncated => {
                    BlockAccumulator::Other(ContentBlock::text(format!(
                        "Incomplete tool input for {name} ({id}) was not executed because the output token limit was reached. Issue a fresh complete call if needed."
                    )))
                }
                _ => {
                    return Err(ClaudeError::Protocol(format!(
                        "message_stop before content_block_stop: stop_reason={}, block_kind={}, block_index={index}",
                        match message.stop_reason {
                            Some(StopReason::EndTurn) => "end_turn",
                            Some(StopReason::MaxTokens) => "max_tokens",
                            Some(StopReason::StopSequence) => "stop_sequence",
                            Some(StopReason::ToolUse) => "tool_use",
                            Some(StopReason::PauseTurn) => "pause_turn",
                            Some(StopReason::Refusal) => "refusal",
                            Some(StopReason::ModelContextWindowExceeded) =>
                                "model_context_window_exceeded",
                            Some(StopReason::Unknown) => "unknown",
                            None => "missing",
                        },
                        block.diagnostic_kind(),
                    )));
                }
            };
            completed.insert(index, block);
        }
        let count = completed.len();
        if completed.keys().copied().ne(0..count) {
            return Err(ClaudeError::Protocol(
                "non-contiguous content block indices".into(),
            ));
        }
        message.content = completed
            .into_values()
            .map(|block| block.finish(truncated))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(message)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct CompactedHistory {
    pub messages: Vec<Message>,
    pub summary: String,
    pub dropped_messages: usize,
}

impl CompactedHistory {
    pub fn system_context(&self, system: &str) -> String {
        if self.summary.is_empty() {
            return system.to_owned();
        }
        if system.is_empty() {
            return format!("Conversation summary:\n{}", self.summary);
        }
        format!("{system}\n\nConversation summary:\n{}", self.summary)
    }
}

pub fn compact_history(
    history: &[Message],
    keep_recent: usize,
    summary: impl Into<String>,
) -> CompactedHistory {
    let mut start = history.len().saturating_sub(keep_recent);
    if start < history.len() && start > 0 {
        while start > 0 && !is_user_turn_start(&history[start]) {
            start -= 1;
        }
    }
    let summary = summary.into();
    let mut messages = history[start..].to_vec();
    if start > 0 || !summary.is_empty() {
        strip_thinking(&mut messages);
    }
    CompactedHistory {
        dropped_messages: history.len() - messages.len(),
        messages,
        summary,
    }
}

#[doc(hidden)]
pub fn strip_thinking(messages: &mut Vec<Message>) {
    messages.retain_mut(|message| {
        message.content.retain(|block| {
            !matches!(
                block,
                ContentBlock::Thinking { .. } | ContentBlock::RedactedThinking { .. }
            )
        });
        !message.content.is_empty()
    });
}

#[doc(hidden)]
pub fn is_user_turn_start(message: &Message) -> bool {
    message.role == Role::User
        && !message
            .content
            .iter()
            .any(|block| matches!(block, ContentBlock::ToolResult { .. }))
}
