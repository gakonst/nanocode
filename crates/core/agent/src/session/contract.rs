use std::{any::Any, fmt, sync::Arc};

use serde::{Deserialize, Serialize};

use crate::{
    HarnessFamily, ModelSelection, NanocodeError, Result, ServiceTier, Thinking, TurnResult,
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct SessionInfo {
    pub session_id: String,

    pub family: HarnessFamily,

    pub lineage: Lineage,
}

impl SessionInfo {
    #[must_use]
    pub fn new(session_id: impl Into<String>, family: HarnessFamily, lineage: Lineage) -> Self {
        Self {
            session_id: session_id.into(),
            family,
            lineage,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Lineage {
    pub root_session_id: String,

    pub parent_session_id: Option<String>,

    pub origin: Origin,

    pub depth: u32,
}

impl Lineage {
    #[must_use]
    pub fn root(session_id: impl Into<String>) -> Self {
        Self {
            root_session_id: session_id.into(),
            parent_session_id: None,
            origin: Origin::Root,
            depth: 0,
        }
    }

    #[must_use]
    pub fn new(
        root_session_id: impl Into<String>,
        parent_session_id: Option<String>,
        origin: Origin,
        depth: u32,
    ) -> Self {
        Self {
            root_session_id: root_session_id.into(),
            parent_session_id,
            origin,
            depth,
        }
    }

    #[must_use]
    pub fn child_of(parent: &Self, parent_session_id: impl Into<String>, origin: Origin) -> Self {
        Self {
            root_session_id: parent.root_session_id.clone(),
            parent_session_id: Some(parent_session_id.into()),
            origin,
            depth: parent.depth.saturating_add(1),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Origin {
    Root,

    Fork,

    SideConversation,

    Subagent,

    Branch,
}

const CHECKPOINT_FORMAT: &str = "nanocode-session-checkpoint/1";

#[doc(hidden)]
#[derive(Clone)]
pub enum CheckpointPayload {
    Tree(serde_json::Value),
    Json(Box<serde_json::value::RawValue>),
}

impl CheckpointPayload {
    pub fn decode<T: serde::de::DeserializeOwned>(&self) -> serde_json::Result<T> {
        match self {
            Self::Tree(tree) => T::deserialize(tree),
            Self::Json(json) => serde_json::from_str(json.get()),
        }
    }
}

impl From<serde_json::Value> for CheckpointPayload {
    fn from(tree: serde_json::Value) -> Self {
        Self::Tree(tree)
    }
}

impl From<Box<serde_json::value::RawValue>> for CheckpointPayload {
    fn from(json: Box<serde_json::value::RawValue>) -> Self {
        Self::Json(json)
    }
}

impl PartialEq for CheckpointPayload {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Tree(tree), Self::Tree(other)) => tree == other,
            (Self::Json(json), Self::Json(other)) if json.get() == other.get() => true,
            _ => match (self.decode(), other.decode()) {
                (Ok(tree), Ok(other)) => {
                    let (tree, other): (serde_json::Value, serde_json::Value) = (tree, other);
                    tree == other
                }
                _ => false,
            },
        }
    }
}

impl Serialize for CheckpointPayload {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        match self {
            Self::Tree(tree) => tree.serialize(serializer),
            Self::Json(json) => json.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for CheckpointPayload {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        serde_json::Value::deserialize(deserializer).map(Self::Tree)
    }
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionCheckpoint {
    format: String,
    session_id: String,
    model: ModelSelection,
    thinking: Thinking,
    #[serde(default, skip_serializing_if = "crate::ReasoningMode::is_standard")]
    reasoning_mode: crate::ReasoningMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    service_tier: Option<ServiceTier>,
    lineage: Lineage,

    conversation_id: String,
    turn_id: Option<String>,
    has_conversation: bool,
    payload: CheckpointPayload,
}

impl SessionCheckpoint {
    #[doc(hidden)]
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn native(
        session_id: impl Into<String>,
        model: ModelSelection,
        thinking: Thinking,
        lineage: Lineage,
        conversation_id: impl Into<String>,
        turn_id: Option<String>,
        has_conversation: bool,
        payload: impl Into<CheckpointPayload>,
    ) -> Self {
        Self {
            format: CHECKPOINT_FORMAT.to_owned(),
            session_id: session_id.into(),
            model,
            thinking,
            reasoning_mode: crate::ReasoningMode::Standard,
            service_tier: None,
            lineage,
            conversation_id: conversation_id.into(),
            turn_id,
            has_conversation,
            payload: payload.into(),
        }
    }

    pub fn from_json(json: &str) -> Result<Self> {
        let checkpoint: Self = serde_json::from_str(json)
            .map_err(|error| NanocodeError::InvalidCheckpoint(error.to_string()))?;
        checkpoint.validate()?;
        Ok(checkpoint)
    }

    pub fn to_json(&self) -> Result<String> {
        serde_json::to_string(self)
            .map_err(|error| NanocodeError::InvalidCheckpoint(error.to_string()))
    }

    pub fn validate(&self) -> Result<()> {
        let invalid = |reason: String| Err(NanocodeError::InvalidCheckpoint(reason));
        if self.format != CHECKPOINT_FORMAT {
            return invalid(format!(
                "unsupported session checkpoint format {:?}; expected {CHECKPOINT_FORMAT:?}",
                self.format
            ));
        }
        if self.session_id.trim().is_empty() || self.conversation_id.trim().is_empty() {
            return invalid("session and conversation identities must not be empty".into());
        }
        if self
            .turn_id
            .as_deref()
            .is_some_and(|turn| turn.trim().is_empty())
        {
            return invalid("recorded turn identity must not be empty".into());
        }
        self.model
            .validate()
            .map_err(|error| NanocodeError::InvalidCheckpoint(error.to_string()))?;
        if !self.model.supports_thinking(self.thinking) {
            return invalid(format!(
                "model {} does not support {} thinking",
                self.model,
                self.thinking.as_str()
            ));
        }
        self.model
            .capabilities(crate::ModelTransport::Native)
            .check_reasoning_mode(self.reasoning_mode)
            .map_err(|error| NanocodeError::InvalidCheckpoint(error.to_string()))?;
        let lineage = &self.lineage;
        if lineage.root_session_id.trim().is_empty() {
            return invalid("lineage root session identity must not be empty".into());
        }
        let derived = lineage.parent_session_id.is_some();
        if (lineage.origin == Origin::Root) == derived || derived != (lineage.depth > 0) {
            return invalid(format!(
                "inconsistent {:?} lineage: parent {:?} at depth {}",
                lineage.origin, lineage.parent_session_id, lineage.depth
            ));
        }
        Ok(())
    }

    #[must_use]
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    #[must_use]
    pub fn family(&self) -> HarnessFamily {
        self.model.family()
    }

    #[must_use]
    pub fn model(&self) -> ModelSelection {
        self.model.clone()
    }

    #[must_use]
    pub const fn thinking(&self) -> Thinking {
        self.thinking
    }

    #[must_use]
    pub const fn reasoning_mode(&self) -> crate::ReasoningMode {
        self.reasoning_mode
    }

    #[must_use]
    pub const fn with_reasoning_mode(mut self, reasoning_mode: crate::ReasoningMode) -> Self {
        self.reasoning_mode = reasoning_mode;
        self
    }

    #[must_use]
    pub const fn service_tier(&self) -> Option<ServiceTier> {
        self.service_tier
    }

    #[must_use]
    pub const fn lineage(&self) -> &Lineage {
        &self.lineage
    }

    #[must_use]
    pub fn conversation_id(&self) -> &str {
        &self.conversation_id
    }

    #[must_use]
    pub fn turn_id(&self) -> Option<&str> {
        self.turn_id.as_deref()
    }

    #[must_use]
    pub const fn has_conversation(&self) -> bool {
        self.has_conversation
    }

    #[doc(hidden)]
    #[must_use]
    pub const fn payload(&self) -> &CheckpointPayload {
        &self.payload
    }

    #[doc(hidden)]
    #[must_use]
    pub fn into_payload(self) -> CheckpointPayload {
        self.payload
    }

    #[doc(hidden)]
    #[must_use]
    pub fn with_identity(mut self, session_id: impl Into<String>, lineage: Lineage) -> Self {
        self.session_id = session_id.into();
        self.lineage = lineage;
        self
    }

    #[doc(hidden)]
    #[must_use]
    pub fn with_turn_id(mut self, turn_id: Option<String>) -> Self {
        self.turn_id = turn_id;
        self
    }

    #[doc(hidden)]
    #[must_use]
    pub const fn with_service_tier(mut self, service_tier: Option<ServiceTier>) -> Self {
        self.service_tier = service_tier;
        self
    }

    pub fn require_family(&self, family: HarnessFamily) -> Result<()> {
        if self.family() == family {
            Ok(())
        } else {
            Err(NanocodeError::CheckpointFamilyMismatch {
                expected: family,
                found: self.family(),
            })
        }
    }
}

impl fmt::Debug for SessionCheckpoint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SessionCheckpoint")
            .field("session_id", &self.session_id)
            .field("model", &self.model)
            .field("reasoning_mode", &self.reasoning_mode)
            .field("service_tier", &self.service_tier)
            .field("lineage", &self.lineage)
            .field("turn_id", &self.turn_id)
            .field("has_conversation", &self.has_conversation)
            .finish_non_exhaustive()
    }
}

type Materialize = dyn Fn(&(dyn Any + Send + Sync)) -> Result<SessionCheckpoint> + Send + Sync;

#[derive(Clone)]
#[doc(hidden)]
pub struct TurnBoundary {
    live: Arc<dyn Any + Send + Sync>,
    materialize: Arc<Materialize>,
}

impl TurnBoundary {
    #[must_use]
    pub fn live<T, F>(state: Arc<T>, materialize: F) -> Self
    where
        T: Any + Send + Sync,
        F: Fn(&T) -> Result<SessionCheckpoint> + Send + Sync + 'static,
    {
        Self {
            live: state,
            materialize: Arc::new(move |state| {
                let state = state
                    .downcast_ref::<T>()
                    .expect("turn boundary state type is fixed at construction");
                materialize(state)
            }),
        }
    }

    #[must_use]
    pub fn portable(checkpoint: SessionCheckpoint) -> Self {
        Self::live(Arc::new(checkpoint), |checkpoint: &SessionCheckpoint| {
            Ok(checkpoint.clone())
        })
    }

    #[must_use]
    pub fn downcast<T: Any + Send + Sync>(&self) -> Option<Arc<T>> {
        Arc::clone(&self.live).downcast::<T>().ok()
    }

    #[must_use]
    pub fn is<T: Any + Send + Sync>(&self) -> bool {
        self.live.is::<T>()
    }

    pub fn checkpoint(&self) -> Result<SessionCheckpoint> {
        (self.materialize)(self.live.as_ref())
    }
}

#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum ForkPoint {
    Latest,

    Turn(TurnResult),

    Checkpoint(SessionCheckpoint),
}

#[derive(Clone, Debug)]
pub struct ForkRequest {
    point: ForkPoint,
    origin: Origin,
}

impl ForkRequest {
    #[must_use]
    pub const fn latest() -> Self {
        Self {
            point: ForkPoint::Latest,
            origin: Origin::Fork,
        }
    }

    #[must_use]
    pub fn at_turn(result: &TurnResult) -> Self {
        Self {
            point: ForkPoint::Turn(result.clone()),
            origin: Origin::Fork,
        }
    }

    #[must_use]
    pub const fn at(checkpoint: SessionCheckpoint) -> Self {
        Self {
            point: ForkPoint::Checkpoint(checkpoint),
            origin: Origin::Fork,
        }
    }

    #[must_use]
    pub const fn side_conversation(mut self) -> Self {
        self.origin = Origin::SideConversation;
        self
    }

    #[must_use]
    pub const fn point(&self) -> &ForkPoint {
        &self.point
    }

    #[must_use]
    pub const fn origin(&self) -> Origin {
        self.origin
    }

    #[doc(hidden)]
    #[must_use]
    pub fn into_parts(self) -> (ForkPoint, Origin) {
        (self.point, self.origin)
    }
}

impl Default for ForkRequest {
    fn default() -> Self {
        Self::latest()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mutability {
    #[default]
    Fixed,

    BeforeFirstPrompt,

    BetweenTurns,

    Anytime,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
#[allow(clippy::struct_excessive_bools)]
pub struct Capabilities {
    pub checkpoint: bool,

    pub resume: bool,

    pub fork: bool,

    pub fork_at: bool,

    pub side_conversation: bool,

    pub side_question: bool,

    pub spawn: bool,

    pub steering: bool,

    pub identified_steering: bool,

    pub compaction: bool,

    pub developer_messages: bool,

    pub context: bool,

    pub model: Mutability,

    pub thinking: Mutability,

    pub service_tier: Mutability,

    pub ultrafast_service_tier: bool,
}

impl Capabilities {
    pub const NONE: Self = Self {
        checkpoint: false,
        resume: false,
        fork: false,
        fork_at: false,
        side_conversation: false,
        side_question: false,
        spawn: false,
        steering: false,
        identified_steering: false,
        compaction: false,
        developer_messages: false,
        context: false,
        model: Mutability::Fixed,
        thinking: Mutability::Fixed,
        service_tier: Mutability::Fixed,
        ultrafast_service_tier: false,
    };
}

#[derive(Clone, Debug, Default)]
#[non_exhaustive]
pub struct Persistence {
    pub durable_state_id: Option<String>,

    pub server_session_id: Option<String>,
}

impl Persistence {
    #[must_use]
    pub fn durable(state_id: impl Into<String>) -> Self {
        Self {
            durable_state_id: Some(state_id.into()),
            ..Self::default()
        }
    }

    #[must_use]
    pub fn server(session_id: impl Into<String>) -> Self {
        Self {
            server_session_id: Some(session_id.into()),
            ..Self::default()
        }
    }

    #[must_use]
    pub const fn resumable(&self) -> bool {
        self.durable_state_id.is_some() || self.server_session_id.is_some()
    }
}
