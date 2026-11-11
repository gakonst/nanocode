//! Decoding: [`Insn`], [`decode`], the [`Insns`] iterator, and raw immediate readers for
//! the interpreter.

use super::code::Label;
use super::ops::{OP_COUNT, OPS, Op, OpInfo};
use crate::heap::Atom;

/// A decoded instruction.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Insn {
    pub op: Op,
    /// Position of the opcode byte (in phase-1 code: the instruction's index).
    pub pc: u32,
    /// Raw immediates: unsigned kinds zero-extended, signed kinds as `i32` bits.
    imms: [u32; 2],
}

/// Why bytes are not a valid instruction.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DecodeError {
    InvalidOpcode {
        pc: u32,
        byte: u8,
    },
    /// The instruction runs past the end of the code.
    Truncated {
        pc: u32,
    },
}

impl Insn {
    pub fn info(&self) -> &'static OpInfo {
        self.op.info()
    }

    /// The same instruction with opcode `op`, which must have the same immediate layout
    /// (the peepholes turn `name_set` into `name_put`, `not; jump_if_false` into
    /// `jump_if_true`).
    pub fn with_op(self, op: Op) -> Insn {
        debug_assert_eq!(op.info().imms, self.op.info().imms);
        Insn { op, ..self }
    }

    /// An instruction from raw immediates as `CodeBuf` encodes them (their low bytes);
    /// normalized as `decode` reads them back (narrow kinds masked, `I8` sign-extended).
    pub(crate) fn raw(op: Op, pc: u32, mut imms: [u32; 2]) -> Insn {
        let mut layout = LAYOUT[op as usize];
        let mut i = 0;
        while layout != 0 {
            imms[i] = match layout & 0xF {
                1 => imms[i] & 0xFF,
                2 => imms[i] as u8 as i8 as i32 as u32,
                3 => imms[i] & 0xFFFF,
                _ => imms[i],
            };
            layout >>= 4;
            i += 1;
        }
        Insn { op, pc, imms }
    }

    /// An instruction from already normalized immediates (as `raw` left them).
    pub(crate) fn from_parts(op: Op, pc: u32, imms: [u32; 2]) -> Insn {
        Insn { op, pc, imms }
    }

    /// An instruction without immediates (`return` in place of a jump to one).
    pub fn bare(op: Op, pc: u32) -> Insn {
        debug_assert!(op.info().imms.is_empty());
        Insn {
            op,
            pc,
            imms: [0; 2],
        }
    }

    /// A phase-1 `label` (placed before the instruction at `pc`).
    pub fn label_at(id: u32, pc: u32) -> Insn {
        Insn {
            op: Op::Label,
            pc,
            imms: [id, 0],
        }
    }

    /// A phase-1 jump retargeted to label `l`.
    pub fn with_label(self, l: Label) -> Insn {
        let mut insn = self;
        if let Some(i) = self.target_index() {
            insn.imms[i] = l.0;
        }
        insn
    }

    /// Encoded size in bytes.
    pub fn size(&self) -> u32 {
        self.op.size() as u32
    }

    pub fn next_pc(&self) -> u32 {
        self.pc + self.size()
    }

    /// Immediate `i` as a raw `u32`.
    pub fn imm(&self, i: usize) -> u32 {
        self.imms[i]
    }

    /// Immediate `i` as a signed value (for `I8`, `I32` and final-code `Target`s).
    pub fn imm_i32(&self, i: usize) -> i32 {
        self.imms[i] as i32
    }

    /// Immediate `i` as an atom.
    pub fn atom(&self, i: usize) -> Atom {
        Atom::from_id(self.imms[i])
    }

    /// The `Target` of a phase-1 jump, as a label.
    pub fn label(&self) -> Option<Label> {
        self.target_index().map(|i| Label(self.imms[i]))
    }

    /// The absolute target of a final-code jump.
    pub fn jump_target(&self) -> Option<u32> {
        let i = self.target_index()?;
        Some((self.next_pc() as i64 + self.imm_i32(i) as i64) as u32)
    }

    /// The number of values popped (resolving `pop + imm` for calls and counts).
    pub fn pops(&self) -> u32 {
        let info = self.info();
        info.pop as u32
            + info.pop_imm.map_or(0, |k| self.imms[k as usize])
            + info.pop_arity.map_or(0, |k| arity(self.imms[k as usize]))
    }

    pub fn pushes(&self) -> u32 {
        let info = self.info();
        info.push as u32 + info.push_imm.map_or(0, |k| self.imms[k as usize])
    }

    fn target_index(&self) -> Option<usize> {
        (TARGET[self.op as usize] as usize).checked_sub(1)
    }
}

/// Per opcode, the number of its immediates if the last one is a `Target` (a jump's
/// target is its last immediate), else 0: one table load instead of a look at
/// `OpInfo::imms` for every jump the compiler's passes ask about.
const TARGET: [u8; OP_COUNT] = {
    let mut t = [0u8; OP_COUNT];
    let mut i = 0;
    while i < OP_COUNT {
        let imms = OPS[i].imms;
        if !imms.is_empty() && imms[imms.len() - 1].is_target() {
            t[i] = imms.len() as u8;
        }
        i += 1;
    }
    t
};

/// The declared parameters of intrinsic `id` (`call_intrinsic8`'s argument count; 0
/// for an unknown id, which the interpreter rejects).
#[inline]
pub fn arity(id: u32) -> u32 {
    super::intrinsics::params(id)
}

/// Per opcode, how its immediates decode (0092: one table load instead of walking
/// `OpInfo::imms`): bits 0-3 and 4-7 describe immediate 0 and 1 (0 none, 1 u8, 2 i8,
/// 3 u16, 4 u32/i32).
pub(crate) const LAYOUT: [u8; OP_COUNT] = {
    let mut t = [0u8; OP_COUNT];
    let mut i = 0;
    while i < OP_COUNT {
        let imms = OPS[i].imms;
        let mut j = 0;
        while j < imms.len() {
            let code = match imms[j].width() {
                1 if imms[j].is_signed() => 2,
                1 => 1,
                2 => 3,
                _ => 4,
            };
            t[i] |= code << (4 * j);
            j += 1;
        }
        i += 1;
    }
    t
};

/// Decodes the instruction at `pc`.
pub fn decode(code: &[u8], pc: u32) -> Result<Insn, DecodeError> {
    let at = pc as usize;
    let byte = *code.get(at).ok_or(DecodeError::Truncated { pc })?;
    let op = Op::from_u8(byte).ok_or(DecodeError::InvalidOpcode { pc, byte })?;
    let Some(b) = code.get(at + 1..at + op.size()) else {
        return Err(DecodeError::Truncated { pc });
    };
    let mut imms = [0u32; 2];
    let mut layout = LAYOUT[op as usize];
    let mut p = 0;
    let mut i = 0;
    while layout != 0 {
        let (v, w) = match layout & 0xF {
            1 => (u32::from(b[p]), 1),
            2 => (b[p] as i8 as i32 as u32, 1),
            3 => (u32::from(u16::from_le_bytes([b[p], b[p + 1]])), 2),
            _ => (u32::from_le_bytes([b[p], b[p + 1], b[p + 2], b[p + 3]]), 4),
        };
        imms[i] = v;
        p += w;
        i += 1;
        layout >>= 4;
    }
    Ok(Insn { op, pc, imms })
}

/// Iterates over the instructions of `code` from the start. After an error it yields
/// that error once and stops.
pub fn insns(code: &[u8]) -> Insns<'_> {
    Insns {
        code,
        pc: 0,
        done: false,
    }
}

pub struct Insns<'a> {
    code: &'a [u8],
    pc: u32,
    done: bool,
}

impl Iterator for Insns<'_> {
    type Item = Result<Insn, DecodeError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done || self.pc as usize >= self.code.len() {
            return None;
        }
        let r = decode(self.code, self.pc);
        match r {
            Ok(insn) => self.pc = insn.next_pc(),
            Err(_) => self.done = true,
        }
        Some(r)
    }
}

// Raw readers for the interpreter's dispatch loop, which knows each opcode's layout
// from the table (`pc + 1` is the first immediate). Out-of-range reads panic (a wasm
// trap on corrupt code), never undefined behavior.

#[inline]
pub fn read_u8(code: &[u8], at: usize) -> u8 {
    code[at]
}

#[inline]
pub fn read_i8(code: &[u8], at: usize) -> i8 {
    code[at] as i8
}

#[inline]
pub fn read_u16(code: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([code[at], code[at + 1]])
}

#[inline]
pub fn read_u32(code: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([code[at], code[at + 1], code[at + 2], code[at + 3]])
}

#[inline]
pub fn read_i32(code: &[u8], at: usize) -> i32 {
    read_u32(code, at) as i32
}
