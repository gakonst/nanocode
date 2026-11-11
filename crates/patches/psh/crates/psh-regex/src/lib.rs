//! POSIX regular expressions for psh: extended (ERE) and basic (BRE) syntax with the GNU
//! extensions bash and the GNU tools accept, leftmost-longest matching, subexpression
//! positions and back-references.
//!
//! The behavior follows what GNU's `regcomp`/`regexec` do (bash's `[[ =~ ]]` and the
//! GNU tools use them), checked by differential tests against GNU libc in the recording
//! container: the overall match is the leftmost-longest one; within it, subexpressions
//! take what a backtracking matcher trying alternatives in order and repetitions
//! greedily finds first (GNU's choice, which differs from strict POSIX in a few corner
//! cases such as `(a|ab)(c|bcd)`).
//!
//! Matching is linear in the text for patterns without back-references: the bounds of
//! the match come from an NFA simulation (one pass, all states at once), the
//! subexpressions from a backtracking search that never visits a (state, position) pair
//! twice. Back-references make matching NP-hard; patterns with them use a backtracking
//! search with a step budget.
//!
//! `no_std` + `alloc`, no dependencies.
#![no_std]

extern crate alloc;

mod class;
mod compile;
mod exec;
mod parse;
mod rust;

use alloc::vec::Vec;

pub use rust::{Translation, to_rust_syntax};

/// How a pattern is read and matched (the `regcomp` flags).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Options {
    /// Extended syntax (ERE, `REG_EXTENDED`); otherwise basic (BRE).
    pub extended: bool,
    /// Case-insensitive (`REG_ICASE`).
    pub icase: bool,
    /// Newline-sensitive (`REG_NEWLINE`): `.` and `[^…]` do not match a newline, `^`
    /// and `$` also match after and before one.
    pub newline: bool,
    /// UTF-8 text: a character is a UTF-8 sequence (a byte that is not part of a valid
    /// sequence is a character of its own); otherwise every byte is a character (the C
    /// locale).
    pub utf8: bool,
}

/// Flags of one match (the `regexec` flags).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ExecOptions {
    /// The start of the text is not the start of a line (`REG_NOTBOL`).
    pub not_bol: bool,
    /// The end of the text is not the end of a line (`REG_NOTEOL`).
    pub not_eol: bool,
}

/// Why a pattern does not compile. The codes are those of GNU's `regcomp`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    BadPattern = 2,
    Collate = 3,
    Ctype = 4,
    Escape = 5,
    Subreg = 6,
    Brack = 7,
    Paren = 8,
    Brace = 9,
    BadBrace = 10,
    Range = 11,
    Space = 12,
    BadRepeat = 13,
    End = 14,
    Size = 15,
    RParen = 16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Error {
    pub kind: ErrorKind,
}

impl Error {
    pub(crate) fn new(kind: ErrorKind) -> Error {
        Error { kind }
    }

    /// GNU's message for the error (what `regerror` returns).
    pub fn message(&self) -> &'static str {
        match self.kind {
            ErrorKind::BadPattern => "Invalid regular expression",
            ErrorKind::Collate => "Invalid collation character",
            ErrorKind::Ctype => "Invalid character class name",
            ErrorKind::Escape => "Trailing backslash",
            ErrorKind::Subreg => "Invalid back reference",
            ErrorKind::Brack => "Unmatched [, [^, [:, [., or [=",
            ErrorKind::Paren => "Unmatched ( or \\(",
            ErrorKind::Brace => "Unmatched \\{",
            ErrorKind::BadBrace => "Invalid content of \\{\\}",
            ErrorKind::Range => "Invalid range end",
            ErrorKind::Space => "Memory exhausted",
            ErrorKind::BadRepeat => "Invalid preceding regular expression",
            ErrorKind::End => "Premature end of regular expression",
            ErrorKind::Size => "Regular expression too big",
            ErrorKind::RParen => "Unmatched ) or \\)",
        }
    }
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.message())
    }
}

/// A compiled pattern.
#[derive(Clone, Debug)]
pub struct Regex {
    prog: compile::Prog,
    groups: usize,
    opts: Options,
}

/// The positions of a match: `[0]` is the whole match, `[n]` the `n`th parenthesized
/// subexpression (`None` when it did not take part). Byte offsets into the text.
pub type Captures = Vec<Option<(usize, usize)>>;

impl Regex {
    pub fn new(pattern: &[u8], opts: Options) -> Result<Regex, Error> {
        let parsed = parse::parse(pattern, opts)?;
        let prog = compile::compile(&parsed, opts)?;
        Ok(Regex {
            prog,
            groups: parsed.groups,
            opts,
        })
    }

    /// The number of parenthesized subexpressions (`re_nsub`).
    pub fn groups(&self) -> usize {
        self.groups
    }

    pub fn options(&self) -> Options {
        self.opts
    }

    /// The leftmost-longest match in `text` with its subexpressions, or `None`.
    pub fn exec(&self, text: &[u8], eopts: ExecOptions) -> Option<Captures> {
        exec::exec(&self.prog, self.groups, text, 0, eopts, true)
    }

    /// The bounds of the leftmost-longest match only (cheaper: no subexpressions).
    pub fn find(&self, text: &[u8], eopts: ExecOptions) -> Option<(usize, usize)> {
        exec::exec(&self.prog, self.groups, text, 0, eopts, false).and_then(|c| c[0])
    }

    /// [`Self::exec`] for a match that starts at `start` or later, with the text before
    /// it in view of `^` (start of line), `\b`, `\<` and the like: the next match of a
    /// global substitution. Offsets are into the whole `text`.
    pub fn exec_at(&self, text: &[u8], start: usize, eopts: ExecOptions) -> Option<Captures> {
        exec::exec(&self.prog, self.groups, text, start, eopts, true)
    }

    /// [`Self::find`] from `start`, as [`Self::exec_at`].
    pub fn find_at(&self, text: &[u8], start: usize, eopts: ExecOptions) -> Option<(usize, usize)> {
        exec::exec(&self.prog, self.groups, text, start, eopts, false).and_then(|c| c[0])
    }

    pub fn is_match(&self, text: &[u8]) -> bool {
        self.find(text, ExecOptions::default()).is_some()
    }
}

#[cfg(test)]
mod tests;
