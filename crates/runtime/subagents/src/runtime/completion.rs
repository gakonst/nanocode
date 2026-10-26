use super::{AgentStatus, SubagentControl, completion_continuation_prompt};
use crate::model::AgentId;
use std::time::Duration;

#[derive(Clone)]
pub struct RunningSubagents {
    control: SubagentControl,
    root_session_id: String,
    ids: Vec<AgentId>,
}

#[derive(Clone, Debug)]
pub struct SubagentCompletion {
    pub agent_id: AgentId,
    pub status: AgentStatus,
    pub prompt: String,
}

impl SubagentControl {
    pub async fn running(&self, root_session_id: &str) -> Option<RunningSubagents> {
        let ids: Vec<_> = self
            .agents(root_session_id)
            .await
            .ok()?
            .into_iter()
            .filter(|entry| entry.status.is_active())
            .map(|entry| entry.agent_id)
            .collect();
        (!ids.is_empty()).then(|| RunningSubagents {
            control: self.clone(),
            root_session_id: root_session_id.to_owned(),
            ids,
        })
    }

    pub async fn stop_all(&self, root_session_id: &str, timeout: Duration) -> Option<bool> {
        if !self
            .agents(root_session_id)
            .await
            .is_ok_and(|agents| agents.iter().any(|entry| entry.status.is_active()))
        {
            return None;
        }
        Some(
            nanocode_platform::timeout(timeout, self.cancel_all(root_session_id))
                .await
                .is_ok(),
        )
    }
}

impl RunningSubagents {
    pub fn ids(&self) -> &[AgentId] {
        &self.ids
    }

    pub async fn next_completion(&self, timeout: Duration) -> Option<SubagentCompletion> {
        let mut changes = self.control.registry.revision.subscribe();
        let next = async {
            loop {
                let agents = self.control.agents(&self.root_session_id).await.ok()?;
                let mut active = false;
                for entry in agents
                    .into_iter()
                    .filter(|entry| self.ids.contains(&entry.agent_id))
                {
                    match entry.status {
                        status if status.is_active() => active = true,
                        AgentStatus::Closed => {}
                        status => {
                            return Some(SubagentCompletion {
                                agent_id: entry.agent_id,
                                prompt: completion_continuation_prompt(entry.agent_id, &status),
                                status,
                            });
                        }
                    }
                }
                if !active {
                    return None;
                }
                changes.changed().await.ok()?;
            }
        };
        nanocode_platform::timeout(timeout, next)
            .await
            .ok()
            .flatten()
    }
}
