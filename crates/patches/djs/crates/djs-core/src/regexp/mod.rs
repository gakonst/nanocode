//! The regular expression engine: a backtracking matcher over compact, pointer-free
//! bytecode, fuel-metered and resumable (ARCH.md, "regex").
//!
//! - [`Flags::parse`] parses a flags string (`"gimsuvyd"`).
//! - [`compile`] parses a pattern (UTF-16 code units) into a [`Program`], or returns a
//!   [`SyntaxError`] with QuickJS' message.
//! - [`exec`] runs a program on a Latin-1 or UTF-16 input from a start index, with a fuel
//!   budget. It returns the capture positions (UTF-16 indices), no match, or an
//!   [`ExecState`] to [`resume`] later. The state is plain data ([`ExecState::to_words`]).
//!   [`exec_in`] and [`resume_in`] do the same on a program read in place ([`Prog`]) and
//!   in a state the caller keeps, which one state can do match after match (D3135).
//!
//! `g` and `y` are the caller's business except that a sticky program only matches at the
//! start index. The module has no heap or VM dependencies.

mod charset;
mod compiler;
mod exec;
mod parser;

#[cfg(test)]
mod corpus_tests;
#[cfg(test)]
mod tests;

use alloc::string::String;
use alloc::vec::Vec;

pub use exec::{
    Caps, Captures, ExecLimits, ExecResult, ExecState, Input, Outcome, Prog, exec, exec_at,
    exec_in, resume, resume_in,
};

/// RegExp flags.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Flags(u16);

impl Flags {
    pub const HAS_INDICES: Flags = Flags(1 << 0);
    pub const GLOBAL: Flags = Flags(1 << 1);
    pub const IGNORE_CASE: Flags = Flags(1 << 2);
    pub const MULTILINE: Flags = Flags(1 << 3);
    pub const DOT_ALL: Flags = Flags(1 << 4);
    pub const UNICODE: Flags = Flags(1 << 5);
    pub const UNICODE_SETS: Flags = Flags(1 << 6);
    pub const STICKY: Flags = Flags(1 << 7);

    pub const fn empty() -> Flags {
        Flags(0)
    }

    pub const fn bits(self) -> u16 {
        self.0
    }

    pub const fn from_bits(bits: u16) -> Flags {
        Flags(bits & 0xFF)
    }

    pub const fn contains(self, other: Flags) -> bool {
        self.0 & other.0 == other.0
    }

    pub const fn union(self, other: Flags) -> Flags {
        Flags(self.0 | other.0)
    }

    /// `u` or `v`: the pattern and the input are code points.
    pub const fn unicode_mode(self) -> bool {
        self.0 & (Self::UNICODE.0 | Self::UNICODE_SETS.0) != 0
    }

    /// Parses a flags string. Unknown or repeated letters, or `u` with `v`, are a
    /// SyntaxError.
    pub fn parse(s: &[u16]) -> Result<Flags, SyntaxError> {
        let mut f = Flags(0);
        for &c in s {
            let bit = match c {
                0x64 => Self::HAS_INDICES,
                0x67 => Self::GLOBAL,
                0x69 => Self::IGNORE_CASE,
                0x6D => Self::MULTILINE,
                0x73 => Self::DOT_ALL,
                0x75 => Self::UNICODE,
                0x76 => Self::UNICODE_SETS,
                0x79 => Self::STICKY,
                _ => return Err(SyntaxError::flags()),
            };
            if f.contains(bit) {
                return Err(SyntaxError::flags());
            }
            f = f.union(bit);
        }
        if f.contains(Self::UNICODE) && f.contains(Self::UNICODE_SETS) {
            return Err(SyntaxError::flags());
        }
        Ok(f)
    }
}

/// A pattern or flags SyntaxError, with QuickJS' message.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SyntaxError {
    message: String,
}

impl SyntaxError {
    fn new(message: String) -> SyntaxError {
        SyntaxError { message }
    }

    fn flags() -> SyntaxError {
        SyntaxError::new(String::from("invalid regular expression flags"))
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

/// A compiled regular expression: header, group names and code, all `u32` words with no
/// pointers, so it can be stored in a heap cell as is.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Program {
    words: Vec<u32>,
}

impl Program {
    /// The program as words (for storing it).
    pub fn words(&self) -> &[u32] {
        &self.words
    }

    /// Rebuilds a program from [`Program::words`] of the same build.
    pub fn from_words(words: Vec<u32>) -> Program {
        Program { words }
    }

    pub fn flags(&self) -> Flags {
        Flags::from_bits(self.words[compiler::H_FLAGS] as u16)
    }

    /// Number of capture groups, including group 0 (the whole match).
    pub fn capture_count(&self) -> usize {
        self.words[compiler::H_CAPTURES] as usize
    }

    /// The name of capture group `index` as UTF-16, if it has one.
    pub fn group_name(&self, index: usize) -> Option<&[u32]> {
        compiler::group_name(&self.words, index)
    }

    /// True if any group has a name.
    pub fn has_group_names(&self) -> bool {
        self.words[compiler::H_MODE] & compiler::M_NAMES != 0
    }
}

/// Compiles `pattern` (UTF-16 code units, as in the JS string) with `flags`.
pub fn compile(pattern: &[u16], flags: Flags) -> Result<Program, SyntaxError> {
    let parsed = parser::parse(pattern, flags)?;
    Ok(compiler::compile(&parsed, flags))
}
