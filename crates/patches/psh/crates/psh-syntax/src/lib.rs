//! The psh parser: lexer, recursive-descent parser, AST with byte spans and a printer.
//!
//! `no_std` + `alloc` with zero dependencies, so the parser runs anywhere the classifier
//! does, including `wasm32-unknown-unknown`. Nothing here may touch the OS.
//!
//! ```
//! let ast = psh_syntax::parse("echo hi | wc -l").unwrap();
//! let list = ast.list(ast.root());
//! assert_eq!(list.items.len(), 1);
//! assert_eq!(list.items[0].first.stages.len(), 2);
//! ```
#![no_std]

extern crate alloc;

mod alias;
mod arith;
mod arith_eval;
mod ast;
mod bash_print;
mod brace;
mod dump;
mod error;
mod outline;
mod parser;
mod printer;
mod remap;
mod source;
mod span;
mod words;

#[cfg(test)]
mod tests;

pub use alias::{AliasUse, Aliases};
pub use arith::{
    ArithError, ArithErrorKind, ArithExpr, ArithKind, BinOp, LValue, UnaryOp, arith_dump,
    parse_arith,
};
pub use arith_eval::{
    ARITH_RECURSION_LIMIT, ArithEnv, ArithIndex, EvalError, eval_arith, eval_arith_expr,
};
pub use ast::*;
pub use bash_print::print_function_bash;
pub use dump::dump;
pub use error::ParseError;
pub use outline::{OutlineRow, TEXT_CHARS, outline, outline_text};
pub use parser::{Options, Parser, parse, parse_bytes, parse_bytes_with_aliases, parse_with};
pub use printer::print;
pub use source::Source;
pub use span::{LineCol, LineIndex, Span};
pub use words::{backtick_body, decode_ansi_c, literal_text};
