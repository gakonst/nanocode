use std::{fmt, ops::Deref};

use serde::{Deserialize, Serialize};

use super::FunctionOutputContent;
use super::{
    AgentMessageContent, ContentItem, FunctionOutputBody, InternalMessageMetadata, ItemStatus,
    JsonValue, LocalShellAction, LocalShellStatus, MessagePhase, MessageRole, ReasoningContent,
    ReasoningSummary, ToolCaller, ToolDefinition, WebSearchAction,
};
use crate::Thinking;

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub struct ConfigurationUpdateReasoning {
    pub effort: Thinking,
}

#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct ResponseItemId(Box<str>);

impl ResponseItemId {
    #[must_use]
    pub fn with_suffix(prefix: &str, suffix: impl fmt::Display) -> Self {
        Self(format!("{prefix}_{suffix}").into_boxed_str())
    }

    #[must_use]
    pub fn from_server(value: impl Into<Box<str>>) -> Self {
        Self(value.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    #[must_use]
    pub fn is_prefixed(&self) -> bool {
        self.split_once('_')
            .is_some_and(|(prefix, suffix)| !prefix.is_empty() && !suffix.is_empty())
    }
}

impl Deref for ResponseItemId {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        self.as_str()
    }
}

impl AsRef<str> for ResponseItemId {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for ResponseItemId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl From<String> for ResponseItemId {
    fn from(value: String) -> Self {
        Self(value.into_boxed_str())
    }
}

impl From<&str> for ResponseItemId {
    fn from(value: &str) -> Self {
        Self(value.into())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResponseItem {
    AdditionalTools {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<ResponseItemId>,
        role: MessageRole,
        tools: Vec<ToolDefinition>,
    },
    Message {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<ResponseItemId>,
        role: MessageRole,
        content: Vec<ContentItem>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        status: Option<ItemStatus>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        phase: Option<MessagePhase>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        internal_chat_message_metadata_passthrough: Option<InternalMessageMetadata>,
    },
    AgentMessage {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<ResponseItemId>,
        author: Box<str>,
        recipient: Box<str>,
        content: Vec<AgentMessageContent>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        internal_chat_message_metadata_passthrough: Option<InternalMessageMetadata>,
    },
    Reasoning {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<ResponseItemId>,
        #[serde(default)]
        summary: Vec<ReasoningSummary>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content: Option<Vec<ReasoningContent>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        encrypted_content: Option<Box<str>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        status: Option<ItemStatus>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        internal_chat_message_metadata_passthrough: Option<InternalMessageMetadata>,
    },
    LocalShellCall {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<ResponseItemId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        call_id: Option<Box<str>>,
        status: LocalShellStatus,
        action: LocalShellAction,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        internal_chat_message_metadata_passthrough: Option<InternalMessageMetadata>,
    },
    FunctionCall {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<ResponseItemId>,
        name: Box<str>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        namespace: Option<Box<str>>,
        arguments: Box<str>,
        #[serde(default, rename = "async", skip_serializing_if = "is_false")]
        asynchronous: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        encrypted_function_args: Option<Vec<Box<str>>>,
        call_id: Box<str>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        caller: Option<ToolCaller>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        status: Option<ItemStatus>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        created_by: Option<Box<str>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        internal_chat_message_metadata_passthrough: Option<InternalMessageMetadata>,
    },
    FunctionCallOutput {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<ResponseItemId>,
        call_id: Box<str>,
        output: FunctionOutputBody,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        caller: Option<ToolCaller>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        status: Option<ItemStatus>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        created_by: Option<Box<str>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        internal_chat_message_metadata_passthrough: Option<InternalMessageMetadata>,
    },
    ToolSearchCall {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<ResponseItemId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        call_id: Option<Box<str>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        status: Option<Box<str>>,
        execution: Box<str>,
        arguments: JsonValue,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        internal_chat_message_metadata_passthrough: Option<InternalMessageMetadata>,
    },
    CustomToolCall {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<ResponseItemId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        status: Option<ItemStatus>,
        call_id: Box<str>,
        name: Box<str>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        namespace: Option<Box<str>>,
        input: Box<str>,
        #[serde(default, rename = "async", skip_serializing_if = "is_false")]
        asynchronous: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        caller: Option<ToolCaller>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        created_by: Option<Box<str>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        internal_chat_message_metadata_passthrough: Option<InternalMessageMetadata>,
    },
    CustomToolCallOutput {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<ResponseItemId>,
        call_id: Box<str>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<Box<str>>,
        output: FunctionOutputBody,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        caller: Option<ToolCaller>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        status: Option<ItemStatus>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        created_by: Option<Box<str>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        internal_chat_message_metadata_passthrough: Option<InternalMessageMetadata>,
    },
    ToolSearchOutput {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<ResponseItemId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        call_id: Option<Box<str>>,
        status: Box<str>,
        execution: Box<str>,
        tools: Vec<JsonValue>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        internal_chat_message_metadata_passthrough: Option<InternalMessageMetadata>,
    },
    WebSearchCall {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<ResponseItemId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        status: Option<Box<str>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        action: Option<WebSearchAction>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        internal_chat_message_metadata_passthrough: Option<InternalMessageMetadata>,
    },
    ImageGenerationCall {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<ResponseItemId>,
        status: Box<str>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        revised_prompt: Option<Box<str>>,
        result: Box<str>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        internal_chat_message_metadata_passthrough: Option<InternalMessageMetadata>,
    },
    #[serde(alias = "compaction_summary")]
    Compaction {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<ResponseItemId>,
        encrypted_content: Box<str>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        created_by: Option<Box<str>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        internal_chat_message_metadata_passthrough: Option<InternalMessageMetadata>,
    },
    CompactionTrigger {},
    ContextCompaction {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<ResponseItemId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        encrypted_content: Option<Box<str>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        internal_chat_message_metadata_passthrough: Option<InternalMessageMetadata>,
    },
    ConfigurationUpdate {
        reasoning: ConfigurationUpdateReasoning,
    },
    #[serde(untagged)]
    Other(JsonValue),
}

const fn is_false(value: &bool) -> bool {
    !*value
}

impl ResponseItem {
    #[doc(hidden)]
    pub fn strip_image_details(&mut self) {
        match self {
            Self::Message { content, .. } => {
                for item in content {
                    if let ContentItem::InputImage { detail, .. }
                    | ContentItem::InputImageFile { detail, .. } = item
                    {
                        *detail = None;
                    }
                }
            }
            Self::FunctionCallOutput { output, .. } | Self::CustomToolCallOutput { output, .. } => {
                if let FunctionOutputBody::Content(content) = output {
                    for item in content {
                        if let FunctionOutputContent::InputImage { detail, .. }
                        | FunctionOutputContent::InputImageFile { detail, .. } = item
                        {
                            *detail = None;
                        }
                    }
                }
            }
            _ => {}
        }
    }

    #[must_use]
    pub const fn additional_tools(tools: Vec<ToolDefinition>) -> Self {
        Self::AdditionalTools {
            id: None,
            role: MessageRole::Developer,
            tools,
        }
    }

    #[must_use]
    pub fn message(role: MessageRole, content: impl IntoIterator<Item = ContentItem>) -> Self {
        Self::Message {
            id: None,
            role,
            content: content.into_iter().collect(),
            status: None,
            phase: None,
            internal_chat_message_metadata_passthrough: None,
        }
    }

    #[must_use]
    pub fn custom_tool_output(
        call_id: String,
        name: Option<String>,
        output: FunctionOutputBody,
    ) -> Self {
        Self::CustomToolCallOutput {
            id: None,
            call_id: call_id.into_boxed_str(),
            name: name.map(String::into_boxed_str),
            output,
            caller: None,
            status: None,
            created_by: None,
            internal_chat_message_metadata_passthrough: None,
        }
    }

    #[must_use]
    pub fn function_call_output(call_id: String, output: FunctionOutputBody) -> Self {
        Self::FunctionCallOutput {
            id: None,
            call_id: call_id.into_boxed_str(),
            output,
            caller: None,
            status: None,
            created_by: None,
            internal_chat_message_metadata_passthrough: None,
        }
    }

    #[must_use]
    pub const fn compaction_trigger() -> Self {
        Self::CompactionTrigger {}
    }

    #[must_use]
    pub const fn configuration_update(effort: Thinking) -> Self {
        Self::ConfigurationUpdate {
            reasoning: ConfigurationUpdateReasoning { effort },
        }
    }

    #[must_use]
    pub const fn is_user_message(&self) -> bool {
        matches!(
            self,
            Self::Message {
                role: MessageRole::User,
                ..
            }
        )
    }

    #[must_use]
    pub const fn id(&self) -> Option<&ResponseItemId> {
        match self {
            Self::AdditionalTools { id, .. }
            | Self::Message { id, .. }
            | Self::AgentMessage { id, .. }
            | Self::Reasoning { id, .. }
            | Self::LocalShellCall { id, .. }
            | Self::FunctionCall { id, .. }
            | Self::FunctionCallOutput { id, .. }
            | Self::ToolSearchCall { id, .. }
            | Self::CustomToolCall { id, .. }
            | Self::CustomToolCallOutput { id, .. }
            | Self::ToolSearchOutput { id, .. }
            | Self::WebSearchCall { id, .. }
            | Self::ImageGenerationCall { id, .. }
            | Self::Compaction { id, .. }
            | Self::ContextCompaction { id, .. } => id.as_ref(),
            Self::CompactionTrigger {} | Self::ConfigurationUpdate { .. } | Self::Other(_) => None,
        }
    }

    pub fn set_id(&mut self, new_id: Option<ResponseItemId>) {
        match self {
            Self::AdditionalTools { id, .. }
            | Self::Message { id, .. }
            | Self::AgentMessage { id, .. }
            | Self::Reasoning { id, .. }
            | Self::LocalShellCall { id, .. }
            | Self::FunctionCall { id, .. }
            | Self::FunctionCallOutput { id, .. }
            | Self::ToolSearchCall { id, .. }
            | Self::CustomToolCall { id, .. }
            | Self::CustomToolCallOutput { id, .. }
            | Self::ToolSearchOutput { id, .. }
            | Self::WebSearchCall { id, .. }
            | Self::ImageGenerationCall { id, .. }
            | Self::Compaction { id, .. }
            | Self::ContextCompaction { id, .. } => *id = new_id,
            Self::CompactionTrigger {} | Self::ConfigurationUpdate { .. } | Self::Other(_) => {}
        }
    }

    #[must_use]
    pub const fn id_prefix(&self) -> Option<&'static str> {
        match self {
            Self::AdditionalTools { .. } => Some("at"),
            Self::Message { .. } => Some("msg"),
            Self::AgentMessage { .. } => Some("amsg"),
            Self::Reasoning { .. } => Some("rs"),
            Self::LocalShellCall { .. } => Some("lsh"),
            Self::FunctionCall { .. } => Some("fc"),
            Self::FunctionCallOutput { .. } => Some("fco"),
            Self::ToolSearchCall { .. } => Some("tsc"),
            Self::CustomToolCall { .. } => Some("ctc"),
            Self::CustomToolCallOutput { .. } => Some("ctco"),
            Self::ToolSearchOutput { .. } => Some("tso"),
            Self::WebSearchCall { .. } => Some("ws"),
            Self::ImageGenerationCall { .. } => Some("ig"),
            Self::Compaction { .. } | Self::ContextCompaction { .. } => Some("cmp"),
            Self::CompactionTrigger {} | Self::ConfigurationUpdate { .. } | Self::Other(_) => None,
        }
    }

    pub fn strip_id(&mut self) {
        self.set_id(None);
    }

    pub fn strip_unbound_id(&mut self) {
        let provider_bound = matches!(
            self,
            Self::Reasoning {
                encrypted_content: Some(_),
                ..
            } | Self::FunctionCall {
                encrypted_function_args: Some(_),
                ..
            } | Self::Compaction { .. }
                | Self::ContextCompaction {
                    encrypted_content: Some(_),
                    ..
                }
        );
        if !provider_bound {
            self.strip_id();
        }
    }
}
