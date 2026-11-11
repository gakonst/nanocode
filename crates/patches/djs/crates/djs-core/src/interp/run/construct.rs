//! The construct path's instructions that `run_hot` declines, done without `step` (0236).
//!
//! An instruction that leaves the tier-0 loop for `step` costs about 500-1000 wasm
//! instructions before its own work starts (`step_once`, `step` and `exec` are large
//! functions). `new C()` took four such trips (`construct`, `check_ctor`, the return into
//! the constructing frame, `special_object` of new.target) and a derived constructor
//! eight, more than everything else they do. `Exec::decline_fast` sends these opcodes
//! here. Each handler completes its instruction exactly as `step` would (fuel, pc, stack,
//! frame, safepoints), or returns false having changed nothing and `step` runs it: an
//! error to throw, a callee or a `prototype` that needs the general path, a stack that
//! must grow, the heap cap.

use super::{Exec, NO_RETRY, fast};
use crate::bytecode::template::func_flags;
use crate::bytecode::{OPS, Op, special};
use crate::heap::ObjectClass;
use crate::interp::frame::{self, Continuation, HEADER};
use crate::value::Value;

const CONSTRUCT: u8 = Op::Construct as u8;
const SUPER_CALL: u8 = Op::SuperCall as u8;
const SUPER_CALL_SPREAD: u8 = Op::SuperCallSpread as u8;
const REST: u8 = Op::Rest as u8;
const GET_SUPER: u8 = Op::GetSuper as u8;
const CHECK_CTOR: u8 = Op::CheckCtor as u8;
const CHECK_THIS: u8 = Op::CheckThis as u8;
const CHECK_THIS_UNINIT: u8 = Op::CheckThisUninit as u8;
const CHECK_CTOR_RETURN: u8 = Op::CheckCtorReturn as u8;

/// The opcodes `decline_construct` takes: the test before its call, so that the other
/// declined instructions pay no call on their way to `step`.
#[inline(always)]
pub(super) fn takes(op: u8) -> bool {
    matches!(
        op,
        CONSTRUCT
            | SUPER_CALL
            | SUPER_CALL_SPREAD
            | REST
            | fast::RETURN
            | fast::RETURN_UNDEF
            | fast::SPECIAL_OBJECT
            | CHECK_CTOR
            | CHECK_THIS
            | CHECK_THIS_UNINIT
            | CHECK_CTOR_RETURN
            | GET_SUPER
    )
}

impl Exec<'_> {
    /// The instruction `op` at `pc` (declined by `run_hot`), if it is one of the
    /// construct path's and its case needs nothing of `step`.
    #[inline(never)]
    pub(super) fn decline_construct(&mut self, op: u8) -> bool {
        if self.pc >= self.code_len {
            return false;
        }
        let done = match op {
            CONSTRUCT | SUPER_CALL | SUPER_CALL_SPREAD => self.construct_fast(op),
            REST => {
                // An allocation that fails changes nothing: `step` runs it then.
                let Ok(a) = self.rest_array() else {
                    return false;
                };
                self.push(a);
                self.advance_op(Op::Rest)
            }
            fast::RETURN | fast::RETURN_UNDEF => self.return_constructed(op),
            fast::SPECIAL_OBJECT => {
                let v = match self.byte(self.pc + 1) {
                    special::NEW_TARGET => self.header(frame::NEW_TARGET),
                    special::ACTIVE_FUNCTION => self.get(self.bp),
                    _ => return false,
                };
                self.push(v);
                self.advance_op(Op::SpecialObject)
            }
            CHECK_CTOR => {
                self.header_int(frame::FLAGS) as i32 & frame::FLAG_CONSTRUCT != 0
                    && self.advance_op(Op::CheckCtor)
            }
            CHECK_THIS => !self.top(0).is_undefined() && self.advance_op(Op::CheckThis),
            CHECK_THIS_UNINIT => {
                if !self.top(0).is_undefined() {
                    return false;
                }
                self.sp -= 1;
                self.advance_op(Op::CheckThisUninit)
            }
            CHECK_CTOR_RETURN => {
                let v = self.top(0);
                (v.is_undefined() || v.as_object().is_some())
                    && self.advance_op(Op::CheckCtorReturn)
            }
            GET_SUPER => {
                // The home object or the active function: never a proxy.
                let Some(o) = self.top(0).as_object() else {
                    return false;
                };
                let p = self
                    .heap
                    .get_prototype_of(o)
                    .map_or(Value::NULL, Value::object);
                self.set(self.sp - 1, p);
                self.advance_op(Op::GetSuper)
            }
            _ => false,
        };
        // As `step_once` leaves it: a retried instruction is over (D1700).
        if done && self.retry_len == 0 {
            self.retry_len = NO_RETRY;
        }
        done
    }

    /// Charges `op` and moves past it (true, for the handlers' `&&`).
    fn advance_op(&mut self, op: Op) -> bool {
        let info = &OPS[op as usize];
        self.fuel -= info.cost.fuel() as i64;
        self.pc += info.size as u32;
        true
    }

    /// `construct argc` (`f a1 .. an`), `super_call argc` (`f a1 .. an newTarget`) or
    /// `super_call_spread` (`f args newTarget`, the arguments in an array) of a
    /// bytecode function or builtin constructor whose frame fits the stack: what
    /// `construct_with` does after its checks.
    fn construct_fast(&mut self, op: u8) -> bool {
        let sup = op != CONSTRUCT;
        let (base, argc, spread) = if op == SUPER_CALL_SPREAD {
            // The compiler's argument array: its elements, in place of it (`spread_args`).
            let Some(a) = self.top(1).as_object() else {
                return false;
            };
            let n = self.heap.array_length(a);
            if (0..n).any(|i| self.heap.get_element_fast(a, i).is_none()) {
                return false;
            }
            (self.sp - 3, n, Some(a))
        } else {
            let argc = self.imm_u16(1);
            (self.sp - 1 - argc - sup as u32, argc, None)
        };
        let Some(fo) = self.get(base).as_object() else {
            return false;
        };
        // (Bound functions and proxies are not of this class.)
        if self.heap.object_class(fo) != ObjectClass::Function || !self.is_constructor(fo) {
            return false;
        }
        let t = self.template_of(fo);
        let th = super::hot::template_head(self.heap.arena().bytes(), t);
        // A native frame, or a sloppy `this` to bind (builtins have none; another
        // function's `this` is an object): `construct_call`'s other paths.
        let builtin =
            self.heap.cell_flags(fo.offset()) & crate::heap::cells::flags::OBJ_BUILTIN != 0;
        if th.flags & self.slow_call_flags & func_flags::NATIVE != 0
            || (builtin && th.flags & func_flags::SLOPPY_THIS != 0)
        {
            return false;
        }
        let top = base + 2 + argc.max(th.params) + HEADER + th.locals + th.stack;
        if top > self.heap.value_array_capacity(self.stack) {
            return false;
        }
        let nt = sup.then(|| self.top(0));
        let info = &OPS[op as usize];
        let next = self.pc + info.size as u32;
        // A constructor's `new` entry, as `construct_with` tries it: a host refusal
        // leaves the instruction to `step`.
        if !sup && th.flags & self.slow_call_flags & func_flags::FAST != 0 {
            match self.try_fast_new(t, base, argc, next) {
                Ok(true) => {
                    self.fuel -= info.cost.fuel() as i64;
                    return true;
                }
                Ok(false) => {}
                Err(_) => return false,
            }
        }
        // Allocates `this` (or fails) before anything changes.
        let Ok(Some(this)) = self.construct_this(fo, nt) else {
            return false;
        };
        self.fuel -= info.cost.fuel() as i64;
        if let Some(a) = spread {
            // `f this a1 .. an` (the frame overwrites what lies above).
            for i in 0..argc {
                let v = self.heap.get_element_fast(a, i).unwrap_or(Value::UNDEFINED);
                self.set(base + 2 + i, v);
            }
            self.set(base + 1, this);
            self.sp = base + 2 + argc;
        } else {
            self.sp -= sup as u32;
            self.insert_this(base, argc, this);
        }
        // Cannot fail: the frame fits and `this` needs no binding.
        let r = self.construct_call(base, argc, nt, next);
        debug_assert!(matches!(r, Ok(None)));
        true
    }

    /// `return` / `return_undef` from a frame `construct` entered: the result, or
    /// `this` for a non-object (`do_return`, which cannot fail for such a frame).
    fn return_constructed(&mut self, op: u8) -> bool {
        let flags = self.header_int(frame::FLAGS) as i32;
        if Continuation::from_flags(flags) != Continuation::ConstructResult {
            return false;
        }
        self.fuel -= OPS[op as usize].cost.fuel() as i64;
        let v = if op == fast::RETURN {
            self.pop()
        } else {
            Value::UNDEFINED
        };
        let r = self.do_return(v);
        debug_assert!(matches!(r, Ok(None)));
        true
    }
}
