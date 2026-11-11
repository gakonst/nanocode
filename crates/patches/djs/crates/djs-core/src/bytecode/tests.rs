//! Bytecode unit tests: the table, emit/decode/disassemble round trips, labels, the stack
//! checker, the hash, line and handler tables, and the code cells.

extern crate std;

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use std::collections::HashSet;

use super::ops::hash_ops;
use super::template::{
    self, TemplateError, TemplateParts, alloc_script, alloc_template, bytes_of, debug_info,
    disasm_template,
};
use super::*;
use crate::heap::{Atom, Heap, Ref};
use crate::value::Value;

// ---------------------------------------------------------------------------------
// The table

#[test]
fn table_is_consistent() {
    assert_eq!(OPS.len(), OP_COUNT);
    assert_eq!(Op::ALL.len(), OP_COUNT);
    let mut names = HashSet::new();
    for (i, op) in Op::ALL.iter().enumerate() {
        assert_eq!(*op as usize, i);
        assert_eq!(Op::from_u8(i as u8), Some(*op));
        let info = op.info();
        let name = op.name();
        assert!(names.insert(name), "duplicate name {}", name);
        assert!(!name.ends_with('_'), "{}", name);
        let width: usize = info.imms.iter().map(|k| k.width() as usize).sum();
        assert_eq!(op.size(), 1 + width, "{}", name);
        assert!(info.imms.len() <= 2, "{}", name);
        // Phase-1 ops are exactly the scope-generic names and markers.
        let phase1 = name.starts_with("name_")
            || matches!(
                *op,
                Op::EnterScope | Op::LeaveScope | Op::IterScope | Op::Label
            );
        assert_eq!(info.has(flags::PHASE1), phase1, "{}", name);
        // Calling implies the op can also throw.
        if info.has(flags::CALLS) {
            assert!(info.has(flags::THROWS), "{}", name);
        }
    }
    assert_eq!(Op::from_u8(OP_COUNT as u8), None);
    assert_eq!(Op::from_u8(255), None);
    assert_eq!(Op::In.name(), "in");
    assert_eq!(Op::Typeof.name(), "typeof");
    assert_eq!(Op::Return.name(), "return");
    assert_eq!(Op::Call.size(), 3);
    assert_eq!(Op::Jump.size(), 5);
    assert_eq!(Op::CallIntrinsic.info().pop_imm, Some(1));
}

/// Raw immediate values exercising each kind's range.
fn sample_imm(kind: ImmKind, label: Label) -> u32 {
    match kind {
        ImmKind::U8 => 200,
        ImmKind::I8 => (-5i32) as u32,
        ImmKind::I32 => (-100_000i32) as u32,
        ImmKind::Count | ImmKind::Argc => 3,
        ImmKind::Loc
        | ImmKind::Arg
        | ImmKind::Cap
        | ImmKind::Scope
        | ImmKind::Intrinsic
        | ImmKind::Builtin
        | ImmKind::Cache => 0xBEEF,
        ImmKind::Const | ImmKind::Child | ImmKind::NameRef => 0x1234_5678,
        ImmKind::LabelId | ImmKind::Target | ImmKind::Target8 => label.0,
        ImmKind::Atom => 7,
        ImmKind::Loc8
        | ImmKind::Arg8
        | ImmKind::Const8
        | ImmKind::Intrinsic8
        | ImmKind::Builtin8 => 0xEF,
    }
}

#[test]
fn every_op_round_trips() {
    // Emit every opcode once (phase-1 mode keeps label ids), decode it back.
    let mut buf = CodeBuf::phase1();
    let label = buf.new_label();
    let mut expected = Vec::new();
    let mut byte_pc = 0;
    for op in Op::ALL {
        let imms: Vec<u32> = op
            .info()
            .imms
            .iter()
            .map(|&k| sample_imm(k, label))
            .collect();
        let pc = buf.pos();
        let mut raw = [0u32; 2];
        raw[..imms.len()].copy_from_slice(&imms);
        buf.emit_insn(&encode_raw(op, pc, raw));
        // Phase-1 positions are instruction indices.
        assert_eq!(buf.pos() - pc, 1, "{}", op.name());
        expected.push(encode_raw(op, byte_pc, raw));
        byte_pc += op.size() as u32;
    }
    buf.place_label(label);
    let code = buf.finish().unwrap();
    for (i, (p, e)) in code.insns.iter().zip(&expected).enumerate() {
        let insn = p.insn(i as u32);
        assert_eq!(
            (insn.op, insn.pc, insn.imm(0), insn.imm(1)),
            (e.op, i as u32, e.imm(0), e.imm(1))
        );
    }
    let bytes = code.bytes();
    let decoded: Vec<Insn> = insns(&bytes).map(|r| r.unwrap()).collect();
    // The trailing `label` pseudo-op from place_label.
    assert_eq!(decoded.len(), expected.len() + 1);
    for (d, e) in decoded.iter().zip(&expected) {
        assert_eq!(d, e, "{}", e.op.name());
    }
    assert_eq!(decoded[1].imm_i32(0), 0); // `null` has no immediates
    // Every line of the listing starts with the op's name.
    let text = disasm(
        &bytes,
        &DisasmContext {
            phase1: true,
            ..Default::default()
        },
    );
    for (line, e) in text.lines().zip(&expected) {
        if e.op != Op::Label {
            assert!(
                line.split_whitespace().nth(1) == Some(e.op.name()),
                "{line}"
            );
        }
    }
}

/// Builds an `Insn` with raw immediates by encoding and decoding it.
fn encode_raw(op: Op, pc: u32, imms: [u32; 2]) -> Insn {
    let mut bytes = vec![0u8; pc as usize];
    bytes.push(op as u8);
    for (i, k) in op.info().imms.iter().enumerate() {
        bytes.extend_from_slice(&imms[i].to_le_bytes()[..k.width() as usize]);
    }
    decode::decode(&bytes, pc).unwrap()
}

#[test]
fn typed_emitters() {
    let mut b = CodeBuf::new();
    b.push_i8(-1);
    b.push_i32(i32::MIN);
    b.get_loc(65535);
    b.get_field(Atom::from_id(42), 0);
    b.call(2);
    b.call_intrinsic(9, 4);
    b.return_();
    let code = b.finish().unwrap().code;
    let v: Vec<Insn> = insns(&code).map(|r| r.unwrap()).collect();
    assert_eq!(v[0].op, Op::PushI8);
    assert_eq!(v[0].imm_i32(0), -1);
    assert_eq!(v[1].imm_i32(0), i32::MIN);
    assert_eq!((v[2].op, v[2].imm(0)), (Op::GetLoc, 65535));
    assert_eq!(v[3].atom(0), Atom::from_id(42));
    // `call 2` in its short form (D2443).
    assert_eq!((v[4].op, v[4].pops(), v[4].pushes()), (Op::Call2, 4, 1));
    assert_eq!((v[5].imm(0), v[5].imm(1), v[5].pops()), (9, 4, 4));
    assert_eq!(v[6].op, Op::Return);
    // Raw readers agree with the decoder.
    assert_eq!(decode::read_i8(&code, 1), -1);
    assert_eq!(decode::read_i32(&code, 3), i32::MIN);
    assert_eq!(decode::read_u16(&code, v[2].pc as usize + 1), 65535);
    assert_eq!(decode::read_u32(&code, v[3].pc as usize + 1), 42);
}

#[test]
fn decode_errors() {
    assert_eq!(
        decode::decode(&[255], 0),
        Err(DecodeError::InvalidOpcode { pc: 0, byte: 255 })
    );
    assert_eq!(
        decode::decode(&[Op::GetLoc as u8, 1], 0),
        Err(DecodeError::Truncated { pc: 0 })
    );
    assert_eq!(
        decode::decode(&[], 0),
        Err(DecodeError::Truncated { pc: 0 })
    );
    let mut it = insns(&[Op::Nop as u8, 255, Op::Nop as u8]);
    assert!(it.next().unwrap().is_ok());
    assert!(it.next().unwrap().is_err());
    assert!(it.next().is_none());
    let text = disasm(&[Op::Nop as u8, 255], &DisasmContext::default());
    assert!(text.contains("<invalid"), "{text}");
}

// ---------------------------------------------------------------------------------
// Labels and fixups

#[test]
fn forward_and_backward_jumps() {
    let mut b = CodeBuf::new();
    let top = b.new_label();
    let end = b.new_label();
    b.place_label(top); // 0
    b.get_loc(300); // 0..3
    b.jump_if_false(end); // 3..8, shortened to 3..5
    b.nop(); // 8 -> 5
    b.jump(top); // 9..14 -> 6..8
    b.place_label(end); // 14 -> 8
    b.return_undef();
    let code = b.finish().unwrap();
    assert_eq!(code.label_pos(top), Some(0));
    assert_eq!(code.label_pos(end), Some(8));
    assert_eq!(code.map_pos(8), 5);
    assert_eq!(code.map_pos(14), 8);
    let v: Vec<Insn> = insns(&code.code).map(|r| r.unwrap()).collect();
    assert_eq!(v[1].op, Op::JumpIfFalse8);
    assert_eq!(v[1].jump_target(), Some(8));
    assert_eq!(v[1].imm_i32(0), 3); // relative to the end of the instruction
    assert_eq!(v[3].op, Op::Jump8);
    assert_eq!(v[3].jump_target(), Some(0));
    assert_eq!(v[3].imm_i32(0), -8);
    assert_eq!(check_stack(&code.code, &[]), Ok(1));
    let text = disasm(&code.code, &DisasmContext::default());
    assert!(text.contains("jump_if_false8 -> 8"), "{text}");
    assert!(text.contains("jump8 -> 0"), "{text}");
}

#[test]
fn jumps_stay_wide_when_far() {
    // A forward jump over 125 bytes fits a byte only once the jump inside the span
    // is short: the fixpoint shortens both. One over more stays wide.
    let mut b = CodeBuf::new();
    let (far, near, back) = (b.new_label(), b.new_label(), b.new_label());
    b.place_label(back);
    b.jump(far); // 0..5 -> 0..2
    b.jump(near); // 5..10 -> 2..4
    for _ in 0..124 {
        b.nop();
    }
    b.place_label(near); // 134 -> 128
    b.jump(back); // backward over 128 + 2: stays wide
    b.place_label(far); // 139 -> 133
    b.return_undef();
    let code = b.finish().unwrap();
    let v: Vec<Insn> = insns(&code.code).map(|r| r.unwrap()).collect();
    assert_eq!(v[0].op, Op::Jump);
    assert_eq!(v[1].op, Op::Jump8);
    assert_eq!(v[1].jump_target(), code.label_pos(near));
    let last = &v[v.len() - 2];
    assert_eq!(last.op, Op::Jump);
    assert_eq!(last.jump_target(), Some(0));
    assert_eq!(v[0].jump_target(), code.label_pos(far));
    assert_eq!(code.code.len() as u32, code.label_pos(far).unwrap() + 1);
}

#[test]
fn short_forms() {
    // The emitter picks the short form when the operand fits (D2443).
    let mut b = CodeBuf::new();
    b.get_loc(2);
    b.get_loc(200);
    b.put_loc(3);
    b.set_loc(4);
    b.get_arg(1);
    b.push_const(255);
    b.push_const(256);
    b.get_builtin(7);
    b.undefined();
    b.get_builtin(300);
    b.undefined();
    let is_object = intrinsics::const_id("IsObject");
    b.call_intrinsic(is_object, 1);
    // Above 256: wide.
    let get_iterator = intrinsics::const_id("GetIterator");
    b.call_intrinsic(get_iterator, 1);
    b.call(1);
    let code = b.finish().unwrap().code;
    let v: Vec<(Op, u32)> = insns(&code)
        .map(|r| r.unwrap())
        .map(|i| {
            (
                i.op,
                if i.info().imms.is_empty() {
                    0
                } else {
                    i.imm(0)
                },
            )
        })
        .collect();
    assert_eq!(
        v,
        [
            (Op::GetLoc2, 0),
            (Op::GetLoc8, 200),
            (Op::PutLoc3, 0),
            (Op::SetLoc8, 4),
            (Op::GetArg1, 0),
            (Op::PushConst8, 255),
            (Op::PushConst, 256),
            (Op::GetBuiltinCallee8, 7),
            (Op::GetBuiltin, 300),
            (Op::Undefined, 0),
            (Op::CallIntrinsic8, is_object as u32),
            (Op::CallIntrinsic, get_iterator as u32),
            (Op::Call1, 0),
        ]
    );
    let ci = insns(&code).nth(10).unwrap().unwrap();
    assert_eq!(ci.pops(), 1);
    for op in Op::ALL {
        assert_eq!(op.wide().wide(), op.wide(), "{}", op.name());
    }
}

#[test]
fn label_errors() {
    let mut b = CodeBuf::new();
    let l = b.new_label();
    b.jump(l);
    assert_eq!(b.finish(), Err(CodeError::UnplacedLabel(l)));

    let mut b = CodeBuf::new();
    let l = b.new_label();
    b.place_label(l);
    b.place_label(l);
    assert_eq!(b.finish(), Err(CodeError::LabelPlacedTwice(l)));

    let mut b = CodeBuf::new();
    b.jump(Label(5));
    assert_eq!(b.finish(), Err(CodeError::UnknownLabel(Label(5))));

    let mut b = CodeBuf::phase1();
    let l = b.new_label();
    b.jump(l);
    assert_eq!(b.finish(), Err(CodeError::UnplacedLabel(l)));

    // An unused, unplaced label is fine.
    let mut b = CodeBuf::new();
    let l = b.new_label();
    b.return_undef();
    assert_eq!(b.finish().unwrap().label_pos(l), None);
}

#[test]
fn phase1_reemit() {
    // Phase 1: symbolic labels as pseudo-ops, then a re-emit into final code with the
    // same label ids, the way the resolution pass does it.
    let mut p = CodeBuf::phase1();
    let l_else = p.new_label();
    let l_end = p.new_label();
    p.name_get(0);
    p.jump_if_false(l_else);
    p.push_i8(1);
    p.jump(l_end);
    p.place_label(l_else);
    p.push_i8(2);
    p.place_label(l_end);
    p.return_();
    let p1 = p.finish().unwrap();
    let p1_bytes = p1.bytes();
    let listing = disasm(
        &p1_bytes,
        &DisasmContext {
            phase1: true,
            ..Default::default()
        },
    );
    assert!(listing.contains("jump_if_false L0"), "{listing}");
    assert!(listing.contains("L1:"), "{listing}");
    assert!(listing.contains("name_get r0"), "{listing}");
    assert!(check_stack(&p1_bytes, &[]).is_err());

    let mut out = CodeBuf::with_labels(p1.label_count);
    for (i, p) in p1.insns.iter().enumerate() {
        let insn = p.insn(i as u32);
        match insn.op {
            Op::Label => out.place_label(Label(insn.imm(0))),
            Op::NameGet => out.get_loc(0),
            _ => out.emit_insn(&insn),
        }
    }
    let code = out.finish().unwrap();
    let v: Vec<Insn> = insns(&code.code).map(|r| r.unwrap()).collect();
    let ops: Vec<Op> = v.iter().map(|i| i.op).collect();
    assert_eq!(
        ops,
        [
            Op::GetLoc0,
            Op::JumpIfFalse8,
            Op::Push1,
            Op::Jump8,
            Op::Push2,
            Op::Return
        ]
    );
    assert_eq!(v[1].jump_target(), Some(v[4].pc));
    assert_eq!(v[3].jump_target(), Some(v[5].pc));
    assert_eq!(check_stack(&code.code, &[]), Ok(1));
}

#[test]
fn truncate_last() {
    let mut b = CodeBuf::new();
    b.get_loc(300);
    b.get_field(Atom::from_id(3), 0);
    let last = b.last_insn().unwrap();
    assert_eq!((last.op, last.atom(0)), (Op::GetField, Atom::from_id(3)));
    assert_eq!(b.truncate_last().unwrap().op, Op::GetField);
    assert_eq!(b.pos(), 3);
    // Only one level: the previous instruction is not tracked.
    assert!(b.truncate_last().is_none());

    // Not across a label.
    let mut b = CodeBuf::new();
    let l = b.new_label();
    b.get_loc(0);
    b.place_label(l);
    assert!(b.last_insn().is_none() && b.truncate_last().is_none());

    // Removing a jump removes its fixup.
    let mut b = CodeBuf::new();
    let l = b.new_label();
    b.jump(l);
    assert_eq!(b.truncate_last().unwrap().op, Op::Jump);
    b.return_undef();
    assert!(b.finish().is_ok());

    // Phase 1: a label pseudo-op is not removable either.
    let mut p = CodeBuf::phase1();
    let l = p.new_label();
    p.name_get(0);
    p.place_label(l);
    assert!(p.truncate_last().is_none());
}

// ---------------------------------------------------------------------------------
// Stack checker

fn final_code(f: impl FnOnce(&mut CodeBuf)) -> Vec<u8> {
    let mut b = CodeBuf::new();
    f(&mut b);
    b.finish().unwrap().code
}

#[test]
fn stack_good() {
    // 1 + 2 * 3
    let c = final_code(|b| {
        b.push_i8(1);
        b.push_i8(2);
        b.push_i8(3);
        b.mul();
        b.add();
        b.return_();
    });
    assert_eq!(check_stack(&c, &[]), Ok(3));

    // f(a, b, c): callee, this, three args.
    let c = final_code(|b| {
        b.get_global(Atom::from_id(1), 0);
        b.undefined();
        b.push_i8(1);
        b.push_i8(2);
        b.push_i8(3);
        b.call(3);
        b.return_();
    });
    assert_eq!(check_stack(&c, &[]), Ok(5));

    // `a && b` joins at depth 1.
    let c = final_code(|b| {
        let l = b.new_label();
        b.get_loc(0);
        b.dup();
        b.jump_if_false(l);
        b.drop();
        b.get_loc(1);
        b.place_label(l);
        b.return_();
    });
    assert_eq!(check_stack(&c, &[]), Ok(2));

    // Empty code and unreachable garbage after a return are fine.
    assert_eq!(check_stack(&[], &[]), Ok(0));
    let c = final_code(|b| {
        b.return_undef();
        b.drop();
        b.drop();
    });
    assert_eq!(check_stack(&c, &[]), Ok(0));

    // A loop: the back edge arrives at the same depth.
    let c = final_code(|b| {
        let top = b.new_label();
        let end = b.new_label();
        b.place_label(top);
        b.get_loc(0);
        b.jump_if_false(end);
        b.jump(top);
        b.place_label(end);
        b.return_undef();
    });
    assert_eq!(check_stack(&c, &[]), Ok(1));
}

#[test]
fn stack_try_finally() {
    // docs/compiler.md 7.4: try { g() } finally { h() }, the completion's kind in local
    // 0 and its value in local 1.
    let mut b = CodeBuf::new();
    let l_fin = b.new_label();
    let l_exc = b.new_label();
    let l_try_end = b.new_label();
    let l_end = b.new_label();
    let start = b.pos();
    b.get_global(Atom::from_id(1), 0);
    b.undefined();
    b.call(0);
    b.drop();
    b.place_label(l_try_end);
    b.push_i8(0);
    b.put_loc(0);
    b.place_label(l_fin);
    b.get_global(Atom::from_id(2), 0);
    b.undefined();
    b.call(0);
    b.drop();
    b.get_loc(0);
    b.jump_if_false(l_end);
    b.get_loc(1);
    b.throw();
    b.place_label(l_exc);
    b.put_loc(1);
    b.push_i8(1);
    b.put_loc(0);
    b.jump(l_fin);
    b.place_label(l_end);
    b.return_undef();
    let code = b.finish().unwrap();
    let h = Handler {
        start,
        end: code.label_pos(l_try_end).unwrap(),
        target: code.label_pos(l_exc).unwrap(),
        depth: 0,
    };
    // Every entry reaches the finally block at depth 0: nothing is on the stack.
    assert_eq!(check_stack(&code.code, &[h]), Ok(2));
}

#[test]
fn stack_bad() {
    let c = final_code(|b| {
        b.add();
        b.return_();
    });
    assert_eq!(
        check_stack(&c, &[]),
        Err(StackError::Underflow {
            pc: 0,
            depth: 0,
            pops: 2
        })
    );

    // call 3 needs 5 values.
    let c = final_code(|b| {
        b.undefined();
        b.undefined();
        b.call(3);
        b.return_();
    });
    assert!(matches!(
        check_stack(&c, &[]),
        Err(StackError::Underflow { pops: 5, .. })
    ));

    // `pick n` and `poke n` reach n values down: their effects grow with the immediate.
    let c = final_code(|b| {
        b.undefined();
        b.undefined();
        b.pick(1);
        b.poke(1);
        b.drop();
        b.return_();
    });
    assert_eq!(check_stack(&c, &[]), Ok(3));
    let c = final_code(|b| {
        b.undefined();
        b.pick(1);
        b.return_();
    });
    assert!(matches!(
        check_stack(&c, &[]),
        Err(StackError::Underflow { pops: 2, .. })
    ));

    // Branches joining at different depths.
    let c = final_code(|b| {
        let l = b.new_label();
        b.get_loc(0);
        b.jump_if_false(l);
        b.push_i8(1);
        b.place_label(l);
        b.return_undef();
    });
    assert!(matches!(
        check_stack(&c, &[]),
        Err(StackError::Mismatch { .. })
    ));

    // Falling off the end.
    let c = final_code(|b| b.push_i8(1));
    assert_eq!(check_stack(&c, &[]), Err(StackError::FallsOffEnd { pc: 0 }));

    // A jump into the middle of an instruction (hand-patched offset).
    let mut c = final_code(|b| {
        let l = b.new_label();
        b.jump(l);
        b.place_label(l);
        b.push_i32(5);
        b.return_();
    });
    // (`jump8` at 0..2, `push_i32` at 2..7.)
    c[1] = 1;
    assert_eq!(
        check_stack(&c, &[]),
        Err(StackError::BadTarget { pc: 0, target: 3 })
    );
    // And outside the code.
    c[1] = 100;
    assert!(matches!(
        check_stack(&c, &[]),
        Err(StackError::BadTarget { .. })
    ));

    // Phase-1 ops are not allowed.
    let c = final_code(|b| {
        b.name_get(0);
        b.return_();
    });
    assert_eq!(check_stack(&c, &[]), Err(StackError::Phase1Op { pc: 0 }));

    // Undecodable code.
    assert!(matches!(
        check_stack(&[255], &[]),
        Err(StackError::Decode(_))
    ));

    // Bad handlers: misaligned region, target outside.
    let c = final_code(|b| {
        b.push_i32(1);
        b.return_();
    });
    let bad = Handler {
        start: 1,
        end: 5,
        target: 0,
        depth: 0,
    };
    assert_eq!(
        check_stack(&c, &[bad]),
        Err(StackError::BadHandler { index: 0 })
    );
    let bad = Handler {
        start: 0,
        end: 5,
        target: 99,
        depth: 0,
    };
    assert_eq!(
        check_stack(&c, &[bad]),
        Err(StackError::BadHandler { index: 0 })
    );
}

// ---------------------------------------------------------------------------------
// OPS_HASH

#[test]
fn ops_hash_tracks_the_table() {
    assert_eq!(OPS_HASH, hash_ops(OPS, &OP_NAMES));
    assert_ne!(OPS_HASH, 0);
    let base: Vec<OpInfo> = OPS.to_vec();
    let mut seen = HashSet::new();
    seen.insert(OPS_HASH);
    // Any change to any field of any entry (or the order) changes the hash.
    for i in 0..base.len() {
        let mut variants: Vec<Vec<OpInfo>> = Vec::new();
        let mut names = OP_NAMES;
        names[i] = "renamed";
        assert_ne!(hash_ops(OPS, &names), OPS_HASH, "op {i} renamed");
        let mut t = base.clone();
        t[i].push += 1;
        variants.push(t);
        let mut t = base.clone();
        t[i].pop += 1;
        variants.push(t);
        let mut t = base.clone();
        t[i].flags ^= flags::THROWS;
        variants.push(t);
        let mut t = base.clone();
        t[i].flags ^= flags::ALLOC;
        variants.push(t);
        let mut t = base.clone();
        t[i].cost = if t[i].cost == CostClass::Call {
            CostClass::Basic
        } else {
            CostClass::Call
        };
        variants.push(t);
        let mut t = base.clone();
        t[i].imms = if t[i].imms.is_empty() {
            &[ImmKind::U8]
        } else {
            &[]
        };
        variants.push(t);
        let mut t = base.clone();
        t[i].push_imm = if t[i].push_imm.is_some() {
            None
        } else {
            Some(0)
        };
        variants.push(t);
        let mut t = base.clone();
        t[i].pop_imm = if t[i].pop_imm.is_some() {
            None
        } else {
            Some(0)
        };
        variants.push(t);
        if i + 1 < base.len() {
            let mut t = base.clone();
            t.swap(i, i + 1);
            let mut names = OP_NAMES;
            names.swap(i, i + 1);
            assert_ne!(hash_ops(&t, &names), OPS_HASH, "op {i} swapped");
        }
        for v in variants {
            let h = hash_ops(&v, &OP_NAMES);
            assert_ne!(h, OPS_HASH, "op {i}");
        }
    }
    assert_ne!(build_hash(1, OPS_HASH), build_hash(2, OPS_HASH));
    assert_ne!(build_hash(1, OPS_HASH), build_hash(1, OPS_HASH ^ 1));
}

// ---------------------------------------------------------------------------------
// Line and handler tables

#[test]
fn line_table_round_trip() {
    let mut b = LineTableBuilder::new();
    b.add(0, 1, 1);
    b.add(0, 1, 1); // repeat: skipped
    b.add(5, 1, 9);
    b.add(12, 3, 2);
    b.add(12, 3, 2);
    b.add(20, 2, 7); // lines may go back (loop conditions, finally blocks)
    b.add(70_000, 100_000, 300);
    let bytes = b.finish();
    let entries: Vec<_> = line_entries(&bytes)
        .map(|e| (e.pc, e.line, e.col))
        .collect();
    assert_eq!(
        entries,
        [
            (0, 1, 1),
            (5, 1, 9),
            (12, 3, 2),
            (20, 2, 7),
            (70_000, 100_000, 300)
        ]
    );
    assert_eq!(lookup_line(&bytes, 0), Some((1, 1)));
    assert_eq!(lookup_line(&bytes, 4), Some((1, 1)));
    assert_eq!(lookup_line(&bytes, 5), Some((1, 9)));
    assert_eq!(lookup_line(&bytes, 19), Some((3, 2)));
    assert_eq!(lookup_line(&bytes, 25), Some((2, 7)));
    assert_eq!(lookup_line(&bytes, u32::MAX), Some((100_000, 300)));
    // Compact: small deltas are one byte each.
    assert!(bytes.len() <= 3 * 5 + 6, "{}", bytes.len());

    // No entries before the first one.
    let mut b = LineTableBuilder::new();
    b.add(10, 2, 1);
    let bytes = b.finish();
    assert_eq!(lookup_line(&bytes, 9), None);
    assert_eq!(lookup_line(&[], 0), None);
    // Malformed data stops the iteration instead of panicking.
    assert_eq!(line_entries(&[0x80]).count(), 0);
    assert_eq!(line_entries(&[0, 0x7F]).count(), 0);
    let _ = line_entries(&[0xFF; 40]).count();
}

#[test]
fn handler_table_round_trip() {
    let hs = [
        Handler {
            start: 10,
            end: 20,
            target: 30,
            depth: 1,
        },
        Handler {
            start: 0,
            end: 40,
            target: 50,
            depth: 0,
        },
    ];
    let bytes = encode_handlers(&hs);
    assert_eq!(bytes.len(), 32);
    let t = HandlerTable::new(&bytes).unwrap();
    assert_eq!(t.len(), 2);
    assert_eq!(t.iter().collect::<Vec<_>>(), hs);
    // Innermost first.
    assert_eq!(t.find(15), Some(hs[0]));
    assert_eq!(t.find(20), Some(hs[1]));
    assert_eq!(t.find(5), Some(hs[1]));
    assert_eq!(t.find(40), None);
    assert!(HandlerTable::new(&bytes[..31]).is_err());
    assert!(HandlerTable::empty().is_empty());
    assert_eq!(HandlerTable::empty().find(0), None);
}

// ---------------------------------------------------------------------------------
// Code cells and the template disassembler

#[test]
fn template_cells() {
    let mut heap = Heap::new().unwrap();
    let a_len = heap.atom_str("length").unwrap();
    let a_x = heap.atom_str("x").unwrap();
    let a_f = heap.atom_str("f").unwrap();

    // child: return 7
    let mut b = CodeBuf::new();
    b.push_i8(7);
    b.return_();
    let child_code = b.finish().unwrap().code;
    let child = alloc_template(
        &mut heap,
        &TemplateParts {
            name: a_f,
            code: &child_code,
            ..Default::default()
        },
    )
    .unwrap();

    // parent: x = "hi"; return x.length + 1.5
    let mut b = CodeBuf::new();
    b.push_const(0);
    b.put_loc(0);
    b.get_loc(0);
    b.get_field(a_len, 0);
    b.push_const(1);
    b.add();
    b.fclosure(0);
    b.drop();
    b.return_();
    let code = b.finish().unwrap().code;
    let hi = heap.alloc_str("hi").unwrap();
    let consts = [Value::string(hi), Value::number(1.5)];
    let handlers = [Handler {
        start: 0,
        end: 4,
        // The exception lands where the stack holds one value: the `return`.
        target: 20,
        depth: 0,
    }];
    let name = heap.alloc_str("test.js").unwrap();
    let script = alloc_script(&mut heap, name, None, 0, None).unwrap();
    let mut lines = LineTableBuilder::new();
    lines.add(0, 1, 1);
    lines.add(4, 1, 9);
    let lines = lines.finish();
    let parent = alloc_template(
        &mut heap,
        &TemplateParts {
            locals: 1,
            code: &code,
            consts: &consts,
            children: &[child],
            handlers: &handlers,
            lines: &lines,
            names: &[a_x],
            script,
            ..Default::default()
        },
    )
    .unwrap();
    let debug = debug_info(&heap, parent.0);
    let line_bytes = bytes_of(&heap, debug.lines);
    assert_eq!(lookup_line(line_bytes, 5), Some((1, 9)));
    assert_eq!(debug.script, script);
    // Inline code, capture descriptors and handlers (D2444).
    assert_eq!(template::code(&heap, parent.0), &code[..]);
    assert_eq!(template::handlers(&heap, parent.0).len(), 16);
    assert!(template::handlers(&heap, child.0).is_empty());

    assert_eq!(bytes_of(&heap, Ref::NULL), &[] as &[u8]);
    assert_eq!(template::child_count(&heap, parent.0), 1);
    assert_eq!(template::child(&heap, parent.0, 0), Some(child));
    assert_eq!(template::child(&heap, parent.0, 1), None);

    let text = disasm_template(&heap, parent);
    let expect = [
        "function 0 <anonymous>: params=0 locals=1 stack=2 captures=0",
        "  handler [0, 4) -> 20 depth 0",
        "    0  push_const8 #0 (\"hi\")  ; 1:1",
        "    2  put_loc0 (x)",
        "    3  get_loc0 (x)",
        "    4  get_field \"length\" ic0  ; 1:9",
        "   11  push_const8 #1 (1.5)",
        "   13  add",
        "   14  fclosure child0",
        "   19  drop",
        "   20  return",
        "function 0.0 f: params=0 locals=0 stack=1 captures=0",
        "    0  push_i8 7",
        "    2  return",
    ];
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines, expect, "{text}");

    // Bad code is rejected.
    assert!(matches!(
        alloc_template(
            &mut heap,
            &TemplateParts {
                code: &[Op::Add as u8],
                ..Default::default()
            }
        ),
        Err(TemplateError::Stack(StackError::Underflow { .. }))
    ));

    // Bad code is rejected.
    let bad = TemplateParts {
        code: &[Op::Add as u8],
        ..Default::default()
    };
    assert!(matches!(
        alloc_template(&mut heap, &bad),
        Err(TemplateError::Stack(StackError::Underflow { .. }))
    ));

    // The new cell kinds walk and verify, and survive a byte copy of the heap.
    heap.verify().unwrap();
    let copy = Heap::from_bytes(heap.bytes().to_vec(), crate::heap::DEFAULT_LIMIT).unwrap();
    assert_eq!(disasm_template(&copy, parent), text);
}

#[test]
fn capture_descriptors() {
    use capture_desc::{decode, local, outer};
    assert_eq!(decode(local(7)), (false, 7));
    assert_eq!(decode(outer(65535)), (true, 65535));
    assert_ne!(local(1), outer(1));
}

#[test]
fn disasm_without_heap() {
    let mut b = CodeBuf::new();
    b.get_field(Atom::from_id(5), 0);
    b.get_cap(1);
    b.push_const(3);
    let code = b.finish().unwrap().code;
    let text: String = disasm(&code, &DisasmContext::default());
    assert!(text.contains("get_field atom#5"), "{text}");
    assert!(text.contains("get_cap cap1\n"), "{text}");
    assert!(text.contains("push_const8 #3\n"), "{text}");
}
