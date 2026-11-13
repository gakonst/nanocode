use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(
    Clone, Copy, Debug, Default, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize,
)]
#[serde(rename_all = "snake_case")]
pub enum EffectClass {
    #[default]
    External,
    Idempotent,
    None,
}

impl EffectClass {
    #[must_use]
    pub const fn mcp(read_only: bool, resumable: bool) -> Self {
        match (read_only, resumable) {
            (true, _) => Self::None,
            (false, true) => Self::Idempotent,
            (false, false) => Self::External,
        }
    }

    #[must_use]
    pub const fn needs_row(self) -> bool {
        matches!(self, Self::External)
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Effects(BTreeMap<String, EffectClass>);

impl Effects {
    #[must_use]
    pub fn builtin() -> Self {
        [
            "Glob",
            "Grep",
            "list_agents",
            "LS",
            "memories__list",
            "memories__read",
            "memories__search",
            "Read",
            "TaskGet",
            "TaskList",
            "TaskOutput",
            "ToolSearch",
            "update_plan",
            "view_image",
            "wait_agent",
            "web_search",
            "WebFetch",
            "WebSearch",
        ]
        .into_iter()
        .fold(Self::default(), |effects, tool| {
            effects.declare(tool, EffectClass::None)
        })
    }

    #[must_use]
    pub fn merged(mut self, other: Self) -> Self {
        self.0.extend(other.0);
        self
    }

    #[must_use]
    pub fn declare(mut self, tool: impl Into<String>, class: EffectClass) -> Self {
        self.0.insert(tool.into(), class);
        self
    }

    pub fn insert(&mut self, tool: impl Into<String>, class: EffectClass) {
        self.0.insert(tool.into(), class);
    }

    #[must_use]
    pub fn of(&self, tool: &str) -> EffectClass {
        self.0.get(tool).copied().unwrap_or_default()
    }
}
