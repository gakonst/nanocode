//! Byte spans into the source and a line table to turn them into lines and columns.

use alloc::vec::Vec;
use core::ops::Range;

/// A half-open byte range `start..end` into the source a node was parsed from.
///
/// Spans are bytes, not chars: slicing the source with [`Span::range`] is always on char
/// boundaries for spans the parser produced.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Span {
    pub start: u32,
    pub end: u32,
}

impl Span {
    pub const fn new(start: u32, end: u32) -> Span {
        Span { start, end }
    }

    pub(crate) fn from_usize(start: usize, end: usize) -> Span {
        Span {
            start: start as u32,
            end: end as u32,
        }
    }

    pub const fn len(self) -> u32 {
        self.end - self.start
    }

    pub const fn is_empty(self) -> bool {
        self.start == self.end
    }

    pub fn range(self) -> Range<usize> {
        self.start as usize..self.end as usize
    }

    /// The smallest span covering both.
    pub fn to(self, other: Span) -> Span {
        Span {
            start: self.start.min(other.start),
            end: self.end.max(other.end),
        }
    }

    pub fn contains(self, other: Span) -> bool {
        self.start <= other.start && other.end <= self.end
    }
}

/// A 1-based line and column. The column counts chars, not bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LineCol {
    pub line: u32,
    pub column: u32,
}

/// Start offsets of every line of a source, for turning byte offsets into lines and
/// columns on demand.
#[derive(Clone, Debug)]
pub struct LineIndex {
    starts: Vec<u32>,
}

impl LineIndex {
    pub fn new(source: &str) -> LineIndex {
        LineIndex::from_bytes(source.as_bytes())
    }

    /// For sources that may not be UTF-8 (spans are offsets into these bytes).
    pub fn from_bytes(source: &[u8]) -> LineIndex {
        let mut starts = Vec::with_capacity(source.len() / 32 + 1);
        starts.push(0);
        for (i, &b) in source.iter().enumerate() {
            if b == b'\n' {
                starts.push(i as u32 + 1);
            }
        }
        LineIndex { starts }
    }

    /// The 1-based line containing `offset`.
    pub fn line(&self, offset: u32) -> u32 {
        match self.starts.binary_search(&offset) {
            Ok(i) => i as u32 + 1,
            Err(i) => i as u32,
        }
    }

    /// The 1-based line and char column of `offset` in `source`, which must be the source
    /// this index was built from.
    pub fn line_col(&self, source: &str, offset: u32) -> LineCol {
        self.line_col_bytes(source.as_bytes(), offset)
    }

    /// Like [`LineIndex::line_col`] for a source given as bytes; a byte that is not
    /// part of valid UTF-8 counts as one column.
    pub fn line_col_bytes(&self, source: &[u8], offset: u32) -> LineCol {
        let line = self.line(offset);
        let start = self.starts[line as usize - 1] as usize;
        let end = (offset as usize).min(source.len());
        let mut column = 1;
        let mut i = start;
        while i < end {
            i += crate::parser::char_len(source, i, end);
            column += 1;
        }
        LineCol { line, column }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_col() {
        let src = "ab\nçd\n\nx";
        let idx = LineIndex::new(src);
        assert_eq!(idx.line_col(src, 0), LineCol { line: 1, column: 1 });
        assert_eq!(idx.line_col(src, 2), LineCol { line: 1, column: 3 });
        assert_eq!(idx.line_col(src, 3), LineCol { line: 2, column: 1 });
        // 'ç' is two bytes; 'd' is the second char.
        assert_eq!(idx.line_col(src, 5), LineCol { line: 2, column: 2 });
        assert_eq!(idx.line_col(src, 7), LineCol { line: 3, column: 1 });
        assert_eq!(idx.line_col(src, 8), LineCol { line: 4, column: 1 });
        assert_eq!(idx.line(9), 4);
    }
}
