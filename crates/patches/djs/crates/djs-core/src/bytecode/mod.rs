//! Bytecode: the instruction table and everything derived from it
//! (docs/compiler.md section 4).
//!
//! - [`ops`]: the `ops!` table, [`Op`], [`OpInfo`], [`OPS`], [`OPS_HASH`].
//! - [`imm`]: immediate kinds and their encodings.
//! - [`code`]: [`CodeBuf`], the emitter with typed per-opcode methods, labels and fixups.
//! - [`decode`]: [`Insn`], [`decode`](decode::decode), [`insns`], raw readers for the
//!   interpreter.
//! - [`stack`]: the stack-depth checker.
//! - [`disasm`]: the disassembler; [`template`] adds heap helpers and template listings.
//! - [`lines`], [`handlers`]: line table and exception handler table encodings.
//!
//! Encoding: an instruction is the opcode byte followed by its immediates, little-endian
//! and unaligned (D1082). Jumps are 5 bytes with an `i32` offset relative to the end of
//! the instruction. Every opcode has a fixed size ([`Op::size`]).

pub mod code;
pub mod decode;
pub mod disasm;
pub mod handlers;
pub mod imm;
pub mod intrinsics;
pub mod lines;
pub mod ops;
pub mod stack;
pub mod template;

#[cfg(test)]
mod tests;

pub use code::{Code, CodeBuf, CodeError, Final, Label, P1, Phase1, Phase1Code};
pub use decode::{DecodeError, Insn, insns};
pub use disasm::{DisasmContext, disasm};
pub use handlers::{Handler, HandlerTable, encode_handlers};
pub use imm::ImmKind;
pub use lines::{LineTableBuilder, line_entries, lookup_line};
pub use ops::{
    CostClass, OP_COUNT, OP_NAMES, OPS, OPS_HASH, Op, OpInfo, build_hash, capture_desc, flags,
    global_decls, special,
};
pub use stack::{StackError, StackScratch, UNKNOWN_DEPTH, check_stack, stack_depths};
pub use template::{
    TemplateError, TemplateParts, alloc_script, alloc_template, alloc_template_in, bytes_of,
    func_flags,
};
