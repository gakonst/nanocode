use std::ops::BitOr;

use super::{
    buffer::{Buffer, symbol_width},
    layout::{Alignment, Rect},
    style::Style,
    text::{Line, Text},
};

pub trait Widget {
    fn render(self, area: Rect, buffer: &mut Buffer);
}

pub struct Clear;

impl Widget for Clear {
    fn render(self, area: Rect, buffer: &mut Buffer) {
        let area = area.intersection(buffer.area);
        if area.is_empty() {
            return;
        }
        for x in area.left()..area.right() {
            for y in area.top()..area.bottom() {
                buffer[(x, y)].reset();
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct Wrap {
    pub trim: bool,
}

pub struct Paragraph<'a> {
    text: Text<'a>,
    style: Style,
    alignment: Alignment,
    wrap: Option<Wrap>,
}

impl<'a> Paragraph<'a> {
    pub fn new(text: impl Into<Text<'a>>) -> Self {
        let text = text.into();
        let alignment = text.alignment.unwrap_or(Alignment::Left);
        Self {
            text,
            style: Style::default(),
            alignment,
            wrap: None,
        }
    }

    #[must_use]
    pub const fn wrap(mut self, wrap: Wrap) -> Self {
        self.wrap = Some(wrap);
        self
    }

    #[must_use]
    pub fn style(mut self, style: impl Into<Style>) -> Self {
        self.style = style.into();
        self
    }

    #[must_use]
    pub const fn alignment(mut self, alignment: Alignment) -> Self {
        self.alignment = alignment;
        self
    }
}

fn wrap_lines(lines: &[Line<'_>], width: u16, trim: bool) -> Vec<Line<'static>> {
    let mut wrapped = Vec::with_capacity(lines.len());
    for line in lines {
        let rows = crate::markdown::wrap_spans_with_whitespace(&line.spans, width, true, !trim);
        if rows.is_empty() {
            wrapped.push(Line {
                style: line.style,
                alignment: line.alignment,
                spans: Vec::new(),
            });
        }
        wrapped.extend(rows.into_iter().map(|row| Line {
            style: line.style,
            alignment: line.alignment,
            spans: row.spans,
        }));
    }
    wrapped
}

impl Widget for Paragraph<'_> {
    fn render(self, area: Rect, buffer: &mut Buffer) {
        let area = area.intersection(buffer.area);
        buffer.set_style(area, self.style);
        if area.is_empty() {
            return;
        }
        let wrapped;
        let lines = match self.wrap {
            None => &self.text.lines,
            Some(Wrap { trim }) => {
                wrapped = wrap_lines(&self.text.lines, area.width, trim);
                &wrapped
            }
        };
        let mut graphemes = Vec::new();
        for (row, line) in (0..area.height).zip(lines) {
            graphemes.clear();
            let mut line_width = 0_u16;
            for (symbol, style) in line.styled_graphemes(self.text.style) {
                let width = symbol_width(symbol);
                if width > area.width {
                    continue;
                }
                if line_width + width > area.width {
                    break;
                }
                line_width += width;
                graphemes.push((symbol, style));
            }
            let mut x = match line.alignment.unwrap_or(self.alignment) {
                Alignment::Center => (area.width / 2).saturating_sub(line_width / 2),
                Alignment::Right => area.width.saturating_sub(line_width),
                Alignment::Left => 0,
            };
            for (symbol, style) in &graphemes {
                let width = symbol_width(symbol);
                if width == 0 {
                    continue;
                }
                buffer[(area.left() + x, area.top() + row)]
                    .set_symbol(symbol)
                    .set_style(*style);
                x += width;
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Borders(u8);

impl Borders {
    pub const NONE: Self = Self(0);
    pub const TOP: Self = Self(1);
    pub const RIGHT: Self = Self(1 << 1);
    pub const BOTTOM: Self = Self(1 << 2);
    pub const LEFT: Self = Self(1 << 3);
    pub const ALL: Self = Self(Self::TOP.0 | Self::RIGHT.0 | Self::BOTTOM.0 | Self::LEFT.0);

    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}

impl BitOr for Borders {
    type Output = Self;

    fn bitor(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum BorderType {
    #[default]
    Plain,
    Rounded,
}

struct BorderSet {
    top_left: &'static str,
    top_right: &'static str,
    bottom_left: &'static str,
    bottom_right: &'static str,
    vertical: &'static str,
    horizontal: &'static str,
}

impl BorderType {
    const fn set(self) -> BorderSet {
        let (top_left, top_right, bottom_left, bottom_right) = match self {
            Self::Plain => ("┌", "┐", "└", "┘"),
            Self::Rounded => ("╭", "╮", "╰", "╯"),
        };
        BorderSet {
            top_left,
            top_right,
            bottom_left,
            bottom_right,
            vertical: "│",
            horizontal: "─",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TitlePosition {
    Top,
    Bottom,
}

#[derive(Default)]
pub struct Block<'a> {
    titles: Vec<(TitlePosition, Line<'a>)>,
    borders: Borders,
    style: Style,
    border_style: Style,
    border_type: BorderType,
}

impl<'a> Block<'a> {
    #[must_use]
    pub fn bordered() -> Self {
        Self::default().borders(Borders::ALL)
    }

    #[must_use]
    pub const fn borders(mut self, borders: Borders) -> Self {
        self.borders = borders;
        self
    }

    #[must_use]
    pub const fn border_type(mut self, border_type: BorderType) -> Self {
        self.border_type = border_type;
        self
    }

    #[must_use]
    pub fn style(mut self, style: impl Into<Style>) -> Self {
        self.style = style.into();
        self
    }

    #[must_use]
    pub fn border_style(mut self, style: impl Into<Style>) -> Self {
        self.border_style = style.into();
        self
    }

    #[must_use]
    pub fn title(self, title: impl Into<Line<'a>>) -> Self {
        self.titled(TitlePosition::Top, title.into())
    }

    #[must_use]
    pub fn title_bottom(self, title: impl Into<Line<'a>>) -> Self {
        self.titled(TitlePosition::Bottom, title.into())
    }

    fn titled(mut self, position: TitlePosition, title: Line<'a>) -> Self {
        assert!(
            matches!(title.alignment, None | Some(Alignment::Left)),
            "block titles are left aligned"
        );
        self.titles.push((position, title));
        self
    }

    fn titled_at(&self, position: TitlePosition) -> bool {
        self.titles.iter().any(|(at, _)| *at == position)
    }

    #[must_use]
    pub fn inner(&self, area: Rect) -> Rect {
        let mut inner = area;
        if self.borders.contains(Borders::LEFT) {
            inner.x = inner.x.saturating_add(1).min(inner.right());
            inner.width = inner.width.saturating_sub(1);
        }
        if self.borders.contains(Borders::TOP) || self.titled_at(TitlePosition::Top) {
            inner.y = inner.y.saturating_add(1).min(inner.bottom());
            inner.height = inner.height.saturating_sub(1);
        }
        if self.borders.contains(Borders::RIGHT) {
            inner.width = inner.width.saturating_sub(1);
        }
        if self.borders.contains(Borders::BOTTOM) || self.titled_at(TitlePosition::Bottom) {
            inner.height = inner.height.saturating_sub(1);
        }
        inner
    }

    fn render_borders(&self, area: Rect, buffer: &mut Buffer) {
        let set = self.border_type.set();
        let (left, top) = (area.left(), area.top());
        let right = area.right().saturating_sub(1);
        let bottom = area.bottom().saturating_sub(1);
        let sides = [
            (
                Borders::LEFT,
                Rect::new(left, top, 1, area.height),
                set.vertical,
            ),
            (
                Borders::TOP,
                Rect::new(left, top, area.width, 1),
                set.horizontal,
            ),
            (
                Borders::RIGHT,
                Rect::new(right, top, 1, area.height),
                set.vertical,
            ),
            (
                Borders::BOTTOM,
                Rect::new(left, bottom, area.width, 1),
                set.horizontal,
            ),
        ];
        for (border, side, symbol) in sides {
            if self.borders.contains(border) {
                buffer.set_symbols(side, symbol, self.border_style);
            }
        }
        let corners = [
            (
                Borders::RIGHT | Borders::BOTTOM,
                right,
                bottom,
                set.bottom_right,
            ),
            (Borders::RIGHT | Borders::TOP, right, top, set.top_right),
            (
                Borders::LEFT | Borders::BOTTOM,
                left,
                bottom,
                set.bottom_left,
            ),
            (Borders::LEFT | Borders::TOP, left, top, set.top_left),
        ];
        for (border, x, y, symbol) in corners {
            if self.borders.contains(border) {
                buffer[(x, y)]
                    .set_symbol(symbol)
                    .set_style(self.border_style);
            }
        }
    }

    fn render_titles(&self, position: TitlePosition, area: Rect, buffer: &mut Buffer) {
        let left_border = u16::from(self.borders.contains(Borders::LEFT));
        let right_border = u16::from(self.borders.contains(Borders::RIGHT));
        let mut titles_area = Rect {
            x: area.left().saturating_add(left_border),
            y: match position {
                TitlePosition::Top => area.top(),
                TitlePosition::Bottom => area.bottom().saturating_sub(1),
            },
            width: area
                .width
                .saturating_sub(left_border)
                .saturating_sub(right_border),
            height: 1,
        };
        for (_, title) in self.titles.iter().filter(|(at, _)| *at == position) {
            if titles_area.is_empty() {
                break;
            }
            let title_width =
                u16::try_from(title.width().min(usize::from(u16::MAX))).expect("clamped to u16");
            let title_area = Rect {
                width: title_width.min(titles_area.width),
                ..titles_area
            };
            title.render(title_area, buffer);
            let advance = title_width.saturating_add(1);
            titles_area.x = titles_area.x.saturating_add(advance);
            titles_area.width = titles_area.width.saturating_sub(advance);
        }
    }
}

impl Widget for Block<'_> {
    fn render(self, area: Rect, buffer: &mut Buffer) {
        let area = area.intersection(buffer.area);
        if area.is_empty() {
            return;
        }
        buffer.set_style(area, self.style);
        self.render_borders(area, buffer);
        self.render_titles(TitlePosition::Top, area, buffer);
        self.render_titles(TitlePosition::Bottom, area, buffer);
    }
}

impl Widget for &Block<'_> {
    fn render(self, area: Rect, buffer: &mut Buffer) {
        let area = area.intersection(buffer.area);
        if area.is_empty() {
            return;
        }
        buffer.set_style(area, self.style);
        self.render_borders(area, buffer);
        self.render_titles(TitlePosition::Top, area, buffer);
        self.render_titles(TitlePosition::Bottom, area, buffer);
    }
}
