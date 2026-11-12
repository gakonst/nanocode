use std::borrow::Cow;

use super::{
    buffer::{Buffer, render_symbol, symbol_width},
    layout::{Alignment, Position, Rect},
    style::Style,
    widgets::Widget,
};

#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct Span<'a> {
    pub style: Style,
    pub content: Cow<'a, str>,
}

impl<'a> Span<'a> {
    pub fn raw(content: impl Into<Cow<'a, str>>) -> Self {
        Self {
            content: content.into(),
            style: Style::default(),
        }
    }

    pub fn styled(content: impl Into<Cow<'a, str>>, style: impl Into<Style>) -> Self {
        Self {
            content: content.into(),
            style: style.into(),
        }
    }

    #[must_use]
    pub fn style(mut self, style: impl Into<Style>) -> Self {
        self.style = style.into();
        self
    }

    #[must_use]
    pub fn patch_style(mut self, style: impl Into<Style>) -> Self {
        self.style = self.style.patch(style);
        self
    }

    #[must_use]
    pub fn width(&self) -> usize {
        crate::unicode::width(&self.content)
    }

    pub(crate) fn styled_graphemes(&self, base: Style) -> impl Iterator<Item = (&str, Style)> {
        let style = base.patch(self.style);
        crate::unicode::graphemes(&self.content)
            .filter(|grapheme| !grapheme.contains(char::is_control))
            .map(move |grapheme| (grapheme, style))
    }
}

impl<'a, T: Into<Cow<'a, str>>> From<T> for Span<'a> {
    fn from(content: T) -> Self {
        Self::raw(content)
    }
}

impl Widget for &Span<'_> {
    fn render(self, area: Rect, buffer: &mut Buffer) {
        let area = area.intersection(buffer.area);
        if area.is_empty() {
            return;
        }
        let Rect { mut x, y, .. } = area;
        for (index, (symbol, style)) in self.styled_graphemes(Style::default()).enumerate() {
            let width = symbol_width(symbol);
            let next = x.saturating_add(width);
            if next > area.right() {
                break;
            }
            let (position, append) = if index == 0 {
                (Position::new(x, y), false)
            } else if x == area.x {
                (Position::new(x, y), true)
            } else if width == 0 {
                (Position::new(x - 1, y), true)
            } else {
                (Position::new(x, y), false)
            };
            render_symbol(buffer, position, symbol, append).set_style(style);
            for hidden in (x + 1)..next {
                buffer[(hidden, y)].reset();
            }
            x = next;
        }
    }
}

#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct Line<'a> {
    pub style: Style,
    pub alignment: Option<Alignment>,
    pub spans: Vec<Span<'a>>,
}

fn spans_of<'a>(content: impl Into<Cow<'a, str>>) -> Vec<Span<'a>> {
    match content.into() {
        Cow::Borrowed(text) => text.lines().map(Span::raw).collect(),
        Cow::Owned(text) => text
            .lines()
            .map(|line| Span::raw(line.to_owned()))
            .collect(),
    }
}

impl<'a> Line<'a> {
    pub fn raw(content: impl Into<Cow<'a, str>>) -> Self {
        Self {
            spans: spans_of(content),
            ..Self::default()
        }
    }

    pub fn styled(content: impl Into<Cow<'a, str>>, style: impl Into<Style>) -> Self {
        Self {
            spans: spans_of(content),
            style: style.into(),
            ..Self::default()
        }
    }

    #[must_use]
    pub fn style(mut self, style: impl Into<Style>) -> Self {
        self.style = style.into();
        self
    }

    #[must_use]
    pub const fn alignment(mut self, alignment: Alignment) -> Self {
        self.alignment = Some(alignment);
        self
    }

    #[must_use]
    pub const fn centered(self) -> Self {
        self.alignment(Alignment::Center)
    }

    #[must_use]
    pub fn width(&self) -> usize {
        self.spans.iter().map(Span::width).sum()
    }

    pub(crate) fn styled_graphemes(&self, base: Style) -> impl Iterator<Item = (&str, Style)> {
        let style = base.patch(self.style);
        self.spans
            .iter()
            .flat_map(move |span| span.styled_graphemes(style))
    }

    pub fn iter(&self) -> std::slice::Iter<'_, Span<'a>> {
        self.spans.iter()
    }

    pub fn push_span(&mut self, span: impl Into<Span<'a>>) {
        self.spans.push(span.into());
    }

    fn render_aligned(&self, area: Rect, buffer: &mut Buffer, parent: Option<Alignment>) {
        let area = area.intersection(buffer.area);
        if area.is_empty() {
            return;
        }
        let area = Rect { height: 1, ..area };
        let line_width = self.width();
        if line_width == 0 {
            return;
        }
        buffer.set_style(area, self.style);
        let alignment = self.alignment.or(parent);
        let area_width = usize::from(area.width);
        if line_width <= area_width {
            let indent = match alignment {
                Some(Alignment::Center) => area_width.saturating_sub(line_width) / 2,
                Some(Alignment::Right) => area_width.saturating_sub(line_width),
                Some(Alignment::Left) | None => 0,
            };
            let indent = u16::try_from(indent).expect("the indent is within the area width");
            render_spans(&self.spans, area.indent_x(indent), buffer, 0);
        } else {
            let skip = match alignment {
                Some(Alignment::Center) => line_width.saturating_sub(area_width) / 2,
                Some(Alignment::Right) => line_width.saturating_sub(area_width),
                Some(Alignment::Left) | None => 0,
            };
            render_spans(&self.spans, area, buffer, skip);
        }
    }
}

fn saturating_u16(value: usize) -> u16 {
    u16::try_from(value.min(usize::from(u16::MAX))).expect("the value is at most u16::MAX")
}

fn render_spans(spans: &[Span<'_>], mut area: Rect, buffer: &mut Buffer, mut skip: usize) {
    for span in spans {
        let span_width = span.width();
        if skip >= span_width {
            skip -= span_width;
            continue;
        }
        let available = span_width - skip;
        skip = 0;
        let (span, span_width, offset) = if span_width <= available {
            (Cow::Borrowed(span), span_width, 0)
        } else {
            let (content, width) = truncate_start(&span.content, available);
            (
                Cow::Owned(Span::styled(content, span.style)),
                width,
                saturating_u16(available - width),
            )
        };
        area = area.indent_x(offset);
        if area.is_empty() {
            break;
        }
        span.as_ref().render(area, buffer);
        area = area.indent_x(saturating_u16(span_width));
    }
}

fn truncate_start(text: &str, width_max: usize) -> (&str, usize) {
    let mut start = text.len();
    let mut width = 0_usize;
    for (offset, grapheme) in crate::unicode::grapheme_indices(text).rev() {
        let Some(next) = width.checked_add(crate::unicode::width(grapheme)) else {
            break;
        };
        if next > width_max {
            break;
        }
        width = next;
        start = offset;
    }
    (&text[start..], width)
}

impl<'a> IntoIterator for &'a Line<'a> {
    type Item = &'a Span<'a>;
    type IntoIter = std::slice::Iter<'a, Span<'a>>;

    fn into_iter(self) -> Self::IntoIter {
        self.spans.iter()
    }
}

impl From<String> for Line<'_> {
    fn from(text: String) -> Self {
        Self::raw(text)
    }
}

impl<'a> From<&'a str> for Line<'a> {
    fn from(text: &'a str) -> Self {
        Self::raw(text)
    }
}

impl<'a> From<Cow<'a, str>> for Line<'a> {
    fn from(text: Cow<'a, str>) -> Self {
        Self::raw(text)
    }
}

impl<'a> From<Vec<Span<'a>>> for Line<'a> {
    fn from(spans: Vec<Span<'a>>) -> Self {
        Self {
            spans,
            ..Self::default()
        }
    }
}

impl<'a> From<Span<'a>> for Line<'a> {
    fn from(span: Span<'a>) -> Self {
        Self::from(vec![span])
    }
}

impl<'a, T: Into<Span<'a>>> FromIterator<T> for Line<'a> {
    fn from_iter<I: IntoIterator<Item = T>>(spans: I) -> Self {
        Self::from(spans.into_iter().map(Into::into).collect::<Vec<_>>())
    }
}

impl Widget for &Line<'_> {
    fn render(self, area: Rect, buffer: &mut Buffer) {
        self.render_aligned(area, buffer, None);
    }
}

impl Widget for Line<'_> {
    fn render(self, area: Rect, buffer: &mut Buffer) {
        self.render_aligned(area, buffer, None);
    }
}

#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct Text<'a> {
    pub style: Style,
    pub alignment: Option<Alignment>,
    pub lines: Vec<Line<'a>>,
}

impl<'a> Text<'a> {
    pub fn raw(content: impl Into<Cow<'a, str>>) -> Self {
        let lines = match content.into() {
            Cow::Borrowed("") => vec![Line::from("")],
            Cow::Borrowed(text) => text.lines().map(Line::from).collect(),
            Cow::Owned(text) if text.is_empty() => vec![Line::from("")],
            Cow::Owned(text) => text
                .lines()
                .map(|line| Line::from(line.to_owned()))
                .collect(),
        };
        Self::from(lines)
    }
}

impl From<String> for Text<'_> {
    fn from(text: String) -> Self {
        Self::raw(text)
    }
}

impl<'a> From<&'a str> for Text<'a> {
    fn from(text: &'a str) -> Self {
        Self::raw(text)
    }
}

impl<'a> From<Span<'a>> for Text<'a> {
    fn from(span: Span<'a>) -> Self {
        Self::from(Line::from(span))
    }
}

impl<'a> From<Line<'a>> for Text<'a> {
    fn from(line: Line<'a>) -> Self {
        Self::from(vec![line])
    }
}

impl<'a> From<Vec<Line<'a>>> for Text<'a> {
    fn from(lines: Vec<Line<'a>>) -> Self {
        Self {
            lines,
            ..Self::default()
        }
    }
}
