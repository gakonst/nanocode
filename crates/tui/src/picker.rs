use crate::input::{KeyCode, KeyEvent, KeyModifiers};
use crate::ui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Widget},
};

use crate::{overlay, theme::Theme, unicode};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ItemKind {
    Header,
    Choice,
}

#[derive(Clone, Debug)]
pub struct PickerItem {
    pub kind: ItemKind,
    pub label: String,
    pub detail: String,
    pub badge: Option<String>,
    pub current: bool,
}

impl PickerItem {
    #[must_use]
    pub fn header(label: impl Into<String>) -> Self {
        Self {
            kind: ItemKind::Header,
            label: label.into(),
            detail: String::new(),
            badge: None,
            current: false,
        }
    }

    #[must_use]
    pub fn choice(label: impl Into<String>) -> Self {
        Self {
            kind: ItemKind::Choice,
            label: label.into(),
            detail: String::new(),
            badge: None,
            current: false,
        }
    }

    #[must_use]
    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = detail.into();
        self
    }

    #[must_use]
    pub fn badge(mut self, badge: impl Into<String>) -> Self {
        self.badge = Some(badge.into());
        self
    }

    #[must_use]
    pub const fn current(mut self, current: bool) -> Self {
        self.current = current;
        self
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PickerAction {
    Ignored,
    Changed,
    Selected(usize),
    Cancelled,
}

fn columns(items: &[PickerItem], shown: &[usize]) -> Vec<(usize, usize)> {
    let mut widths = vec![(0, 0); shown.len()];
    let mut start = 0;
    for end in 1..=shown.len() {
        if end < shown.len() && items[shown[end]].kind != ItemKind::Header {
            continue;
        }
        let choices = shown[start..end]
            .iter()
            .map(|index| &items[*index])
            .filter(|item| item.kind == ItemKind::Choice);
        let label = choices
            .clone()
            .map(|item| unicode::width(&item.label))
            .max()
            .unwrap_or(0);
        let badge = choices
            .map(|item| item.badge.as_ref().map_or(0, |badge| unicode::width(badge)))
            .max()
            .unwrap_or(0);
        widths[start..end].fill((label, badge));
        start = end;
    }
    widths
}

#[derive(Clone, Debug)]
pub struct Picker {
    title: String,
    footer: Option<String>,
    items: Vec<PickerItem>,
    filterable: bool,
    query: String,
    visible: Vec<usize>,
    selected: Option<usize>,
    offset: usize,
}

impl Picker {
    #[must_use]
    pub fn new(title: impl Into<String>, items: Vec<PickerItem>) -> Self {
        let mut picker = Self {
            title: title.into(),
            footer: None,
            items,
            filterable: false,
            query: String::new(),
            visible: Vec::new(),
            selected: None,
            offset: 0,
        };
        picker.refilter();
        let current = picker.visible.iter().position(|index| {
            picker.items[*index].current && picker.items[*index].kind == ItemKind::Choice
        });
        if current.is_some() {
            picker.selected = current;
        }
        picker
    }

    #[must_use]
    pub const fn filterable(mut self) -> Self {
        self.filterable = true;
        self
    }

    #[must_use]
    pub fn footer(mut self, footer: impl Into<String>) -> Self {
        self.footer = Some(footer.into());
        self
    }

    #[must_use]
    pub fn items(&self) -> &[PickerItem] {
        &self.items
    }

    #[must_use]
    pub fn query(&self) -> &str {
        &self.query
    }

    #[must_use]
    pub fn selected(&self) -> Option<usize> {
        self.selected.map(|position| self.visible[position])
    }

    fn refilter(&mut self) {
        let query = self.query.to_lowercase();
        let matches = |item: &PickerItem| {
            query.is_empty()
                || item.label.to_lowercase().contains(&query)
                || item.detail.to_lowercase().contains(&query)
        };
        let mut visible = Vec::new();
        let mut header = None;
        let mut grouped = false;
        for (index, item) in self.items.iter().enumerate() {
            match item.kind {
                ItemKind::Header => {
                    header = Some(index);
                    grouped = !query.is_empty() && item.label.to_lowercase().contains(&query);
                }
                ItemKind::Choice if grouped || matches(item) => {
                    if let Some(header) = header.take() {
                        visible.push(header);
                    }
                    visible.push(index);
                }
                ItemKind::Choice => {}
            }
        }
        self.visible = visible;
        self.selected = self
            .visible
            .iter()
            .position(|index| self.items[*index].kind == ItemKind::Choice);
        self.offset = 0;
    }

    fn step(&mut self, forward: bool, count: usize) -> PickerAction {
        let Some(mut position) = self.selected else {
            return PickerAction::Ignored;
        };
        let choices = self
            .visible
            .iter()
            .enumerate()
            .filter(|(_, index)| self.items[**index].kind == ItemKind::Choice)
            .map(|(position, _)| position)
            .collect::<Vec<_>>();
        let current = choices
            .iter()
            .position(|choice| *choice == position)
            .unwrap_or(0);
        let next = if forward {
            (current + count).min(choices.len() - 1)
        } else {
            current.saturating_sub(count)
        };
        position = choices[next];
        self.selected = Some(position);
        PickerAction::Changed
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> PickerAction {
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => PickerAction::Cancelled,
            KeyCode::Char('c') if control => PickerAction::Cancelled,
            KeyCode::Enter => self
                .selected()
                .map_or(PickerAction::Ignored, PickerAction::Selected),
            KeyCode::Up => self.step(false, 1),
            KeyCode::Down | KeyCode::Tab => self.step(true, 1),
            KeyCode::Char('p') if control => self.step(false, 1),
            KeyCode::Char('n') if control => self.step(true, 1),
            KeyCode::Char('k') if !self.filterable => self.step(false, 1),
            KeyCode::Char('j') if !self.filterable => self.step(true, 1),
            KeyCode::PageUp => self.step(false, 10),
            KeyCode::PageDown => self.step(true, 10),
            KeyCode::Home => self.step(false, usize::MAX),
            KeyCode::End => self.step(true, usize::MAX),
            KeyCode::Backspace if self.filterable => {
                if self.query.pop().is_none() {
                    return PickerAction::Ignored;
                }
                self.refilter();
                PickerAction::Changed
            }
            KeyCode::Char(character) if self.filterable && !control => {
                self.query.push(character);
                self.refilter();
                PickerAction::Changed
            }
            _ => PickerAction::Ignored,
        }
    }

    #[must_use]
    pub fn desired_size(&self, area: Rect) -> (u16, u16) {
        let all = (0..self.items.len()).collect::<Vec<_>>();
        let widest = self
            .items
            .iter()
            .zip(columns(&self.items, &all))
            .map(|(item, (label, badge))| match item.kind {
                ItemKind::Header => unicode::width(&item.label),
                ItemKind::Choice => {
                    label
                        + 4
                        + if badge == 0 { 0 } else { badge + 2 }
                        + if item.detail.is_empty() {
                            0
                        } else {
                            unicode::width(&item.detail) + 2
                        }
                }
            })
            .max()
            .unwrap_or(0)
            .max(unicode::width(&self.title) + 4)
            .max(
                self.footer
                    .as_ref()
                    .map_or(0, |footer| unicode::width(footer) + 4),
            );
        let width = u16::try_from(widest + 8)
            .unwrap_or(u16::MAX)
            .min(area.width.saturating_sub(4));
        let rows = self.visible.len().max(1) + usize::from(self.filterable) + 2;
        let height = u16::try_from(rows)
            .unwrap_or(u16::MAX)
            .min(area.height.saturating_sub(2));
        (width, height)
    }

    pub fn render(&mut self, area: Rect, buffer: &mut Buffer, theme: &Theme) {
        let _span = nanocode_trace::info_span!(
            "tui.picker.render",
            width = area.width,
            height = area.height,
            items = self.items.len()
        )
        .entered();
        let (width, height) = self.desired_size(area);
        let popup = overlay::centered(area, width, height);
        let inner = overlay::frame(popup, buffer, theme, &self.title, self.footer.as_deref());
        if inner.height == 0 {
            return;
        }
        let mut lines = Vec::new();
        let mut list_height = usize::from(inner.height);
        if self.filterable {
            lines.push(Line::from(vec![
                Span::styled("filter: ", Style::default().fg(theme.muted())),
                Span::styled(self.query.clone(), Style::default().fg(theme.text())),
                Span::styled("▏", Style::default().fg(theme.accent())),
            ]));
            list_height = list_height.saturating_sub(1);
        }
        if let Some(selected) = self.selected {
            if selected < self.offset {
                self.offset = selected;
            } else if list_height > 0 && selected >= self.offset + list_height {
                self.offset = selected + 1 - list_height;
            }
        }
        if self.visible.is_empty() {
            lines.push(Line::styled(
                "  no matches",
                Style::default().fg(theme.muted()),
            ));
        }
        let widths = columns(&self.items, &self.visible);
        for (position, index) in self
            .visible
            .iter()
            .enumerate()
            .skip(self.offset)
            .take(list_height)
        {
            let item = &self.items[*index];
            let selected = self.selected == Some(position);
            let line = match item.kind {
                ItemKind::Header => Line::styled(
                    item.label.clone(),
                    Style::default()
                        .fg(theme.muted())
                        .add_modifier(Modifier::BOLD),
                ),
                ItemKind::Choice => {
                    let marker = if selected { "› " } else { "  " };
                    let mut label_style = Style::default().fg(theme.text());
                    if selected {
                        label_style = label_style.fg(theme.accent()).add_modifier(Modifier::BOLD);
                    }
                    let (label_width, badge_width) = widths[position];
                    let padding = " ".repeat(label_width - unicode::width(&item.label));
                    let mut spans = vec![
                        Span::styled(marker, Style::default().fg(theme.accent())),
                        Span::styled(item.label.clone(), label_style),
                        Span::styled(
                            if item.current { " ✓" } else { "  " },
                            Style::default().fg(theme.accent()),
                        ),
                        Span::raw(padding),
                    ];
                    if badge_width > 0 {
                        let badge = item.badge.clone().unwrap_or_default();
                        let padding = " ".repeat(badge_width - unicode::width(&badge));
                        spans.push(Span::styled(
                            format!("  {badge}{padding}"),
                            Style::default().fg(theme.thinking_high()),
                        ));
                    }
                    if !item.detail.is_empty() {
                        spans.push(Span::styled(
                            format!("  {}", item.detail),
                            Style::default().fg(theme.muted()),
                        ));
                    }
                    Line::from(spans)
                }
            };
            lines.push(line);
        }
        Paragraph::new(lines).render(inner, buffer);
    }
}
