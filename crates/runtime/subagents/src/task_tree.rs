use super::model::AgentId;
use std::collections::{BTreeSet, HashMap};

#[derive(Default)]
pub(super) struct TaskTree {
    next_id: u64,
    nodes: HashMap<AgentId, TaskNode>,
    sessions: HashMap<String, AgentId>,
    children: HashMap<Option<AgentId>, BTreeSet<AgentId>>,
}

struct TaskNode {
    parent: Option<AgentId>,
}

impl TaskTree {
    pub(super) fn reserve(&mut self, parent: Option<AgentId>) -> std::io::Result<AgentId> {
        if let Some(parent) = parent
            && !self.nodes.contains_key(&parent)
        {
            return Err(std::io::Error::other(format!(
                "unknown parent_agent_id {parent}"
            )));
        }

        Ok(AgentId::next(&mut self.next_id))
    }

    pub(super) fn insert(
        &mut self,
        id: AgentId,
        session_id: String,
        parent: Option<AgentId>,
    ) -> std::io::Result<()> {
        if id.get() == 0 {
            return Err(std::io::Error::other("agent ID must be greater than zero"));
        }
        if id.get() == u64::MAX {
            return Err(std::io::Error::other(
                "agent ID must be less than the maximum u64 value",
            ));
        }
        if self.nodes.contains_key(&id) {
            return Err(std::io::Error::other(format!("duplicate agent_id {id}")));
        }
        if self.sessions.contains_key(&session_id) {
            return Err(std::io::Error::other(format!(
                "duplicate subagent session ID {session_id}"
            )));
        }
        if let Some(parent) = parent
            && !self.nodes.contains_key(&parent)
        {
            return Err(std::io::Error::other(format!(
                "unknown parent agent {parent}"
            )));
        }

        self.nodes.insert(id, TaskNode { parent });
        self.sessions.insert(session_id, id);
        self.children.entry(parent).or_default().insert(id);
        self.next_id = self.next_id.max(id.get());
        Ok(())
    }

    pub(super) fn contains(&self, id: AgentId) -> bool {
        self.nodes.contains_key(&id)
    }

    pub(super) fn agent_for_session(&self, session_id: &str) -> Option<AgentId> {
        self.sessions.get(session_id).copied()
    }

    pub(super) fn parent(&self, id: AgentId) -> Option<AgentId> {
        self.nodes.get(&id).and_then(|node| node.parent)
    }

    pub(super) fn authorize(&self, session_id: &str, id: AgentId) -> std::io::Result<()> {
        if !self.contains(id) {
            return Err(std::io::Error::other(format!("unknown agent_id {id}")));
        }

        if let Some(caller) = self.agent_for_session(session_id)
            && !self.is_descendant(id, caller)
        {
            return Err(std::io::Error::other(format!(
                "agent {caller} may only manage its descendants"
            )));
        }

        Ok(())
    }

    pub(super) fn ids(&self) -> Vec<AgentId> {
        self.nodes.keys().copied().collect()
    }

    pub(super) fn has_children(&self, id: AgentId) -> bool {
        self.children
            .get(&Some(id))
            .is_some_and(|children| !children.is_empty())
    }

    pub(super) fn subtree_postorder(&self, id: AgentId) -> std::io::Result<Vec<AgentId>> {
        if !self.contains(id) {
            return Err(std::io::Error::other(format!("unknown agent_id {id}")));
        }

        let mut order = Vec::new();
        self.append_subtree_postorder(id, &mut order);
        Ok(order)
    }

    pub(super) fn all_postorder(&self) -> Vec<AgentId> {
        let mut order = Vec::with_capacity(self.nodes.len());
        for &root in self.children.get(&None).into_iter().flatten() {
            self.append_subtree_postorder(root, &mut order);
        }
        order
    }

    pub(super) fn is_descendant(&self, candidate: AgentId, ancestor: AgentId) -> bool {
        let mut parent = self.parent(candidate);
        while let Some(id) = parent {
            if id == ancestor {
                return true;
            }
            parent = self.parent(id);
        }
        false
    }

    fn append_subtree_postorder(&self, id: AgentId, order: &mut Vec<AgentId>) {
        for &child in self.children.get(&Some(id)).into_iter().flatten() {
            self.append_subtree_postorder(child, order);
        }
        order.push(id);
    }
}
