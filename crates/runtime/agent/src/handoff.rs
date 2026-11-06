use std::{error::Error as StdError, fmt};

use crate::{
    Agent, AgentRuntime, NanocodeError, PromptRequest,
    durability::{CancelTo, EncodedPayload, Error as DurabilityError, SessionStore},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Unresumable {
    ChangedRequest,
    Undecodable,
    Missing,
}

impl Unresumable {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ChangedRequest => "changed_request",
            Self::Undecodable => "undecodable",
            Self::Missing => "missing",
        }
    }

    #[must_use]
    pub fn of(error: &(dyn StdError + 'static)) -> Option<Self> {
        if chain(error).any(|cause| {
            matches!(
                cause.downcast_ref::<DurabilityError>(),
                Some(DurabilityError::StepChanged { .. })
            )
        }) {
            return Some(Self::ChangedRequest);
        }
        chain(error).find_map(|cause| {
            if let Some(missing) = cause.downcast_ref::<Missing>() {
                return Some(missing.unresumable());
            }
            match cause.downcast_ref::<NanocodeError>()? {
                NanocodeError::InvalidCheckpoint(_) => Some(Self::Undecodable),
                NanocodeError::ExecutionPolicy { layer, .. } if layer.ends_with("recovery") => {
                    Some(Self::Undecodable)
                }
                _ => None,
            }
        })
    }
}

fn chain<'a>(
    error: &'a (dyn StdError + 'static),
) -> impl Iterator<Item = &'a (dyn StdError + 'static)> {
    std::iter::successors(Some(error), |&error: &&'a (dyn StdError + 'static)| {
        error.source()
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Missing {
    Prompt,
    Session,
}

impl Missing {
    const fn unresumable(self) -> Unresumable {
        Unresumable::Missing
    }
}

impl fmt::Display for Missing {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Prompt => "the handed-off pass's prompt is not stored",
            Self::Session => "the handed-off turn's session is not stored",
        })
    }
}

impl StdError for Missing {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Base {
    Latest,
    To(String),
    Fresh,
}

impl Base {
    #[must_use]
    pub fn of(base: Option<&str>) -> Self {
        match base {
            None => Self::Latest,
            Some("") => Self::Fresh,
            Some(base) => Self::To(base.to_owned()),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Settled {
    pub cancelled: usize,
    pub base: Option<String>,
    pub unresolved: Option<String>,
}

#[derive(Debug)]
pub enum ResumeError {
    Missing(Missing),
    Agent(NanocodeError),
}

impl fmt::Display for ResumeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing(missing) => missing.fmt(formatter),
            Self::Agent(error) => error.fmt(formatter),
        }
    }
}

impl StdError for ResumeError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            Self::Missing(missing) => Some(missing),
            Self::Agent(error) => Some(error),
        }
    }
}

pub async fn settle_pending(
    sessions: &SessionStore,
    id: &str,
    to: &Base,
) -> Result<Option<Settled>, DurabilityError> {
    let to = match to {
        Base::Latest => CancelTo::Latest,
        Base::To(key) => CancelTo::Checkpoint(EncodedPayload::from_key(key)),
        Base::Fresh => CancelTo::Fresh,
    };
    Ok(sessions.cancel_pending(id, to).await?.map(|settled| Settled {
        cancelled: settled.cancelled,
        base: settled
            .checkpoint
            .map(|checkpoint| checkpoint.key().to_owned()),
        unresolved: settled.unresolved,
    }))
}

pub async fn base_checkpoint(sessions: &SessionStore, id: &str) -> Result<String, DurabilityError> {
    if sessions.record(id).await?.is_none() {
        return Ok(String::new());
    }
    let latest = sessions.resume(id).await?.latest_checkpoint().await?;
    Ok(latest.map_or_else(String::new, |checkpoint| checkpoint.key().to_owned()))
}

impl AgentRuntime {
    pub async fn resumed_pass(
        &self,
        agent: &Agent,
        operation: &str,
    ) -> Result<PromptRequest, ResumeError> {
        let stored = self
            .durable_prompt(agent, operation)
            .await
            .map_err(ResumeError::Agent)?
            .ok_or(ResumeError::Missing(Missing::Prompt))?;
        Ok(PromptRequest::new(stored).request_id(operation))
    }

    pub async fn persist_subagents(&self, agent: &Agent) -> Option<std::io::Result<()>> {
        Some(self.subagents()?.persist(agent.session_id()).await)
    }
}
