use super::{AgentDirectoryEntry, AgentSummary, MessageReceipt, Registry, SubagentControl};
use crate::{
    harness::{HarnessHandle, SideQuestionLease},
    model::{AgentId, MessagePriority, MessagePurpose},
    platform,
};
use nanocode_agent_core::{NanocodeError, SideAnswer, UsdAmount};
use std::sync::Arc;

struct SideQuestion {
    lease: Option<SideQuestionLease>,
    registry: Arc<Registry>,
    root_session_id: String,
}

impl Drop for SideQuestion {
    fn drop(&mut self) {
        drop(self.lease.take());
        let registry = Arc::clone(&self.registry);
        let root_session_id = std::mem::take(&mut self.root_session_id);
        drop(platform::spawn(async move {
            registry.enforce_resident_limit(&root_session_id).await;
        }));
    }
}

impl SubagentControl {
    pub async fn send_user_message(
        &self,
        root_session_id: &str,
        id: AgentId,
        priority: MessagePriority,
        body: impl Into<String>,
    ) -> std::io::Result<MessageReceipt> {
        self.require_root(root_session_id).await?;
        self.registry.await_restored(root_session_id).await?;
        self.registry
            .deliver_message(
                root_session_id,
                id,
                priority,
                MessagePurpose::Coordinate,
                None,
                body.into(),
                true,
            )
            .await
    }

    pub async fn interrupt(
        &self,
        root_session_id: &str,
        id: AgentId,
    ) -> std::io::Result<Vec<AgentSummary>> {
        self.require_root(root_session_id).await?;
        self.registry.interrupt(root_session_id, id).await
    }

    pub async fn close(
        &self,
        root_session_id: &str,
        id: AgentId,
    ) -> std::io::Result<Vec<AgentSummary>> {
        self.require_root(root_session_id).await?;
        self.registry.close(root_session_id, id).await
    }

    pub async fn set_budget(
        &self,
        root_session_id: &str,
        id: AgentId,
        budget: Option<UsdAmount>,
    ) -> std::io::Result<AgentSummary> {
        self.require_root(root_session_id).await?;
        self.registry.await_restored(root_session_id).await?;
        self.registry.set_budget(root_session_id, id, budget).await
    }

    pub async fn budgets(&self, session_id: &str) -> Vec<(AgentId, UsdAmount)> {
        self.registry.budgets(session_id).await
    }

    pub async fn agents(&self, root_session_id: &str) -> std::io::Result<Vec<AgentDirectoryEntry>> {
        self.require_root(root_session_id).await?;
        self.registry.directory(root_session_id, true, false).await
    }

    pub async fn session_id(&self, root_session_id: &str, id: AgentId) -> std::io::Result<String> {
        self.require_root(root_session_id).await?;
        self.registry.child_session_id(root_session_id, id).await
    }

    pub async fn side_question(
        &self,
        root_session_id: &str,
        id: AgentId,
        question: impl Into<String>,
    ) -> nanocode_agent_core::Result<SideAnswer> {
        let invalid = |error: std::io::Error| NanocodeError::InvalidRequest(error.to_string());
        let (harness, _side_question) = self
            .lease_side_question(root_session_id, id)
            .await
            .map_err(invalid)?;
        let agent = harness.agent().await.map_err(invalid)?;
        agent.side_question(question).await
    }

    async fn lease_side_question(
        &self,
        root_session_id: &str,
        id: AgentId,
    ) -> std::io::Result<(HarnessHandle, SideQuestion)> {
        self.require_root(root_session_id).await?;
        self.registry.await_restored(root_session_id).await?;
        let _target = self
            .registry
            .rehydrate(root_session_id, id, MessagePurpose::Coordinate)
            .await?;
        let root = root_session_id.to_owned();
        let (harness, side_lease) = self
            .registry
            .when_messages_flow(root_session_id, move |state| {
                state.harness_in_scope(&root, id).map(|harness| {
                    let lease = harness.lease_side_question();
                    (harness, lease)
                })
            })
            .await??;
        let lease = SideQuestion {
            lease: Some(side_lease),
            registry: Arc::clone(&self.registry),
            root_session_id: root_session_id.to_owned(),
        };
        Ok((harness, lease))
    }

    async fn require_root(&self, root_session_id: &str) -> std::io::Result<()> {
        if self.registry.is_root_session(root_session_id).await {
            Ok(())
        } else {
            Err(std::io::Error::other(format!(
                "{root_session_id} is not a root session with subagents"
            )))
        }
    }
}
