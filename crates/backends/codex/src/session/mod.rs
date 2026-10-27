use std::fmt;

use std::sync::Arc;

mod contract;
pub use contract::{
    Capabilities, ForkPoint, ForkRequest, Lineage, Mutability, Origin, Persistence,
    SessionCheckpoint, SessionInfo, TurnBoundary,
};

use crate::client::Model;

use crate::client::responses::MessageRole;
use crate::client::responses::{ResponseHistory, ResponseItem, Usage};

pub use crate::client::session::SessionId;

use crate::model::run::ModelCheckpoint;

use crate::{NanocodeError, Result};

const SESSION_SNAPSHOT_VERSION: u32 = 1;

mod shared_history {
    use serde::{Deserialize, Deserializer, Serializer};

    use crate::client::responses::{ResponseHistory, ResponseItem};

    pub(super) fn serialize<S: Serializer>(
        history: &ResponseHistory,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(history.iter())
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<ResponseHistory, D::Error> {
        Vec::<ResponseItem>::deserialize(deserializer).map(ResponseHistory::new)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SessionStart {
    New(Origin),

    Resume,

    Restore,
}

impl SessionStart {
    pub(crate) const fn kind(self) -> &'static str {
        match self {
            Self::New(Origin::Root) => "root",
            Self::New(Origin::Fork | Origin::Branch) => "fork",
            Self::New(Origin::SideConversation) => "side_conversation",
            Self::New(Origin::Subagent) => "spawn",
            Self::Resume => "resume",
            Self::Restore => "restore",
            Self::New(_) => "root",
        }
    }
}

pub use nanocode_agent_core::session::{ToolOutcome, TranscriptItem};

#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub(crate) struct ContextSnapshot {
    pub(crate) agents_md: Option<AgentsMdSnapshot>,
    pub(crate) environment: Option<EnvironmentSnapshot>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(tag = "kind", content = "snapshot", rename_all = "snake_case")]
pub(crate) enum ContextBaseline {
    Missing,
    Known(ContextSnapshot),
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub(crate) struct AgentsMdSnapshot {
    pub(crate) directory: String,
    pub(crate) text: String,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub(crate) struct EnvironmentSnapshot {
    pub(crate) cwd: String,
    pub(crate) shell: String,
    pub(crate) current_date: String,
    pub(crate) timezone: String,
}

#[derive(Clone)]

pub(crate) struct CommittedSession {
    lineage_id: Arc<str>,
    selected_model: Model,
    model: ModelCheckpoint,

    retained_snapshot: Option<SessionSnapshot>,

    thinking: crate::Thinking,
    service_tier: crate::ServiceTier,
    reasoning_mode: crate::ReasoningMode,
}

impl CommittedSession {
    pub(crate) const fn new(
        lineage_id: Arc<str>,
        selected_model: Model,
        thinking: crate::Thinking,
        service_tier: crate::ServiceTier,
        reasoning_mode: crate::ReasoningMode,
        model: ModelCheckpoint,
    ) -> Self {
        Self {
            lineage_id,
            selected_model,
            model,
            retained_snapshot: None,
            thinking,
            service_tier,
            reasoning_mode,
        }
    }

    pub(crate) fn with_retained_snapshot(mut self, snapshot: Option<SessionSnapshot>) -> Self {
        self.retained_snapshot = snapshot;
        self
    }

    pub(crate) fn lineage_id(&self) -> &str {
        &self.lineage_id
    }

    pub(crate) const fn model(&self) -> &ModelCheckpoint {
        &self.model
    }

    pub(crate) const fn selected_model(&self) -> Model {
        self.selected_model
    }

    pub(crate) fn snapshot(&self) -> SessionSnapshot {
        if let Some(snapshot) = &self.retained_snapshot {
            let mut snapshot = snapshot.clone();
            snapshot.thinking = Some(self.thinking);
            snapshot.service_tier = Some(self.service_tier);
            snapshot.reasoning_mode = Some(self.reasoning_mode);
            return snapshot;
        }
        SessionSnapshot {
            version: SESSION_SNAPSHOT_VERSION,
            model: self.selected_model.as_str().to_owned(),
            lineage_id: self.lineage_id.to_string(),
            prompt_cache_key: self.model.prompt_cache_key().to_owned(),
            workspace: self.model.workspace().to_owned(),
            base_instructions: None,
            request_prefix: Some(Arc::clone(self.model.shared_request_prefix())),
            canonical_context: self.model.canonical_context().clone(),
            history: self.model.history(),
            client_authored: self.model.client_authored().clone(),
            context_snapshot: Some(self.model.context_baseline().clone()),
            context_usage: Some(self.model.context_usage()),
            reasoning: self.model.reasoning().clone(),
            thinking: Some(self.thinking),
            service_tier: Some(self.service_tier),
            reasoning_mode: Some(self.reasoning_mode),
        }
    }
}

#[derive(Clone, serde::Deserialize, serde::Serialize)]
pub(crate) struct ContextUsage {
    pub(crate) usage: Option<Usage>,
    #[serde(default)]
    pub(crate) server_reasoning_included: bool,
    #[serde(default)]
    pub(crate) is_estimate: bool,
}

#[derive(Clone, serde::Deserialize, serde::Serialize)]
pub struct SessionSnapshot {
    version: u32,
    model: String,
    lineage_id: String,
    prompt_cache_key: String,
    workspace: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    base_instructions: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    request_prefix: Option<Arc<[ResponseItem]>>,
    canonical_context: ResponseItem,
    #[serde(with = "shared_history")]
    history: ResponseHistory,
    #[serde(default, skip_serializing_if = "std::collections::BTreeSet::is_empty")]
    client_authored: std::collections::BTreeSet<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    context_snapshot: Option<ContextBaseline>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    context_usage: Option<ContextUsage>,
    #[serde(default)]
    reasoning: crate::reasoning::ReasoningState,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    thinking: Option<crate::Thinking>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    service_tier: Option<crate::ServiceTier>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    reasoning_mode: Option<crate::ReasoningMode>,
}

#[derive(Clone, serde::Deserialize, serde::Serialize)]
#[serde(transparent)]
pub struct SessionSnapshotHead(SessionSnapshot);

fn content_text(content: &[crate::client::ContentItem]) -> String {
    content
        .iter()
        .map(|item| match item {
            crate::client::ContentItem::InputText { text }
            | crate::client::ContentItem::OutputText { text, .. } => text.to_string(),
            crate::client::ContentItem::InputImage { .. }
            | crate::client::ContentItem::InputImageFile { .. } => "[image]".to_owned(),
            crate::client::ContentItem::InputAudio { .. } => "[audio]".to_owned(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn output_text(output: &crate::client::FunctionOutputBody) -> String {
    match output {
        crate::client::FunctionOutputBody::Text(text) => text.to_string(),
        crate::client::FunctionOutputBody::Content(items) => items
            .iter()
            .map(|item| match item {
                crate::client::FunctionOutputContent::InputText { text } => text.to_string(),
                crate::client::FunctionOutputContent::InputImage { .. }
                | crate::client::FunctionOutputContent::InputImageFile { .. } => {
                    "[image]".to_owned()
                }
                crate::client::FunctionOutputContent::InputAudio { .. } => "[audio]".to_owned(),
                crate::client::FunctionOutputContent::EncryptedContent { .. } => {
                    "[encrypted content]".to_owned()
                }
            })
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

fn harness_context(text: &str) -> bool {
    let text = text.trim_start();
    text.starts_with("<environment_context>") || text.starts_with("# AGENTS.md instructions")
}

impl SessionSnapshot {
    pub(crate) fn transcript(&self) -> Vec<crate::input::PromptMessage> {
        use crate::client::{MessageRole, ResponseItem};
        let mut transcript = nanocode_agent_core::backend::TranscriptBuilder::default();
        let mut tools = std::collections::HashMap::<&str, &str>::new();
        for item in &self.history {
            match item {
                ResponseItem::Message { role, content, .. } => {
                    let text = content_text(content);
                    match role {
                        MessageRole::User if !harness_context(&text) => transcript.user(&text),
                        MessageRole::Assistant => transcript.assistant(&text),
                        MessageRole::User | MessageRole::Developer => {}
                    }
                }
                ResponseItem::FunctionCall {
                    name,
                    arguments,
                    call_id,
                    ..
                } => {
                    tools.insert(call_id, name);
                    transcript.tool_call(name, arguments);
                }
                ResponseItem::CustomToolCall {
                    name,
                    input,
                    call_id,
                    ..
                } => {
                    tools.insert(call_id, name);
                    transcript.tool_call(name, input);
                }
                ResponseItem::FunctionCallOutput {
                    call_id, output, ..
                }
                | ResponseItem::CustomToolCallOutput {
                    call_id, output, ..
                } => transcript.tool_result(
                    tools.get(call_id.as_ref()).copied().unwrap_or(call_id),
                    &output_text(output),
                ),
                ResponseItem::LocalShellCall { action, .. } => transcript.tool_call(
                    "local_shell",
                    &serde_json::to_string(action).unwrap_or_default(),
                ),
                ResponseItem::WebSearchCall { action, .. } => transcript.tool_call(
                    "web_search",
                    &serde_json::to_string(action).unwrap_or_default(),
                ),
                ResponseItem::ImageGenerationCall { revised_prompt, .. } => {
                    transcript.assistant(&format!(
                        "[Generated an image: {}]",
                        revised_prompt.as_deref().unwrap_or_default()
                    ));
                }
                ResponseItem::Compaction { .. } | ResponseItem::ContextCompaction { .. } => {
                    transcript.user(
                        "[Earlier conversation was compacted by the previous model; its summary is not readable here.]",
                    );
                }
                ResponseItem::AdditionalTools { .. }
                | ResponseItem::AgentMessage { .. }
                | ResponseItem::Reasoning { .. }
                | ResponseItem::ToolSearchCall { .. }
                | ResponseItem::ToolSearchOutput { .. }
                | ResponseItem::CompactionTrigger {}
                | ResponseItem::ConfigurationUpdate { .. }
                | ResponseItem::Other(_) => {}
            }
        }
        transcript.finish()
    }
}

impl SessionSnapshotHead {
    #[must_use]
    pub fn with_context(
        mut self,
        history: ResponseHistory,
        prefix: Option<Arc<[ResponseItem]>>,
    ) -> SessionSnapshot {
        self.0.history = history;
        self.0.request_prefix = prefix;
        self.0
    }
}

impl fmt::Debug for SessionSnapshot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SessionSnapshot")
            .field("version", &self.version)
            .field("model", &self.model)
            .field("history_items", &self.history.len())
            .finish_non_exhaustive()
    }
}

impl SessionSnapshot {
    #[must_use]
    pub fn into_context_parts(
        mut self,
    ) -> (
        SessionSnapshotHead,
        ResponseHistory,
        Option<Arc<[ResponseItem]>>,
    ) {
        let history = std::mem::take(&mut self.history);
        let prefix = self.request_prefix.take();
        (SessionSnapshotHead(self), history, prefix)
    }

    #[must_use]
    pub const fn thinking(&self) -> Option<crate::Thinking> {
        self.thinking
    }

    #[must_use]
    pub const fn service_tier(&self) -> Option<crate::ServiceTier> {
        self.service_tier
    }

    #[must_use]
    pub const fn reasoning_mode(&self) -> Option<crate::ReasoningMode> {
        self.reasoning_mode
    }

    #[must_use]
    pub const fn version(&self) -> u32 {
        self.version
    }

    #[must_use]
    pub fn workspace(&self) -> &str {
        &self.workspace
    }

    pub(crate) fn lineage_id(&self) -> &str {
        &self.lineage_id
    }

    pub(crate) fn model(&self) -> Result<Model> {
        self.model.parse::<Model>().map_err(|error| {
            NanocodeError::InvalidCheckpoint(format!("snapshot model is unsupported: {error}"))
        })
    }

    pub(crate) fn into_resume(self) -> Result<SessionResume> {
        if self.version != SESSION_SNAPSHOT_VERSION {
            return Err(NanocodeError::InvalidCheckpoint(format!(
                "unsupported format version {}; expected {SESSION_SNAPSHOT_VERSION}",
                self.version
            )));
        }
        let model = self.model.parse::<Model>().map_err(|error| {
            NanocodeError::InvalidCheckpoint(format!("snapshot model is unsupported: {error}"))
        })?;
        if self.lineage_id.trim().is_empty() {
            return Err(NanocodeError::InvalidCheckpoint(
                "cache lineage must not be empty".to_owned(),
            ));
        }
        if self.prompt_cache_key.trim().is_empty() {
            return Err(NanocodeError::InvalidCheckpoint(
                "prompt cache key must not be empty".to_owned(),
            ));
        }
        if self.workspace.trim().is_empty() {
            return Err(NanocodeError::InvalidCheckpoint(
                "workspace must not be empty".to_owned(),
            ));
        }
        if let Some(request_prefix) = self.request_prefix.as_ref()
            && !matches!(
                &**request_prefix,
                [
                    ResponseItem::AdditionalTools {
                        role: MessageRole::Developer,
                        ..
                    },
                    ResponseItem::Message {
                        role: MessageRole::Developer,
                        ..
                    }
                ]
            )
        {
            return Err(NanocodeError::InvalidCheckpoint(
                "request prefix does not match the supported model contract".to_owned(),
            ));
        }
        let history = self.history.iter().cloned().collect::<Vec<_>>();
        self.reasoning.validate(&history)?;
        let lineage_id = Arc::<str>::from(self.lineage_id);
        let prompt_cache_key = Arc::<str>::from(self.prompt_cache_key);
        let checkpoint = self
            .request_prefix
            .map(|request_prefix| {
                let mut checkpoint = ModelCheckpoint::resume(
                    self.workspace.clone(),
                    Arc::clone(&lineage_id),
                    request_prefix.to_vec(),
                    Arc::clone(&prompt_cache_key),
                    self.canonical_context.clone(),
                    history.clone(),
                    self.client_authored.clone(),
                    None,
                    self.context_snapshot.clone(),
                )?;
                checkpoint.restore_reasoning(self.reasoning.clone());
                if let Some(usage) = self.context_usage.as_ref() {
                    checkpoint.restore_context_usage(usage);
                }
                Ok::<_, NanocodeError>(checkpoint)
            })
            .transpose()?;
        Ok(SessionResume {
            model,
            lineage_id,
            prompt_cache_key,
            workspace: self.workspace,
            canonical_context: self.canonical_context,
            history,
            client_authored: self.client_authored,
            context_baseline: self.context_snapshot,
            reasoning: self.reasoning,
            checkpoint,
        })
    }
}

pub(crate) struct SessionResume {
    pub(crate) model: Model,
    pub(crate) lineage_id: Arc<str>,
    pub(crate) prompt_cache_key: Arc<str>,
    pub(crate) workspace: String,
    pub(crate) canonical_context: ResponseItem,
    pub(crate) history: Vec<ResponseItem>,
    pub(crate) client_authored: std::collections::BTreeSet<String>,
    pub(crate) context_baseline: Option<ContextBaseline>,
    pub(crate) reasoning: crate::reasoning::ReasoningState,
    pub(crate) checkpoint: Option<ModelCheckpoint>,
}
