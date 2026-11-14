use crate::ui::{
    style::{Modifier, Style},
    text::{Line, Span},
};

use crate::{format::sanitize_terminal_text_inline, spinner::SPINNER_SLOT, theme::Theme};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NodeStatus {
    Pending,
    Running,
    Idle,
    Completed,
    Failed,
    Closed,
    Unrecoverable,
}

impl NodeStatus {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Idle => "idle",
            Self::Completed => "done",
            Self::Failed => "failed",
            Self::Closed => "closed",
            Self::Unrecoverable => "interrupted, not recoverable",
        }
    }

    #[must_use]
    pub const fn is_active(self) -> bool {
        matches!(self, Self::Pending | Self::Running)
    }

    #[must_use]
    pub const fn is_listed(self) -> bool {
        matches!(self, Self::Pending | Self::Running | Self::Idle)
    }
}

#[derive(Clone, Debug)]
pub struct TreeNode {
    pub id: String,
    pub parent: Option<String>,
    pub label: String,
    pub detail: String,
    pub status: NodeStatus,
}

#[derive(Clone, Debug, Default)]
pub struct AgentTree {
    nodes: Vec<TreeNode>,
}

impl AgentTree {
    #[must_use]
    pub fn nodes(&self) -> &[TreeNode] {
        &self.nodes
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    #[must_use]
    pub fn listed(&self) -> usize {
        self.nodes
            .iter()
            .filter(|node| node.status.is_listed())
            .count()
    }

    #[must_use]
    pub fn finished(&self) -> usize {
        self.nodes.len() - self.listed()
    }

    pub fn upsert(&mut self, node: TreeNode) {
        match self
            .nodes
            .iter_mut()
            .find(|existing| existing.id == node.id)
        {
            Some(existing) => *existing = node,
            None => self.nodes.push(node),
        }
    }

    pub fn retain(&mut self, keep: impl FnMut(&TreeNode) -> bool) {
        self.nodes.retain(keep);
    }

    pub fn update(&mut self, id: &str, update: impl FnOnce(&mut TreeNode)) -> bool {
        self.nodes
            .iter_mut()
            .find(|node| node.id == id)
            .map(update)
            .is_some()
    }

    #[must_use]
    pub fn active(&self) -> usize {
        self.nodes
            .iter()
            .filter(|node| node.status.is_active())
            .count()
    }

    #[must_use]
    pub fn ordered(&self) -> Vec<(usize, Vec<bool>, &TreeNode)> {
        let mut ordered = Vec::new();
        let roots =
            self.nodes
                .iter()
                .filter(|node| node.status.is_listed())
                .filter(|node| {
                    node.parent.as_ref().is_none_or(|parent| {
                        !self.nodes.iter().any(|candidate| {
                            &candidate.id == parent && candidate.status.is_listed()
                        })
                    })
                })
                .collect::<Vec<_>>();
        let count = roots.len();
        for (index, root) in roots.into_iter().enumerate() {
            self.visit(root, Vec::new(), index + 1 == count, &mut ordered);
        }
        ordered
    }

    fn visit<'a>(
        &'a self,
        node: &'a TreeNode,
        ancestors: Vec<bool>,
        last: bool,
        ordered: &mut Vec<(usize, Vec<bool>, &'a TreeNode)>,
    ) {
        let mut guides = ancestors.clone();
        guides.push(last);
        ordered.push((ancestors.len(), guides.clone(), node));
        let children = self
            .nodes
            .iter()
            .filter(|child| {
                child.status.is_listed() && child.parent.as_deref() == Some(node.id.as_str())
            })
            .collect::<Vec<_>>();
        let count = children.len();
        for (index, child) in children.into_iter().enumerate() {
            if ordered.iter().any(|(_, _, seen)| seen.id == child.id) {
                continue;
            }
            self.visit(child, guides.clone(), index + 1 == count, ordered);
        }
    }

    #[must_use]
    pub fn lines(&self, theme: &Theme, selected: Option<usize>) -> Vec<Line<'static>> {
        let _span = nanocode_trace::info_span!("tui.tree.layout", nodes = self.nodes.len()).entered();
        self.ordered()
            .into_iter()
            .enumerate()
            .map(|(row, (_, guides, node))| {
                let mut prefix = String::new();
                for (depth, last) in guides.iter().enumerate() {
                    if depth + 1 == guides.len() {
                        prefix.push_str(if *last { "└─ " } else { "├─ " });
                    } else {
                        prefix.push_str(if *last { "   " } else { "│  " });
                    }
                }
                let (icon, color) = match node.status {
                    NodeStatus::Pending | NodeStatus::Running => (SPINNER_SLOT, theme.accent()),
                    NodeStatus::Idle => ("○", theme.muted()),
                    NodeStatus::Completed => ("✓", theme.thinking_low()),
                    NodeStatus::Failed | NodeStatus::Unrecoverable => ("✗", theme.thinking_max()),
                    NodeStatus::Closed => ("·", theme.muted()),
                };
                let mut label_style = Style::default().fg(theme.text());
                if selected == Some(row) {
                    label_style = label_style.add_modifier(Modifier::REVERSED);
                }
                let mut spans = vec![
                    Span::styled(prefix, Style::default().fg(theme.border())),
                    Span::styled(icon, Style::default().fg(color)),
                    Span::styled(" ", Style::default().fg(color)),
                    Span::styled(
                        sanitize_terminal_text_inline(&node.label).into_owned(),
                        label_style,
                    ),
                    Span::styled(
                        format!("  {}", node.status.label()),
                        Style::default().fg(color),
                    ),
                ];
                if !node.detail.is_empty() {
                    spans.push(Span::styled(
                        format!("  {}", sanitize_terminal_text_inline(&node.detail)),
                        Style::default().fg(theme.muted()),
                    ));
                }
                Line::from(spans)
            })
            .collect()
    }
}
