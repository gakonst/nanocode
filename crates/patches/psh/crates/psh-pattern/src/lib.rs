#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

pub mod ast;
pub mod bytes;
mod class;
mod dfa;
mod engine;
pub mod error;
mod extract;
pub mod hir;
mod literal;
mod look;
pub mod matcher;
mod meta;
mod nfa;
mod pikevm;
mod string;
mod translate;
mod unicode;
mod unicode_data;
mod utf8;

use alloc::string::String;
use core::fmt;

pub use crate::bytes::SetMatches;
pub use crate::string::{
    CaptureLocations, CaptureMatches, Captures, Match, Matches, NoExpand, Regex, RegexBuilder,
    RegexSet, RegexSetBuilder, Replacer, Split, SplitN,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Error {
    Syntax(String),
    CompiledTooBig(usize),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Syntax(message) => f.write_str(message),
            Error::CompiledTooBig(limit) => {
                write!(f, "Compiled regex exceeds size limit of {limit} bytes.")
            }
        }
    }
}

impl core::error::Error for Error {}

pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if ast::is_meta_character(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

pub mod syntax {
    pub use crate::ast::{Ast, ParseOptions, is_meta_character, parse};
    pub use crate::hir::{Hir, HirKind, Look};
    pub use crate::meta::Options;

    pub fn hir(pattern: &str, options: &Options, utf8: bool) -> Result<Hir, crate::Error> {
        crate::meta::hir(pattern, options, utf8)
    }
}
