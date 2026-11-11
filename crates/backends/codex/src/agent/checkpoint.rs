use super::*;
use crate::{HarnessFamily, ModelSelection};

#[derive(serde::Serialize, serde::Deserialize)]
struct CodexPayload {
    #[serde(flatten, with = "crate::service_tier_serde")]
    service_tier: ServiceTier,

    #[serde(default)]
    stateless_http: bool,

    conversation: Option<SessionSnapshot>,
}

pub(super) struct ChildState {
    pub(super) session_id: String,
    pub(super) model: Model,
    pub(super) thinking: Thinking,
    pub(super) reasoning_mode: ReasoningMode,
    pub(super) service_tier: ServiceTier,
    pub(super) stateless_http: bool,
    pub(super) conversation: Option<SessionSnapshot>,
    pub(super) lineage: Lineage,
    pub(super) conversation_id: Arc<str>,
}

impl ChildState {
    pub(super) fn from_checkpoint(checkpoint: SessionCheckpoint) -> Result<Self> {
        checkpoint.validate()?;
        checkpoint.require_family(HarnessFamily::Codex)?;
        let Some(model) = checkpoint.model().as_codex() else {
            return Err(NanocodeError::CheckpointFamilyMismatch {
                expected: HarnessFamily::Codex,
                found: checkpoint.family(),
            });
        };
        let session_id = checkpoint.session_id().to_owned();
        let thinking = checkpoint.thinking();
        let reasoning_mode = checkpoint.reasoning_mode();
        let lineage = checkpoint.lineage().clone();
        let conversation_id = Arc::<str>::from(checkpoint.conversation_id());
        let has_conversation = checkpoint.has_conversation();
        let payload: CodexPayload = checkpoint
            .payload()
            .decode()
            .map_err(|error| NanocodeError::InvalidCheckpoint(error.to_string()))?;
        if payload.conversation.is_some() != has_conversation {
            return Err(NanocodeError::InvalidCheckpoint(
                "checkpoint conversation flag does not match its payload".into(),
            ));
        }
        if payload
            .conversation
            .as_ref()
            .is_some_and(|conversation| conversation.lineage_id() != conversation_id.as_ref())
        {
            return Err(NanocodeError::InvalidCheckpoint(
                "checkpoint conversation belongs to another cache lineage".into(),
            ));
        }
        let state = Self {
            session_id,
            model,
            thinking,
            reasoning_mode,
            service_tier: payload.service_tier,
            stateless_http: payload.stateless_http,
            conversation: payload.conversation,
            lineage,
            conversation_id,
        };
        state.validate()?;
        Ok(state)
    }

    pub(super) fn validate(&self) -> Result<()> {
        self.session_id.parse::<SessionId>().map_err(|error| {
            NanocodeError::InvalidCheckpoint(format!("invalid session ID: {error}"))
        })?;
        super::spawn::validate_model_thinking(self.model, self.thinking)?;
        super::spawn::validate_model_reasoning_mode(self.model, self.reasoning_mode)?;
        if let Some(conversation) = &self.conversation {
            if conversation
                .reasoning_mode()
                .is_some_and(|mode| mode != self.reasoning_mode)
            {
                return Err(NanocodeError::InvalidCheckpoint(
                    "checkpoint reasoning mode does not match its conversation".into(),
                ));
            }
            conversation.clone().into_resume()?;
        }
        Ok(())
    }

    pub(super) fn into_checkpoint(self) -> Result<SessionCheckpoint> {
        let has_conversation = self.conversation.is_some();
        let payload = serde_json::to_value(CodexPayload {
            service_tier: self.service_tier,
            stateless_http: self.stateless_http,
            conversation: self.conversation,
        })
        .map_err(|error| NanocodeError::InvalidCheckpoint(error.to_string()))?;
        Ok(SessionCheckpoint::native(
            self.session_id,
            ModelSelection::Codex(self.model),
            self.thinking,
            self.lineage,
            self.conversation_id.as_ref(),
            None,
            has_conversation,
            payload,
        )
        .with_service_tier(Some(self.service_tier))
        .with_reasoning_mode(self.reasoning_mode))
    }
}

pub trait CodexCheckpointExt: Sized {
    fn from_legacy_codex_child(value: serde_json::Value, lineage: Lineage) -> Result<Self>;

    fn codex_snapshot(&self) -> Result<Option<SessionSnapshot>>;

    fn codex(
        session_id: impl Into<String>,
        lineage: Lineage,
        thinking: Thinking,
        snapshot: SessionSnapshot,
    ) -> Result<Self>;
}
impl CodexCheckpointExt for SessionCheckpoint {
    #[doc(hidden)]
    fn codex(
        session_id: impl Into<String>,
        lineage: Lineage,
        thinking: Thinking,
        snapshot: SessionSnapshot,
    ) -> Result<Self> {
        let state = ChildState {
            session_id: session_id.into(),
            model: snapshot.model()?,
            thinking: snapshot.thinking().unwrap_or(thinking),
            reasoning_mode: snapshot.reasoning_mode().unwrap_or(ReasoningMode::Standard),
            service_tier: snapshot.service_tier().unwrap_or(ServiceTier::Standard),
            stateless_http: false,
            lineage,
            conversation_id: Arc::from(snapshot.lineage_id()),
            conversation: Some(snapshot),
        };
        state.validate()?;
        state.into_checkpoint()
    }

    #[doc(hidden)]
    fn from_legacy_codex_child(value: serde_json::Value, lineage: Lineage) -> Result<Self> {
        #[derive(serde::Deserialize)]
        struct Legacy {
            session_id: String,
            model: Model,
            thinking: Thinking,
            #[serde(flatten)]
            payload: serde_json::Value,
        }
        let invalid =
            |error: serde_json::Error| NanocodeError::InvalidCheckpoint(error.to_string());
        let legacy: Legacy = serde_json::from_value(value).map_err(invalid)?;
        let payload: CodexPayload = serde_json::from_value(legacy.payload).map_err(invalid)?;
        let conversation_id = payload.conversation.as_ref().map_or_else(
            || Arc::from(legacy.session_id.as_str()),
            |conversation| Arc::from(conversation.lineage_id()),
        );
        let state = ChildState {
            session_id: legacy.session_id,
            model: legacy.model,
            thinking: legacy.thinking,
            reasoning_mode: ReasoningMode::Standard,
            service_tier: payload.service_tier,
            stateless_http: payload.stateless_http,
            conversation: payload.conversation,
            lineage,
            conversation_id,
        };
        state.validate()?;
        state.into_checkpoint()
    }

    #[doc(hidden)]
    fn codex_snapshot(&self) -> Result<Option<SessionSnapshot>> {
        Ok(ChildState::from_checkpoint(self.clone())?.conversation)
    }
}

pub(super) const fn is_stateless_http(config: &ModelConfig) -> bool {
    matches!(config.responses_transport, ResponsesTransport::Https)
        && matches!(config.responses_history, ResponsesHistory::FullReplay)
        && !config.store_responses
}

pub(super) struct CheckpointSource {
    session_id: Arc<str>,
    lineage: Lineage,
    conversation_id: Arc<str>,
    stateless_http: bool,
    reasoning_mode: ReasoningMode,
}

pub(super) struct ReplayedBoundary(SessionSnapshot);

impl CheckpointSource {
    pub(super) const fn new(
        session_id: Arc<str>,
        lineage: Lineage,
        conversation_id: Arc<str>,
        stateless_http: bool,
        reasoning_mode: ReasoningMode,
    ) -> Self {
        Self {
            session_id,
            lineage,
            conversation_id,
            stateless_http,
            reasoning_mode,
        }
    }

    pub(super) fn child_info(&self, session_id: &SessionId, origin: crate::Origin) -> SessionInfo {
        SessionInfo::new(
            session_id.to_string(),
            HarnessFamily::Codex,
            Lineage::child_of(&self.lineage, self.session_id.as_ref(), origin),
        )
    }

    pub(super) fn conversation_id(&self) -> &str {
        &self.conversation_id
    }

    fn checkpoint(
        &self,
        model: Model,
        thinking: Thinking,
        service_tier: ServiceTier,
        conversation: Option<SessionSnapshot>,
    ) -> Result<SessionCheckpoint> {
        ChildState {
            session_id: self.session_id.to_string(),
            model,
            thinking,
            service_tier,
            stateless_http: self.stateless_http,
            reasoning_mode: self.reasoning_mode,
            conversation,
            lineage: self.lineage.clone(),
            conversation_id: Arc::clone(&self.conversation_id),
        }
        .into_checkpoint()
    }

    pub(super) fn live(
        self: &Arc<Self>,
        committed: Arc<CommittedSession>,
        thinking: Thinking,
        service_tier: ServiceTier,
    ) -> TurnBoundary {
        let source = Arc::clone(self);
        TurnBoundary::live(committed, move |committed: &CommittedSession| {
            source.checkpoint(
                committed.selected_model(),
                thinking,
                service_tier,
                Some(committed.snapshot()),
            )
        })
    }

    pub(super) fn replayed(
        self: &Arc<Self>,
        snapshot: SessionSnapshot,
        model: Model,
        thinking: Thinking,
        service_tier: ServiceTier,
    ) -> TurnBoundary {
        let source = Arc::clone(self);
        TurnBoundary::live(
            Arc::new(ReplayedBoundary(snapshot)),
            move |replayed: &ReplayedBoundary| {
                source.checkpoint(model, thinking, service_tier, Some(replayed.0.clone()))
            },
        )
    }
}
