//! Parse errors.

use alloc::string::String;
use core::fmt;

use crate::span::Span;

/// A syntax error: where, and bash's message for it (without bash's `line N:` prefix).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseError {
    /// The offending token, or an empty span at the end of the input.
    pub span: Span,
    pub message: String,
    /// The input ended inside a construct: more input could complete it.
    pub incomplete: bool,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}
