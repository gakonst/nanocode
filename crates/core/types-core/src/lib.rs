pub mod identity;
pub use identity::BackendId;

use serde::{Deserialize, Serialize};
use std::{fmt, path::PathBuf, str::FromStr};
pub mod events;
pub mod preview;
pub mod pricing;
pub mod responses;
pub mod tools;
pub use responses::{ContentItem, FunctionOutputContent, ResponseEvent, ResponseItem};

pub const MODEL: &str = Model::Astra.as_str();

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum Model {
    Sol,

    Terra,

    Luna,

    #[default]
    Astra,

    #[serde(rename = "glm-5.3")]
    Glm53,

    #[serde(rename = "kimi-k3", alias = "kimi")]
    Kimi,

    #[serde(rename = "mimo-v2.6-pro", alias = "mimo")]
    Mimo,

    #[serde(rename = "gpt-daybreak-blue-latest", alias = "daybreak-blue")]
    DaybreakBlue,

    #[serde(untagged)]
    Custom(CustomModel),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CustomModel(&'static str);

impl CustomModel {
    #[must_use]
    pub fn new(id: &str) -> Self {
        Self(intern(id))
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

#[cfg(not(target_family = "wasm"))]
fn intern(id: &str) -> &'static str {
    static INTERNED: std::sync::OnceLock<
        nanocode_sys::Swap<std::collections::BTreeSet<&'static str>>,
    > = std::sync::OnceLock::new();
    let interned = INTERNED.get_or_init(Default::default);
    if let Some(existing) = interned.load().get(id).copied() {
        return existing;
    }
    let leaked: &'static str = Box::leak(id.to_owned().into_boxed_str());
    let mut interned_id = leaked;
    interned.rcu(|ids| {
        let mut ids = std::collections::BTreeSet::clone(ids);
        match ids.get(id) {
            Some(existing) => interned_id = existing,
            None => {
                ids.insert(leaked);
                interned_id = leaked;
            }
        }
        ids
    });
    interned_id
}

#[cfg(target_family = "wasm")]
fn intern(id: &str) -> &'static str {
    thread_local! {
        static INTERNED: std::cell::RefCell<std::collections::BTreeSet<&'static str>> =
            const { std::cell::RefCell::new(std::collections::BTreeSet::new()) };
    }
    INTERNED.with_borrow_mut(|ids| {
        if let Some(existing) = ids.get(id) {
            return *existing;
        }
        let leaked: &'static str = Box::leak(id.to_owned().into_boxed_str());
        ids.insert(leaked);
        leaked
    })
}

impl Serialize for CustomModel {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.0)
    }
}

impl<'de> Deserialize<'de> for CustomModel {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let id = String::deserialize(deserializer)?;
        if id.trim().is_empty() {
            return Err(serde::de::Error::custom(
                "model identities must not be empty",
            ));
        }
        Ok(Self::new(&id))
    }
}

impl Model {
    pub const ALL: [Self; 4] = [Self::Astra, Self::Sol, Self::Terra, Self::Luna];

    #[must_use]
    pub const fn default_thinking(self) -> Thinking {
        match self {
            Self::Astra
            | Self::Terra
            | Self::DaybreakBlue
            | Self::Glm53
            | Self::Kimi
            | Self::Mimo
            | Self::Custom(_) => Thinking::Low,
            Self::Sol | Self::Luna => Thinking::Medium,
        }
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Sol => "gpt-6.1-sol",
            Self::Terra => "gpt-6.1-terra",
            Self::Luna => "gpt-6-luna",
            Self::Astra => "gpt-6-astra",
            Self::Glm53 => "@cf/zai-org/glm-5.3",
            Self::Kimi => "kimi-k3",
            Self::Mimo => "mimo-v2.6-pro",
            Self::DaybreakBlue => "gpt-daybreak-blue-latest",
            Self::Custom(model) => model.as_str(),
        }
    }

    #[must_use]
    pub const fn is_custom(self) -> bool {
        matches!(self, Self::Custom(_))
    }

    #[must_use]
    pub const fn supports_thinking(self, thinking: Thinking) -> bool {
        match self {
            Self::Kimi => matches!(thinking, Thinking::Low | Thinking::High),
            Self::Glm53 | Self::Mimo => {
                matches!(thinking, Thinking::Low | Thinking::Medium | Thinking::High)
            }
            Self::Terra | Self::Luna | Self::Custom(_) => true,
            Self::Astra | Self::Sol | Self::DaybreakBlue => !matches!(thinking, Thinking::None),
        }
    }

    #[must_use]
    pub const fn supports_reasoning_effort_updates(self) -> bool {
        matches!(
            self,
            Self::Astra | Self::Sol | Self::Terra | Self::DaybreakBlue
        )
    }

    #[must_use]
    pub const fn supports_reasoning_mode(self, mode: ReasoningMode) -> bool {
        !matches!(
            (self, mode),
            (
                Self::Glm53 | Self::Kimi | Self::Mimo | Self::Custom(_),
                ReasoningMode::Pro
            )
        )
    }

    #[must_use]
    pub const fn max_context_window_tokens(self) -> u64 {
        match self {
            Self::Glm53 => 1_310_720,
            Self::Kimi => 1_000_000,
            Self::Mimo => 1_048_576,
            _ => MAX_CONTEXT_WINDOW_TOKENS,
        }
    }

    #[must_use]
    pub const fn context_window_limit(self) -> Option<u64> {
        match self {
            Self::Custom(_) => None,
            _ => Some(self.max_context_window_tokens()),
        }
    }
}

impl fmt::Display for Model {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl Model {
    #[must_use]
    pub fn known(value: &str) -> Option<Self> {
        match value {
            "gpt-6.1-sol" | "sol" => Some(Self::Sol),
            "gpt-6.1-terra" | "terra" => Some(Self::Terra),
            "gpt-6-luna" | "luna" => Some(Self::Luna),
            "gpt-6-astra" | "astra" => Some(Self::Astra),
            "@cf/zai-org/glm-5.3" | "glm-5.3" | "glm53" => Some(Self::Glm53),
            "kimi-k3" | "kimi" => Some(Self::Kimi),
            "mimo-v2.6-pro" | "mimo" => Some(Self::Mimo),
            "gpt-daybreak-blue-latest" | "daybreak-blue" => Some(Self::DaybreakBlue),
            _ => None,
        }
    }
}

impl FromStr for Model {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.trim().is_empty() {
            return Err("model identities must not be empty".to_owned());
        }
        Ok(Self::known(value).unwrap_or_else(|| Self::Custom(CustomModel::new(value))))
    }
}

pub const CONTEXT_WINDOW_TOKENS: u64 = 272_000;

pub const MAX_CONTEXT_WINDOW_TOKENS: u64 = 872_000;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Prompt {
    pub instruction: PromptInput,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    transcript: Vec<PromptMessage>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    instruction_revision: Option<u64>,
}

impl Prompt {
    #[must_use]
    pub fn new(instruction: impl Into<String>) -> Self {
        Self {
            instruction: PromptInput::Text(instruction.into()),
            transcript: Vec::new(),
            instruction_revision: None,
        }
    }

    #[must_use]
    pub fn content(input: impl IntoIterator<Item = UserInput>) -> Self {
        Self {
            instruction: PromptInput::Content(input.into_iter().collect()),
            transcript: Vec::new(),
            instruction_revision: None,
        }
    }

    #[must_use]
    pub const fn with_instruction_revision(mut self, revision: u64) -> Self {
        self.instruction_revision = Some(revision);
        self
    }

    #[must_use]
    pub const fn instruction_revision(&self) -> Option<u64> {
        self.instruction_revision
    }

    #[must_use]
    pub fn with_transcript(mut self, messages: impl IntoIterator<Item = PromptMessage>) -> Self {
        self.transcript = messages.into_iter().collect();
        self
    }

    #[must_use]
    pub fn transcript(&self) -> &[PromptMessage] {
        &self.transcript
    }

    #[must_use]
    pub fn text_bytes(&self) -> usize {
        self.transcript
            .iter()
            .map(PromptMessage::text_bytes)
            .sum::<usize>()
            .saturating_add(self.instruction.text_bytes())
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.instruction.is_empty()
    }

    pub fn validate(&self) -> Result<(), PromptValidationError> {
        if self.instruction.is_empty() {
            return Err(PromptValidationError::EmptyInstruction);
        }
        if self.transcript.iter().any(PromptMessage::is_empty) {
            return Err(PromptValidationError::EmptyTranscriptMessage);
        }
        if self
            .transcript
            .first()
            .is_some_and(|message| message.role() != PromptMessageRole::User)
            || self
                .transcript
                .last()
                .is_some_and(|message| message.role() != PromptMessageRole::Assistant)
        {
            return Err(PromptValidationError::InvalidTranscriptEndpoints);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PromptValidationError {
    EmptyInstruction,

    EmptyTranscriptMessage,

    InvalidTranscriptEndpoints,
}

impl std::fmt::Display for PromptValidationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyInstruction => formatter.write_str("prompt instruction must not be empty"),
            Self::EmptyTranscriptMessage => formatter.write_str("prompt transcript messages must not be empty"),
            Self::InvalidTranscriptEndpoints => formatter.write_str("prompt transcript must start with a user message and end with an assistant message"),
        }
    }
}

impl std::error::Error for PromptValidationError {}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PromptMessage {
    role: PromptMessageRole,
    content: String,
}

impl PromptMessage {
    #[must_use]
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: PromptMessageRole::User,
            content: content.into(),
        }
    }

    #[must_use]
    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: PromptMessageRole::Assistant,
            content: content.into(),
        }
    }

    #[must_use]
    pub const fn role(&self) -> PromptMessageRole {
        self.role
    }

    #[must_use]
    pub fn content(&self) -> &str {
        &self.content
    }

    const fn text_bytes(&self) -> usize {
        self.content.len()
    }

    fn is_empty(&self) -> bool {
        self.content.trim().is_empty()
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PromptMessageRole {
    User,

    Assistant,
}

impl From<String> for Prompt {
    fn from(instruction: String) -> Self {
        Self::new(instruction)
    }
}

impl From<&str> for Prompt {
    fn from(instruction: &str) -> Self {
        Self::new(instruction)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(untagged)]
pub enum PromptInput {
    Text(String),

    Content(Vec<UserInput>),
}

impl PromptInput {
    #[must_use]
    pub fn text_bytes(&self) -> usize {
        match self {
            Self::Text(text) => text.len(),
            Self::Content(items) => items.iter().map(UserInput::text_bytes).sum(),
        }
    }

    #[must_use]
    pub fn text_chars(&self) -> usize {
        match self {
            Self::Text(text) => text.chars().count(),
            Self::Content(items) => items.iter().map(UserInput::text_chars).sum(),
        }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        match self {
            Self::Text(text) => text.trim().is_empty(),
            Self::Content(items) => items.is_empty() || items.iter().all(UserInput::is_empty),
        }
    }
}

impl From<String> for PromptInput {
    fn from(value: String) -> Self {
        Self::Text(value)
    }
}

impl From<&str> for PromptInput {
    fn from(value: &str) -> Self {
        Self::Text(value.to_owned())
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum UserInput {
    Text {
        text: String,
    },

    Image {
        image_url: String,

        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<ImageDetail>,
    },

    #[serde(rename = "image")]
    ImageFile {
        #[serde(serialize_with = "serialize_image_file_id")]
        file_id: String,

        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<ImageDetail>,
    },

    LocalImage {
        path: PathBuf,

        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<ImageDetail>,
    },

    Audio {
        audio_url: String,
    },

    LocalAudio {
        path: PathBuf,
    },

    File {
        file_data: String,

        #[serde(default, skip_serializing_if = "Option::is_none")]
        filename: Option<String>,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ImageDetail {
    Auto,

    Low,

    High,

    Original,
}

impl UserInput {
    #[must_use]
    pub const fn text_bytes(&self) -> usize {
        match self {
            Self::Text { text } => text.len(),
            Self::Image { .. }
            | Self::ImageFile { .. }
            | Self::LocalImage { .. }
            | Self::Audio { .. }
            | Self::LocalAudio { .. }
            | Self::File { .. } => 0,
        }
    }

    #[must_use]
    pub fn text_chars(&self) -> usize {
        match self {
            Self::Text { text } => text.chars().count(),
            Self::Image { .. }
            | Self::ImageFile { .. }
            | Self::LocalImage { .. }
            | Self::Audio { .. }
            | Self::LocalAudio { .. }
            | Self::File { .. } => 0,
        }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        match self {
            Self::Text { text } => text.trim().is_empty(),
            Self::Image { .. }
            | Self::ImageFile { .. }
            | Self::LocalImage { .. }
            | Self::Audio { .. }
            | Self::LocalAudio { .. }
            | Self::File { .. } => false,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningMode {
    #[default]
    Standard,

    Pro,
}

impl ReasoningMode {
    #[must_use]
    pub const fn is_standard(&self) -> bool {
        matches!(self, Self::Standard)
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::Pro => "pro",
        }
    }

    #[doc(hidden)]
    pub const fn request_value(self) -> Option<&'static str> {
        match self {
            Self::Standard => None,
            Self::Pro => Some("pro"),
        }
    }
}

impl fmt::Display for ReasoningMode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for ReasoningMode {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "standard" => Ok(Self::Standard),
            "pro" => Ok(Self::Pro),
            _ => Err(format!(
                "invalid reasoning mode {value:?}; expected standard or pro"
            )),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Thinking {
    None,

    #[default]
    Low,

    Medium,

    High,

    Xhigh,

    Max,
}

impl Thinking {
    pub const ALL: [Self; 6] = [
        Self::None,
        Self::Low,
        Self::Medium,
        Self::High,
        Self::Xhigh,
        Self::Max,
    ];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Xhigh => "xhigh",
            Self::Max => "max",
        }
    }
}

impl fmt::Display for Thinking {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for Thinking {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "none" => Ok(Self::None),
            "low" => Ok(Self::Low),
            "medium" => Ok(Self::Medium),
            "high" => Ok(Self::High),
            "xhigh" => Ok(Self::Xhigh),
            "max" => Ok(Self::Max),
            _ => Err(format!(
                "invalid reasoning effort {value:?}; expected none, low, medium, high, xhigh, or max"
            )),
        }
    }
}

#[derive(Deserialize)]
#[serde(
    remote = "UserInput",
    tag = "type",
    rename_all = "snake_case",
    deny_unknown_fields
)]
enum UserInputFields {
    Text {
        text: String,
    },

    Image {
        image_url: String,

        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<ImageDetail>,
    },

    LocalImage {
        path: PathBuf,

        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<ImageDetail>,
    },

    Audio {
        audio_url: String,
    },

    LocalAudio {
        path: PathBuf,
    },

    File {
        file_data: String,

        #[serde(default)]
        filename: Option<String>,
    },
}

fn serialize_image_file_id<S: serde::Serializer>(
    file_id: &str,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    if !crate::responses::valid_image_file_id(file_id) {
        return Err(serde::ser::Error::custom("invalid image file_id"));
    }
    serializer.serialize_str(file_id)
}
impl<'de> Deserialize<'de> for UserInput {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(deserializer)?;
        if value.get("type").and_then(serde_json::Value::as_str) == Some("image") {
            let serde_json::Value::Object(mut object) = value else {
                return Err(serde::de::Error::custom("expected image object"));
            };
            object.remove("type");
            let detail = object.remove("detail");
            let reference: crate::responses::ImageReference =
                serde_json::from_value(serde_json::Value::Object(object))
                    .map_err(serde::de::Error::custom)?;
            let detail: Option<ImageDetail> =
                serde_json::from_value(detail.unwrap_or(serde_json::Value::Null))
                    .map_err(serde::de::Error::custom)?;
            return Ok(match reference {
                crate::responses::ImageReference::Inline { image_url } => {
                    Self::Image { image_url, detail }
                }
                crate::responses::ImageReference::File { file_id } => {
                    Self::ImageFile { file_id, detail }
                }
            });
        }
        UserInputFields::deserialize(value).map_err(serde::de::Error::custom)
    }
}
