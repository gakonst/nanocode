use std::ops::Range;

use crate::ui::{
    buffer::{Buffer, Cell},
    layout::{Position, Rect},
    style::Modifier,
};

#[derive(Debug)]
pub(crate) struct Selection {
    region: Rect,
    anchor: Position,
    pointer: Option<Position>,
    screen: Buffer,
}

impl Selection {
    pub(crate) fn begin(regions: &[Rect], anchor: Position, screen: Buffer) -> Option<Self> {
        region_at(regions, anchor).map(|region| Self {
            region,
            anchor,
            pointer: None,
            screen,
        })
    }

    pub(crate) fn extend(&mut self, pointer: Position) -> bool {
        let previous = self.head();
        self.pointer = Some(pointer);
        self.head() != previous
    }

    pub(crate) const fn active(&self) -> bool {
        self.pointer.is_some()
    }

    pub(crate) fn paint(&self, buffer: &mut Buffer) {
        if !self.active() {
            return;
        }
        for y in self.region.top()..self.region.bottom() {
            for x in self.region.left()..self.region.right() {
                buffer[(x, y)].clone_from(&self.screen[(x, y)]);
            }
        }
        self.highlight(buffer);
    }

    pub(crate) fn highlight(&self, buffer: &mut Buffer) {
        for (y, from, to) in self.rows() {
            for x in columns(buffer, self.region, y, from, to) {
                buffer[(x, y)].modifier.toggle(Modifier::REVERSED);
            }
        }
    }

    pub(crate) fn text(&self) -> String {
        let buffer = &self.screen;
        let mut text = String::new();
        for (index, (y, from, to)) in self.rows().enumerate() {
            if index > 0 {
                text.push('\n');
            }
            let columns = columns(buffer, self.region, y, from, to);
            let mut x = columns.start;
            while x < columns.end {
                let cell = &buffer[(x, y)];
                text.push_str(cell.symbol());
                x += width(cell);
            }
            text.truncate(text.trim_end_matches(' ').len());
        }
        text.trim_matches('\n').to_owned()
    }

    fn head(&self) -> Option<Position> {
        let region = self.region;
        self.pointer.map(|pointer| {
            if pointer.y < region.top() {
                Position::new(region.left(), region.top())
            } else if pointer.y >= region.bottom() {
                Position::new(region.right() - 1, region.bottom() - 1)
            } else {
                Position::new(
                    pointer.x.clamp(region.left(), region.right() - 1),
                    pointer.y,
                )
            }
        })
    }

    fn rows(&self) -> impl Iterator<Item = (u16, u16, u16)> + use<> {
        let (anchor, region) = (self.anchor, self.region);
        self.head().into_iter().flat_map(move |head| {
            let (start, end) = if (anchor.y, anchor.x) <= (head.y, head.x) {
                (anchor, head)
            } else {
                (head, anchor)
            };
            (start.y..=end.y).map(move |y| {
                (
                    y,
                    if y == start.y { start.x } else { region.left() },
                    if y == end.y {
                        end.x
                    } else {
                        region.right() - 1
                    },
                )
            })
        })
    }
}

fn region_at(regions: &[Rect], position: Position) -> Option<Rect> {
    regions
        .iter()
        .rev()
        .copied()
        .find(|region| region.contains(position))
}

fn columns(buffer: &Buffer, region: Rect, y: u16, from: u16, to: u16) -> Range<u16> {
    let mut start = region.left();
    let mut x = start;
    while x <= to {
        x += width(&buffer[(x, y)]);
        if x <= from {
            start = x;
        }
    }
    start..x.min(region.right())
}

fn width(cell: &Cell) -> u16 {
    cell.cell_width().max(1)
}
