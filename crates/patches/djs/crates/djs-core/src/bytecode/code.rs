//! The emitter: [`CodeBuf`], labels and jump fixups (docs/compiler.md 4.3).
//!
//! Two modes, chosen by type:
//!
//! - [`CodeBuf<Phase1>`]: what the parser emits. Labels stay symbolic: `place_label`
//!   emits the `label` pseudo-op and jump targets hold label ids.
//! - [`CodeBuf<Final>`] (the default): final code. `place_label` records the position;
//!   jumps are patched to relative offsets by `finish`. The resolution
//!   re-emit uses this mode with the phase-1 label ids
//!   ([`CodeBuf::with_labels`]), and so do hand-assembled tests.
//!
//! Every opcode has a typed emitter method generated from the table (`buf.get_loc(3)`,
//! `buf.call(2)`, `buf.jump(label)`).

use alloc::vec::Vec;
use core::marker::PhantomData;

use super::decode::{Insn, decode};
use super::ops::{Op, short_form};

/// A jump target, created by [`CodeBuf::new_label`].
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash, PartialOrd, Ord)]
pub struct Label(pub u32);

/// Label handling of a [`CodeBuf`]: [`Final`] or [`Phase1`].
pub trait LabelMode: sealed::Sealed {
    const SYMBOLIC: bool;
}

/// Final code: labels are resolved to offsets.
pub struct Final;
/// Phase-1 code: labels stay symbolic.
pub struct Phase1;

impl LabelMode for Final {
    const SYMBOLIC: bool = false;
}
impl LabelMode for Phase1 {
    const SYMBOLIC: bool = true;
}

mod sealed {
    pub trait Sealed {}
    impl Sealed for super::Final {}
    impl Sealed for super::Phase1 {}
}

const UNPLACED: u32 = u32::MAX;

/// Errors found by [`CodeBuf::finish`]. They are compiler bugs, not user errors.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CodeError {
    /// A jump refers to a label that was never placed.
    UnplacedLabel(Label),
    /// A label was placed twice.
    LabelPlacedTwice(Label),
    /// A label id that this buffer did not create.
    UnknownLabel(Label),
    /// The code does not fit `i32` offsets.
    TooLarge,
}

/// Finished final code.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Code {
    pub code: Vec<u8>,
    /// Position of every label (`u32::MAX` if never placed and never used).
    pub labels: Vec<u32>,
    /// Where `finish` shortened a jump, as positions in the code as emitted
    /// (ascending): [`Code::map_pos`] moves other positions recorded while emitting.
    shortened: Vec<u32>,
}

/// Bytes a shortened jump saves (`jump` 5 -> `jump8` 2).
const JUMP_SAVING: u32 = 3;

impl Code {
    /// The position of a placed label.
    pub fn label_pos(&self, l: Label) -> Option<u32> {
        self.labels
            .get(l.0 as usize)
            .copied()
            .filter(|&p| p != UNPLACED)
    }

    /// The final position of an instruction boundary recorded as `pos` (`pos()`)
    /// while emitting.
    pub fn map_pos(&self, pos: u32) -> u32 {
        map_pos(&self.shortened, pos)
    }
}

/// `pos` after the jumps starting at `shortened` (ascending) lost `JUMP_SAVING` bytes
/// each.
fn map_pos(shortened: &[u32], pos: u32) -> u32 {
    pos - JUMP_SAVING * shortened.partition_point(|&s| s < pos) as u32
}

/// A stored phase-1 instruction: the opcode and its raw immediates, 9 bytes (0096:
/// phase-1 code is never encoded as bytes, the re-emit reads the instructions; the
/// whole unit's phase-1 code stays alive until resolution, so it is packed).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(C, packed)]
pub struct P1 {
    a: u32,
    b: u32,
    op: Op,
}

impl P1 {
    /// The instruction, with `pc` = its index.
    pub fn insn(self, index: u32) -> Insn {
        Insn::from_parts(self.op, index, [self.a, self.b])
    }
}

/// Finished phase-1 code. An instruction's `pc` is its index.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Phase1Code {
    pub insns: Vec<P1>,
    /// Number of labels; pass it to [`CodeBuf::with_labels`] for the re-emit.
    pub label_count: u32,
}

impl Phase1Code {
    /// The instructions encoded as bytes (label ids in the jumps), for listings and
    /// tests.
    pub fn bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for (i, p) in self.insns.iter().enumerate() {
            let insn = p.insn(i as u32);
            out.push(insn.op as u8);
            for (i, &k) in insn.op.info().imms.iter().enumerate() {
                out.extend_from_slice(&insn.imm(i).to_le_bytes()[..k.width() as usize]);
            }
        }
        out
    }
}

/// A bytecode buffer with typed emitters.
pub struct CodeBuf<M: LabelMode = Final> {
    /// Final mode: the code.
    code: Vec<u8>,
    /// Phase-1 mode: the instructions (`pc` = index).
    insns: Vec<P1>,
    /// Final: label positions (`UNPLACED` until placed). Phase 1: 0/1 placed flags.
    labels: Vec<u32>,
    /// The start of each jump and the position of its `Target` immediate, which holds
    /// the label id (until `finish` patches it, in final mode).
    fixups: Vec<(u32, u32)>,
    /// Start of the last instruction, if nothing (a label) came after it.
    last: Option<u32>,
    error: Option<CodeError>,
    _mode: PhantomData<M>,
}

impl<M: LabelMode> Default for CodeBuf<M> {
    fn default() -> Self {
        Self::make(0)
    }
}

impl CodeBuf<Final> {
    /// An empty buffer for final code.
    pub fn new() -> Self {
        Self::make(0)
    }

    /// A final-code buffer with labels `0..count` already created: the re-emit reuses
    /// the label ids of the phase-1 code.
    pub fn with_labels(count: u32) -> Self {
        Self::make(count)
    }
}

impl CodeBuf<Phase1> {
    /// An empty buffer for phase-1 code.
    pub fn phase1() -> Self {
        let mut b = Self::make(0);
        // Most functions are small: start where doubling from 4 would take three steps.
        b.insns.reserve(32);
        b
    }
}

impl<M: LabelMode> CodeBuf<M> {
    fn make(count: u32) -> Self {
        let init = if M::SYMBOLIC { 0 } else { UNPLACED };
        CodeBuf {
            code: Vec::new(),
            insns: Vec::new(),
            labels: alloc::vec![init; count as usize],
            fixups: Vec::new(),
            last: None,
            error: None,
            _mode: PhantomData,
        }
    }

    pub fn new_label(&mut self) -> Label {
        let l = Label(self.labels.len() as u32);
        self.labels.push(if M::SYMBOLIC { 0 } else { UNPLACED });
        l
    }

    /// Room for `bytes` more code (0092: the re-emit knows its size roughly, so it
    /// does not grow the buffer step by step).
    pub fn reserve(&mut self, bytes: usize) {
        if M::SYMBOLIC {
            self.insns.reserve(bytes / 3);
        } else {
            self.code.reserve(bytes);
        }
    }

    pub fn label_count(&self) -> u32 {
        self.labels.len() as u32
    }

    /// Places `l` at the current position. Nothing emitted before a label can be removed
    /// with [`CodeBuf::truncate_last`] afterwards (a jump may land here).
    pub fn place_label(&mut self, l: Label) {
        let Some(&state) = self.labels.get(l.0 as usize) else {
            self.fail(CodeError::UnknownLabel(l));
            return;
        };
        let unplaced = if M::SYMBOLIC { 0 } else { UNPLACED };
        if state != unplaced {
            self.fail(CodeError::LabelPlacedTwice(l));
        }
        if M::SYMBOLIC {
            self.labels[l.0 as usize] = 1;
            self.label(l);
        } else {
            self.labels[l.0 as usize] = self.code.len() as u32;
        }
        self.last = None;
    }

    /// The current position (where the next instruction starts).
    /// The current position (where the next instruction starts): a byte offset in
    /// final code, an instruction index in phase-1 code.
    pub fn pos(&self) -> u32 {
        if M::SYMBOLIC {
            self.insns.len() as u32
        } else {
            self.code.len() as u32
        }
    }

    /// The final code so far (empty in phase-1 mode).
    pub fn code(&self) -> &[u8] {
        &self.code
    }

    pub fn is_empty(&self) -> bool {
        self.pos() == 0
    }

    /// The last instruction, unless a label was placed after it.
    pub fn last_insn(&self) -> Option<Insn> {
        let start = self.last?;
        if M::SYMBOLIC {
            return self.insns.get(start as usize).map(|p| p.insn(start));
        }
        decode(&self.code, start).ok()
    }

    /// Removes and returns the last instruction (lvalue conversion, docs/compiler.md 3.7).
    /// `None` if there is none or a label was placed after it.
    pub fn truncate_last(&mut self) -> Option<Insn> {
        let insn = self.last_insn()?;
        let start = insn.pc;
        if M::SYMBOLIC {
            self.insns.truncate(start as usize);
            self.last = None;
            return Some(insn);
        }
        self.code.truncate(start as usize);
        while self.fixups.last().is_some_and(|&f| f.0 >= start) {
            self.fixups.pop();
        }
        self.last = None;
        Some(insn)
    }

    /// Re-emits a decoded instruction. `Target` immediates are taken as label ids, so
    /// this copies phase-1 instructions (it is how the re-emit passes through everything
    /// that needs no rewriting).
    pub fn emit_insn(&mut self, insn: &Insn) {
        self.encode(insn.op, insn.imm(0), insn.imm(1));
    }

    /// Emits `op` with the raw immediates `a`, `b` (as many as it has: the low bytes of
    /// each, a label id for a `Target`). Every generated emitter calls this.
    ///
    /// It stays out of line: the per-opcode emitters are inlined into hundreds of
    /// compiler sites, and an inlined `Vec::push` (length check, grow call, store) at
    /// each of them was ~35 KB of wasm (D1571); one call with the opcode and the raw
    /// values is the smallest call site (0092).
    #[inline(never)]
    pub(super) fn encode(&mut self, op: Op, a: u32, b: u32) {
        if M::SYMBOLIC {
            // Phase 1: the instruction itself, and its label id checked.
            let insn = Insn::raw(op, self.insns.len() as u32, [a, b]);
            if let Some(l) = insn.label()
                && (l.0 as usize) >= self.labels.len()
            {
                self.fail(CodeError::UnknownLabel(l));
            }
            self.last = Some(insn.pc);
            self.insns.push(P1 {
                a: insn.imm(0),
                b: insn.imm(1),
                op,
            });
            return;
        }
        let start = self.code.len() as u32;
        // Short forms (D2443): jumps are emitted wide and shortened by `finish`;
        // `get_builtin idx; undefined` becomes one instruction.
        let mut op = op.wide();
        if let Some(s) = short_form(op, a, b) {
            op = s;
        } else if op == Op::Undefined
            && let Some(prev) = self.last
            && self.code[prev as usize] == Op::GetBuiltin as u8
            && self.code[prev as usize + 2] == 0
        {
            // (`get_builtin idx` with idx < 256: its high byte is 0.)
            self.code[prev as usize] = Op::GetBuiltinCallee8 as u8;
            self.code.truncate(prev as usize + 2);
            return;
        }
        self.last = Some(start);
        // The instruction in a local buffer, then one append.
        let mut bytes = [0u8; 9];
        bytes[0] = op as u8;
        let mut p = 1;
        for (i, &kind) in op.info().imms.iter().enumerate() {
            let v = if i == 0 { a } else { b };
            if kind.is_target() {
                if (v as usize) >= self.labels.len() {
                    self.fail(CodeError::UnknownLabel(Label(v)));
                }
                self.fixups.push((start, start + p as u32));
            }
            bytes[p..p + 4].copy_from_slice(&v.to_le_bytes());
            p += kind.width() as usize;
        }
        // The whole buffer and then the length: an append of a length known here is
        // two stores, one of `p` bytes is a call to `memmove`.
        let len = self.code.len();
        self.code.extend_from_slice(&bytes);
        self.code.truncate(len + p);
    }

    fn fail(&mut self, e: CodeError) {
        self.error.get_or_insert(e);
    }
}

impl CodeBuf<Final> {
    /// Resolves the jump targets, shortens the jumps whose offset fits a byte
    /// (D2443), and returns the code.
    pub fn finish(self) -> Result<Code, CodeError> {
        if let Some(e) = self.error {
            return Err(e);
        }
        if self.code.len() > i32::MAX as usize {
            return Err(CodeError::TooLarge);
        }
        let CodeBuf {
            code,
            mut labels,
            fixups,
            ..
        } = self;
        // Each fixup: the instruction's start and the target's position, as emitted
        // (a jump's target is its last immediate, `iter_next_loc`'s after its local).
        let mut jumps = crate::buf::vec_with_capacity(fixups.len());
        let mut target_at = crate::buf::vec_with_capacity(fixups.len());
        for &(start, at) in &fixups {
            let at = at as usize;
            let id = u32::from_le_bytes(code[at..at + 4].try_into().unwrap_or([0; 4]));
            let target = labels[id as usize];
            if target == UNPLACED {
                return Err(CodeError::UnplacedLabel(Label(id)));
            }
            jumps.push((start, target));
            target_at.push(at as u32);
        }
        // Start with every shortenable jump short and widen the ones whose offset
        // does not fit, until none changes: widening only moves targets further away,
        // so this ends.
        let mut short: Vec<bool> = jumps
            .iter()
            .map(|&(start, _)| {
                Op::from_u8(code[start as usize])
                    .and_then(Op::short_jump)
                    .is_some()
            })
            .collect();
        let mut shortened: Vec<u32> = Vec::new();
        loop {
            shortened.clear();
            shortened.extend(jumps.iter().zip(&short).filter(|j| *j.1).map(|j| j.0.0));
            let mut changed = false;
            for (&(start, target), s) in jumps.iter().zip(short.iter_mut()) {
                if *s {
                    let end = map_pos(&shortened, start) + 2;
                    let rel = map_pos(&shortened, target) as i64 - end as i64;
                    if i8::try_from(rel).is_err() {
                        *s = false;
                        changed = true;
                    }
                }
            }
            if !changed {
                break;
            }
        }
        // The code with the final offsets.
        let mut out =
            crate::buf::vec_with_capacity(code.len() - JUMP_SAVING as usize * shortened.len());
        let mut from = 0usize;
        for ((&(start, target), &s), &at) in jumps.iter().zip(&short).zip(&target_at) {
            let (start_us, at_us) = (start as usize, at as usize);
            out.extend_from_slice(&code[from..start_us]);
            let new_target = map_pos(&shortened, target) as i64;
            let op = Op::from_u8(code[start_us]).unwrap_or(Op::Nop);
            match op.short_jump() {
                Some(short_op) if s => {
                    let rel = new_target - (out.len() as i64 + 2);
                    out.push(short_op as u8);
                    out.push(rel as i8 as u8);
                }
                _ => {
                    // The opcode and the immediates before the target, then the offset
                    // from the instruction's end.
                    let rel = new_target - (out.len() as i64 + (at_us - start_us) as i64 + 4);
                    out.extend_from_slice(&code[start_us..at_us]);
                    out.extend_from_slice(&(rel as i32).to_le_bytes());
                }
            }
            from = at_us + 4;
        }
        out.extend_from_slice(&code[from..]);
        for l in labels.iter_mut().filter(|l| **l != UNPLACED) {
            *l = map_pos(&shortened, *l);
        }
        Ok(Code {
            code: out,
            labels,
            shortened,
        })
    }
}

impl CodeBuf<Phase1> {
    /// Moves the code in `mid..end` in front of `start..mid` (loop rotation, 0086).
    /// Phase-1 code refers to labels, not positions, so only the jump fixups move along.
    /// Nothing before `end` can be truncated afterwards, so the fixups need not stay
    /// sorted: `truncate_last` only pops the ones of instructions emitted later.
    pub fn rotate(&mut self, start: u32, mid: u32, end: u32) {
        debug_assert!(start <= mid && mid <= end && end as usize <= self.insns.len());
        let (start, mid, end) = (start as usize, mid as usize, end as usize);
        // (A copy, not `rotate_left`: less code in the wasm build.)
        let moved = self.insns[mid..end].to_vec();
        self.insns.copy_within(start..mid, start + moved.len());
        self.insns[start..start + moved.len()].copy_from_slice(&moved);
        self.last = None;
    }

    /// Checks that every label used by a jump was placed and returns the code.
    pub fn finish(self) -> Result<Phase1Code, CodeError> {
        if let Some(e) = self.error {
            return Err(e);
        }
        for l in self.insns.iter().filter_map(|p| p.insn(0).label()) {
            if self.labels[l.0 as usize] == 0 {
                return Err(CodeError::UnplacedLabel(l));
            }
        }
        Ok(Phase1Code {
            insns: self.insns,
            label_count: self.labels.len() as u32,
        })
    }
}
