//! The line table (docs/compiler.md 6.2): code position to source line and column.
//!
//! Encoding: a sequence of entries, each three LEB128 numbers relative to the previous
//! entry (initially pc 0, line 1, column 1):
//!
//! ```text
//! pc_delta    unsigned, >= 0 (entries are in increasing pc order)
//! line_delta  signed, zigzag-encoded
//! column      unsigned, absolute (1-based)
//! ```
//!
//! An entry applies from its pc up to the next entry's pc. Lines and columns are 1-based
//! (columns count code points, D1032). The compiler converts source offsets with
//! `lexer::LineTable` before adding entries, so no source is needed to read the table.

use alloc::vec::Vec;

/// Builds an encoded line table.
#[derive(Clone, Debug, Default)]
pub struct LineTableBuilder {
    out: Vec<u8>,
    pc: u32,
    line: u32,
    col: u32,
    started: bool,
}

impl LineTableBuilder {
    pub fn new() -> Self {
        LineTableBuilder {
            out: Vec::new(),
            pc: 0,
            line: 1,
            col: 1,
            started: false,
        }
    }

    /// A builder that writes into `out`, emptied: a buffer used for another table
    /// before.
    pub fn with_buffer(mut out: Vec<u8>) -> Self {
        out.clear();
        LineTableBuilder { out, ..Self::new() }
    }

    /// Records that code from `pc` on comes from `line:col`. Entries must come in
    /// non-decreasing pc order; an entry at the same pc as the previous one replaces it
    /// only in effect (both are kept, the later wins on lookup). Repeats of the current
    /// position are skipped.
    pub fn add(&mut self, pc: u32, line: u32, col: u32) {
        debug_assert!(pc >= self.pc, "line table entries out of order");
        if self.started && line == self.line && col == self.col {
            return;
        }
        let pc_delta = pc.saturating_sub(self.pc);
        let line_delta = line as i64 - self.line as i64;
        write_uleb(&mut self.out, pc_delta as u64);
        write_uleb(&mut self.out, zigzag(line_delta));
        write_uleb(&mut self.out, col as u64);
        self.pc = pc;
        self.line = line;
        self.col = col;
        self.started = true;
    }

    pub fn finish(self) -> Vec<u8> {
        self.out
    }
}

/// One decoded entry.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct LineEntry {
    pub pc: u32,
    pub line: u32,
    pub col: u32,
}

/// Iterates over the entries of an encoded line table. Stops at the end or at
/// malformed data.
pub fn line_entries(bytes: &[u8]) -> LineEntries<'_> {
    LineEntries {
        bytes,
        at: 0,
        pc: 0,
        line: 1,
    }
}

pub struct LineEntries<'a> {
    bytes: &'a [u8],
    at: usize,
    pc: u32,
    line: u32,
}

impl Iterator for LineEntries<'_> {
    type Item = LineEntry;

    fn next(&mut self) -> Option<LineEntry> {
        if self.at >= self.bytes.len() {
            return None;
        }
        let pc_delta = read_uleb(self.bytes, &mut self.at)?;
        let line_delta = unzigzag(read_uleb(self.bytes, &mut self.at)?);
        let col = read_uleb(self.bytes, &mut self.at)?;
        let pc = self.pc as u64 + pc_delta;
        let line = self.line as i64 + line_delta;
        if pc > u32::MAX as u64 || !(1..=u32::MAX as i64).contains(&line) || col > u32::MAX as u64 {
            self.at = self.bytes.len();
            return None;
        }
        self.pc = pc as u32;
        self.line = line as u32;
        Some(LineEntry {
            pc: self.pc,
            line: self.line,
            col: col as u32,
        })
    }
}

/// The source position of the instruction at `pc`: the last entry at or before it.
pub fn lookup_line(bytes: &[u8], pc: u32) -> Option<(u32, u32)> {
    let mut found = None;
    for e in line_entries(bytes) {
        if e.pc > pc {
            break;
        }
        found = Some((e.line, e.col));
    }
    found
}

fn zigzag(v: i64) -> u64 {
    ((v << 1) ^ (v >> 63)) as u64
}

fn unzigzag(v: u64) -> i64 {
    ((v >> 1) as i64) ^ -((v & 1) as i64)
}

fn write_uleb(out: &mut Vec<u8>, mut v: u64) {
    loop {
        let b = (v & 0x7F) as u8;
        v >>= 7;
        if v == 0 {
            out.push(b);
            return;
        }
        out.push(b | 0x80);
    }
}

fn read_uleb(bytes: &[u8], at: &mut usize) -> Option<u64> {
    let mut v: u64 = 0;
    let mut shift = 0;
    loop {
        let b = *bytes.get(*at)?;
        *at += 1;
        if shift >= 64 {
            return None;
        }
        v |= u64::from(b & 0x7F) << shift;
        if b & 0x80 == 0 {
            return Some(v);
        }
        shift += 7;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leb_and_zigzag() {
        for v in [0u64, 1, 127, 128, 300, u32::MAX as u64, u64::MAX] {
            let mut out = Vec::new();
            write_uleb(&mut out, v);
            let mut at = 0;
            assert_eq!(read_uleb(&out, &mut at), Some(v));
            assert_eq!(at, out.len());
        }
        for v in [0i64, 1, -1, 63, -64, i32::MAX as i64, i32::MIN as i64] {
            assert_eq!(unzigzag(zigzag(v)), v);
        }
        assert_eq!(read_uleb(&[0x80], &mut 0), None);
    }
}
