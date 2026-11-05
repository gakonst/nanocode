use nanocode_agent_core::{ModelSelection, ReasoningMode, Thinking};
use std::fmt;

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SpawnPolicy {
    pub models: Option<Vec<ModelSelection>>,
    pub min_thinking: Option<Thinking>,
    pub max_thinking: Option<Thinking>,
    /// Refuses a child whose thinking ranks above its spawning agent's own,
    /// whether that agent is the root or a registered subagent, so a
    /// lower-effort agent cannot escalate into higher-effort descendants.
    pub bounded_by_parent_thinking: bool,
    pub max_reasoning_mode: Option<ReasoningMode>,
    pub budgets: bool,
}

impl Default for SpawnPolicy {
    fn default() -> Self {
        Self {
            models: None,
            min_thinking: Some(Thinking::Low),
            max_thinking: None,
            bounded_by_parent_thinking: false,
            max_reasoning_mode: None,
            budgets: false,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SpawnPolicyError {
    ModelNotOffered {
        requested: ModelSelection,
    },
    ThinkingBelowMinimum {
        requested: Thinking,
        minimum: Thinking,
    },
    ThinkingExceedsMaximum {
        requested: Thinking,
        maximum: Thinking,
    },
    ThinkingExceedsParent {
        requested: Thinking,
        parent: Thinking,
    },
    ParentThinkingUnknown,
    ReasoningModeExceedsMaximum {
        requested: ReasoningMode,
        maximum: ReasoningMode,
    },
    ReasoningModeExceedsParent {
        requested: ReasoningMode,
        parent: ReasoningMode,
    },
    ChildSettingsUnknown {
        reason: String,
    },
}

impl fmt::Display for SpawnPolicyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ModelNotOffered { requested } => write!(
                formatter,
                "model {} ({}) is not offered to subagents",
                requested.as_str(),
                requested.family()
            ),
            Self::ThinkingBelowMinimum { requested, minimum } => write!(
                formatter,
                "{requested} thinking is below the subagent minimum of {minimum}"
            ),
            Self::ThinkingExceedsMaximum { requested, maximum } => write!(
                formatter,
                "{requested} thinking exceeds the subagent maximum of {maximum}"
            ),
            Self::ThinkingExceedsParent { requested, parent } => write!(
                formatter,
                "{requested} thinking exceeds the spawning agent's own {parent} thinking"
            ),
            Self::ParentThinkingUnknown => formatter.write_str(
                "the spawning agent's thinking is not recorded, so its children cannot inherit it",
            ),
            Self::ReasoningModeExceedsMaximum { requested, maximum } => write!(
                formatter,
                "{requested} reasoning mode exceeds the subagent maximum of {maximum}"
            ),
            Self::ReasoningModeExceedsParent { requested, parent } => write!(
                formatter,
                "{requested} reasoning mode exceeds the spawning agent's own {parent} reasoning mode"
            ),
            Self::ChildSettingsUnknown { reason } => write!(
                formatter,
                "the child's effective model and thinking could not be read for policy admission: {reason}"
            ),
        }
    }
}

impl std::error::Error for SpawnPolicyError {}

impl From<SpawnPolicyError> for std::io::Error {
    fn from(error: SpawnPolicyError) -> Self {
        Self::other(error)
    }
}

pub(crate) struct SpawnParent {
    pub(crate) model: ModelSelection,
    pub(crate) registered: bool,
    pub(crate) thinking: Option<Thinking>,
    pub(crate) reasoning_mode: ReasoningMode,
}

impl SpawnPolicy {
    pub const fn is_unrestricted(&self) -> bool {
        self.models.is_none()
            && self.min_thinking.is_none()
            && self.max_thinking.is_none()
            && !self.bounded_by_parent_thinking
            && self.max_reasoning_mode.is_none()
    }

    pub fn offered_models(&self) -> Option<&[ModelSelection]> {
        self.models.as_deref()
    }

    pub fn offered_thinking(&self) -> Vec<Thinking> {
        Thinking::ALL
            .into_iter()
            .filter(|thinking| {
                self.min_thinking
                    .is_none_or(|minimum| rank(*thinking) >= rank(minimum))
                    && self
                        .max_thinking
                        .is_none_or(|maximum| rank(*thinking) <= rank(maximum))
            })
            .collect()
    }

    pub fn admit_model(&self, model: &ModelSelection) -> Result<(), SpawnPolicyError> {
        if let Some(models) = &self.models
            && !models.contains(model)
        {
            return Err(SpawnPolicyError::ModelNotOffered {
                requested: model.clone(),
            });
        }
        Ok(())
    }

    pub const fn admit_thinking(&self, thinking: Thinking) -> Result<(), SpawnPolicyError> {
        if let Some(minimum) = self.min_thinking
            && rank(thinking) < rank(minimum)
        {
            return Err(SpawnPolicyError::ThinkingBelowMinimum {
                requested: thinking,
                minimum,
            });
        }
        if let Some(maximum) = self.max_thinking
            && rank(thinking) > rank(maximum)
        {
            return Err(SpawnPolicyError::ThinkingExceedsMaximum {
                requested: thinking,
                maximum,
            });
        }
        Ok(())
    }

    pub(crate) fn admit(
        &self,
        parent: &SpawnParent,
        model: Option<&ModelSelection>,
        thinking: Option<Thinking>,
    ) -> Result<(), SpawnPolicyError> {
        if let Some(model) = model {
            self.admit_model(model)?;
        }
        if let Some(thinking) = thinking {
            self.admit_thinking(thinking)?;
            self.admit_parent_thinking(parent, thinking)?;
        }
        Ok(())
    }

    fn admit_parent_thinking(
        &self,
        parent: &SpawnParent,
        requested: Thinking,
    ) -> Result<(), SpawnPolicyError> {
        if !self.bounded_by_parent_thinking {
            return Ok(());
        }
        let parent = parent
            .thinking
            .ok_or(SpawnPolicyError::ParentThinkingUnknown)?;
        if rank(requested) > rank(parent) {
            return Err(SpawnPolicyError::ThinkingExceedsParent { requested, parent });
        }
        Ok(())
    }

    pub(crate) fn admit_reasoning_mode(
        &self,
        parent: &SpawnParent,
        requested: ReasoningMode,
    ) -> Result<(), SpawnPolicyError> {
        let maximum = self.max_reasoning_mode.unwrap_or(parent.reasoning_mode);
        if requested == ReasoningMode::Pro && maximum == ReasoningMode::Standard {
            return Err(SpawnPolicyError::ReasoningModeExceedsMaximum { requested, maximum });
        }
        if parent.registered
            && requested == ReasoningMode::Pro
            && parent.reasoning_mode == ReasoningMode::Standard
        {
            return Err(SpawnPolicyError::ReasoningModeExceedsParent {
                requested,
                parent: parent.reasoning_mode,
            });
        }
        Ok(())
    }
}

impl crate::Registry {
    pub fn spawn_policy(&self) -> SpawnPolicy {
        SpawnPolicy::clone(&self.spawn_policy.get())
    }

    pub fn set_spawn_policy(&self, policy: SpawnPolicy) {
        self.spawn_policy.set(policy);
    }

    pub fn update_spawn_policy(&self, update: impl FnMut(&mut SpawnPolicy)) {
        self.spawn_policy.update(update);
    }
}

const fn rank(thinking: Thinking) -> u8 {
    thinking as u8
}
