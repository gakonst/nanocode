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
use super::ops::flags;

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
    at_pc: Vec<u32>,
    insns: Vec<super::decode::Insn>,
    depth: Vec<u32>,
    work: Vec<u32>,
    pub caches: Vec<u32>,
}

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
        at_pc,
        insns,
        depth,
        work,
        caches,
    } = scratch;
    // Instruction starts; the code must decode linearly from 0 to the end. Each is
    // decoded once (0092): `at_pc[pc]` is 1 + its index in `insns`, 0 inside an
    // instruction.
    at_pc.clear();
    at_pc.resize(code.len() + 1, 0);
    insns.clear();
    caches.clear();
    work.clear();
    let mut pc = 0u32;
    while (pc as usize) < code.len() {
        let insn = decode(code, pc).map_err(StackError::Decode)?;
        let info = insn.info();
        if info.has(flags::PHASE1) {
            return Err(StackError::Phase1Op { pc });
        }
        let mut at = pc + 1;
        for &k in info.imms {
            if k == super::imm::ImmKind::Cache {
                caches.push(at);
            }
            at += k.width() as u32;
        }
        insns.push(insn);
        at_pc[pc as usize] = insns.len() as u32;
        pc = insn.next_pc();
    }
    let is_start = |pc: usize| at_pc[pc] != 0;
    let len = code.len() as u32;

    depth.clear();
    depth.resize(code.len(), UNKNOWN);
    let mut max: u32 = 0;

    // Records the depth at `target`, reached from `from`.
    let reach = |depth: &mut Vec<u32>,
                 work: &mut Vec<u32>,
                 from: u32,
                 target: u32,
                 d: u32|
     -> Result<(), StackError> {
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
            work.push(target);
        } else if *slot != d {
            return Err(StackError::Mismatch {
                pc: target,
                depth: *slot,
                other: d,
            });
        }
        Ok(())
    };

    if len == 0 {
        return Ok(0);
    }
    reach(depth, work, 0, 0, 0)?;
    for (index, h) in handlers.iter().enumerate() {
        if skip_unknown && h.depth == UNKNOWN_DEPTH {
            continue;
        }
        let start_ok = (h.start as usize) < at_pc.len() && is_start(h.start as usize);
        let end_ok = h.end == len || ((h.end as usize) < at_pc.len() && is_start(h.end as usize));
        if !start_ok || !end_ok || h.start > h.end {
            return Err(StackError::BadHandler { index });
        }
        let entry = h
            .depth
            .checked_add(1)
            .ok_or(StackError::BadHandler { index })?;
        reach(depth, work, h.start, h.target, entry)
            .map_err(|_| StackError::BadHandler { index })?;
        max = max.max(entry);
    }

    while let Some(pc) = work.pop() {
        let insn = insns[at_pc[pc as usize] as usize - 1];
        let info = insn.info();
        let d = depth[pc as usize];
        let pops = insn.pops();
        if pops > d {
            return Err(StackError::Underflow { pc, depth: d, pops });
        }
        let after = d - pops + insn.pushes();
        max = max.max(after).max(d);
        if max > u16::MAX as u32 {
            return Err(StackError::TooDeep { pc });
        }
        if info.has(flags::JUMP) || info.has(flags::BRANCH) {
            let target = insn.jump_target().unwrap_or(len);
            reach(depth, work, pc, target, after)?;
        }
        if !info.no_fallthrough() {
            reach(depth, work, pc, insn.next_pc(), after)?;
        }
    }
    Ok(max as u16)
}
