use std::f64::consts::{FRAC_PI_2, TAU};

use crate::input::{KeyCode, KeyEvent, KeyModifiers};
use crate::ui::{
    buffer::Buffer,
    layout::{Alignment, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Widget},
};

use crate::{overlay, theme::Theme, unicode};

const DIAL_WIDTH: u16 = 17;
const DIAL_HEIGHT: u16 = 9;
const DIAL_SAMPLES: usize = 96;
const FILLER_DOT: &str = "•";
const THICK_DOT: &str = "●";
const HINT: &str = "←/→ change · enter apply · esc cancel";

#[derive(Clone, Debug)]
pub struct Level {
    pub label: String,
    pub color: Color,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DialAction {
    Ignored,
    Changed,
    Selected(usize),
    Cancelled,
}

#[derive(Clone, Debug)]
pub struct EffortDial {
    title: String,
    levels: Vec<Level>,
    selected: usize,
}

impl EffortDial {
    #[must_use]
    pub fn new(title: impl Into<String>, levels: Vec<Level>, selected: usize) -> Option<Self> {
        (selected < levels.len()).then(|| Self {
            title: title.into(),
            levels,
            selected,
        })
    }

    #[must_use]
    pub const fn selected(&self) -> usize {
        self.selected
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> DialAction {
        let count = self.levels.len();
        match key.code {
            KeyCode::Esc => DialAction::Cancelled,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                DialAction::Cancelled
            }
            KeyCode::Enter => DialAction::Selected(self.selected),
            KeyCode::Left | KeyCode::Down | KeyCode::Char('h' | 'j') => {
                self.selected = (self.selected + count - 1) % count;
                DialAction::Changed
            }
            KeyCode::Right | KeyCode::Up | KeyCode::Tab | KeyCode::Char('l' | 'k') => {
                self.selected = (self.selected + 1) % count;
                DialAction::Changed
            }
            KeyCode::Char(digit @ '1'..='9') => {
                let index = usize::from(digit as u8 - b'1');
                if index < count {
                    self.selected = index;
                    DialAction::Changed
                } else {
                    DialAction::Ignored
                }
            }
            _ => DialAction::Ignored,
        }
    }

    pub fn render(&self, area: Rect, buffer: &mut Buffer, theme: &Theme) {
        let _span = nanocode_trace::info_span!(
            "tui.dial.render",
            width = area.width,
            height = area.height,
            levels = self.levels.len()
        )
        .entered();
        let scale_width = self
            .levels
            .iter()
            .map(|level| unicode::width(&level.label))
            .sum::<usize>()
            + 3 * self.levels.len().saturating_sub(1);
        let width = u16::try_from(
            unicode::width(HINT)
                .max(scale_width)
                .max(unicode::width(&self.title))
                + 4,
        )
        .unwrap_or(u16::MAX)
        .max(DIAL_WIDTH + 4);
        let height = DIAL_HEIGHT + 5;
        let popup = overlay::centered(area, width, height);
        let inner = overlay::frame(popup, buffer, theme, &self.title, None);
        if inner.height < DIAL_HEIGHT || inner.width < DIAL_WIDTH {
            Paragraph::new(self.levels[self.selected].label.clone()).render(inner, buffer);
            return;
        }
        let dial = Rect {
            x: inner.x + (inner.width - DIAL_WIDTH) / 2,
            y: inner.y,
            width: DIAL_WIDTH,
            height: DIAL_HEIGHT,
        };
        let level = &self.levels[self.selected];
        let fill = (self.selected + 1) as f64 / self.levels.len() as f64;
        let rx = f64::from(DIAL_WIDTH - 1) / 2.0;
        let ry = f64::from(DIAL_HEIGHT - 1) / 2.0;
        let mut cells = std::collections::BTreeMap::new();
        for sample in 0..DIAL_SAMPLES {
            let progress = sample as f64 / DIAL_SAMPLES as f64;
            let angle = progress.mul_add(TAU, -FRAC_PI_2);
            let x = rx.mul_add(angle.cos(), rx).round() as u16;
            let y = ry.mul_add(angle.sin(), ry).round() as u16;
            let filled = progress < fill;
            let entry = cells.entry((x, y)).or_insert(filled);
            *entry |= filled;
        }
        for ((x, y), filled) in cells {
            let (symbol, style) = if filled {
                (THICK_DOT, Style::default().fg(level.color))
            } else {
                (FILLER_DOT, Style::default().fg(theme.border()))
            };
            buffer.set_string(dial.x + x, dial.y + y, symbol, style);
        }
        let label = Line::styled(
            level.label.clone(),
            Style::default()
                .fg(level.color)
                .add_modifier(Modifier::BOLD),
        );
        let counter = Line::styled(
            format!("{}/{}", self.selected + 1, self.levels.len()),
            Style::default().fg(theme.muted()),
        );
        Paragraph::new(vec![label, counter])
            .alignment(Alignment::Center)
            .render(
                Rect {
                    x: dial.x + 2,
                    y: dial.y + DIAL_HEIGHT / 2 - 1,
                    width: DIAL_WIDTH - 4,
                    height: 2,
                },
                buffer,
            );
        let scale = self
            .levels
            .iter()
            .enumerate()
            .flat_map(|(index, level)| {
                let mut style = Style::default().fg(theme.muted());
                if index == self.selected {
                    style = Style::default()
                        .fg(level.color)
                        .add_modifier(Modifier::BOLD);
                }
                let separator = if index == 0 { "" } else { " · " };
                [
                    Span::styled(separator, Style::default().fg(theme.border())),
                    Span::styled(level.label.clone(), style),
                ]
            })
            .collect::<Vec<_>>();
        Paragraph::new(vec![
            Line::default(),
            Line::from(scale),
            Line::styled(HINT, Style::default().fg(theme.muted())),
        ])
        .alignment(Alignment::Center)
        .render(
            Rect {
                y: inner.y + DIAL_HEIGHT,
                height: inner.height.saturating_sub(DIAL_HEIGHT),
                ..inner
            },
            buffer,
        );
    }
}
