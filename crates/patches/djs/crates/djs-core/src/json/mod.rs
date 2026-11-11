//! Native JSON: the leaf parts of `JSON.parse` and `JSON.stringify` (ARCH "The leaf
//! rule").
//!
//! - [`parse`] / [`parse_utf8`]: `JSON.parse` without a reviver, straight into heap values.
//!   It never runs user code, so the whole parse is one native op.
//! - [`stringify_fast`]: `JSON.stringify` without a replacer, for plain data. Anything that
//!   could run user code (accessors, `toJSON`, wrapper objects, exotic objects) makes it
//!   return [`StringifyError::Slow`] before it has had any effect other than allocating
//!   garbage, and the caller then runs the full algorithm.
//!
//! Both follow the GC contract: they allocate but never collect, and hold intermediate
//! values in Rust memory, which is safe because no collection can happen until they
//! return. The caller checks [`Heap::gc_pending`](crate::heap::Heap::gc_pending) afterwards.
//!
//! Messages match QuickJS (`SyntaxError` for parse errors, `TypeError: circular reference`,
//! `RangeError: Maximum call stack size exceeded` for nesting, D013).

mod parse;
mod stringify;

#[cfg(test)]
mod corpus_tests;
#[cfg(test)]
mod tests;

use alloc::string::String;
use core::fmt;

pub use parse::{ParseOptions, parse, parse_utf8};
pub use stringify::{
    StringifyError, gap_from_number, gap_from_units, quote, stringify_fast, stringify_fast_at,
};

use crate::heap::HeapError;
#[cfg(test)]
use stringify::SPILL_AT;

/// Default maximum nesting depth of arrays and objects, for parse and stringify. QuickJS'
/// limit depends on its C stack (about 6500 levels for parse and 4600 for stringify on a
/// typical build); ours is fixed.
pub const DEFAULT_MAX_DEPTH: u32 = 5000;

/// A JSON syntax error, positioned like QuickJS: `line` and `column` are 1-based, lines end
/// at `\n` only, and columns count code points. `offset` is the position in the input's code
/// units (bytes for Latin-1 and UTF-8 input).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SyntaxError {
    pub message: String,
    pub offset: u32,
    pub line: u32,
    pub column: u32,
}

impl SyntaxError {
    /// `SyntaxError: <message> at <input>:<line>:<column>` (the Display form, built
    /// without core::fmt, D2339).
    pub fn text(&self) -> String {
        crate::text::cat!(
            "SyntaxError: ",
            self.message,
            " at <input>:",
            self.line,
            ":",
            self.column
        )
    }
}

impl fmt::Display for SyntaxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text())
    }
}

/// Why [`parse`] failed.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ParseError {
    /// Invalid JSON: a JS `SyntaxError`.
    Syntax(SyntaxError),
    /// The heap ran out of memory, or a string got too long.
    Heap(HeapError),
}

impl From<HeapError> for ParseError {
    fn from(e: HeapError) -> ParseError {
        ParseError::Heap(e)
    }
}

/// The high bit of each byte of `w` that a JSON string can't hold as it is: a control
/// character, `"` or `\\` (stringify escapes them, parse stops at them). The tests can
/// flag bytes above a real one, never below, so the lowest flag is exact (0129).
#[inline]
pub(crate) fn escape_mask8(w: u64) -> u64 {
    const ONES: u64 = 0x0101_0101_0101_0101;
    const HIGH: u64 = 0x8080_8080_8080_8080;
    let lt = |x: u64, n: u64| x.wrapping_sub(ONES * n) & !x & HIGH;
    let zero = |x: u64| x.wrapping_sub(ONES) & !x & HIGH;
    lt(w, 0x20) | zero(w ^ (ONES * 0x22)) | zero(w ^ (ONES * 0x5C))
}
