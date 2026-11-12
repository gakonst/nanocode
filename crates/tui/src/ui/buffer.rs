use std::ops::{Index, IndexMut};

use super::{
    layout::{Position, Rect},
    style::{Color, Modifier, Style},
    text::Line,
};

const SYMBOL_INLINE_BYTES: usize = 22;
const BLANK: &str = " ";
const HALFWIDTH_KATAKANA_VOICED_SOUND_MARK: char = '\u{FF9E}';
const HALFWIDTH_KATAKANA_SEMI_VOICED_SOUND_MARK: char = '\u{FF9F}';
const VARIATION_SELECTOR_EMOJI: char = '\u{FE0F}';
const VISIBLE_ON_BLANK: Modifier = Modifier::REVERSED
    .union(Modifier::UNDERLINED)
    .union(Modifier::SLOW_BLINK)
    .union(Modifier::RAPID_BLINK)
    .union(Modifier::CROSSED_OUT);

#[derive(Clone, Debug)]
enum Symbol {
    Unset,
    Inline {
        bytes: [u8; SYMBOL_INLINE_BYTES],
        length: u8,
    },
    Heap(Box<str>),
}

impl Symbol {
    fn new(text: &str) -> Self {
        let length = text.len();
        if length <= SYMBOL_INLINE_BYTES {
            let mut bytes = [0; SYMBOL_INLINE_BYTES];
            bytes[..length].copy_from_slice(text.as_bytes());
            Self::Inline {
                bytes,
                length: u8::try_from(length).expect("inline symbols fit in a byte length"),
            }
        } else {
            Self::Heap(text.into())
        }
    }

    fn bytes(&self) -> &[u8] {
        match self {
            Self::Unset => BLANK.as_bytes(),
            Self::Inline { bytes, length } => &bytes[..usize::from(*length)],
            Self::Heap(text) => text.as_bytes(),
        }
    }

    fn as_str(&self) -> &str {
        match self {
            Self::Unset => BLANK,
            Self::Inline { bytes, length } => std::str::from_utf8(&bytes[..usize::from(*length)])
                .expect("inline symbols hold the UTF-8 they were built from"),
            Self::Heap(text) => text,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Cell {
    symbol: Symbol,
    pub fg: Color,
    pub bg: Color,
    pub modifier: Modifier,
}

impl Cell {
    pub const EMPTY: Self = Self {
        symbol: Symbol::Unset,
        fg: Color::Reset,
        bg: Color::Reset,
        modifier: Modifier::empty(),
    };

    #[must_use]
    pub fn symbol(&self) -> &str {
        self.symbol.as_str()
    }

    #[must_use]
    pub fn symbol_bytes(&self) -> &[u8] {
        self.symbol.bytes()
    }

    pub fn set_symbol(&mut self, symbol: &str) -> &mut Self {
        self.symbol = Symbol::new(symbol);
        self
    }

    fn append_symbol(&mut self, symbol: &str) -> &mut Self {
        self.symbol = match &self.symbol {
            Symbol::Unset => Symbol::new(symbol),
            existing => Symbol::new(&[existing.as_str(), symbol].concat()),
        };
        self
    }

    pub fn set_style(&mut self, style: impl Into<Style>) -> &mut Self {
        let style = style.into();
        if let Some(color) = style.fg {
            self.fg = color;
        }
        if let Some(color) = style.bg {
            self.bg = color;
        }
        self.modifier.insert(style.add_modifier);
        self.modifier.remove(style.sub_modifier);
        self
    }

    pub fn reset(&mut self) {
        *self = Self::EMPTY;
    }

    #[must_use]
    pub fn cell_width(&self) -> u16 {
        match self.symbol.bytes() {
            [_] => 1,
            _ => symbol_width(self.symbol()),
        }
    }
}

impl Default for Cell {
    fn default() -> Self {
        Self::EMPTY
    }
}

impl PartialEq for Cell {
    fn eq(&self, other: &Self) -> bool {
        self.symbol.bytes() == other.symbol.bytes()
            && self.fg == other.fg
            && self.bg == other.bg
            && self.modifier == other.modifier
    }
}

impl Eq for Cell {}

#[must_use]
pub fn symbol_width(symbol: &str) -> u16 {
    if symbol.len() == 1 {
        return 1;
    }
    let sound_marks = symbol
        .chars()
        .filter(|character| {
            matches!(
                *character,
                HALFWIDTH_KATAKANA_VOICED_SOUND_MARK | HALFWIDTH_KATAKANA_SEMI_VOICED_SOUND_MARK
            )
        })
        .count();
    (crate::unicode::width(symbol) as u16).saturating_add(sound_marks as u16)
}

pub(crate) fn next_grapheme(text: &str) -> &str {
    let bytes = text.as_bytes();
    if bytes.len() == 1 || (bytes[0].is_ascii() && bytes[1].is_ascii() && bytes[..2] != *b"\r\n") {
        return &text[..1];
    }
    crate::unicode::graphemes(text)
        .next()
        .expect("a non-empty string has a first grapheme")
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Buffer {
    pub area: Rect,
    pub content: Vec<Cell>,
}

impl Buffer {
    #[must_use]
    pub fn empty(area: Rect) -> Self {
        Self {
            area,
            content: vec![Cell::EMPTY; area.area() as usize],
        }
    }

    #[must_use]
    pub fn cell(&self, position: impl Into<Position>) -> Option<&Cell> {
        let index = self.index(position.into())?;
        self.content.get(index)
    }

    pub fn cell_mut(&mut self, position: impl Into<Position>) -> Option<&mut Cell> {
        let index = self.index(position.into())?;
        self.content.get_mut(index)
    }

    const fn index(&self, position: Position) -> Option<usize> {
        if !self.area.contains(position) {
            return None;
        }
        let row = (position.y - self.area.y) as usize;
        let column = (position.x - self.area.x) as usize;
        Some(row * self.area.width as usize + column)
    }

    #[must_use]
    pub fn index_of(&self, x: u16, y: u16) -> usize {
        self.index(Position { x, y }).unwrap_or_else(|| {
            panic!(
                "index outside of buffer: the area is {:?} but index is ({x}, {y})",
                self.area
            )
        })
    }

    pub fn set_string(&mut self, x: u16, y: u16, text: impl AsRef<str>, style: impl Into<Style>) {
        self.set_stringn(x, y, text, usize::MAX, style);
    }

    pub fn set_stringn(
        &mut self,
        mut x: u16,
        y: u16,
        text: impl AsRef<str>,
        width_max: usize,
        style: impl Into<Style>,
    ) -> (u16, u16) {
        let width_max = u16::try_from(width_max.min(usize::from(u16::MAX)))
            .expect("the width is at most u16::MAX");
        let mut remaining = self.area.right().saturating_sub(x).min(width_max);
        let style = style.into();
        let text = text.as_ref();
        let mut offset = 0;
        while offset < text.len() {
            let symbol = next_grapheme(&text[offset..]);
            offset += symbol.len();
            if symbol.contains(char::is_control) {
                continue;
            }
            let width = symbol_width(symbol);
            if width == 0 {
                continue;
            }
            let Some(left) = remaining.checked_sub(width) else {
                break;
            };
            remaining = left;
            self[(x, y)].set_symbol(symbol).set_style(style);
            let next = x + width;
            x += 1;
            while x < next {
                self[(x, y)].reset();
                x += 1;
            }
        }
        (x, y)
    }

    #[must_use]
    pub const fn area(&self) -> &Rect {
        &self.area
    }

    pub fn set_line(&mut self, x: u16, y: u16, line: &Line<'_>, width_max: u16) -> (u16, u16) {
        let mut remaining = width_max;
        let mut x = x;
        for span in &line.spans {
            if remaining == 0 {
                break;
            }
            let (next, _) = self.set_stringn(
                x,
                y,
                span.content.as_ref(),
                usize::from(remaining),
                line.style.patch(span.style),
            );
            remaining = remaining.saturating_sub(next.saturating_sub(x));
            x = next;
        }
        (x, y)
    }

    pub fn set_style(&mut self, area: Rect, style: impl Into<Style>) {
        let style = style.into();
        let area = self.area.intersection(area);
        for y in area.top()..area.bottom() {
            for x in area.left()..area.right() {
                self[(x, y)].set_style(style);
            }
        }
    }

    pub fn resize(&mut self, area: Rect) {
        let length = area.area() as usize;
        if self.content.len() > length {
            self.content.truncate(length);
        } else {
            self.content.resize(length, Cell::EMPTY);
        }
        self.area = area;
    }

    pub fn reset(&mut self) {
        for cell in &mut self.content {
            cell.reset();
        }
    }

    #[must_use]
    pub fn diff_iter<'next>(&self, next: &'next Self) -> BufferDiff<'_, 'next> {
        BufferDiff::new(self, next)
    }
}

impl<P: Into<Position>> Index<P> for Buffer {
    type Output = Cell;

    fn index(&self, position: P) -> &Cell {
        let position = position.into();
        &self.content[self.index_of(position.x, position.y)]
    }
}

impl<P: Into<Position>> IndexMut<P> for Buffer {
    fn index_mut(&mut self, position: P) -> &mut Cell {
        let position = position.into();
        let index = self.index_of(position.x, position.y);
        &mut self.content[index]
    }
}

pub(crate) fn render_symbol<'a>(
    buffer: &'a mut Buffer,
    position: Position,
    symbol: &str,
    append: bool,
) -> &'a mut Cell {
    let cell = &mut buffer[position];
    if append {
        cell.append_symbol(symbol)
    } else {
        cell.set_symbol(symbol)
    }
}

#[derive(Debug)]
struct Trailing {
    next: usize,
    end: usize,
    force: bool,
}

#[derive(Debug)]
pub struct BufferDiff<'previous, 'next> {
    previous: &'previous [Cell],
    next: &'next [Cell],
    area: Rect,
    position: usize,
    trailing: Option<Trailing>,
}

impl<'previous, 'next> BufferDiff<'previous, 'next> {
    fn new(previous: &'previous Buffer, next: &'next Buffer) -> Self {
        assert_eq!(previous.area.x, next.area.x, "diffed buffers share x");
        assert_eq!(previous.area.y, next.area.y, "diffed buffers share y");
        assert_eq!(
            previous.area.width, next.area.width,
            "diffed buffers share a width"
        );
        let mut area = previous.area;
        area.height = area.height.min(next.area.height);
        Self {
            previous: &previous.content,
            next: &next.content,
            area,
            position: 0,
            trailing: None,
        }
    }

    const fn position_of(&self, index: usize) -> (u16, u16) {
        let width = self.area.width as usize;
        (
            (index % width + self.area.x as usize) as u16,
            (index / width + self.area.y as usize) as u16,
        )
    }
}

impl<'next> Iterator for BufferDiff<'_, 'next> {
    type Item = (u16, u16, &'next Cell);

    fn next(&mut self) -> Option<Self::Item> {
        let length = self.next.len().min(self.previous.len());
        if let Some(Trailing { next, end, force }) = &mut self.trailing {
            while *next < *end {
                let index = *next;
                *next += usize::from(self.next[index].cell_width().max(1));
                *end = (*end).max(*next).min(length);
                if *force || self.previous[index].symbol_bytes() != self.next[index].symbol_bytes()
                {
                    let (x, y) = self.position_of(index);
                    return Some((x, y, &self.next[index]));
                }
            }
            self.position = *end;
            self.trailing = None;
        }
        while self.position < length {
            let index = self.position;
            self.position += 1;
            let current = &self.next[index];
            let previous = &self.previous[index];
            let width = usize::from(current.cell_width());
            if current == previous {
                self.position += width.saturating_sub(1);
                continue;
            }
            let previous_width = usize::from(previous.cell_width());
            if width > 1 && current.symbol().contains(VARIATION_SELECTOR_EMOJI) {
                self.trailing = Some(Trailing {
                    next: index + 1,
                    end: (index + width).min(length),
                    force: false,
                });
            } else if width > 1 {
                self.position += width - 1;
            } else if previous_width > width
                && (previous.bg != Color::Reset || previous.modifier.intersects(VISIBLE_ON_BLANK))
            {
                self.trailing = Some(Trailing {
                    next: index + 1,
                    end: index + previous_width,
                    force: true,
                });
            }
            let (x, y) = self.position_of(index);
            return Some((x, y, &self.next[index]));
        }
        None
    }
}
