use crate::ui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Widget},
};

use crate::{overlay, theme::Theme, unicode};

#[derive(Clone, Copy, Debug)]
pub struct Binding {
    pub keys: &'static str,
    pub action: &'static str,
}

pub fn render(bindings: &[Binding], area: Rect, buffer: &mut Buffer, theme: &Theme) {
    let _span = nanocode_trace::info_span!(
        "tui.help.render",
        width = area.width,
        height = area.height,
        bindings = bindings.len()
    )
    .entered();
    let keys_width = bindings
        .iter()
        .map(|binding| unicode::width(binding.keys))
        .max()
        .unwrap_or(0);
    let action_width = bindings
        .iter()
        .map(|binding| unicode::width(binding.action))
        .max()
        .unwrap_or(0);
    let width = u16::try_from(keys_width + action_width + 7).unwrap_or(u16::MAX);
    let height = u16::try_from(bindings.len() + 2).unwrap_or(u16::MAX);
    let popup = overlay::centered(area, width, height);
    let inner = overlay::frame(popup, buffer, theme, "Keys", Some("esc to close"));
    let lines = bindings
        .iter()
        .map(|binding| {
            Line::from(vec![
                Span::styled(
                    format!(" {:width$}  ", binding.keys, width = keys_width),
                    Style::default()
                        .fg(theme.accent())
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(binding.action, Style::default().fg(theme.text())),
            ])
        })
        .collect::<Vec<_>>();
    Paragraph::new(lines).render(inner, buffer);
}
