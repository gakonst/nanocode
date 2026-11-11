use super::{Registry, RestorationState, SubagentControl};
use crate::model::{AgentId, AgentStatus};
use nanocode_agent_core::Agent;
use serde::Serialize;
use std::sync::Arc;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Restoration {
    #[default]
    Restored,
    Restoring,
    Failed(String),
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct SubagentActivity {
    pub revision: u64,
    pub durable: bool,
    pub restoration: Restoration,
    pub active: Vec<AgentId>,
    pub resuming: Vec<AgentId>,
    pub pending_completions: Vec<(AgentId, u64)>,
    pub pending_messages: Vec<AgentId>,
    pub checkpoint_captures: usize,
}

impl SubagentActivity {
    #[must_use]
    pub const fn is_quiescent(&self) -> bool {
        !matches!(self.restoration, Restoration::Restoring)
            && self.active.is_empty()
            && self.resuming.is_empty()
            && self.pending_completions.is_empty()
            && self.pending_messages.is_empty()
            && self.checkpoint_captures == 0
    }
}

fn not_root(root_session_id: &str) -> String {
    format!("{root_session_id} is a subagent session, not a root session")
}

impl Registry {
    pub async fn activity(&self, root_session_id: &str) -> SubagentActivity {
        let revision = *self.revision.borrow();
        let durable = self.store_for(root_session_id).is_some();
        let (gate, reserved) = self.claims.read(|claims| {
            (
                claims
                    .restored
                    .get(root_session_id)
                    .map(|gate| gate.borrow().clone()),
                claims.reserved.contains_key(root_session_id),
            )
        });
        let restoration = match gate {
            None if reserved => Restoration::Restoring,
            None | Some(RestorationState::Ready) => Restoration::Restored,
            Some(RestorationState::Failed(error)) => {
                return SubagentActivity {
                    revision,
                    durable,
                    restoration: Restoration::Failed(error),
                    ..SubagentActivity::default()
                };
            }
            Some(RestorationState::Loading | RestorationState::Resuming) => Restoration::Restoring,
        };
        let resuming = self
            .pending_resume
            .read(|pending| {
                pending
                    .get(root_session_id)
                    .map(|ids| ids.iter().copied().collect())
            })
            .unwrap_or_default();
        let checkpoint_captures = self
            .scope_locks(root_session_id)
            .pending_checkpoints
            .load(std::sync::atomic::Ordering::Acquire);
        let mut activity = SubagentActivity {
            revision,
            durable,
            restoration,
            resuming,
            checkpoint_captures,
            ..SubagentActivity::default()
        };
        let root = root_session_id.to_owned();
        self.state
            .call(move |state| {
                let root_session_id = root.as_str();
                if state.root_by_session.contains_key(root_session_id) {
                    activity.restoration = Restoration::Failed(not_root(root_session_id));
                    return activity;
                }
                let Some(scope) = state.scopes.get(root_session_id) else {
                    return activity;
                };
                let mut ids = scope.sessions.keys().copied().collect::<Vec<_>>();
                ids.sort_unstable();
                for id in ids {
                    let session = &scope.sessions[&id];
                    if session.active()
                        || matches!(
                            session.status,
                            AgentStatus::Pending | AgentStatus::Running | AgentStatus::Closing
                        )
                    {
                        activity.active.push(id);
                    }
                    if session.descriptor.parent.is_none()
                        && session.status.wakes_parent()
                        && let super::Completion::Pending { revision, .. } = session.completion
                    {
                        activity.pending_completions.push((id, revision));
                    }
                    let deliverable = (session.harness().is_some()
                        || session.stored_runtime.recoverable())
                        && !matches!(session.status, AgentStatus::Closing | AgentStatus::Closed);
                    if deliverable && scope.messages.has_pending_for(id) {
                        activity.pending_messages.push(id);
                    }
                }
                activity
            })
            .await
    }

    pub async fn deliver_completions(
        self: &Arc<Self>,
        parent: &Agent,
        root_session_id: &str,
    ) -> Vec<(AgentId, std::io::Result<bool>)> {
        if self.await_restored(root_session_id).await.is_err() {
            return Vec::new();
        }
        let pending = self.activity(root_session_id).await.pending_completions;
        let mut results = Vec::with_capacity(pending.len());
        for (id, revision) in pending {
            results.push((
                id,
                self.continue_parent(parent, root_session_id, id, revision)
                    .await,
            ));
        }
        results
    }

    pub async fn await_restored_root(self: &Arc<Self>, parent: &Agent, root_session_id: &str) {
        if self.await_restored(root_session_id).await.is_ok()
            && self.needs_root_watch(root_session_id).await
        {
            self.watch_root(root_session_id, parent);
        }
    }

    pub async fn persist(&self, root_session_id: &str) -> std::io::Result<()> {
        if self.store_for(root_session_id).is_none() {
            return Ok(());
        }
        let root = root_session_id.to_owned();
        let skip = self
            .state
            .call(move |state| {
                if state.root_by_session.contains_key(&root) {
                    return Err(std::io::Error::other(not_root(&root)));
                }
                Ok(state
                    .scopes
                    .get(&root)
                    .is_none_or(|scope| scope.life.frozen()))
            })
            .await?;
        if skip {
            return Ok(());
        }
        if self.claims.read(|claims| {
            claims
                .restored
                .get(root_session_id)
                .is_some_and(|gate| matches!(*gate.borrow(), RestorationState::Failed(_)))
        }) {
            return Ok(());
        }
        self.persist_durably(root_session_id).await
    }
}

impl SubagentControl {
    pub async fn activity(&self, root_session_id: &str) -> SubagentActivity {
        self.registry.activity(root_session_id).await
    }

    pub async fn deliver_completions(
        &self,
        parent: &Agent,
        root_session_id: &str,
    ) -> Vec<(AgentId, std::io::Result<bool>)> {
        self.registry
            .deliver_completions(parent, root_session_id)
            .await
    }

    pub async fn await_restored(&self, parent: &Agent, root_session_id: &str) {
        self.registry
            .await_restored_root(parent, root_session_id)
            .await;
    }

    pub async fn persist(&self, root_session_id: &str) -> std::io::Result<()> {
        self.registry.persist(root_session_id).await
    }
}
