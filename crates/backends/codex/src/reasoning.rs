use crate::client::Thinking;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
pub(crate) struct ReasoningState {
    pin: ReasoningPin,
    latest_override: Option<AuthoredOverride>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
struct AuthoredOverride {
    history_index: usize,
    effort: Thinking,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
enum ReasoningPin {
    #[default]
    Unset,
    Compacted,
    Active {
        model: String,
        effort: Thinking,
    },
}

impl ReasoningState {
    pub(crate) fn validate(
        &self,
        history: &[crate::client::responses::ResponseItem],
    ) -> crate::Result<()> {
        use crate::client::{Model, responses::ResponseItem};
        let invalid = |detail: &str| crate::NanocodeError::InvalidCheckpoint(detail.to_owned());
        let ReasoningPin::Active { model, effort } = &self.pin else {
            return if self.latest_override.is_none() {
                Ok(())
            } else {
                Err(invalid("reasoning override has no active baseline"))
            };
        };
        let model = model
            .parse::<Model>()
            .map_err(|_| invalid("reasoning baseline model is unsupported"))?;
        if !model.supports_reasoning_effort_updates() || !model.supports_thinking(*effort) {
            return Err(invalid("reasoning baseline effort is unsupported"));
        }
        if let Some(authored) = &self.latest_override
            && (!model.supports_thinking(authored.effort)
                || !matches!(history.get(authored.history_index),
                    Some(ResponseItem::ConfigurationUpdate { reasoning })
                        if reasoning.effort == authored.effort))
        {
            return Err(invalid(
                "authored reasoning override does not match retained history",
            ));
        }
        Ok(())
    }

    pub(crate) fn reconcile_history_repair<'a, 'b>(
        &mut self,
        before: impl Iterator<Item = &'a crate::client::responses::ResponseItem>,
        after: impl Iterator<Item = &'b crate::client::responses::ResponseItem>,
    ) {
        use crate::client::responses::ResponseItem;
        let Some(authored) = &mut self.latest_override else {
            return;
        };
        let ordinal = before
            .take(authored.history_index)
            .filter(|item| matches!(item, ResponseItem::ConfigurationUpdate { .. }))
            .count();
        if let Some((index, _)) = after
            .enumerate()
            .filter(|(_, item)| matches!(item, ResponseItem::ConfigurationUpdate { .. }))
            .nth(ordinal)
        {
            authored.history_index = index;
        }
    }

    pub(crate) fn request_effort(
        &self,
        model: crate::client::Model,
        selected: Thinking,
        supported: bool,
    ) -> Thinking {
        if supported
            && let ReasoningPin::Active {
                model: pinned_model,
                effort,
            } = &self.pin
            && pinned_model == model.as_str()
        {
            return *effort;
        }
        selected
    }

    /// The effort the context's next response runs at without a new update: its latest
    /// authored update, else its baseline, else the selection.
    pub(crate) fn effective_effort(
        &self,
        model: crate::client::Model,
        selected: Thinking,
        supported: bool,
    ) -> Thinking {
        if supported
            && let ReasoningPin::Active {
                model: pinned_model,
                effort,
            } = &self.pin
            && pinned_model == model.as_str()
        {
            return self
                .latest_override
                .as_ref()
                .map_or(*effort, |authored| authored.effort);
        }
        selected
    }

    pub(crate) fn pin(
        &mut self,
        model: crate::client::Model,
        selected: Thinking,
        supported: bool,
    ) -> Thinking {
        if !supported {
            *self = Self::default();
            return selected;
        }
        if let ReasoningPin::Active {
            model: pinned_model,
            effort,
        } = &self.pin
            && pinned_model == model.as_str()
        {
            return *effort;
        }
        self.pin = ReasoningPin::Active {
            model: model.as_str().to_owned(),
            effort: selected,
        };
        self.latest_override = None;
        selected
    }

    pub(crate) fn sampling_update(
        &mut self,
        model: crate::client::Model,
        selected: Thinking,
        supported: bool,
        history_len: usize,
    ) -> Option<crate::client::responses::ResponseItem> {
        if !supported {
            *self = Self::default();
            return None;
        }
        if matches!(self.pin, ReasoningPin::Compacted) {
            self.pin(model, selected, supported);
            return None;
        }
        let established = match &self.pin {
            ReasoningPin::Active {
                model: pinned_model,
                effort,
            } if pinned_model == model.as_str() => Some(
                self.latest_override
                    .as_ref()
                    .map_or(*effort, |authored| authored.effort),
            ),
            _ => None,
        };
        self.pin(model, selected, supported);
        if established == Some(selected) {
            return None;
        }
        self.latest_override = Some(AuthoredOverride {
            history_index: history_len,
            effort: selected,
        });
        Some(crate::client::responses::ResponseItem::configuration_update(selected))
    }

    pub(crate) fn compacted(&mut self) {
        self.pin = ReasoningPin::Compacted;
        self.latest_override = None;
    }
}
