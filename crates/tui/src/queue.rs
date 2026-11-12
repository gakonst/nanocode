use crate::ui::{
    style::{Modifier, Style},
    text::{Line, Span},
};

use crate::{format::sanitize_terminal_text_inline, theme::Theme, unicode};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QueueKind {
    Steer,
    FollowUp,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QueueState {
    Pending,
    Sent,
    Failed,
}

#[derive(Clone, Debug)]
pub struct QueuedPrompt {
    pub id: String,
    pub kind: QueueKind,
    pub state: QueueState,
    pub text: String,
}

#[derive(Clone, Debug, Default)]
pub struct Queue {
    items: Vec<QueuedPrompt>,
    selected: Option<usize>,
}

impl Queue {
    #[must_use]
    pub fn items(&self) -> &[QueuedPrompt] {
        &self.items
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    #[must_use]
    pub const fn len(&self) -> usize {
        self.items.len()
    }

    pub fn push(&mut self, item: QueuedPrompt) {
        self.items.push(item);
    }

    pub fn set_state(&mut self, id: &str, state: QueueState) -> bool {
        self.items
            .iter_mut()
            .find(|item| item.id == id)
            .map(|item| item.state = state)
            .is_some()
    }

    pub fn remove(&mut self, id: &str) -> Option<QueuedPrompt> {
        let index = self.items.iter().position(|item| item.id == id)?;
        let removed = self.items.remove(index);
        self.clamp_selection();
        Some(removed)
    }

    pub fn remove_first(&mut self, kind: QueueKind) -> Option<QueuedPrompt> {
        let index = self.items.iter().position(|item| item.kind == kind)?;
        let removed = self.items.remove(index);
        self.clamp_selection();
        Some(removed)
    }

    pub fn drain_kind(&mut self, kind: QueueKind) -> Vec<QueuedPrompt> {
        let (drained, kept) = std::mem::take(&mut self.items)
            .into_iter()
            .partition(|item| item.kind == kind);
        self.items = kept;
        self.clamp_selection();
        drained
    }

    pub fn convert(&mut self, id: &str, kind: QueueKind) -> bool {
        self.items
            .iter_mut()
            .find(|item| item.id == id)
            .map(|item| {
                item.kind = kind;
                item.state = QueueState::Pending;
            })
            .is_some()
    }

    #[must_use]
    pub fn selected(&self) -> Option<&QueuedPrompt> {
        self.selected.and_then(|index| self.items.get(index))
    }

    pub const fn select_previous(&mut self) -> bool {
        if self.items.is_empty() {
            return false;
        }
        self.selected = Some(match self.selected {
            None => self.items.len() - 1,
            Some(index) => index.saturating_sub(1),
        });
        true
    }

    pub const fn select_next(&mut self) -> bool {
        match self.selected {
            Some(index) if index + 1 < self.items.len() => {
                self.selected = Some(index + 1);
                true
            }
            Some(_) => {
                self.selected = None;
                true
            }
            None => false,
        }
    }

    pub const fn clear_selection(&mut self) {
        self.selected = None;
    }

    fn clamp_selection(&mut self) {
        if self.items.is_empty() {
            self.selected = None;
        } else if let Some(index) = self.selected {
            self.selected = Some(index.min(self.items.len() - 1));
        }
    }

    #[must_use]
    pub fn lines(&self, width: u16, theme: &Theme) -> Vec<Line<'static>> {
        let _span =
            tracing::info_span!("tui.queue.layout", width, items = self.items.len()).entered();
        self.items
            .iter()
            .enumerate()
            .map(|(index, item)| {
                let (label, color) = match (item.kind, item.state) {
                    (_, QueueState::Failed) => ("failed", theme.thinking_max()),
                    (QueueKind::Steer, QueueState::Sent) => ("steering", theme.accent()),
                    (QueueKind::Steer, QueueState::Pending) => ("steer", theme.accent()),
                    (QueueKind::FollowUp, _) => ("queued", theme.muted()),
                };
                let selected = self.selected == Some(index);
                let marker = if selected { "› " } else { "↳ " };
                let prefix = format!("{marker}{label}: ");
                let text = sanitize_terminal_text_inline(&item.text).into_owned();
                let available = usize::from(width).saturating_sub(unicode::width(&prefix));
                let text = truncate(&text, available);
                let mut text_style = Style::default().fg(theme.muted());
                if selected {
                    text_style = text_style.fg(theme.text()).add_modifier(Modifier::BOLD);
                }
                Line::from(vec![
                    Span::styled(prefix, Style::default().fg(color)),
                    Span::styled(text, text_style),
                ])
            })
            .collect()
    }
}

fn truncate(text: &str, width: usize) -> String {
    if unicode::width(text) <= width {
        return text.to_owned();
    }
    let mut result = String::new();
    let mut used = 0;
    for grapheme in unicode::graphemes(text) {
        let grapheme_width = unicode::width(grapheme);
        if used + grapheme_width + 1 > width {
            break;
        }
        used += grapheme_width;
        result.push_str(grapheme);
    }
    result.push('…');
    result
}
