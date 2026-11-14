//! Byte offset to line/column mapping, built on demand (for error messages and the
//! bytecode line tables), so the lexer itself never tracks lines.

use alloc::vec::Vec;

/// The start offsets of all lines of a source.
///
/// Lines end at LF, CR, CRLF, U+2028 and U+2029 (the ECMAScript line terminators).
/// Lines and columns are 1-based; columns count code points, like QuickJS.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct LineTable {
    starts: Vec<u32>,
    /// No byte above 0x7F: a column is a byte count.
    ascii: bool,
}

impl LineTable {
    pub fn new(src: &[u8]) -> Self {
        let mut table = LineTable::default();
        table.rebuild(src);
        table
    }

    pub fn rebuild(&mut self, src: &[u8]) {
        let starts = &mut self.starts;
        starts.clear();
        starts.reserve(src.len() / 32 + 1);
        starts.push(0);
        let mut ascii = true;
        let mut p = 0;
        while p < src.len() {
            // Eight bytes at a time while none is LF, CR or above 0x7F (0092).
            while let Some(w) = src.get(p..p + 8) {
                let w = u64::from_le_bytes([w[0], w[1], w[2], w[3], w[4], w[5], w[6], w[7]]);
                if w & HIGH != 0 || has_byte(w, b'\n') || has_byte(w, b'\r') {
                    break;
                }
                p += 8;
            }
            let Some(&b) = src.get(p) else { break };
            ascii &= b < 0x80;
            match b {
                b'\n' => starts.push((p + 1) as u32),
                b'\r' if src.get(p + 1) != Some(&b'\n') => starts.push((p + 1) as u32),
                // U+2028 / U+2029 are E2 80 A8 / E2 80 A9.
                0xE2 if src.get(p + 1) == Some(&0x80)
                    && matches!(src.get(p + 2), Some(0xA8 | 0xA9)) =>
                {
                    p += 2;
                    starts.push((p + 1) as u32);
                }
                _ => {}
            }
            p += 1;
        }
        self.ascii = ascii;
    }

    /// The number of lines (at least 1).
    pub fn line_count(&self) -> u32 {
        self.starts.len() as u32
    }

    /// The 1-based line containing byte offset `pos`. Offsets past the end map to the
    /// last line.
    pub fn line(&self, pos: u32) -> u32 {
        self.starts.partition_point(|&s| s <= pos) as u32
    }

    /// The byte offset where 1-based line `line` starts.
    pub fn line_start(&self, line: u32) -> u32 {
        let idx = (line.max(1) - 1) as usize;
        self.starts
            .get(idx)
            .or(self.starts.last())
            .copied()
            .unwrap_or(0)
    }

    /// The 1-based line and column of byte offset `pos` in `src` (the source this table
    /// was built from). The column counts code points from the start of the line.
    pub fn line_col(&self, src: &[u8], pos: u32) -> (u32, u32) {
        let line = self.line(pos);
        let start = self.line_start(line) as usize;
        let end = (pos as usize).min(src.len()).max(start);
        (line, self.count_chars(&src[start..end]) + 1)
    }

    /// `line_col` for a run of mostly increasing offsets (the line table of a function,
    /// 0092): continues counting from the previous offset on the same line instead of
    /// from the line start, which is quadratic on long (minified) lines.
    pub fn line_col_from(&self, src: &[u8], pos: u32, c: &mut LineCursor) -> (u32, u32) {
        if c.line != 0 && pos >= c.end && pos < c.next_start {
            let end = (pos as usize).min(src.len());
            c.col += self.count_chars(&src[(c.end as usize).min(end)..end]);
            c.end = end as u32;
            return (c.line, c.col + 1);
        }
        let line = self.line(pos);
        let start = self.line_start(line) as usize;
        let end = (pos as usize).min(src.len()).max(start);
        *c = LineCursor {
            line,
            end: end as u32,
            next_start: self.starts.get(line as usize).copied().unwrap_or(u32::MAX),
            col: self.count_chars(&src[start..end]),
        };
        (line, c.col + 1)
    }
}

/// The state of [`LineTable::line_col_from`].
#[derive(Clone, Copy, Debug, Default)]
pub struct LineCursor {
    /// The line of the previous offset (0: none yet).
    line: u32,
    /// The previous offset (clamped to the source) and its 0-based column.
    end: u32,
    col: u32,
    /// The start of the next line.
    next_start: u32,
}

impl LineTable {
    /// Code points in UTF-8 `b` (of this source): the bytes that are not continuation
    /// bytes.
    fn count_chars(&self, b: &[u8]) -> u32 {
        if self.ascii {
            return b.len() as u32;
        }
        b.iter().filter(|&&b| b & 0xC0 != 0x80).count() as u32
    }
}

const HIGH: u64 = 0x8080_8080_8080_8080;

/// Whether a byte of `w` is `b` (`b` < 0x80, and no byte of `w` above 0x7F).
fn has_byte(w: u64, b: u8) -> bool {
    const LOW: u64 = 0x0101_0101_0101_0101;
    let x = w ^ (LOW * u64::from(b));
    x.wrapping_sub(LOW) & !x & HIGH != 0
}
