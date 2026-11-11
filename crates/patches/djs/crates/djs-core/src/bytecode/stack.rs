//! The stack-depth checker (docs/compiler.md 4.4).
//!
//! Walks every reachable instruction of final code and computes the operand stack depth
//! before it, from the table's stack effects. It rejects code where the depth would go
//! negative, where two paths reach an instruction with different depths, where a jump
//! lands inside an instruction or outside the code, or where execution can fall off the
//! end. The result is the maximum depth, which the frame reserves (`FuncTemplate::STACK`).
//!
//! Successors: the next instruction, unless the op is `JUMP` or `TERM`; the jump target
//! for `JUMP`/`BRANCH`; for `gosub`, the target with the return address pushed and the
//! next instruction at the current depth (where `ret` comes back, having popped the
//! address). Handler targets are entry points with `depth + 1` (the exception).
//! Unreachable code is decoded but not checked.

use alloc::vec::Vec;

use super::decode::{DecodeError, decode};
use super::handlers::Handler;
use super::imm::ImmKind;
use super::ops::{OP_COUNT, OPS, Op, flags};

/// Why code was rejected. `pc` is the position of the offending instruction.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StackError {
    Decode(DecodeError),
    /// A phase-1 pseudo-op in final code.
    Phase1Op {
        pc: u32,
    },
    /// The instruction pops more values than the stack holds.
    Underflow {
        pc: u32,
        depth: u32,
        pops: u32,
    },
    /// Two paths reach `pc` with different depths.
    Mismatch {
        pc: u32,
        depth: u32,
        other: u32,
    },
    /// A jump (or handler) target that is not an instruction start.
    BadTarget {
        pc: u32,
        target: u32,
    },
    /// Execution continues past the last instruction.
    FallsOffEnd {
        pc: u32,
    },
    /// A handler region that is not a range of instruction starts.
    BadHandler {
        index: usize,
    },
    /// More than `u16::MAX` values.
    TooDeep {
        pc: u32,
    },
}

const UNKNOWN: u32 = u32::MAX;

/// Per opcode, what the walk needs of an instruction whose immediates do not matter:
/// the values it pops in bits 0-7 and pushes in bits 8-15, its size in bits 16-23 and
/// [`NO_FALLTHROUGH`]. [`DECODE`] marks the others: a count of values in an immediate,
/// a jump's target.
const EFFECT: [u32; OP_COUNT] = {
    let mut t = [0u32; OP_COUNT];
    let mut i = 0;
    while i < OP_COUNT {
        let info = &OPS[i];
        t[i] = info.pop as u32 | (info.push as u32) << 8 | (info.size as u32) << 16;
        if info.no_fallthrough() {
            t[i] |= NO_FALLTHROUGH;
        }
        if info.pop_imm.is_some()
            || info.pop_arity.is_some()
            || info.push_imm.is_some()
            || info.has(flags::JUMP | flags::BRANCH)
        {
            t[i] |= DECODE;
        }
        i += 1;
    }
    t
};
const NO_FALLTHROUGH: u32 = 1 << 30;
const DECODE: u32 = 1 << 31;

/// A handler depth that [`stack_depths`] skips: to be computed from the code.
pub const UNKNOWN_DEPTH: u32 = u32::MAX;

/// Checks final code and returns its maximum operand stack depth.
pub fn check_stack(code: &[u8], handlers: &[Handler]) -> Result<u16, StackError> {
    walk(code, handlers, false, &mut StackScratch::default())
}

/// The buffers of the stack checker, reused across the functions of a compile (0096:
/// four allocations per function otherwise). After [`check_stack_in`], `caches` holds
/// the positions of the `Cache` immediates in code order.
#[derive(Default)]
pub struct StackScratch {
    /// One bit per code position: set at instruction starts.
    starts: Vec<u64>,
    depth: Vec<u32>,
    work: Vec<u32>,
    pub caches: Vec<u32>,
}

/// Per opcode, where its `Cache` immediates are: the offset from the opcode byte of
/// the first in bits 0-3 and of the second in bits 4-7 (0: none), so the pass over the
/// instruction starts does not walk `OpInfo::imms`.
const CACHE_AT: [u8; OP_COUNT] = {
    let mut t = [0u8; OP_COUNT];
    let mut i = 0;
    while i < OP_COUNT {
        let imms = OPS[i].imms;
        let mut at = 1;
        let mut n = 0;
        let mut j = 0;
        while j < imms.len() {
            if matches!(imms[j], ImmKind::Cache) {
                t[i] |= at << (4 * n);
                n += 1;
            }
            at += imms[j].width();
            j += 1;
        }
        i += 1;
    }
    t
};

/// [`check_stack`] with reused buffers; also finds the `Cache` immediates (for
/// `alloc_template`'s numbering, without decoding the code again).
pub fn check_stack_in(
    code: &[u8],
    handlers: &[Handler],
    scratch: &mut StackScratch,
) -> Result<u16, StackError> {
    walk(code, handlers, false, scratch)
}

/// The operand stack depth before each instruction (indexed by pc; [`UNKNOWN_DEPTH`]
/// for unreachable code and for positions inside instructions), with the same checks
/// as [`check_stack`]. Handlers whose depth is [`UNKNOWN_DEPTH`] are skipped, so code
/// reachable only through them stays unknown. The compiler uses this to find handler
/// depths it only knows relative to the code (array destructuring inside expressions).
pub fn stack_depths(code: &[u8], handlers: &[Handler]) -> Result<Vec<u32>, StackError> {
    let mut scratch = StackScratch::default();
    walk(code, handlers, true, &mut scratch)?;
    Ok(scratch.depth)
}

fn walk(
    code: &[u8],
    handlers: &[Handler],
    skip_unknown: bool,
    scratch: &mut StackScratch,
) -> Result<u16, StackError> {
    let StackScratch {
        starts,
        depth,
        work,
        caches,
    } = scratch;
    // Instruction starts; the code must decode linearly from 0 to the end. Only the
    // opcodes are read here: an instruction is decoded when the walk reaches it.
    starts.clear();
    starts.resize(code.len() / 64 + 1, 0);
    caches.clear();
    work.clear();
    let mut at = 0usize;
    while let Some(&byte) = code.get(at) {
        let pc = at as u32;
        let op =
            Op::from_u8(byte).ok_or(StackError::Decode(DecodeError::InvalidOpcode { pc, byte }))?;
        let info = op.info();
        if at + info.size as usize > code.len() {
            return Err(StackError::Decode(DecodeError::Truncated { pc }));
        }
        if info.has(flags::PHASE1) {
            return Err(StackError::Phase1Op { pc });
        }
        let mut cache = CACHE_AT[op as usize];
        while cache != 0 {
            caches.push(pc + u32::from(cache & 0xF));
            cache >>= 4;
        }
        starts[at / 64] |= 1 << (at % 64);
        at += info.size as usize;
    }
    let is_start = |pc: usize| starts[pc / 64] & (1 << (pc % 64)) != 0;
    let len = code.len() as u32;

    depth.clear();
    depth.resize(code.len(), UNKNOWN);
    let mut max: u32 = 0;

    // Records the depth at `target`, reached from `from`; whether it is new.
    let reach =
        |depth: &mut Vec<u32>, from: u32, target: u32, d: u32| -> Result<bool, StackError> {
            if target >= len {
                return Err(if target == len {
                    StackError::FallsOffEnd { pc: from }
                } else {
                    StackError::BadTarget { pc: from, target }
                });
            }
            if !is_start(target as usize) {
                return Err(StackError::BadTarget { pc: from, target });
            }
            let slot = &mut depth[target as usize];
            if *slot == UNKNOWN {
                *slot = d;
                Ok(true)
            } else if *slot != d {
                Err(StackError::Mismatch {
                    pc: target,
                    depth: *slot,
                    other: d,
                })
            } else {
                Ok(false)
            }
        };

    if len == 0 {
        return Ok(0);
    }
    if reach(depth, 0, 0, 0)? {
        work.push(0);
    }
    for (index, h) in handlers.iter().enumerate() {
        if skip_unknown && h.depth == UNKNOWN_DEPTH {
            continue;
        }
        let start_ok = h.start <= len && is_start(h.start as usize);
        let end_ok = h.end == len || (h.end < len && is_start(h.end as usize));
        if !start_ok || !end_ok || h.start > h.end {
            return Err(StackError::BadHandler { index });
        }
        let entry = h
            .depth
            .checked_add(1)
            .ok_or(StackError::BadHandler { index })?;
        if reach(depth, h.start, h.target, entry).map_err(|_| StackError::BadHandler { index })? {
            work.push(h.target);
        }
        max = max.max(entry);
    }

    // The fallthrough is followed at once and a jump's target later, the order a stack
    // of both gives.
    while let Some(mut pc) = work.pop() {
        loop {
            // (An opcode: the pass above saw every start.)
            let effect = code
                .get(pc as usize)
                .and_then(|&b| Op::from_u8(b))
                .map_or(DECODE, |op| EFFECT[op as usize]);
            let d = depth[pc as usize];
            let (pops, pushes, target) = if effect & DECODE == 0 {
                (effect & 0xFF, effect >> 8 & 0xFF, None)
            } else {
                let insn = decode(code, pc).map_err(StackError::Decode)?;
                let jumps = insn.info().has(flags::JUMP | flags::BRANCH);
                (
                    insn.pops(),
                    insn.pushes(),
                    jumps.then(|| insn.jump_target().unwrap_or(len)),
                )
            };
            if pops > d {
                return Err(StackError::Underflow { pc, depth: d, pops });
            }
            let after = d - pops + pushes;
            max = max.max(after).max(d);
            if max > u16::MAX as u32 {
                return Err(StackError::TooDeep { pc });
            }
            if let Some(target) = target
                && reach(depth, pc, target, after)?
            {
                work.push(target);
            }
            let next = pc + (effect >> 16 & 0xFF);
            if effect & NO_FALLTHROUGH != 0 || !reach(depth, pc, next, after)? {
                break;
            }
            pc = next;
        }
    }
    Ok(max as u16)
}
