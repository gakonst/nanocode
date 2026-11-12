use crate::ui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::Line,
    widgets::{Block, BorderType, Borders, Clear, Widget},
};

use crate::theme::Theme;

#[must_use]
pub fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    }
}

#[must_use]
pub fn bottom(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect {
        x: area.x,
        y: area.y + area.height - height,
        width,
        height,
    }
}

pub fn frame(
    area: Rect,
    buffer: &mut Buffer,
    theme: &Theme,
    title: &str,
    footer: Option<&str>,
) -> Rect {
    Clear.render(area, buffer);
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.border()))
        .title(Line::styled(
            format!(" {title} "),
            Style::default()
                .fg(theme.accent())
                .add_modifier(Modifier::BOLD),
        ));
    if let Some(footer) = footer {
        block = block.title_bottom(Line::styled(
            format!(" {footer} "),
            Style::default().fg(theme.muted()),
        ));
    }
    let inner = block.inner(area);
    block.render(area, buffer);
    inner
}
