//! Peepholes on phase-1 code (0086), before the resolution re-emit.
//!
//! The single-pass emitter produces the general form of every construct; an expression
//! statement drops the value its expression keeps (`x = v;` is `name_set x; drop`,
//! `o.c = v;` is `insert2; put_field; drop`, `i++;` is `to_numeric; dup; inc; name_put;
//! drop`). The rules here remove what the drop makes useless. They match consecutive
//! instructions only: phase-1 labels are `label` instructions, so a jump target never
//! sits inside a match. Resolution-dependent fusions (`inc_loc`) are in the re-emit
//! (resolve.rs).
//!
//! Each kept instruction keeps its phase-1 `pc`, so the line table entries of removed
//! instructions apply to the next kept one, as they would have.

use alloc::vec;
use alloc::vec::Vec;

use crate::bytecode::{Insn, Label, Op};

/// Applies the rules until none matches. `label_count` grows by the labels the rules
/// create.
pub(super) fn simplify(mut code: Vec<Insn>, label_count: &mut u32) -> Vec<Insn> {
    // Threading again is only worth it after a rule changed a jump (0092: each pass
    // costs a few % of compile time). Cycles of jumps could retarget each other
    // forever: a few rounds are enough for everything else.
    for round in 0..16 {
        // Threading until nothing changes: new labels and removed `dup`s open more
        // `&&`/`||` shortcuts (`a && b && c`).
        let mut threaded = false;
        for _ in 0..8 {
            let (changed, labels) = thread_jumps(&mut code, label_count);
            threaded |= changed;
            if !changed && !labels {
                break;
            }
        }
        if round > 0 && !threaded {
            break;
        }
        let (out, jumps) = rewrite_all(code);
        code = out;
        if !jumps {
            break;
        }
    }
    code
}

/// One pass of the local rules to their fixpoint: after a match, the replacement and
/// the instructions before it are looked at again (every rule shortens the code, so
/// this ends). Returns whether a rule consumed a jump.
fn rewrite_all(mut code: Vec<Insn>) -> (Vec<Insn>, bool) {
    // Rules match at most this many instructions, so a replacement can complete a match
    // starting this many before it.
    const WINDOW: usize = 5;
    let mut out = crate::buf::vec_with_capacity(code.len());
    let mut repl = crate::buf::vec_with_capacity(WINDOW);
    let mut jumps = false;
    // `code[..i]` is free (moved to `out`); a replacement is shorter than what it
    // replaces, so it and the instructions taken back from `out` fit in front of `i`.
    let mut i = 0;
    while i < code.len() {
        repl.clear();
        match rewrite(&code[i..], &mut repl) {
            Some(n) => {
                jumps |= code[i..i + n]
                    .iter()
                    .any(|x| matches!(x.op, Op::Jump | Op::JumpIfFalse | Op::JumpIfTrue));
                let mut j = i + n - repl.len();
                code[j..i + n].copy_from_slice(&repl);
                for _ in 0..WINDOW - 1 {
                    match out.pop() {
                        Some(x) => {
                            j -= 1;
                            code[j] = x;
                        }
                        None => break,
                    }
                }
                i = j;
            }
            None => {
                out.push(code[i]);
                i += 1;
            }
        }
    }
    (out, jumps)
}

/// Jump threading: a jump to a `jump` goes to its target instead (`if (x) break;` in a
/// loop, `continue` to the update of a loop), a `jump` to a `return` returns, and the
/// value a `&&` / `||` keeps for a condition is not tested twice (`if (a || b)` is `a;
/// dup; jump_if_true L; drop; b; L: jump_if_false Else`: when `a` is true, the second
/// test falls through, so the first one jumps past it).
/// Returns whether anything changed and whether labels were added.
fn thread_jumps(code: &mut Vec<Insn>, label_count: &mut u32) -> (bool, bool) {
    const NONE: usize = usize::MAX;
    // For each label id: the index of the first instruction after it that is no label.
    let mut at = vec![NONE; *label_count as usize];
    let mut pending = 0;
    for (k, insn) in code.iter().enumerate() {
        if insn.op == Op::Label {
            pending += 1;
            continue;
        }
        for l in &code[k - pending..k] {
            at[l.imm(0) as usize] = k;
        }
        pending = 0;
    }
    // The first instruction a jump lands on, past the `dup`s and `drop`s this pass
    // removed (marked `nop`): processing from the end, a whole `a || b || c` chain is
    // threaded in one pass.
    let skip = |code: &[Insn], mut k: usize| {
        while code.get(k).is_some_and(|i| i.op == Op::Nop) {
            k += 1;
        }
        k
    };
    let target = |code: &[Insn], insn: &Insn| -> Option<usize> {
        let k = *at.get(insn.label()?.0 as usize)?;
        (k != NONE)
            .then(|| skip(code, k))
            .filter(|&k| k < code.len())
    };
    let cond = |op: Op| matches!(op, Op::JumpIfFalse | Op::JumpIfTrue);
    let mut changed = false;
    let mut label_after = vec![false; code.len()];
    for k in (0..code.len()).rev() {
        let insn = code[k];
        if !matches!(insn.op, Op::Jump | Op::JumpIfFalse | Op::JumpIfTrue) {
            continue;
        }
        let Some(mut t) = target(code, &insn) else {
            continue;
        };
        // A cycle of jumps (`for (;;) {}` after rotation) stops after a few steps.
        let mut new = insn;
        for _ in 0..8 {
            match (code[t].op, target(code, &code[t])) {
                (Op::Jump, Some(n)) if n != t => {
                    new = new.with_label(code[t].label().unwrap_or(Label(0)));
                    t = n;
                }
                _ => break,
            }
        }
        if insn.op == Op::Jump && matches!(code[t].op, Op::Return | Op::ReturnUndef) {
            new = Insn::bare(code[t].op, insn.pc);
        } else if k > 0
            && code[k - 1].op == Op::Dup
            && code.get(k + 1).is_some_and(|i| i.op == Op::Drop)
            && cond(insn.op)
            && cond(code[t].op)
        {
            // `dup; jump_if_x L; drop` with `L: jump_if_y M`.
            let next = if insn.op == code[t].op {
                code[t].label()
            } else if let Some(l) = code.get(skip(code, t + 1)).filter(|i| i.op == Op::Label) {
                Some(Label(l.imm(0)))
            } else {
                label_after[t] = true;
                None
            };
            if let Some(l) = next {
                code[k - 1] = Insn::bare(Op::Nop, code[k - 1].pc);
                code[k + 1] = Insn::bare(Op::Nop, code[k + 1].pc);
                new = insn.with_label(l);
            }
        }
        if new != insn {
            code[k] = new;
            changed = true;
        }
    }
    let labels = label_after.contains(&true);
    if changed || labels {
        // Without the removed `dup`s, with the new labels.
        let mut out = crate::buf::vec_with_capacity(code.len() + 4);
        for (k, insn) in code.iter().enumerate() {
            if insn.op != Op::Nop {
                out.push(*insn);
            }
            if label_after[k] {
                let pc = code.get(k + 1).map_or(insn.next_pc(), |i| i.pc);
                out.push(Insn::label_at(*label_count, pc));
                *label_count += 1;
                changed = true;
            }
        }
        *code = out;
    }
    (changed, labels)
}

/// The store of a reference with `refs` values under the stored value, as `emit_store`
/// emits it (compiler/expr.rs), with the `insert` that `emit_keep` puts before it.
fn keep_insert(store: Op) -> Option<Op> {
    Some(match store {
        Op::PutField => Op::Insert2,
        Op::PutElem | Op::PutPrivateField | Op::PutSuperField => Op::Insert3,
        Op::PutSuperElem => Op::Insert4,
        _ => return None,
    })
}

/// Whether `code[0]`, a jump, targets one of the labels right after it.
fn jumps_to_next(code: &[Insn]) -> bool {
    let Some(t) = code[0].label() else {
        return false;
    };
    code[1..]
        .iter()
        .take_while(|i| i.op == Op::Label)
        .any(|i| i.imm(0) == t.0)
}

/// If a rule matches at the start of `code`, pushes its replacement and returns the
/// number of instructions it consumed.
fn rewrite(code: &[Insn], out: &mut Vec<Insn>) -> Option<usize> {
    let op = |k: usize| code.get(k).map(|i| i.op);
    let incdec = |o: Option<Op>| matches!(o, Some(Op::Inc | Op::Dec));
    match (op(0)?, op(1)) {
        // `x = v;`: store without keeping.
        (Op::NameSet, Some(Op::Drop)) => {
            out.push(code[0].with_op(Op::NamePut));
            Some(2)
        }
        // `o.c = v;`, `o.c += v;`, `o.c++;` (with the update between): the copy under
        // the reference is what the drop removes.
        (ins, _) if matches!(ins, Op::Insert2 | Op::Insert3 | Op::Insert4) => {
            let update = usize::from(matches!(op(1), Some(Op::Inc | Op::Dec | Op::ToNumeric)));
            let store = op(1 + update)?;
            if keep_insert(store) != Some(ins) || op(2 + update) != Some(Op::Drop) {
                return None;
            }
            out.extend_from_slice(&code[1..2 + update]);
            Some(3 + update)
        }
        // `x++;`: the old value under the new one is what the drop removes.
        (Op::Dup, o) if incdec(o) && op(2) == Some(Op::NamePut) && op(3) == Some(Op::Drop) => {
            out.extend_from_slice(&code[1..3]);
            Some(4)
        }
        // `inc` and `dec` take ToNumeric themselves.
        (Op::ToNumeric, o) if incdec(o) => {
            out.push(code[1]);
            Some(2)
        }
        // `if (c) break;`: `jump_if_false L; jump M; L:` is `jump_if_true M; L:`.
        (Op::JumpIfFalse | Op::JumpIfTrue, Some(Op::Jump))
            if op(2) == Some(Op::Label) && code[2].imm(0) == code[0].label()?.0 =>
        {
            let inv = if code[0].op == Op::JumpIfFalse {
                Op::JumpIfTrue
            } else {
                Op::JumpIfFalse
            };
            out.push(code[0].with_op(inv).with_label(code[1].label()?));
            Some(2)
        }
        // A jump to the next instruction.
        (Op::Jump, _) if jumps_to_next(code) => Some(1),
        // `if (!x)`
        (Op::Not, Some(Op::JumpIfFalse)) => {
            out.push(code[1].with_op(Op::JumpIfTrue));
            Some(2)
        }
        (Op::Not, Some(Op::JumpIfTrue)) => {
            out.push(code[1].with_op(Op::JumpIfFalse));
            Some(2)
        }
        // `while (true)`, `for (;;)` after loop rotation.
        (Op::PushTrue, Some(Op::JumpIfTrue)) | (Op::PushFalse, Some(Op::JumpIfFalse)) => {
            out.push(code[1].with_op(Op::Jump));
            Some(2)
        }
        (Op::PushTrue, Some(Op::JumpIfFalse)) | (Op::PushFalse, Some(Op::JumpIfTrue)) => Some(2),
        _ => None,
    }
}
