//! The dispatch loop (docs/compiler.md 7).
//!
//! [`Exec`] holds the registers in Rust locals while it runs and flushes them to the
//! `Interp` cell when it stops. It never recurses on the Rust stack: a JS call pushes a
//! frame record on the arena value stack and continues the loop.
//!
//! Fuel: every instruction charges its cost class. `run` stops before the next instruction
//! once the fuel is used up, so resuming continues exactly there.
//!
//! Safepoints (back jumps, frame entry, returns, the back-edges of native builtins and the
//! out-of-memory handler; not the start of `run`, so that collection points depend only
//! on the program, never on where a fuel slice ended): when the heap asks for a collection
//! (or always, with `gc_stress`), the registers are flushed, the stack above `sp` is
//! cleared, the heap is collected, and the cached code and stack offsets are re-derived
//! from the roots and `stack[bp]`. Everything live is in the arena, so the collector needs
//! nothing else.

use alloc::string::String;

use super::conv::{self, Fault, FaultKind, type_error};
use super::frame::{self, Continuation, HEADER};
use super::realm::{self, ErrorKind, Intrinsic, intrinsic, intrinsic_obj};
use super::state::{self, PARK_ENTRY, RunState, park_flags};
use crate::bytecode::decode::{LAYOUT, arity, read_i8};
use crate::bytecode::global_decls::GlobalDecls;
use crate::bytecode::template::{self, func_flags};
use crate::bytecode::{HandlerTable, OPS, Op, special};
use crate::heap::cells::flags::OBJ_HOST_ASYNC;
use crate::heap::cells::{BoxCell, FuncTemplate, HostCall, Interp, Roots, U32Array, ValueArray};
use crate::heap::{
    Atom, CellKind, Heap, HeapError, Lookup, ObjectClass, PropFlags, Property, PropertyDescriptor,
    Ref,
};
use crate::value::{Kind, Obj, Value};

/// Why `run` returned.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Status {
    /// The call returned this value, and no job or async host call is left (D1370).
    Done(Value),
    /// The call (or a job) threw this value; no job or async host call is left.
    Threw(Value),
    /// The fuel ran out; `run` again to continue.
    OutOfFuel,
    /// Parked on a sync host call: resolve it, then `run` again.
    HostCall { call_id: u32, fn_id: u32 },
    /// Nothing to run: no call in progress, or the job queue is empty and only async
    /// host calls are pending (resolve them, then `run` again).
    Idle,
    /// The host could not provide memory below the cap (D1690), or the heap reached its
    /// cap with [`ExecOptions::fatal_out_of_memory`]. Not a JS error: the call is
    /// abandoned (`Vm::run` aborts it) and the VM is idle and usable.
    HostOutOfMemory,
}

/// The message of a stack overflow (a RangeError, D013).
pub const STACK_OVERFLOW: &str = "Maximum call stack size exceeded";

/// Interpreter options that are not VM state.
#[derive(Clone, Copy, Debug)]
pub struct ExecOptions {
    /// Collect at every safepoint (a stress mode for tests).
    pub gc_stress: bool,
    /// Maximum value stack size in slots (`RangeError: Maximum call stack size exceeded`
    /// beyond, D013).
    pub max_stack: u32,
    /// Queue FinalizationRegistry cleanup jobs right after every collection (the
    /// default). Off, the host decides when: `Vm::schedule_finalization_cleanup` (D1513).
    pub finalization_after_gc: bool,
    /// Run `@native` builtins as their generated Rust state machines (0063, the
    /// default); off runs every builtin as bytecode (`--backend bytecode`). The choice
    /// is made per call: frames already running keep their kind.
    pub native_builtins: bool,
    /// Run the hottest simple instructions in the tier-0 loop (D1930, the default).
    /// Results, fuel and heap images are identical either way; off is for tests.
    pub hot_loop: bool,
    /// Backward jumps after which the tier-0 loop returns to the run loop
    /// (`hot::HOT_BACKEDGES`; tests lower it to force those exits).
    pub hot_backedges: u32,
    /// Reaching the heap's memory cap ends the run like host memory exhaustion
    /// (`Status::HostOutOfMemory`, D3181) instead of throwing a catchable
    /// `InternalError`: no `catch` or `finally` runs, the call is abandoned and the VM is
    /// idle. For embedders whose scripts must not swallow it (djs-embed). Only in builds
    /// with the `embed` feature: the wasm modules copy these options around a lot.
    #[cfg(feature = "embed")]
    pub fatal_out_of_memory: bool,
}

impl Default for ExecOptions {
    fn default() -> Self {
        ExecOptions {
            gc_stress: false,
            max_stack: 1 << 20,
            finalization_after_gc: true,
            native_builtins: true,
            hot_loop: true,
            hot_backedges: hot::HOT_BACKEDGES,
            #[cfg(feature = "embed")]
            fatal_out_of_memory: false,
        }
    }
}

mod bigint;
mod class;
mod construct;
mod dense;
mod each;
mod fastpath;
mod hot;
#[cfg(test)]
pub(crate) use hot::HOT_BACKEDGES;
pub(crate) use stack::settled_stack;
mod ic;
mod intrinsics;
mod iter;
mod json_regexp;
pub(crate) mod late;
#[cfg(feature = "lazy")]
mod lazy;
pub(crate) mod native;
pub(crate) mod promise;
mod proxy;
mod stack;
pub(crate) mod suspend;
#[cfg(feature = "dump")]
pub(crate) use intrinsics::builtin_closure_kind;

/// The opcodes `Exec::fast_step` handles, as bytes for its `match`.
mod fast {
    use crate::bytecode::Op;
    pub const GET_LOC: u8 = Op::GetLoc as u8;
    pub const PUT_LOC: u8 = Op::PutLoc as u8;
    pub const SET_LOC: u8 = Op::SetLoc as u8;
    pub const GET_ARG: u8 = Op::GetArg as u8;
    pub const PUSH_I8: u8 = Op::PushI8 as u8;
    pub const PUSH_I32: u8 = Op::PushI32 as u8;
    pub const UNDEFINED: u8 = Op::Undefined as u8;
    pub const DROP: u8 = Op::Drop as u8;
    pub const DUP: u8 = Op::Dup as u8;
    pub const DUP2: u8 = Op::Dup2 as u8;
    pub const ITER_NEXT: u8 = Op::IterNext as u8;
    pub const ITER_NEXT_LOC: u8 = Op::IterNextLoc as u8;
    pub const TO_NUMERIC: u8 = Op::ToNumeric as u8;
    pub const INC: u8 = Op::Inc as u8;
    pub const INC_LOC: u8 = Op::IncLoc as u8;
    pub const DEC_LOC: u8 = Op::DecLoc as u8;
    pub const DEC: u8 = Op::Dec as u8;
    pub const ADD: u8 = Op::Add as u8;
    pub const SUB: u8 = Op::Sub as u8;
    pub const MUL: u8 = Op::Mul as u8;
    pub const DIV: u8 = Op::Div as u8;
    pub const LT: u8 = Op::Lt as u8;
    pub const LE: u8 = Op::Le as u8;
    pub const GT: u8 = Op::Gt as u8;
    pub const GE: u8 = Op::Ge as u8;
    pub const JUMP: u8 = Op::Jump as u8;
    pub const JUMP_IF_FALSE: u8 = Op::JumpIfFalse as u8;
    pub const JUMP_IF_TRUE: u8 = Op::JumpIfTrue as u8;
    pub const CALL: u8 = Op::Call as u8;
    pub const RETURN: u8 = Op::Return as u8;
    pub const RETURN_UNDEF: u8 = Op::ReturnUndef as u8;
    pub const CALL_INTRINSIC: u8 = Op::CallIntrinsic as u8;
    pub const GET_ELEM: u8 = Op::GetElem as u8;
    pub const GET_ELEM2: u8 = Op::GetElem2 as u8;
    pub const PUT_ELEM: u8 = Op::PutElem as u8;
    pub const GET_FIELD: u8 = Op::GetField as u8;
    pub const GET_METHOD: u8 = Op::GetMethod as u8;
    pub const PUT_FIELD: u8 = Op::PutField as u8;
    // `fast_more` (0086)
    pub const DEFINE_FIELD: u8 = Op::DefineField as u8;
    pub const NULL: u8 = Op::Null as u8;
    pub const PUSH_TRUE: u8 = Op::PushTrue as u8;
    pub const PUSH_FALSE: u8 = Op::PushFalse as u8;
    pub const PUSH_CONST: u8 = Op::PushConst as u8;
    pub const SPECIAL_OBJECT: u8 = Op::SpecialObject as u8;
    pub const SWAP: u8 = Op::Swap as u8;
    pub const NIP: u8 = Op::Nip as u8;
    pub const INSERT2: u8 = Op::Insert2 as u8;
    pub const GET_CAP: u8 = Op::GetCap as u8;
    // (0151)
    pub const GET_BUILTIN: u8 = Op::GetBuiltin as u8;
    pub const GET_CAP_CHK: u8 = Op::GetCapChk as u8;
    pub const PUT_CAP: u8 = Op::PutCap as u8;
    pub const GET_BOX: u8 = Op::GetBox as u8;
    pub const PUT_BOX: u8 = Op::PutBox as u8;
    // (0100)
    pub const PUT_CAP_CHK: u8 = Op::PutCapChk as u8;
    pub const SET_CAP: u8 = Op::SetCap as u8;
    pub const SET_CAP_CHK: u8 = Op::SetCapChk as u8;
    pub const GET_BOX_CHK: u8 = Op::GetBoxChk as u8;
    pub const PUT_BOX_CHK: u8 = Op::PutBoxChk as u8;
    pub const SET_BOX: u8 = Op::SetBox as u8;
    pub const SET_BOX_CHK: u8 = Op::SetBoxChk as u8;
    pub const NOT: u8 = Op::Not as u8;
    pub const STRICT_EQ: u8 = Op::StrictEq as u8;
    pub const STRICT_NE: u8 = Op::StrictNe as u8;
    pub const MOD: u8 = Op::Mod as u8;
    pub const BAND: u8 = Op::BAnd as u8;
    pub const BOR: u8 = Op::BOr as u8;
    pub const BXOR: u8 = Op::BXor as u8;
    pub const SHL: u8 = Op::Shl as u8;
    pub const SAR: u8 = Op::Sar as u8;
    pub const IS_UNDEFINED: u8 = Op::IsUndefined as u8;
    pub const IS_NULLISH: u8 = Op::IsNullish as u8;
    // Short forms (D2443)
    pub const GET_LOC0: u8 = Op::GetLoc0 as u8;
    pub const GET_LOC1: u8 = Op::GetLoc1 as u8;
    pub const GET_LOC2: u8 = Op::GetLoc2 as u8;
    pub const GET_LOC3: u8 = Op::GetLoc3 as u8;
    pub const PUT_LOC0: u8 = Op::PutLoc0 as u8;
    pub const PUT_LOC1: u8 = Op::PutLoc1 as u8;
    pub const PUT_LOC2: u8 = Op::PutLoc2 as u8;
    pub const PUT_LOC3: u8 = Op::PutLoc3 as u8;
    pub const GET_ARG0: u8 = Op::GetArg0 as u8;
    pub const GET_ARG1: u8 = Op::GetArg1 as u8;
    pub const GET_ARG2: u8 = Op::GetArg2 as u8;
    pub const GET_ARG3: u8 = Op::GetArg3 as u8;
    pub const SET_LOC0: u8 = Op::SetLoc0 as u8;
    pub const SET_LOC1: u8 = Op::SetLoc1 as u8;
    pub const SET_LOC2: u8 = Op::SetLoc2 as u8;
    pub const SET_LOC3: u8 = Op::SetLoc3 as u8;
    pub const PUSH0: u8 = Op::Push0 as u8;
    pub const PUSH1: u8 = Op::Push1 as u8;
    pub const PUSH2: u8 = Op::Push2 as u8;
    pub const PUSH3: u8 = Op::Push3 as u8;
    pub const CALL0: u8 = Op::Call0 as u8;
    pub const CALL1: u8 = Op::Call1 as u8;
    pub const CALL2: u8 = Op::Call2 as u8;
    pub const CALL3: u8 = Op::Call3 as u8;
    pub const GET_LOC8: u8 = Op::GetLoc8 as u8;
    pub const PUT_LOC8: u8 = Op::PutLoc8 as u8;
    pub const SET_LOC8: u8 = Op::SetLoc8 as u8;
    pub const GET_ARG8: u8 = Op::GetArg8 as u8;
    pub const PUSH_CONST8: u8 = Op::PushConst8 as u8;
    pub const CALL_INTRINSIC8: u8 = Op::CallIntrinsic8 as u8;
    pub const GET_BUILTIN_CALLEE8: u8 = Op::GetBuiltinCallee8 as u8;
    pub const JUMP8: u8 = Op::Jump8 as u8;
    pub const JUMP_IF_FALSE8: u8 = Op::JumpIfFalse8 as u8;
    pub const JUMP_IF_TRUE8: u8 = Op::JumpIfTrue8 as u8;

    /// A `call` in any form.
    pub const fn is_call(op: u8) -> bool {
        op == CALL || (op >= CALL0 && op <= CALL3)
    }
}

/// The instructions `Exec::retry_operands` retries after a collection (D1700), besides
/// `put_elem` and `put_field`.
mod retry {
    use super::intrinsics::id;
    use crate::bytecode::Op;
    pub const DEFINE_FIELD: u8 = Op::DefineField as u8;
    pub const ADD: u8 = Op::Add as u8;
    pub const OBJECT: u8 = Op::Object as u8;
    #[cfg(feature = "lazy")]
    pub const LAZY: u8 = Op::Lazy as u8;
    /// Intrinsics that allocate before they change anything (and take at most 4
    /// arguments).
    pub const INTRINSICS: [u16; 8] = [
        id("Set"),
        id("CreateDataPropertyOrThrow"),
        id("DefineDataPropertyOrThrow"),
        id("ArrayCreate"),
        id("ObjectCreate"),
        id("NewRecord"),
        id("CreateIterResultObject"),
        id("CreateArrayFromPair"),
    ];

    /// `INTRINSICS` as a table by id (`contains` on the hot native call path was ~55
    /// instructions, 0100).
    pub const IS_RETRY: [bool; crate::bytecode::intrinsics::INTRINSICS.len()] = {
        let mut t = [false; crate::bytecode::intrinsics::INTRINSICS.len()];
        let mut i = 0;
        while i < INTRINSICS.len() {
            t[INTRINSICS[i] as usize] = true;
            i += 1;
        }
        t
    };

    /// Whether intrinsic `id` is one of `INTRINSICS`.
    #[inline(always)]
    pub fn retries(id: u16) -> bool {
        IS_RETRY.get(id as usize).copied().unwrap_or(false)
    }
}

pub(crate) type Step = Result<Option<Status>, Fault>;

/// What a property lookup found (`Exec::lookup`).
enum Got {
    Value(Value),
    /// A getter to call with the receiver.
    Getter(Value),
    /// A proxy on the chain: its `get` trap answers (`%ProxyGet(p, key, receiver)`).
    Proxy(Obj),
}

/// Frames in a stack trace: the default `Error.stackTraceLimit` (qjs's and V8's), which
/// errors follow (`realm::stack_trace_limit`, D2030), and the limit of `Vm::backtrace`.
pub const STACK_TRACE_LIMIT: u32 = 10;

pub(crate) struct Exec<'h> {
    pub heap: &'h mut Heap,
    opts: ExecOptions,
    fuel: i64,
    /// The value stack cell.
    stack: u32,
    bp: u32,
    hp: u32,
    sp: u32,
    pc: u32,
    // Cached from the current frame's template; re-derived after every safepoint.
    template: u32,
    code: u32,
    code_len: u32,
    consts: u32,
    lp: u32,
    /// The current frame's native function (index + 1 into `builtins::gen::NATIVES`), or
    /// 0 for a bytecode frame. Native frames have `code_len` 0, so `fast_step` declines
    /// them and `step` hands them to `native_step`.
    native: u32,
    /// Template flags that make `fast_call_at` decline: SLOPPY_THIS, and NATIVE when
    /// native builtins are on.
    slow_call_flags: u32,
    /// The host refused memory (D1690): the run stops at the next instruction boundary.
    host_oom: bool,
    /// The operands of the instruction being executed, if it can be retried after a
    /// collection when it runs out of memory (D1700): their count, or `NO_RETRY`.
    retry_len: u32,
    retry_ops: [Value; 4],
    /// A native builtin's operation is being retried after a collection (D1900): an
    /// operation inside it does not retry again.
    n_retrying: bool,
    /// The result of a reentrant call a native completed inline (`n_inline`), taken by
    /// the resume state's `n_result` right after: it skips the operand stack (D2000).
    /// Never set across anything that allocates, suspends or leaves the native.
    n_res: Option<Value>,
    /// The fuel the run started its instructions with (after paying the debt, D1740);
    /// `None` while it has not executed any (it returned a status right away).
    fuel_start: Option<i64>,
}

/// `Exec::retry_len` when the current instruction is not retried.
const NO_RETRY: u32 = u32::MAX;

impl<'h> Exec<'h> {
    /// Loads the registers from the `Interp` cell.
    pub fn new(heap: &'h mut Heap, opts: ExecOptions) -> Exec<'h> {
        let i = state::interp(heap);
        let a = heap.arena();
        let (bp, hp, sp, pc) = (
            a.get(i, Interp::BP),
            a.get(i, Interp::HP),
            a.get(i, Interp::SP),
            a.get(i, Interp::PC),
        );
        let stack = state::stack(heap);
        Exec {
            heap,
            opts,
            fuel: 0,
            stack,
            bp,
            hp,
            sp,
            pc,
            template: 0,
            code: 0,
            code_len: 0,
            consts: 0,
            lp: 0,
            native: 0,
            slow_call_flags: func_flags::SLOPPY_THIS
                | if opts.native_builtins {
                    func_flags::NATIVE | func_flags::FAST
                } else {
                    0
                },
            host_oom: false,
            retry_len: NO_RETRY,
            retry_ops: [Value::UNDEFINED; 4],
            n_retrying: false,
            n_res: None,
            fuel_start: None,
        }
    }

    /// The host refused memory: stop the run at the next instruction boundary (the
    /// fuel check) with `Status::HostOutOfMemory` (D1690).
    #[cold]
    pub(super) fn host_out_of_memory(&mut self) {
        self.host_oom = true;
        self.fuel = 0;
    }

    /// The fuel the last `run` spent on instructions (its overshoot included, the debt
    /// it paid not: D1740).
    pub(crate) fn fuel_used(&self) -> u64 {
        self.fuel_start
            .map_or(0, |start| start.saturating_sub(self.fuel).max(0) as u64)
    }

    /// The current pc (for `Vm::backtrace`).
    pub(crate) fn pc(&self) -> u32 {
        self.pc
    }

    /// Writes the registers to the `Interp` cell.
    pub(crate) fn flush(&mut self) {
        let i = state::interp(self.heap);
        let a = self.heap.arena_mut();
        a.set(i, Interp::BP, self.bp);
        a.set(i, Interp::HP, self.hp);
        a.set(i, Interp::SP, self.sp);
        a.set(i, Interp::PC, self.pc);
    }

    // ---------------------------------------------------------------- stack

    pub(crate) fn set_slot(&mut self, i: u32, v: Value) {
        self.set(i, v);
    }

    pub(crate) fn set_sp(&mut self, sp: u32) {
        self.sp = sp;
    }

    #[inline(always)]
    fn get(&self, i: u32) -> Value {
        self.heap.arena().read(ValueArray::VALUES.at(self.stack, i))
    }

    #[inline(always)]
    fn set(&mut self, i: u32, v: Value) {
        let at = ValueArray::VALUES.at(self.stack, i);
        self.heap.arena_mut().write_untracked(at, v)
    }

    #[inline(always)]
    pub(crate) fn push(&mut self, v: Value) {
        self.set(self.sp, v);
        self.sp += 1;
    }

    #[inline(always)]
    fn pop(&mut self) -> Value {
        self.sp -= 1;
        self.get(self.sp)
    }

    #[inline(always)]
    fn top(&self, k: u32) -> Value {
        self.get(self.sp - 1 - k)
    }

    #[inline(always)]
    fn header(&self, f: u32) -> Value {
        self.get(self.hp + f)
    }

    #[inline(always)]
    fn header_int(&self, f: u32) -> u32 {
        self.header(f).as_int().unwrap_or(0) as u32
    }

    /// Makes room for `top` slots, growing the stack by copying.
    #[inline(always)]
    pub(crate) fn ensure(&mut self, top: u32) -> Result<(), Fault> {
        if top <= self.heap.value_array_capacity(self.stack) {
            return Ok(());
        }
        self.grow_stack(top)
    }

    #[cold]
    #[inline(never)]
    fn grow_stack(&mut self, top: u32) -> Result<(), Fault> {
        let cap = self.heap.value_array_capacity(self.stack);
        if top > self.opts.max_stack {
            // V8's and quickjs-ng's error, which pi's codemode expects (D013).
            return Err(Fault::error(
                ErrorKind::RangeError,
                String::from(STACK_OVERFLOW),
            ));
        }
        let new_cap = top
            .max(cap.saturating_mul(2))
            .min(self.opts.max_stack.max(top));
        self.stack = state::move_stack(self.heap, self.sp, new_cap)?;
        Ok(())
    }

    // ---------------------------------------------------------------- frames

    /// The template of a closure (slot 0 of a Function object).
    #[inline(always)]
    fn template_of(&self, f: Obj) -> u32 {
        self.heap.closure_template(f)
    }

    /// Re-derives the cached frame data from `stack[bp]`.
    fn load_frame(&mut self) {
        let f = self.get(self.bp).as_object().expect("frame callee");
        let t = self.template_of(f);
        self.enter_template(t);
        if self.header_int(frame::FLAGS) as i32 & frame::FLAG_NATIVE != 0 {
            self.enter_native(t);
        }
    }

    /// Sets the cached frame data for template `t` (with `hp` already set).
    #[inline(always)]
    fn enter_template(&mut self, t: u32) {
        let a = self.heap.arena();
        self.template = t;
        self.code = FuncTemplate::DATA.at(t, 0);
        self.code_len = a.get(t, FuncTemplate::CODE_LEN);
        self.consts = a.get(t, FuncTemplate::CONSTS).0;
        self.lp = self.hp + HEADER;
        self.native = 0;
    }

    /// The stack top a frame for template `t` called with `argc` arguments at `base`
    /// needs: arguments padded to the parameter count, header, locals, operand stack.
    #[cfg(test)]
    #[inline(always)]
    fn frame_top(&self, t: u32, base: u32, argc: u32) -> u32 {
        let a = self.heap.arena();
        let params = a.get(t, FuncTemplate::PARAMS) as u32;
        let locals = a.get(t, FuncTemplate::LOCALS) as u32;
        let stack = a.get(t, FuncTemplate::STACK) as u32;
        base + 2 + argc.max(params) + HEADER + locals + stack
    }

    /// D015: OrdinaryCallBindThis for a sloppy function, on the `this` slot: undefined
    /// and null become the global object, primitives their wrapper objects.
    #[cold]
    fn bind_sloppy_this(&mut self, slot: u32) -> Result<(), Fault> {
        let this = self.get(slot);
        let bound = if this.is_undefined() || this.is_null() {
            realm::intrinsic(self.heap, Intrinsic::Global)
        } else if this.as_object().is_none() {
            match realm::wrap_primitive(self.heap, this, None)? {
                Some(o) => Value::object(o),
                None => this,
            }
        } else {
            return Ok(());
        };
        self.set(slot, bound);
        Ok(())
    }

    /// Pushes the frame for closure template `t` (room checked by the caller with
    /// `frame_top`) and enters it at pc 0.
    #[cfg(test)]
    #[inline(always)]
    fn push_frame(&mut self, t: u32, base: u32, argc: u32, cont: Continuation, ret_pc: u32) {
        let th = hot::template_head(self.heap.arena().bytes(), t);
        self.push_frame_head(t, &th, base, argc, cont, ret_pc, None);
    }

    /// `push_frame` with the template's fields already read. `new_target` is a
    /// construct's: the frame is flagged as one and holds it before the entry's
    /// safepoint, which may move it.
    #[inline(always)]
    #[allow(clippy::too_many_arguments)]
    fn push_frame_head(
        &mut self,
        t: u32,
        th: &hot::TemplateHead,
        base: u32,
        argc: u32,
        cont: Continuation,
        ret_pc: u32,
        new_target: Option<Value>,
    ) {
        let (params, locals) = (th.params, th.locals);
        let n = argc.max(params);
        let hp = base + 2 + n;
        let lp = hp + HEADER;
        let entry = cont == Continuation::Entry;
        let bottom = cont.is_bottom();
        let header = [
            Value::int(if bottom { 0 } else { self.bp as i32 }),
            Value::int(if bottom { 0 } else { self.hp as i32 }),
            Value::int(ret_pc as i32),
            Value::int(argc as i32),
            Value::int(cont.to_flags() | new_target.map_or(0, |_| frame::FLAG_CONSTRUCT)),
            new_target.unwrap_or(Value::UNDEFINED),
        ];
        let vals = ValueArray::VALUES.at(self.stack, 0);
        hot::write_frame(
            self.heap.arena_mut().hot_bytes(),
            vals,
            (base, argc, n, hp, locals),
            header,
        );
        self.bp = base;
        self.hp = hp;
        self.lp = lp;
        self.sp = lp + locals;
        self.pc = 0;
        self.template = t;
        self.code = FuncTemplate::DATA.at(t, 0);
        self.code_len = th.code_len;
        self.consts = th.consts_ref;
        self.native = 0;
        // Frame entry is a safepoint, except for the entry call: `Vm::call` must
        // not collect, so the embedder's raw values stay valid until `run`.
        if !entry {
            self.safepoint();
        }
    }

    fn locals(&self) -> u32 {
        self.heap.arena().get(self.template, FuncTemplate::LOCALS) as u32
    }

    /// Calls `stack[base]` with `this = stack[base + 1]` and the `argc` arguments above
    /// it (`base + 2 + argc == sp`). JS closures get a frame; host functions park the VM.
    pub(crate) fn call(&mut self, base: u32, argc: u32, cont: Continuation, ret_pc: u32) -> Step {
        let f = self.get(base);
        let Some(o) = f.as_object() else {
            return type_error("not a function");
        };
        match self.heap.object_class(o) {
            ObjectClass::Function => {
                let t = self.template_of(o);
                let flags = u32::from(self.heap.arena().get::<u16>(t, FuncTemplate::FLAGS));
                if flags & self.slow_call_flags & func_flags::FAST != 0
                    && self.try_fast_call(t, base, argc, cont, ret_pc)?
                {
                    return Ok(None);
                }
                if flags & self.slow_call_flags & func_flags::NATIVE != 0 {
                    return self.push_native_frame(t, base, argc, cont, ret_pc);
                }
                if flags & func_flags::SLOPPY_THIS != 0 {
                    self.bind_sloppy_this(base + 1)?;
                }
                let th = hot::template_head(self.heap.arena().bytes(), t);
                let top = base + 2 + argc.max(th.params) + HEADER + th.locals + th.stack;
                self.ensure(top)?;
                self.push_frame_head(t, &th, base, argc, cont, ret_pc, None);
                Ok(None)
            }
            ObjectClass::HostFunction => {
                let fn_id = self.heap.object_slot(o, 0).as_int().unwrap_or(0) as u32;
                let args = self.heap.alloc_value_array(argc)?;
                for i in 0..argc {
                    let v = self.get(base + 2 + i);
                    self.heap.value_array_set(args, i, v);
                }
                self.heap.arena_mut().set(args, ValueArray::LEN, argc);
                let this = self.get(base + 1);
                if self.heap.cell_flags(o.offset()) & OBJ_HOST_ASYNC != 0 {
                    return self.async_host_call(fn_id, args, this, base, cont, ret_pc);
                }
                let call_id = add_host_call(self.heap, fn_id, args, this, Value::UNDEFINED)?;
                self.sp = base;
                self.pc = ret_pc;
                let i = state::interp(self.heap);
                let a = self.heap.arena_mut();
                a.set(i, Interp::HOST_CALL, call_id);
                a.set(
                    i,
                    Interp::FLAGS,
                    park_flags(cont, cont == Continuation::Entry),
                );
                state::set_state(self.heap, RunState::HostCall);
                self.flush();
                Ok(Some(Status::HostCall { call_id, fn_id }))
            }
            ObjectClass::BoundFunction => {
                let argc = self.unwrap_bound(base, argc, o)?;
                self.call(base, argc, cont, ret_pc)
            }
            ObjectClass::Proxy if self.heap.is_callable(o) => {
                self.proxy_call(base, argc, cont, ret_pc)
            }
            ObjectClass::BuiltinClosure => {
                // `F this args` becomes `builtin data args` (D1372).
                let id = self.heap.object_slot(o, 0).as_int().unwrap_or(-1);
                let target = u16::try_from(id)
                    .map(|id| realm::installed_intrinsic(self.heap, id))
                    .unwrap_or(Value::UNDEFINED);
                if target.as_object().is_none() {
                    return Err(Fault::error(
                        ErrorKind::InternalError,
                        String::from("bad builtin closure"),
                    ));
                }
                let data = self.heap.object_slot(o, 1);
                self.set(base, target);
                self.set(base + 1, data);
                self.call(base, argc, cont, ret_pc)
            }
            _ => type_error("not a function"),
        }
    }

    /// Calls an async host function (D1374): a new promise is the result, and the call
    /// waits in the pending host call table until the embedder resolves it.
    fn async_host_call(
        &mut self,
        fn_id: u32,
        args: u32,
        this: Value,
        base: u32,
        cont: Continuation,
        ret_pc: u32,
    ) -> Step {
        let proto = intrinsic_obj(self.heap, Intrinsic::PromiseProto);
        let p = Value::object(promise::alloc(self.heap, Some(proto))?);
        add_host_call(self.heap, fn_id, args, this, p)?;
        self.sp = base;
        self.pc = ret_pc;
        match cont {
            Continuation::PushResult | Continuation::ConstructResult => self.push(p),
            Continuation::DiscardResult => {}
            Continuation::Replace(slot) | Continuation::Retry(slot) => self.set(slot, p),
            // (`%EachDense` calls plain closures only.)
            Continuation::Each => unreachable!(),
            // `Vm::call` of an async host function: done at once (it must not run jobs,
            // which could collect); the next `run` drains the queue.
            Continuation::Entry => {
                state::set_outcome(self.heap, RunState::Done, p);
                self.flush();
            }
            Continuation::Job => return Ok(self.bottom_done(cont, Ok(p))),
        }
        Ok(None)
    }

    /// Rewrites `F this a1 .. an` for a bound function F into `target boundThis b1 .. bk
    /// a1 .. an` (the bound `this` is ignored by `construct`, which puts its own object
    /// there). Returns the new argument count.
    fn unwrap_bound(&mut self, base: u32, argc: u32, f: Obj) -> Result<u32, Fault> {
        let target = self.heap.object_slot(f, 0);
        let this = self.heap.object_slot(f, 1);
        let args = self.heap.object_slot(f, 2).as_object().expect("bound args");
        let k = self.heap.array_length(args);
        self.ensure(self.sp + k)?;
        for i in (0..argc).rev() {
            let v = self.get(base + 2 + i);
            self.set(base + 2 + k + i, v);
        }
        for i in 0..k {
            let v = self
                .heap
                .get_element_fast(args, i)
                .unwrap_or(Value::UNDEFINED);
            self.set(base + 2 + i, v);
        }
        self.set(base, target);
        self.set(base + 1, this);
        self.sp += k;
        Ok(argc + k)
    }

    /// Returns `v` from the current frame.
    fn do_return(&mut self, v: Value) -> Step {
        let cont = Continuation::from_flags(self.header_int(frame::FLAGS) as i32);
        let this = self.get(self.bp + 1);
        if cont.is_bottom() {
            return Ok(self.bottom_done(cont, Ok(v)));
        }
        if cont == Continuation::Each {
            return Ok(self.each_return(v));
        }
        self.pop_frame();
        match cont {
            Continuation::PushResult => self.push(v),
            Continuation::DiscardResult => {}
            Continuation::ConstructResult => {
                self.push(if v.as_object().is_some() { v } else { this })
            }
            Continuation::Replace(slot) | Continuation::Retry(slot) => self.set(slot, v),
            Continuation::Entry | Continuation::Job | Continuation::Each => unreachable!(),
        }
        self.safepoint();
        Ok(None)
    }

    /// The bottom frame of the entry call or of a job returned (`Ok`) or threw (`Err`):
    /// records the entry call's outcome, then continues with the next job, if any.
    /// D1370: an uncaught exception stops right away with `Threw`, like qjs, which runs
    /// no more jobs after an uncaught exception (queued jobs run at the next `run`). Only
    /// internal errors escape a job: the promise jobs catch everything else.
    fn bottom_done(&mut self, cont: Continuation, r: Result<Value, Value>) -> Option<Status> {
        self.sp = 0;
        self.bp = 0;
        self.hp = 0;
        self.pc = 0;
        match (cont, r) {
            (Continuation::Job, Ok(_)) => self.next_job(),
            (_, Ok(v)) => {
                state::set_outcome(self.heap, RunState::Done, v);
                self.next_job()
            }
            (_, Err(e)) => {
                state::set_outcome(self.heap, RunState::Threw, e);
                self.flush();
                Some(Status::Threw(e))
            }
        }
    }

    /// With no frame active: enters the next queued job (`None`: keep running), or
    /// stops with the outcome. D1370: `Done`/`Threw` only once the queue is empty and no
    /// async host call is pending; with pending calls, `Idle`.
    fn next_job(&mut self) -> Option<Status> {
        loop {
            // The synchronous run of a job (or of the entry call) is over: ClearKeptObjects
            // (9.10.3), which lets dereferenced WeakRef targets die again.
            late::clear_kept_objects(self.heap);
            let mut job = promise::dequeue(self.heap);
            if job.is_none() {
                // A microtask checkpoint. Then the next host result, as a macrotask of its
                // own (D1670): results are applied only here, one at a time, whenever the
                // embedder delivered them.
                promise::rejection_checkpoint(self.heap);
                match promise::inbox_apply(self.heap) {
                    Ok(true) => job = promise::dequeue(self.heap),
                    Ok(false) => {}
                    // No room for the settle job: the result stays in the inbox.
                    Err(e) => return Some(self.job_failed(Fault::from(e))),
                }
            }
            let Some(job) = job else {
                let outcome = state::outcome(self.heap);
                state::set_state(self.heap, outcome);
                self.flush();
                if pending_async_calls(self.heap) {
                    return Some(Status::Idle);
                }
                let v = state::result(self.heap);
                return Some(match outcome {
                    RunState::Done => Status::Done(v),
                    RunState::Threw => Status::Threw(v),
                    _ => Status::Idle,
                });
            };
            // Jobs are installed builtins: JS closures, which never park.
            debug_assert!(
                job[0]
                    .as_object()
                    .is_some_and(|o| self.heap.object_class(o) == ObjectClass::Function)
            );
            state::set_state(self.heap, RunState::Running);
            if job[0] == realm::installed_intrinsic(self.heap, promise::PROMISE_REACTION_JOB) {
                // The reaction job of an await (D3130): `%PromiseReactionJob` would only
                // resume the async function, so the job's bottom frame is the function's.
                if suspend::as_generator(self.heap, job[2], suspend::ASYNC).is_some() {
                    let mode = Value::int(if job[4] == Value::TRUE { 0 } else { 1 });
                    let r = self.generator_resume(job[2], job[3], mode, 0, Continuation::Job);
                    return r.unwrap_or_else(|f| Some(self.job_failed(f)));
                }
                // A combinator element that settles without user code (D3131, D3158).
                match promise::all_element_job(self.heap, job[1], job[2], job[3]) {
                    Ok(true) => continue,
                    Ok(false) => {}
                    Err(e) => return Some(self.job_failed(Fault::from(e))),
                }
            }
            let r = self.ensure(2 + 4).and_then(|()| {
                self.push(job[0]);
                self.push(Value::UNDEFINED);
                for &a in &job[1..] {
                    self.push(a);
                }
                self.call(0, 4, Continuation::Job, 0)
            });
            // (Else the job could not even start: out of memory, stack limit.)
            return r.map_or_else(|f| Some(self.job_failed(f)), |_| None);
        }
    }

    /// A job that failed outside any frame: the run stops with the exception.
    fn job_failed(&mut self, f: Fault) -> Status {
        let e = self.fault_value(f, None);
        self.sp = 0;
        state::set_outcome(self.heap, RunState::Threw, e);
        self.flush();
        Status::Threw(e)
    }

    /// Pops the current (non-entry) frame and resumes the caller at its return pc.
    #[inline(always)]
    fn pop_frame(&mut self) {
        let (prev_bp, prev_hp, ret_pc) = (
            self.header_int(frame::PREV_BP),
            self.header_int(frame::PREV_HP),
            self.header_int(frame::RET_PC),
        );
        self.sp = self.bp;
        self.bp = prev_bp;
        self.hp = prev_hp;
        self.pc = ret_pc;
        self.load_frame();
    }

    /// Throws `exc`: finds the innermost handler, unwinding frames. `key` is the pc
    /// whose try regions apply in the current frame.
    fn throw_value(&mut self, exc: Value, mut key: u32) -> Option<Status> {
        loop {
            if self.native != 0 {
                if let Some((target, slot)) = self.native_handler(key) {
                    self.sp = self.lp + self.native_locals();
                    self.set(self.lp + slot, exc);
                    self.pc = target;
                    if exc == intrinsic(self.heap, Intrinsic::OutOfMemoryError) {
                        self.safepoint_collect();
                    }
                    return None;
                }
            } else {
                let found = HandlerTable::new(template::handlers(self.heap, self.template))
                    .ok()
                    .and_then(|t| t.find(key));
                if let Some(h) = found {
                    self.sp = self.lp + self.locals() + h.depth;
                    self.push(exc);
                    self.pc = h.target;
                    if exc == intrinsic(self.heap, Intrinsic::OutOfMemoryError) {
                        // The frames that held the data are gone: collect before the
                        // handler allocates (D1600).
                        self.safepoint_collect();
                    }
                    return None;
                }
            }
            let cont = Continuation::from_flags(self.header_int(frame::FLAGS) as i32);
            if cont.is_bottom() {
                return self.bottom_done(cont, Err(exc));
            }
            let ret_pc = self.header_int(frame::RET_PC);
            self.sp = self.bp;
            self.bp = self.header_int(frame::PREV_BP);
            self.hp = self.header_int(frame::PREV_HP);
            self.load_frame();
            key = if cont.is_retry() && self.native == 0 {
                ret_pc
            } else {
                self.call_site_key(ret_pc)
            };
        }
    }

    /// Turns a fault into the thrown value.
    /// Turns a fault at `pc` of the current frame (`None`: outside any frame) into the
    /// thrown value. New errors get a `stack`.
    pub(crate) fn fault_value(&mut self, f: Fault, pc: Option<u32>) -> Value {
        let (kind, msg) = match *f.0 {
            FaultKind::Throw(v) => return v,
            FaultKind::Error(k, m) => (k, m),
            FaultKind::Heap(HeapError::OutOfMemory) => return self.out_of_memory(),
            FaultKind::Heap(HeapError::HostOutOfMemory) => {
                // Not catchable: the run stops before a handler can run (D1690).
                self.host_out_of_memory();
                return intrinsic(self.heap, Intrinsic::OutOfMemoryError);
            }
            FaultKind::Heap(HeapError::StringTooLong) => {
                (ErrorKind::RangeError, String::from("invalid string length"))
            }
            FaultKind::Heap(HeapError::InvalidArrayLength) => {
                (ErrorKind::RangeError, String::from("invalid array length"))
            }
        };
        let e = match realm::new_error(self.heap, kind, &msg) {
            Ok(e) => e,
            Err(_) => return self.out_of_memory(),
        };
        if let Some(pc) = pc {
            let _ = self.set_stack(e, pc);
        }
        e
    }

    /// The heap reached its cap: the `InternalError` to throw, whose handlers get the
    /// reserve below the cap to run in (D1600), or with `fatal_out_of_memory` the end of
    /// the run, as when the host refuses memory (D3181).
    #[inline(always)]
    fn out_of_memory(&mut self) -> Value {
        #[cfg(feature = "embed")]
        if self.opts.fatal_out_of_memory {
            self.host_out_of_memory();
            return intrinsic(self.heap, Intrinsic::OutOfMemoryError);
        }
        self.heap.open_memory_reserve();
        intrinsic(self.heap, Intrinsic::OutOfMemoryError)
    }

    /// Collects if the heap asks for it (or always under `gc_stress`).
    #[inline(always)]
    fn safepoint(&mut self) {
        if self.opts.gc_stress || self.heap.gc_pending() {
            self.safepoint_collect();
        }
    }

    #[cold]
    #[inline(never)]
    fn safepoint_collect(&mut self) {
        self.flush();
        // Popped values above `sp` must not keep garbage alive.
        let cap = self.heap.value_array_capacity(self.stack);
        for i in self.sp..cap {
            self.set(i, Value::UNDEFINED);
        }
        let collected = self.heap.collect_auto();
        if let Err(HeapError::HostOutOfMemory) = collected {
            // Never a silently skipped collection (D1690).
            self.host_out_of_memory();
        }
        if collected.is_ok() {
            self.stack = state::stack(self.heap);
            if self.has_frame() {
                self.load_frame();
            }
            if self.opts.finalization_after_gc {
                // Out of memory here only delays the callbacks to a later collection.
                let _ = late::schedule_finalization_cleanup(self.heap);
            }
        }
    }

    fn has_frame(&self) -> bool {
        self.sp > 0
            && self
                .get(self.bp)
                .as_object()
                .is_some_and(|o| self.heap.object_class(o) == ObjectClass::Function)
    }

    // ---------------------------------------------------------------- the loop

    /// Runs with `fuel`.
    pub fn run(&mut self, fuel: u64) -> Status {
        let st = self.run_loop(fuel);
        // D1740: a run that stops mid-call (out of fuel, or parked on a host call) carries
        // its overshoot to the next one (having paid the debt it started with). Once the
        // VM is quiescent (done, threw, idle) nothing is owed: the next run starts a fresh
        // slice anyway, and the state at those points does not depend on the slicing.
        match st {
            // Out of fuel with fuel left can only be an intrinsic suspended mid-work
            // (`suspend_in`): it gave back its dispatch charge, which the resume charges
            // again, so the remainder is a credit for the next run (D1712).
            Status::OutOfFuel => {
                if self.fuel_start.is_some() {
                    state::set_fuel_debt(self.heap, -self.fuel);
                }
            }
            // Parked on a host call: fuel left over is the host's to forgo.
            Status::HostCall { .. } => {
                if self.fuel_start.is_some() {
                    state::set_fuel_debt(self.heap, (-self.fuel).max(0));
                }
            }
            _ => state::set_fuel_debt(self.heap, 0),
        }
        if self.host_oom {
            self.flush();
            return Status::HostOutOfMemory;
        }
        st
    }

    fn run_loop(&mut self, fuel: u64) -> Status {
        self.fuel = fuel.min(i64::MAX as u64) as i64;
        match state::state(self.heap) {
            RunState::Running | RunState::ThrowPending => self.load_frame(),
            RunState::Idle | RunState::Done | RunState::Threw => {
                if let Some(st) = self.next_job() {
                    return st;
                }
            }
            RunState::HostCall => {
                let i = state::interp(self.heap);
                let call_id = self.heap.arena().get(i, Interp::HOST_CALL);
                let fn_id = host_call_cell(self.heap, call_id)
                    .map_or(0, |c| self.heap.arena().get(c, HostCall::FN));
                return Status::HostCall { call_id, fn_id };
            }
        }
        if state::state(self.heap) == RunState::ThrowPending {
            state::set_state(self.heap, RunState::Running);
            let exc = state::pending_throw(self.heap);
            state::set_pending_throw(self.heap, Value::UNDEFINED);
            if let Some(st) = self.throw_value(exc, self.call_site_key(self.pc)) {
                return st;
            }
        }
        // No safepoint here (D1710): a collection that became pending waits for the next
        // natural safepoint, so where it runs does not depend on where a slice ended.
        // Pay the overshoot of the previous run first (D1740): with a debt larger than
        // the fuel, the run stops at once and the rest of the debt carries on.
        self.fuel = self.fuel.saturating_sub(state::fuel_debt(self.heap));
        self.fuel_start = Some(self.fuel);
        loop {
            if self.fuel <= 0 {
                self.flush();
                if self.host_oom {
                    return Status::HostOutOfMemory;
                }
                return Status::OutOfFuel;
            }
            // The tier-0 loop (D1930) runs everything, `step` included, until fuel runs
            // out, the run stops, or it returns for V8's tier-up. Off is for tests.
            if self.opts.hot_loop {
                if let Some(st) = self.run_hot() {
                    return st;
                }
                continue;
            }
            // Tests only: the fast paths before the tier-0 loop, the reference it must
            // equal (`hot_loop_is_invisible`).
            #[cfg(test)]
            if self.fast_step()
                || self.fast_more()
                // A call of a `@fast` builtin (D1850), outside `fast_step`: its register
                // allocation is sensitive to every addition (D1352).
                || (self.pc < self.code_len
                    && fast::is_call(self.byte(self.pc))
                    && self.fast_leaf_call(self.byte(self.pc)))
            {
                continue;
            }
            if let Some(st) = self.step_once() {
                return st;
            }
        }
    }

    /// One instruction through `step`, with the out-of-memory retry (D1700) and the
    /// throw of a fault: `Some` when the run stops.
    #[inline(never)]
    pub(super) fn step_once(&mut self) -> Option<Status> {
        let pc0 = self.pc;
        let sp0 = self.sp;
        let retry = self.retry_operands();
        match self.step() {
            Ok(None) => None,
            Ok(Some(st)) => Some(st),
            Err(f) => {
                if retry && f.is_out_of_memory() && self.retry_after_collect(pc0, sp0) {
                    return None;
                }
                self.throw_fault(f, pc0)
            }
        }
    }

    /// Before `step`: if the instruction at `pc` may be retried once after a collection
    /// when an allocation fails (D1700), saves its operands (an instruction may overwrite
    /// them before it allocates) and returns true. Only instructions that change nothing
    /// observable before their allocations qualify: they allocate first and commit
    /// after, so running them again from the start is the same as running them once.
    #[inline(always)]
    fn retry_operands(&mut self) -> bool {
        if self.retry_len == 0 || self.pc >= self.code_len {
            // `retry_len` 0: this instruction was just retried; a second failure is real.
            self.retry_len = NO_RETRY;
            return false;
        }
        let n = match self.byte(self.pc) {
            fast::PUT_ELEM => 3,
            fast::PUT_FIELD => 2,
            retry::DEFINE_FIELD | retry::ADD => 2,
            retry::OBJECT => 0,
            // Compiles before it changes anything (D2990).
            #[cfg(feature = "lazy")]
            retry::LAZY => 0,
            fast::CALL_INTRINSIC => {
                let (id, argc) = (self.imm_u16(1) as u16, self.imm_u16(3));
                if !retry::retries(id) || argc > 4 {
                    return false;
                }
                argc
            }
            fast::CALL_INTRINSIC8 => {
                let id = self.byte(self.pc + 1) as u16;
                let argc = arity(id as u32);
                if !retry::retries(id) || argc > 4 {
                    return false;
                }
                argc
            }
            _ => return false,
        };
        for i in 0..n {
            self.retry_ops[i as usize] = self.get(self.sp - n + i);
        }
        self.retry_len = n;
        true
    }

    /// An allocation of the instruction at `pc0` failed: puts its operands back, collects
    /// and lets the loop run it again (D1700). False if the collection could not run.
    #[cold]
    #[inline(never)]
    fn retry_after_collect(&mut self, pc0: u32, sp0: u32) -> bool {
        let n = self.retry_len;
        if n == NO_RETRY || self.host_oom {
            return false;
        }
        self.pc = pc0;
        self.sp = sp0;
        for i in 0..n {
            self.set(sp0 - n + i, self.retry_ops[i as usize]);
        }
        // The values are rooted in the stack now; the copies would go stale.
        self.retry_ops = [Value::UNDEFINED; 4];
        self.safepoint_collect();
        if self.host_oom {
            return false;
        }
        // Marks the retry: `retry_operands` gives up on a second failure.
        self.retry_len = 0;
        true
    }

    #[inline(always)]
    pub(super) fn byte(&self, at: u32) -> u8 {
        // `Arena::read` checks against the backing Vec only (one compare); `pc` is
        // checked against the code length by `step`.
        self.heap.arena().read::<u8>(self.code + at)
    }

    #[inline(always)]
    fn imm_u16(&self, off: u32) -> u32 {
        self.heap.arena().read::<u16>(self.code + self.pc + off) as u32
    }

    #[inline(always)]
    fn imm_u32(&self, off: u32) -> u32 {
        self.heap.arena().read::<u32>(self.code + self.pc + off)
    }

    #[inline(always)]
    fn imm_i32(&self, off: u32) -> i32 {
        self.heap.arena().read::<i32>(self.code + self.pc + off)
    }

    #[inline(always)]
    fn imm_atom(&self) -> Atom {
        Atom::from_id(self.imm_u32(1))
    }

    /// Immediate 0 of the instruction at `pc`, an `op`, as `decode` reads it, or the
    /// operand a short form implies (`get_loc2`: 2, D2443).
    #[inline(always)]
    pub(super) fn operand(&self, op: Op) -> u32 {
        match LAYOUT[op as usize] & 0xF {
            0 => op.implied().unwrap_or(0),
            1 => self.byte(self.pc + 1) as u32,
            2 => self.byte(self.pc + 1) as i8 as i32 as u32,
            3 => self.imm_u16(1),
            _ => self.imm_u32(1),
        }
    }

    /// Executes one of the hottest simple instructions without building a [`Step`]
    /// (D1350): the general path merges every op's result through a `Step` in memory,
    /// which costs more than these ops themselves. Returns false, with nothing changed,
    /// when the next instruction is not one of them or needs the general path (a slow
    /// operand, an overflow); `step` then executes it with the full semantics.
    #[cfg(test)]
    #[inline(always)]
    fn fast_step(&mut self) -> bool {
        let pc = self.pc;
        if pc >= self.code_len {
            return false;
        }
        let sp0 = self.sp;
        // Matching the raw byte (not `Op::from_u8`) and constant sizes and fuel per arm
        // keep the dispatch to one jump table.
        match self.byte(pc) {
            fast::GET_LOC => {
                let v = self.get(self.lp + self.imm_u16(1));
                self.push(v);
                self.advance(Op::GetLoc, sp0);
            }
            fast::PUT_LOC => {
                let v = self.pop();
                self.set(self.lp + self.imm_u16(1), v);
                self.advance(Op::PutLoc, sp0);
            }
            fast::SET_LOC => {
                let v = self.top(0);
                self.set(self.lp + self.imm_u16(1), v);
                self.advance(Op::SetLoc, sp0);
            }
            fast::GET_ARG => {
                let v = self.get(self.bp + 2 + self.imm_u16(1));
                self.push(v);
                self.advance(Op::GetArg, sp0);
            }
            fast::PUSH_I8 => {
                let v = self.heap.arena().read::<u8>(self.code + pc + 1) as i8;
                self.push(Value::int(v as i32));
                self.advance(Op::PushI8, sp0);
            }
            fast::PUSH_I32 => {
                let v = self.imm_i32(1);
                self.push(Value::int(v));
                self.advance(Op::PushI32, sp0);
            }
            fast::UNDEFINED => {
                self.push(Value::UNDEFINED);
                self.advance(Op::Undefined, sp0);
            }
            fast::DROP => {
                self.sp -= 1;
                self.advance(Op::Drop, sp0);
            }
            fast::DUP => {
                let v = self.top(0);
                self.push(v);
                self.advance(Op::Dup, sp0);
            }
            // for-of's step (D1800): `dup2; iter_next`.
            fast::DUP2 => {
                let (a, b) = (self.top(1), self.top(0));
                self.push(a);
                self.push(b);
                self.advance(Op::Dup2, sp0);
            }
            fast::ITER_NEXT => return self.fast_iter_next(),
            // Number operands only: these never need `primitive_operands` (D1280).
            fast::TO_NUMERIC if self.top(0).is_number() => self.advance(Op::ToNumeric, sp0),
            fast::INC => return self.fast_inc(Op::Inc, 1, sp0),
            fast::INC_LOC => return self.fast_inc_loc(Op::IncLoc, 1, sp0),
            fast::DEC_LOC => return self.fast_inc_loc(Op::DecLoc, -1, sp0),
            fast::DEC => return self.fast_inc(Op::Dec, -1, sp0),
            // Two int32s inline; two Numbers (or an int32 overflow) in `fast_float`.
            fast::ADD => {
                return self.fast_int(Op::Add, sp0, |x, y| x.checked_add(y).map(Value::int))
                    || self.fast_float(Op::Add, sp0);
            }
            fast::SUB => {
                return self.fast_int(Op::Sub, sp0, |x, y| x.checked_sub(y).map(Value::int))
                    || self.fast_float(Op::Sub, sp0);
            }
            // An int32 product of 0 may be -0 (`-1 * 0`): the f64 path gets it right.
            fast::MUL => {
                return self.fast_int(Op::Mul, sp0, |x, y| {
                    x.checked_mul(y).filter(|&r| r != 0).map(Value::int)
                }) || self.fast_float(Op::Mul, sp0);
            }
            fast::DIV => return self.fast_float(Op::Div, sp0),
            fast::LT => {
                return self.fast_int(Op::Lt, sp0, |x, y| Some(Value::bool(x < y)))
                    || self.fast_float(Op::Lt, sp0);
            }
            fast::LE => {
                return self.fast_int(Op::Le, sp0, |x, y| Some(Value::bool(x <= y)))
                    || self.fast_float(Op::Le, sp0);
            }
            fast::GT => {
                return self.fast_int(Op::Gt, sp0, |x, y| Some(Value::bool(x > y)))
                    || self.fast_float(Op::Gt, sp0);
            }
            fast::GE => {
                return self.fast_int(Op::Ge, sp0, |x, y| Some(Value::bool(x >= y)))
                    || self.fast_float(Op::Ge, sp0);
            }
            fast::JUMP => self.fast_jump(Op::Jump, true),
            fast::JUMP_IF_FALSE | fast::JUMP_IF_TRUE => {
                let Some(b) = self.top(0).as_bool() else {
                    return false;
                };
                self.sp -= 1;
                if self.byte(pc) == fast::JUMP_IF_TRUE {
                    self.fast_jump(Op::JumpIfTrue, b);
                } else {
                    self.fast_jump(Op::JumpIfFalse, !b);
                }
            }
            // The common cases of `call` and `do_return`.
            fast::CALL => return self.fast_call(),
            fast::RETURN => return self.fast_return(Op::Return),
            fast::RETURN_UNDEF => return self.fast_return(Op::ReturnUndef),
            fast::CALL_INTRINSIC => return self.fast_intrinsic(),
            fast::GET_ELEM | fast::GET_ELEM2 | fast::PUT_ELEM => return self.fast_elem(sp0),
            fast::GET_FIELD | fast::GET_METHOD | fast::PUT_FIELD => return self.fast_field(sp0),
            _ => return false,
        }
        true
    }

    #[cfg(test)]
    /// Charges `op` (a constant at each call site, so this folds) and falls through.
    #[inline(always)]
    fn advance(&mut self, op: Op, sp0: u32) {
        let info = &OPS[op as usize];
        debug_assert_eq!(
            self.sp + info.pop as u32,
            sp0 + info.push as u32,
            "fast {} stack effect disagrees with the table",
            op.name()
        );
        let _ = sp0;
        self.fuel -= info.cost.fuel() as i64;
        self.pc += info.size as u32;
    }

    #[cfg(test)]
    #[inline(always)]
    fn fast_jump(&mut self, op: Op, taken: bool) {
        let info = &OPS[op as usize];
        self.fuel -= info.cost.fuel() as i64;
        let next = self.pc + info.size as u32;
        if taken {
            self.pc = self.jump_target(next);
            if self.pc < next {
                self.safepoint();
            }
        } else {
            self.pc = next;
        }
    }

    #[cfg(test)]
    /// The arithmetic and relational ops on two Numbers, as `binary_num` and `compare`
    /// compute them (NaN compares false). Out of line: the int32 case stays the short one.
    #[inline(never)]
    fn fast_float(&mut self, op: Op, sp0: u32) -> bool {
        let (Some(x), Some(y)) = (self.top(1).as_number(), self.top(0).as_number()) else {
            return false;
        };
        let v = match op {
            Op::Add => conv::number(x + y),
            Op::Sub => conv::number(x - y),
            Op::Mul => conv::number(x * y),
            Op::Div => conv::number(x / y),
            Op::Lt => Value::bool(x < y),
            Op::Le => Value::bool(x <= y),
            Op::Gt => Value::bool(x > y),
            Op::Ge => Value::bool(x >= y),
            _ => return false,
        };
        self.sp -= 1;
        self.set(self.sp - 1, v);
        self.advance(op, sp0);
        true
    }

    #[cfg(test)]
    /// A binary op on two int32 operands; `f` returns `None` when the result needs the
    /// general path (an overflow).
    #[inline(always)]
    fn fast_int(&mut self, op: Op, sp0: u32, f: impl Fn(i32, i32) -> Option<Value>) -> bool {
        let (Some(x), Some(y)) = (self.top(1).as_int(), self.top(0).as_int()) else {
            return false;
        };
        let Some(v) = f(x, y) else { return false };
        self.sp -= 1;
        self.set(self.sp - 1, v);
        self.advance(op, sp0);
        true
    }

    #[cfg(test)]
    #[inline(always)]
    fn fast_inc_loc(&mut self, op: Op, d: i32, sp0: u32) -> bool {
        let slot = self.lp + self.imm_u16(1);
        match self.get(slot).as_int().and_then(|i| i.checked_add(d)) {
            Some(i) => {
                self.set(slot, Value::int(i));
                self.advance(op, sp0);
                true
            }
            None => false,
        }
    }

    #[cfg(test)]
    #[inline(always)]
    fn fast_inc(&mut self, op: Op, d: i32, sp0: u32) -> bool {
        match self.top(0).as_int().and_then(|i| i.checked_add(d)) {
            Some(i) => {
                self.set(self.sp - 1, Value::int(i));
                self.advance(op, sp0);
                true
            }
            None => false,
        }
    }

    /// More simple ops (0086), tried after `fast_step` and out of line: as arms of
    /// `fast_step` they cost the hottest ops more than they saved (D1352's register
    /// allocation). Same contract: false, with nothing changed, when the general path
    /// is needed.
    #[cfg(test)]
    #[inline(never)]
    fn fast_more(&mut self) -> bool {
        if self.pc >= self.code_len {
            return false;
        }
        let sp0 = self.sp;
        match self.byte(self.pc) {
            fast::DEFINE_FIELD => return self.fast_define(sp0),
            fast::NULL => {
                self.push(Value::NULL);
                self.advance(Op::Null, sp0);
            }
            fast::PUSH_TRUE => {
                self.push(Value::TRUE);
                self.advance(Op::PushTrue, sp0);
            }
            fast::PUSH_FALSE => {
                self.push(Value::FALSE);
                self.advance(Op::PushFalse, sp0);
            }
            fast::PUSH_CONST => {
                let v = self.constant(self.imm_u32(1));
                self.push(v);
                self.advance(Op::PushConst, sp0);
            }
            fast::SPECIAL_OBJECT => {
                let kind = self.heap.arena().read::<u8>(self.code + self.pc + 1);
                if kind != special::THIS {
                    return false;
                }
                let v = self.get(self.bp + 1);
                self.push(v);
                self.advance(Op::SpecialObject, sp0);
            }
            fast::SWAP => {
                let (a, b) = (self.top(1), self.top(0));
                self.set(self.sp - 2, b);
                self.set(self.sp - 1, a);
                self.advance(Op::Swap, sp0);
            }
            fast::NIP => {
                let b = self.pop();
                self.set(self.sp - 1, b);
                self.advance(Op::Nip, sp0);
            }
            fast::INSERT2 => {
                let (a, b) = (self.top(1), self.top(0));
                self.set(self.sp - 2, b);
                self.set(self.sp - 1, a);
                self.push(b);
                self.advance(Op::Insert2, sp0);
            }
            fast::GET_CAP | fast::GET_CAP_CHK => {
                let v = self.box_value(self.capture(self.imm_u16(1)));
                if v.is_uninitialized() {
                    // `get_cap_chk` throws; `get_cap` never sees the marker.
                    return false;
                }
                self.push(v);
                // (`get_cap_chk` has the same size and cost.)
                self.advance(Op::GetCap, sp0);
            }
            fast::GET_BUILTIN => {
                let Some(v) = self.prelude_function(self.imm_u16(1)) else {
                    return false;
                };
                self.push(v);
                self.advance(Op::GetBuiltin, sp0);
            }
            fast::PUT_CAP => {
                let b = self.capture(self.imm_u16(1));
                let v = self.pop();
                self.set_box_value(b, v);
                self.advance(Op::PutCap, sp0);
            }
            fast::GET_BOX => {
                let v = self.box_value(self.get(self.lp + self.imm_u16(1)));
                self.push(v);
                self.advance(Op::GetBox, sp0);
            }
            fast::PUT_BOX => {
                let b = self.get(self.lp + self.imm_u16(1));
                let v = self.pop();
                self.set_box_value(b, v);
                self.advance(Op::PutBox, sp0);
            }
            fast::NOT => {
                let a = self.top(0);
                let b = conv::to_boolean(self.heap, a);
                self.set(self.sp - 1, Value::bool(!b));
                self.advance(Op::Not, sp0);
            }
            fast::IS_UNDEFINED => {
                let v = Value::bool(self.top(0).is_undefined());
                self.set(self.sp - 1, v);
                self.advance(Op::IsUndefined, sp0);
            }
            fast::IS_NULLISH => {
                let v = Value::bool(self.top(0).is_nullish());
                self.set(self.sp - 1, v);
                self.advance(Op::IsNullish, sp0);
            }
            fast::STRICT_EQ | fast::STRICT_NE => {
                let eq = self.byte(self.pc) == fast::STRICT_EQ;
                let r = conv::strict_equals(self.heap, self.top(1), self.top(0));
                self.sp -= 1;
                self.set(self.sp - 1, Value::bool(r == eq));
                // (`strict_ne` has the same size and cost.)
                self.advance(Op::StrictEq, sp0);
            }
            fast::MOD => {
                return self.fast_int(Op::Mod, sp0, |x, y| {
                    (x >= 0 && y != 0).then(|| Value::int(x % y))
                });
            }
            fast::BAND => return self.fast_int(Op::BAnd, sp0, |x, y| Some(Value::int(x & y))),
            fast::BOR => return self.fast_int(Op::BOr, sp0, |x, y| Some(Value::int(x | y))),
            fast::BXOR => return self.fast_int(Op::BXor, sp0, |x, y| Some(Value::int(x ^ y))),
            fast::SHL => {
                return self.fast_int(Op::Shl, sp0, |x, y| {
                    Some(Value::int(x.wrapping_shl(y as u32 & 31)))
                });
            }
            fast::SAR => {
                return self.fast_int(Op::Sar, sp0, |x, y| Some(Value::int(x >> (y as u32 & 31))));
            }
            _ => return false,
        }
        true
    }

    #[cfg(test)]
    /// `get_elem` / `put_elem` on an existing dense element with an int key: no key
    /// conversion (D1280), no getter or setter, no growth.
    #[inline(never)]
    fn fast_elem(&mut self, sp0: u32) -> bool {
        let put = self.byte(self.pc) == fast::PUT_ELEM;
        let keep = self.byte(self.pc) == fast::GET_ELEM2;
        let k = if put { 1 } else { 0 };
        let (o, key) = (self.top(k + 1), self.top(k));
        let Some((obj, i)) = o
            .as_object()
            .zip(key.as_int().and_then(|i| u32::try_from(i).ok()))
        else {
            return false;
        };
        if put {
            // `set_element_fast` also appends (and fills holes) where that is unobservable:
            // `a[a.length] = x` in a loop took the generic [[Set]] path, ~1000 native
            // instructions per element (D1783).
            if !matches!(self.heap.set_element_fast(obj, i, self.top(0)), Ok(true)) {
                return false;
            }
            self.sp -= 3;
            self.advance(Op::PutElem, sp0);
            return true;
        }
        let Some(old) = self.heap.get_element_fast(obj, i) else {
            return false;
        };
        if keep {
            self.push(old);
            self.advance(Op::GetElem2, sp0);
        } else {
            self.sp -= 1;
            self.set(self.sp - 1, old);
            self.advance(Op::GetElem, sp0);
        }
        true
    }

    #[cfg(test)]
    /// `fast_step` for `call`: a JS closure with the stack already big enough.
    #[inline(always)]
    fn fast_call(&mut self) -> bool {
        let argc = self.imm_u16(1);
        self.fast_call_at(Op::Call, self.sp - 2 - argc, argc)
    }

    #[cfg(test)]
    /// Calls the JS closure at `stack[base]` with `argc` arguments for instruction `op`
    /// (charged here) if the stack has room; false, with nothing changed, otherwise.
    #[inline(always)]
    fn fast_call_at(&mut self, op: Op, base: u32, argc: u32) -> bool {
        let Some(o) = self.get(base).as_object() else {
            return false;
        };
        if self.heap.object_class(o) != ObjectClass::Function {
            return false;
        }
        let t = self.template_of(o);
        if self.frame_top(t, base, argc) > self.heap.value_array_capacity(self.stack)
            || u32::from(self.heap.arena().get::<u16>(t, FuncTemplate::FLAGS))
                & self.slow_call_flags
                != 0
        {
            return false;
        }
        let info = &OPS[op as usize];
        self.fuel -= info.cost.fuel() as i64;
        let next = self.pc + info.size as u32;
        self.push_frame(t, base, argc, Continuation::PushResult, next);
        true
    }

    #[cfg(test)]
    /// `fast_step` for `return` / `return_undef` into a frame that pushes the result.
    #[inline(always)]
    fn fast_return(&mut self, op: Op) -> bool {
        if self.header_int(frame::FLAGS) as i32 != Continuation::PushResult.to_flags() {
            return false;
        }
        let v = if op == Op::Return {
            self.pop()
        } else {
            Value::UNDEFINED
        };
        self.fuel -= OPS[op as usize].cost.fuel() as i64;
        self.pop_frame();
        self.push(v);
        self.safepoint();
        true
    }

    /// Executes one instruction.
    #[inline(always)]
    fn step(&mut self) -> Step {
        if self.pc >= self.code_len {
            if self.native != 0 {
                return self.native_step();
            }
            return Err(Fault::error(
                ErrorKind::InternalError,
                String::from("bad bytecode: pc out of range"),
            ));
        }
        let b = self.byte(self.pc);
        let Some(op) = Op::from_u8(b) else {
            return Err(Fault::error(
                ErrorKind::InternalError,
                crate::text::cat!("bad bytecode: opcode ", b),
            ));
        };
        let info = &OPS[op as usize];
        self.fuel -= info.cost.fuel() as i64;
        let next = self.pc + info.size as u32;
        #[cfg(debug_assertions)]
        let (pc0, sp0, bp0, t0) = (self.pc, self.sp, self.bp, self.template);
        let r = self.exec(op, next);
        // The table and the interpreter must agree (docs/compiler.md 4.2): when an
        // instruction completes in the same frame, it moved `sp` by its stack effect, and
        // fell through unless it jumps.
        #[cfg(debug_assertions)]
        // (A bottom frame's return can enter a job's frame at the same `bp`, and the
        // suspending ops replace the frame. A collection on the way can move the new
        // frame's template to the old one's offset, so ops that end or replace the frame
        // (TERM, the suspending ops) are not checked.)
        if matches!(r, Ok(None))
            && self.bp == bp0
            && self.template == t0
            && !info.has(crate::bytecode::flags::TERM)
            && !matches!(
                op,
                Op::Await | Op::Yield | Op::YieldStar | Op::GeneratorStart
            )
        {
            use crate::bytecode::flags as opflags;
            // The value of immediate k (a u16 count or a u8).
            let imm = |k: u8| {
                let skip: u32 = info.imms[..k as usize]
                    .iter()
                    .map(|i| i.width() as u32)
                    .sum();
                let at = self.code + pc0 + 1 + skip;
                if info.imms[k as usize].width() == 1 {
                    self.heap.arena().bytes()[at as usize] as u32
                } else {
                    self.heap.arena().read::<u16>(at) as u32
                }
            };
            let pops = info.pop as u32
                + info.pop_imm.map_or(0, imm)
                + info.pop_arity.map_or(0, |k| arity(imm(k)));
            let pushes = info.push as u32 + info.push_imm.map_or(0, imm);
            if info.flags & (opflags::JUMP | opflags::BRANCH | opflags::TERM) == 0 {
                debug_assert_eq!(
                    self.pc,
                    next,
                    "{} did not advance pc by its size",
                    op.name()
                );
            }
            debug_assert_eq!(
                self.sp + pops,
                sp0 + pushes,
                "{} stack effect disagrees with the table",
                op.name()
            );
        }
        r
    }

    fn jump_target(&self, next: u32) -> u32 {
        (next as i64 + self.imm_i32(1) as i64) as u32
    }

    /// The local or capture name `i` from the template's `NAMES` (for TDZ messages).
    fn name_of(&self, i: u32) -> String {
        let names = crate::bytecode::template::debug_info(self.heap, self.template).names;
        match names.get() {
            Some(n) if i < self.heap.arena().get(n, U32Array::LEN) => {
                let id = self.heap.arena().read::<u32>(U32Array::DATA.at(n, i));
                self.heap.atom_name(Atom::from_id(id))
            }
            _ => String::from("variable"),
        }
    }

    fn uninitialized<T>(&self, name_index: u32) -> Result<T, Fault> {
        Err(Fault::error(
            ErrorKind::ReferenceError,
            crate::text::cat!(self.name_of(name_index), " is not initialized"),
        ))
    }

    fn box_value(&self, b: Value) -> Value {
        let cell = b.as_internal().expect("a Box");
        self.heap.arena().get(cell, BoxCell::VALUE)
    }

    fn set_box_value(&mut self, b: Value, v: Value) {
        let cell = b.as_internal().expect("a Box");
        self.heap.arena_mut().set(cell, BoxCell::VALUE, v)
    }

    fn new_box(&mut self, v: Value) -> Result<Value, Fault> {
        let c = self
            .heap
            .alloc_cell(CellKind::BoxCell, BoxCell::LAYOUT.min_size)?;
        self.heap.arena_mut().set(c, BoxCell::VALUE, v);
        Ok(Value::internal(c))
    }

    /// The Box of capture `i` of the current closure.
    /// Entry `i` of the prelude function table (`get_builtin`, D2440).
    #[inline(always)]
    fn prelude_function(&self, i: u32) -> Option<Value> {
        let t = intrinsic(self.heap, Intrinsic::PreludeFunctions).as_internal()?;
        (i < self.heap.value_array_len(t)).then(|| self.heap.value_array_get(t, i))
    }

    fn capture(&self, i: u32) -> Value {
        let f = self.get(self.bp).as_object().unwrap();
        self.heap.object_slot(f, i)
    }

    fn constant(&self, i: u32) -> Value {
        assert!(self.consts != 0, "no constant pool");
        self.heap.value_array_get(self.consts, i)
    }

    /// A numeric binary operator: `int` on two int32 operands (`None` on overflow or
    /// when the result is not an int32), `f` on f64 otherwise.
    #[inline(always)]
    fn binary_num(
        &mut self,
        int: impl Fn(i32, i32) -> Option<i32>,
        f: impl Fn(f64, f64) -> f64,
    ) -> Result<(), Fault> {
        let (a, b) = (self.top(1), self.top(0));
        if let (Some(x), Some(y)) = (a.as_int(), b.as_int())
            && let Some(r) = int(x, y)
        {
            self.sp -= 1;
            self.set(self.sp - 1, Value::int(r));
            return Ok(());
        }
        let v = match (a.as_number(), b.as_number()) {
            (Some(x), Some(y)) => conv::number(f(x, y)),
            _ => {
                let x = conv::to_number(self.heap, a)?;
                let y = conv::to_number(self.heap, b)?;
                conv::number(f(x, y))
            }
        };
        self.sp -= 1;
        self.set(self.sp - 1, v);
        Ok(())
    }

    #[inline(always)]
    fn binary_int(&mut self, f: impl Fn(i32, i32) -> Value) -> Result<(), Fault> {
        let (a, b) = (self.top(1), self.top(0));
        let (x, y) = match (a.as_int(), b.as_int()) {
            (Some(x), Some(y)) => (x, y),
            _ => (
                conv::to_int32(conv::to_number(self.heap, a)?),
                conv::to_int32(conv::to_number(self.heap, b)?),
            ),
        };
        self.sp -= 1;
        self.set(self.sp - 1, f(x, y));
        Ok(())
    }

    #[inline(always)]
    fn compare(&mut self, swap: bool, invert: bool) -> Result<(), Fault> {
        // lt: a < b; gt: b < a; le: !(b < a); ge: !(a < b). Undefined (NaN) is false.
        let (a, b) = (self.top(1), self.top(0));
        let (x, y) = if swap { (b, a) } else { (a, b) };
        if let (Some(i), Some(j)) = (x.as_int(), y.as_int()) {
            self.sp -= 1;
            self.set(self.sp - 1, Value::bool((i < j) != invert));
            return Ok(());
        }
        self.sp -= 2;
        let r = conv::less_than(self.heap, x, y)?;
        let v = match r {
            None => false,
            Some(t) => t != invert,
        };
        self.push(Value::bool(v));
        Ok(())
    }

    #[inline(always)]
    fn exec(&mut self, op: Op, next: u32) -> Step {
        if let Some(step) = self.primitive_operands(op) {
            return step;
        }
        if let Some(r) = self.bigint_operands(op) {
            r?;
            self.pc = next;
            return Ok(None);
        }
        match op {
            // ---- constants ----
            Op::Undefined => self.push(Value::UNDEFINED),
            Op::Null => self.push(Value::NULL),
            Op::PushTrue => self.push(Value::TRUE),
            Op::PushFalse => self.push(Value::FALSE),
            Op::PushI8 => {
                let v = read_i8(
                    self.heap.arena().bytes(),
                    (self.code + self.pc + 1) as usize,
                );
                self.push(Value::int(v as i32));
            }
            Op::PushI32 => {
                let v = self.imm_i32(1);
                self.push(Value::int(v));
            }
            Op::PushConst => {
                let v = self.constant(self.imm_u32(1));
                self.push(v);
            }
            Op::PushAtom => {
                let s = self.heap.atom_to_value(self.imm_atom())?;
                self.push(s);
            }
            Op::Object => {
                let proto = intrinsic_obj(self.heap, Intrinsic::ObjectProto);
                let o = self.heap.new_object(Some(proto))?;
                self.push(Value::object(o));
            }
            Op::ArrayFrom => {
                let n = self.imm_u16(1);
                let proto = intrinsic_obj(self.heap, Intrinsic::ArrayProto);
                let mut vals = alloc::vec::Vec::with_capacity(n as usize);
                for i in 0..n {
                    vals.push(self.get(self.sp - n + i));
                }
                let a = self.heap.new_array_from_values(Some(proto), &vals)?;
                self.sp -= n;
                self.push(Value::object(a));
            }
            Op::ArrayPush => {
                let v = self.pop();
                let a = self.top(0).as_object().expect("array");
                let len = self.heap.array_length(a);
                if !self.heap.set_element_fast(a, len, v)? {
                    let k = self.heap.atom_from_index(len)?;
                    self.heap.create_data_property(a, k, v)?;
                }
            }
            Op::ArrayHole => {
                let a = self.top(0).as_object().expect("array");
                let len = self.heap.array_length(a);
                let d = PropertyDescriptor {
                    value: Some(conv::number(len as f64 + 1.0)),
                    ..Default::default()
                };
                self.heap
                    .define_own_property(a, crate::heap::atoms::LENGTH, &d)?;
            }
            Op::FClosure => {
                let v = self.closure(self.imm_u32(1))?;
                self.push(v);
            }

            // ---- shuffles ----
            Op::Drop => {
                self.sp -= 1;
            }
            Op::Dup => {
                let v = self.top(0);
                self.push(v);
            }
            Op::Dup2 => {
                let (a, b) = (self.top(1), self.top(0));
                self.push(a);
                self.push(b);
            }
            Op::Swap => {
                let (a, b) = (self.top(1), self.top(0));
                self.set(self.sp - 2, b);
                self.set(self.sp - 1, a);
            }
            Op::Nip => {
                let b = self.pop();
                self.set(self.sp - 1, b);
            }
            Op::Insert2 => {
                // a b -> b a b
                let (a, b) = (self.top(1), self.top(0));
                self.set(self.sp - 2, b);
                self.set(self.sp - 1, a);
                self.push(b);
            }
            Op::Insert3 => {
                // a b c -> c a b c
                let (a, b, c) = (self.top(2), self.top(1), self.top(0));
                self.set(self.sp - 3, c);
                self.set(self.sp - 2, a);
                self.set(self.sp - 1, b);
                self.push(c);
            }
            Op::Insert4 => {
                // a b c d -> d a b c d
                let (a, b, c, d) = (self.top(3), self.top(2), self.top(1), self.top(0));
                self.set(self.sp - 4, d);
                self.set(self.sp - 3, a);
                self.set(self.sp - 2, b);
                self.set(self.sp - 1, c);
                self.push(d);
            }
            Op::Rot3L => {
                // x a b -> a b x
                let (x, a, b) = (self.top(2), self.top(1), self.top(0));
                self.set(self.sp - 3, a);
                self.set(self.sp - 2, b);
                self.set(self.sp - 1, x);
            }
            Op::Rot3R => {
                // a b x -> x a b
                let (a, b, x) = (self.top(2), self.top(1), self.top(0));
                self.set(self.sp - 3, x);
                self.set(self.sp - 2, a);
                self.set(self.sp - 1, b);
            }
            Op::Pick => {
                // x a1 .. an -> x a1 .. an x (the stack checker keeps n in range)
                let n = u32::from(self.heap.arena().bytes()[(self.code + self.pc + 1) as usize]);
                let v = self.top(n);
                self.push(v);
            }
            Op::Poke => {
                // x a1 .. an v -> v a1 .. an
                let n = u32::from(self.heap.arena().bytes()[(self.code + self.pc + 1) as usize]);
                let v = self.pop();
                self.set(self.sp - 1 - n, v);
            }

            // ---- locals, arguments, Boxes, captures ----
            Op::GetLoc => {
                let v = self.get(self.lp + self.imm_u16(1));
                self.push(v);
            }
            Op::PutLoc => {
                let v = self.pop();
                self.set(self.lp + self.imm_u16(1), v);
            }
            Op::SetLoc => {
                let v = self.top(0);
                self.set(self.lp + self.imm_u16(1), v);
            }
            Op::GetLocChk => {
                let i = self.imm_u16(1);
                let v = self.get(self.lp + i);
                if v.is_uninitialized() {
                    return self.uninitialized(i);
                }
                self.push(v);
            }
            Op::PutLocChk | Op::SetLocChk => {
                let i = self.imm_u16(1);
                if self.get(self.lp + i).is_uninitialized() {
                    return self.uninitialized(i);
                }
                let v = if op == Op::PutLocChk {
                    self.pop()
                } else {
                    self.top(0)
                };
                self.set(self.lp + i, v);
            }
            Op::SetLocUninit => {
                self.set(self.lp + self.imm_u16(1), Value::UNINITIALIZED);
            }
            Op::IncLoc | Op::DecLoc => {
                let slot = self.lp + self.imm_u16(1);
                let a = self.get(slot);
                if a.as_object().is_some() {
                    // ToNumeric of an object runs user code: its result replaces the
                    // local and the instruction runs again. Nothing else can see the
                    // local in between, and a throw leaves it unchanged.
                    return self.call_converter(intrinsics::TO_NUMERIC, slot);
                }
                let d = if op == Op::IncLoc { 1 } else { -1 };
                let v = match a.as_int().and_then(|i| i.checked_add(d)) {
                    Some(i) => Value::int(i),
                    None if a.as_bigint().is_some() => self.bigint_add_small(a, d)?,
                    None => conv::number(conv::to_number(self.heap, a)? + d as f64),
                };
                self.set(slot, v);
            }
            Op::GetArg => {
                let v = self.get(self.bp + 2 + self.imm_u16(1));
                self.push(v);
            }
            Op::PutArg => {
                let v = self.pop();
                self.set(self.bp + 2 + self.imm_u16(1), v);
            }
            Op::SetArg => {
                let v = self.top(0);
                self.set(self.bp + 2 + self.imm_u16(1), v);
            }
            Op::GetBox | Op::GetBoxChk => {
                let i = self.imm_u16(1);
                let v = self.box_value(self.get(self.lp + i));
                if op == Op::GetBoxChk && v.is_uninitialized() {
                    return self.uninitialized(i);
                }
                self.push(v);
            }
            Op::PutBox | Op::SetBox | Op::PutBoxChk | Op::SetBoxChk => {
                let i = self.imm_u16(1);
                let b = self.get(self.lp + i);
                if matches!(op, Op::PutBoxChk | Op::SetBoxChk)
                    && self.box_value(b).is_uninitialized()
                {
                    return self.uninitialized(i);
                }
                let v = if matches!(op, Op::PutBox | Op::PutBoxChk) {
                    self.pop()
                } else {
                    self.top(0)
                };
                self.set_box_value(b, v);
            }
            Op::NewBox => {
                let b = self.new_box(Value::UNINITIALIZED)?;
                self.set(self.lp + self.imm_u16(1), b);
            }
            Op::NewBoxInit => {
                let v = self.top(0);
                let b = self.new_box(v)?;
                self.sp -= 1;
                self.set(self.lp + self.imm_u16(1), b);
            }
            Op::Rebox => {
                let i = self.imm_u16(1);
                let v = self.box_value(self.get(self.lp + i));
                let b = self.new_box(v)?;
                self.set(self.lp + i, b);
            }
            Op::GetCap | Op::GetCapChk => {
                let i = self.imm_u16(1);
                let v = self.box_value(self.capture(i));
                if op == Op::GetCapChk && v.is_uninitialized() {
                    return self.uninitialized(self.locals() + i);
                }
                self.push(v);
            }
            Op::GetBuiltin => {
                let Some(v) = self.prelude_function(self.imm_u16(1)) else {
                    return Err(Fault::error(
                        ErrorKind::InternalError,
                        String::from("bad bytecode: get_builtin out of range"),
                    ));
                };
                self.push(v);
            }
            Op::PutBuiltin => {
                let (i, len) = (self.imm_u16(1), self.imm_u16(3));
                let mut t = intrinsic(self.heap, Intrinsic::PreludeFunctions);
                if t.as_internal().is_none() {
                    let a = self.heap.alloc_value_array(len)?;
                    self.heap.arena_mut().set(a, ValueArray::LEN, len);
                    t = Value::internal(a);
                    crate::interp::realm::set_intrinsic(self.heap, Intrinsic::PreludeFunctions, t);
                }
                let t = t.as_internal().unwrap();
                if i >= self.heap.value_array_len(t) {
                    return Err(Fault::error(
                        ErrorKind::InternalError,
                        String::from("bad bytecode: put_builtin out of range"),
                    ));
                }
                let v = self.pop();
                self.heap.value_array_set(t, i, v);
            }
            Op::PutCap | Op::SetCap | Op::PutCapChk | Op::SetCapChk => {
                let i = self.imm_u16(1);
                let b = self.capture(i);
                if matches!(op, Op::PutCapChk | Op::SetCapChk)
                    && self.box_value(b).is_uninitialized()
                {
                    return self.uninitialized(self.locals() + i);
                }
                let v = if matches!(op, Op::PutCap | Op::PutCapChk) {
                    self.pop()
                } else {
                    self.top(0)
                };
                self.set_box_value(b, v);
            }

            // ---- globals ----
            Op::GetGlobal | Op::GetGlobalTypeof => {
                let key = self.imm_atom();
                let ic = self.imm_u16(5);
                if let Some(v) = self.ic_global_get(key, ic) {
                    self.push(v);
                    self.pc = next;
                    return Ok(None);
                }
                if let Some(v) = self.global_lexical(key)? {
                    self.ic_global_fill(key, ic);
                    self.push(v);
                    self.pc = next;
                    return Ok(None);
                }
                let g = intrinsic_obj(self.heap, Intrinsic::Global);
                match self.heap.find_property(g, key) {
                    None if op == Op::GetGlobal => return Err(undefined_global(self.heap, key)),
                    None => self.push(Value::UNDEFINED),
                    Some((h, Property::Data { value, .. })) => {
                        if h == g {
                            self.ic_global_fill(key, ic);
                        }
                        self.push(value)
                    }
                    Some((_, Property::Accessor { getter, .. })) => {
                        self.pc = next;
                        return self.call_accessor(getter, Value::object(g), None);
                    }
                }
            }
            Op::PutGlobal => {
                let key = self.imm_atom();
                let ic = self.imm_u16(5);
                let v = self.top(0);
                if self.ic_global_put(key, ic, v) {
                    self.sp -= 1;
                    self.pc = next;
                    return Ok(None);
                }
                let lex = realm::global_lex(self.heap);
                if let Some(Property::Data { flags, .. }) = self.heap.get_own_property(lex, key) {
                    self.global_lexical(key)?;
                    if !flags.writable() {
                        return Err(Fault::error(
                            ErrorKind::TypeError,
                            crate::text::cat!(
                                "'",
                                conv::describe_key(self.heap, key),
                                "' is read-only"
                            ),
                        ));
                    }
                    let v = self.pop();
                    realm::set_global_lexical(self.heap, key, v)?;
                    self.ic_global_fill(key, ic);
                    self.pc = next;
                    return Ok(None);
                }
                let g = intrinsic_obj(self.heap, Intrinsic::Global);
                if !self.heap.has_property(g, key) {
                    return Err(undefined_global(self.heap, key));
                }
                // The cache is filled for the next store if this one is a plain write.
                if matches!(
                    self.heap.get_own_property(g, key),
                    Some(Property::Data { flags, .. }) if flags.writable()
                ) {
                    self.ic_global_fill(key, ic);
                }
                self.pc = next;
                return self.set_property(g, key, v, 1);
            }
            Op::InitGlobalLex => {
                let key = self.imm_atom();
                let v = self.pop();
                realm::set_global_lexical(self.heap, key, v)?;
            }
            Op::GlobalDecls => {
                let idx = self.imm_u32(1);
                self.global_decls(idx)?;
            }
            Op::Rest => {
                let a = self.rest_array()?;
                self.push(a);
            }
            Op::GetTemplateObject => {
                let site = self.imm_u32(1);
                let v = self.template_object(site)?;
                self.push(v);
            }
            Op::SpecialObject => {
                let kind = self.heap.arena().bytes()[(self.code + self.pc + 1) as usize];
                let v = self.special_object(kind)?;
                self.push(v);
            }
            Op::ThrowConstAssign => {
                return Err(Fault::error(
                    ErrorKind::TypeError,
                    crate::text::cat!(
                        "'",
                        conv::describe_key(self.heap, self.imm_atom()),
                        "' is read-only"
                    ),
                ));
            }

            // ---- properties ----
            Op::GetField => {
                let key = self.imm_atom();
                let ic = self.imm_u16(5);
                let o = self.top(0);
                self.pc = next;
                let got = self.lookup_ic(o, key, ic)?;
                return self.get_property_got(o, key, got);
            }
            Op::GetMethod => {
                let key = self.imm_atom();
                let ic = self.imm_u16(5);
                let o = self.top(0);
                match self.lookup_ic(o, key, ic)? {
                    Got::Value(f) => {
                        self.set(self.sp - 1, f);
                        self.push(o);
                    }
                    Got::Getter(getter) => {
                        // `o -> f o`: the getter's result fills the placeholder.
                        self.set(self.sp - 1, Value::UNDEFINED);
                        self.push(o);
                        self.pc = next;
                        return self.call_getter_into(getter, o, self.sp - 2);
                    }
                    Got::Proxy(p) => {
                        self.set(self.sp - 1, Value::UNDEFINED);
                        self.push(o);
                        self.pc = next;
                        let slot = self.sp - 2;
                        return self.call_proxy_get(p, key, o, Continuation::Replace(slot));
                    }
                }
            }
            Op::PutField => {
                let key = self.imm_atom();
                let ic = self.imm_u16(5);
                let (o, v) = (self.top(1), self.top(0));
                let Some(obj) = o.as_object() else {
                    return self.put_on_primitive(o, key, v, 2, next);
                };
                self.pc = next;
                if self.heap.set_own_data_value(obj, key, v) {
                    self.ic_own_fill(obj, key, ic);
                    self.sp -= 2;
                    return Ok(None);
                }
                if self.plain_addition(obj, key) {
                    // [[Set]] of a key that no object on the chain has: CreateDataProperty
                    // on the receiver, which an addition cache can repeat (0083).
                    let old = self.heap.receiver_shape(obj);
                    if self.heap.create_data_property(obj, key, v)? {
                        self.ic_add_fill(obj, key, old, true, ic);
                        self.sp -= 2;
                        return Ok(None);
                    }
                }
                return self.set_property(obj, key, v, 2);
            }
            Op::GetElem => {
                let (o, k) = (self.top(1), self.top(0));
                if let (Some(obj), Some(i)) = (o.as_object(), k.as_int())
                    && i >= 0
                    && let Some(v) = self.heap.get_element_fast(obj, i as u32)
                {
                    self.sp -= 2;
                    self.push(v);
                } else {
                    let key = self.property_key(o, k)?;
                    self.sp -= 1;
                    self.pc = next;
                    return self.get_property(o, key);
                }
            }
            // Object keys were converted by `primitive_operands`.
            Op::ToPropKey => {}
            Op::GetElem2 => {
                let (o, k) = (self.top(1), self.top(0));
                let key = self.property_key(o, k)?;
                self.push(Value::UNDEFINED);
                self.pc = next;
                return self.get_property(o, key);
            }
            Op::GetElemMethod => {
                let (o, k) = (self.top(1), self.top(0));
                let key = self.property_key(o, k)?;
                match self.lookup(o, key)? {
                    Got::Value(f) => {
                        self.set(self.sp - 2, f);
                        self.set(self.sp - 1, o);
                    }
                    Got::Getter(getter) => {
                        self.set(self.sp - 2, Value::UNDEFINED);
                        self.set(self.sp - 1, o);
                        self.pc = next;
                        return self.call_getter_into(getter, o, self.sp - 2);
                    }
                    Got::Proxy(p) => {
                        self.set(self.sp - 2, Value::UNDEFINED);
                        self.set(self.sp - 1, o);
                        self.pc = next;
                        let slot = self.sp - 2;
                        return self.call_proxy_get(p, key, o, Continuation::Replace(slot));
                    }
                }
            }
            Op::PutElem => {
                let (o, k, v) = (self.top(2), self.top(1), self.top(0));
                let Some(obj) = o.as_object() else {
                    let key = conv::to_property_key(self.heap, k)?;
                    return self.put_on_primitive(o, key, v, 3, next);
                };
                if let Some(i) = k.as_int()
                    && i >= 0
                    && self.heap.set_element_fast(obj, i as u32, v)?
                {
                    self.sp -= 3;
                } else {
                    let key = self.property_key(o, k)?;
                    // Rearrange `o k v` to `o v` for set_property.
                    self.set(self.sp - 2, v);
                    self.sp -= 1;
                    self.pc = next;
                    return self.set_property(obj, key, v, 2);
                }
            }
            Op::DefineField => {
                let key = self.imm_atom();
                let ic = self.imm_u16(5);
                let v = self.pop();
                let o = self.top(0).as_object().expect("object literal");
                let old = self.heap.receiver_shape(o);
                self.heap.create_data_property(o, key, v)?;
                self.ic_add_fill(o, key, old, false, ic);
            }
            Op::In => {
                let (k, o) = (self.top(1), self.top(0));
                let Some(obj) = o.as_object() else {
                    return type_error("invalid 'in' operand");
                };
                let key = self.property_key(o, k)?;
                match self.heap.has_property_or_proxy(obj, key) {
                    Ok(r) => {
                        self.sp -= 2;
                        self.push(Value::bool(r));
                    }
                    Err(p) => {
                        self.sp -= 2;
                        self.pc = next;
                        let k = self.heap.atom_to_value(key)?;
                        return self.call_prelude(
                            proxy::PROXY_HAS,
                            &[Value::object(p), k],
                            Continuation::PushResult,
                        );
                    }
                }
            }
            Op::DefineMethod | Op::DefineGetter | Op::DefineSetter => {
                let key = self.imm_atom();
                let f = self.pop();
                let o = self.top(0).as_object().expect("object literal");
                self.define_literal_member(o, key, f, op)?;
            }
            Op::DefineMethodElem | Op::DefineGetterElem | Op::DefineSetterElem => {
                let (o, k, f) = (self.top(2), self.top(1), self.top(0));
                let key = self.property_key(o, k)?;
                let obj = o.as_object().expect("object literal");
                self.define_literal_member(obj, key, f, op)?;
                self.sp -= 2;
            }
            Op::ForInStart => {
                let v = self.pop();
                let it = match v.as_object() {
                    // Traps decide the keys: a Record that %ForInStep walks.
                    Some(o) if self.chain_has_proxy(o) => self.for_in_proxy_start(o)?,
                    _ => self.for_in_start(v)?,
                };
                self.push(it);
            }
            Op::ForInNext => {
                // `it -> it key done`
                let it = self.top(0);
                if let Some(rec) = it.as_object() {
                    return self.for_in_proxy_next(rec, next);
                }
                let (key, done) = self.for_in_next(it)?;
                self.push(key);
                self.push(Value::bool(done));
            }
            Op::IterNext | Op::IterNextLoc => {
                // `it next -> r` (D1800): the value itself when `next` is the original
                // one of an Array or String Iterator and the step runs no user code;
                // otherwise `next.call(it)`, whose result the code after this checks.
                // `iter_next_loc` (D2732) is the same step on two locals: it pushes the
                // result (or the call's callee and `this`), touching no other slot, as
                // the hot loop does.
                let loc = op == Op::IterNextLoc;
                let (it, f, off) = if loc {
                    let at = self.lp + self.imm_u16(1);
                    (self.get(at), self.get(at + 1), 3)
                } else {
                    (self.top(1), self.top(0), 1)
                };
                let r = match self.iter_fast_next(it, f)? {
                    iter::FastStep::Value(v) => {
                        self.pc = (next as i64 + self.imm_i32(off) as i64) as u32;
                        Some(v)
                    }
                    // The one result object of the loop, as `next` would return it.
                    iter::FastStep::Done => {
                        let r = self.done_result()?;
                        self.pc = next;
                        Some(r)
                    }
                    iter::FastStep::Slow => None,
                };
                if let Some(r) = r {
                    if loc {
                        self.push(r);
                    } else {
                        self.sp -= 1;
                        self.set(self.sp - 1, r);
                    }
                    if self.pc < next {
                        self.safepoint();
                    }
                    return Ok(None);
                }
                if loc {
                    self.ensure(self.sp + 2)?;
                    self.sp += 2;
                }
                let base = self.sp - 2;
                self.set(base, f);
                self.set(base + 1, it);
                return self.call(base, 0, Continuation::PushResult, next);
            }
            Op::IterCheckObject => {
                if self.top(0).as_object().is_none() {
                    return type_error("iterator must return an object");
                }
            }
            Op::ArraySpread => {
                let (a, src) = (self.top(1), self.top(0));
                let arr = a.as_object().expect("array literal");
                if !self.array_spread(arr, src)? {
                    // The iterator protocol, in the builtin language (builtins/spread.ts):
                    // `arr src` becomes `%ArraySpread undefined arr src`, which pushes arr.
                    let g = realm::installed_intrinsic(self.heap, intrinsics::ARRAY_SPREAD);
                    if g.is_undefined() {
                        return type_error("spreading an iterable needs the prelude");
                    }
                    let base = self.sp - 2;
                    self.ensure(base + 4)?;
                    self.set(base + 2, a);
                    self.set(base + 3, src);
                    self.set(base, g);
                    self.set(base + 1, Value::UNDEFINED);
                    self.sp = base + 4;
                    return self.call(base, 2, Continuation::PushResult, next);
                }
                self.sp -= 1;
            }
            Op::DefineElem => {
                let (o, k, v) = (self.top(2), self.top(1), self.top(0));
                let key = self.property_key(o, k)?;
                let obj = o.as_object().expect("object literal");
                self.heap.create_data_property(obj, key, v)?;
                self.sp -= 2;
            }
            Op::CopyDataProps => {
                let mode = self.heap.arena().bytes()[(self.code + self.pc + 1) as usize];
                if mode != 0 {
                    return Err(Fault::error(
                        ErrorKind::InternalError,
                        crate::text::cat!("not supported yet: copy_data_props ", mode),
                    ));
                }
                let (o, src) = (self.top(1), self.top(0));
                if !self.copy_data_props(o.as_object().expect("object literal"), src)? {
                    // Getters run user code: in the builtin language (builtins/spread.ts),
                    // `o src` becomes `%CopyDataProperties undefined o src`, pushing o.
                    let g = realm::installed_intrinsic(self.heap, intrinsics::COPY_DATA_PROPERTIES);
                    if g.is_undefined() {
                        return type_error("spreading an object with getters needs the prelude");
                    }
                    let base = self.sp - 2;
                    self.ensure(base + 4)?;
                    self.set(base + 2, o);
                    self.set(base + 3, src);
                    self.set(base, g);
                    self.set(base + 1, Value::UNDEFINED);
                    self.sp = base + 4;
                    return self.call(base, 2, Continuation::PushResult, next);
                }
                self.sp -= 1;
            }
            Op::Delete => {
                let (o, k) = (self.top(1), self.top(0));
                if let Some(p) = o.as_object().filter(|&p| self.heap.is_proxy(p)) {
                    let key = self.property_key(o, k)?;
                    let k = self.heap.atom_to_value(key)?;
                    self.sp -= 2;
                    self.pc = next;
                    let args = [Value::object(p), k];
                    return self.call_prelude(
                        proxy::PROXY_DELETE_OR_THROW,
                        &args,
                        Continuation::PushResult,
                    );
                }
                let r = self.delete(o, k)?;
                self.sp -= 2;
                self.push(Value::bool(r));
            }
            Op::Construct => {
                let argc = self.imm_u16(1);
                return self.construct(argc, next);
            }
            Op::SetProto => {
                let v = self.pop();
                let o = self.top(0).as_object().expect("object literal");
                if v.is_null() || v.as_object().is_some() {
                    // A fresh literal is extensible and cannot be on v's chain.
                    self.heap.set_prototype_of(o, v.as_object());
                }
            }
            Op::SetFunctionName => {
                let prefix = self.heap.arena().bytes()[(self.code + self.pc + 1) as usize];
                let (k, f) = (self.top(1), self.top(0));
                let key = self.property_key(f, k)?;
                self.set_function_name(f, key, prefix)?;
            }
            Op::CallSpread => {
                // `f this args -> r`
                let args = self.top(0);
                let argc = self.spread_args(args)?;
                let base = self.sp - 2 - argc;
                return self.call(base, argc, Continuation::PushResult, next);
            }
            Op::ConstructSpread => {
                // `f args -> r`
                let args = self.top(0);
                let argc = self.spread_args(args)?;
                return self.construct(argc, next);
            }
            Op::CallIntrinsic => {
                let id = self.imm_u16(1) as u16;
                let argc = self.imm_u16(3);
                return self.call_intrinsic(id, argc, next);
            }
            // The short forms that `step` runs often (a declined intrinsic, call or
            // branch on a non-boolean) here, the others in `exec_short` (D2443).
            Op::CallIntrinsic8 => {
                let id = self.byte(self.pc + 1) as u16;
                return self.call_intrinsic(id, arity(id as u32), next);
            }
            Op::Call0 | Op::Call1 | Op::Call2 | Op::Call3 => {
                let argc = op as u32 - Op::Call0 as u32;
                let base = self.sp - 2 - argc;
                return self.call(base, argc, Continuation::PushResult, next);
            }
            Op::GetLoc0
            | Op::GetLoc1
            | Op::GetLoc2
            | Op::GetLoc3
            | Op::PutLoc0
            | Op::PutLoc1
            | Op::PutLoc2
            | Op::PutLoc3
            | Op::GetArg0
            | Op::GetArg1
            | Op::GetArg2
            | Op::GetArg3
            | Op::SetLoc0
            | Op::SetLoc1
            | Op::SetLoc2
            | Op::SetLoc3
            | Op::Push0
            | Op::Push1
            | Op::Push2
            | Op::Push3
            | Op::GetLoc8
            | Op::PutLoc8
            | Op::SetLoc8
            | Op::GetArg8
            | Op::PushConst8
            | Op::GetBuiltinCallee8
            | Op::Jump8 => return self.exec_short(op, next),
            Op::RequireObjectCoercible => {
                if self.top(0).is_nullish() {
                    return type_error("cannot convert to object");
                }
            }

            // ---- operators ----
            Op::Neg => {
                let a = self.pop();
                let v = match a.as_int() {
                    Some(i) if i != 0 && i != i32::MIN => Value::int(-i),
                    _ => Value::number(-conv::to_number(self.heap, a)?),
                };
                self.push(v);
            }
            Op::Plus | Op::ToNumeric => {
                // A Number stays as it is (both forms are Numbers).
                if !self.top(0).is_number() {
                    let a = self.pop();
                    let n = conv::to_number(self.heap, a)?;
                    self.push(conv::number(n));
                }
            }
            Op::BNot => {
                let a = self.pop();
                let n = conv::to_int32(conv::to_number(self.heap, a)?);
                self.push(Value::int(!n));
            }
            Op::Not => {
                let a = self.pop();
                let b = conv::to_boolean(self.heap, a);
                self.push(Value::bool(!b));
            }
            Op::Typeof => {
                let a = self.pop();
                let s = self.heap.atom_string(conv::typeof_atom(self.heap, a))?;
                self.push(Value::string(s));
            }
            Op::Inc | Op::Dec => {
                let a = self.pop();
                let d = if op == Op::Inc { 1 } else { -1 };
                let v = match a.as_int().and_then(|i| i.checked_add(d)) {
                    Some(i) => Value::int(i),
                    None => conv::number(conv::to_number(self.heap, a)? + d as f64),
                };
                self.push(v);
            }
            Op::ToString => {
                let a = self.pop();
                let s = conv::to_string(self.heap, a)?;
                self.push(Value::string(s));
            }
            Op::Add => {
                let (a, b) = (self.top(1), self.top(0));
                let v = conv::add(self.heap, a, b)?;
                self.sp -= 2;
                self.push(v);
            }
            Op::Sub => self.binary_num(i32::checked_sub, |x, y| x - y)?,
            // An int32 product of 0 may be -0 (`-1 * 0`), which only the f64 path gets right.
            Op::Mul => {
                self.binary_num(|x, y| x.checked_mul(y).filter(|&r| r != 0), |x, y| x * y)?
            }
            Op::Div => self.binary_num(|_, _| None, |x, y| x / y)?,
            // x % y for int32: the sign of x, and 0 % y or a zero result from a negative x
            // is -0 for x < 0, so only non-negative x with nonzero y stays int32.
            Op::Mod => self.binary_num(|x, y| (x >= 0 && y != 0).then(|| x % y), conv::rem)?,
            Op::Pow => self.binary_num(|_, _| None, conv::pow)?,
            Op::Shl => self.binary_int(|x, y| Value::int(x.wrapping_shl(y as u32 & 31)))?,
            Op::Sar => self.binary_int(|x, y| Value::int(x >> (y as u32 & 31)))?,
            Op::Shr => {
                self.binary_int(|x, y| conv::number(((x as u32) >> (y as u32 & 31)) as f64))?
            }
            Op::BAnd => self.binary_int(|x, y| Value::int(x & y))?,
            Op::BOr => self.binary_int(|x, y| Value::int(x | y))?,
            Op::BXor => self.binary_int(|x, y| Value::int(x ^ y))?,
            Op::Lt => self.compare(false, false)?,
            Op::Gt => self.compare(true, false)?,
            Op::Le => self.compare(true, true)?,
            Op::Ge => self.compare(false, true)?,
            Op::Eq | Op::Ne => {
                let (a, b) = (self.top(1), self.top(0));
                let r = conv::loose_equals(self.heap, a, b)?;
                self.sp -= 2;
                self.push(Value::bool(r == (op == Op::Eq)));
            }
            Op::StrictEq | Op::StrictNe => {
                let b = self.pop();
                let a = self.pop();
                let r = conv::strict_equals(self.heap, a, b);
                self.push(Value::bool(r == (op == Op::StrictEq)));
            }
            Op::Instanceof => {
                let (v, f) = (self.top(1), self.top(0));
                let g = realm::installed_intrinsic(self.heap, intrinsics::INSTANCEOF_OPERATOR);
                let r = if g.is_undefined() || !self.instanceof_needs_builtin(f) {
                    self.instance_of(v, f, !g.is_undefined())?
                } else {
                    None
                };
                if let Some(r) = r {
                    self.sp -= 2;
                    self.push(Value::bool(r));
                } else {
                    // A custom Symbol.hasInstance, a `prototype` getter or a proxy: the
                    // spec's InstanceofOperator in the builtin language
                    // (builtins/function.ts).
                    let base = self.sp - 2;
                    self.ensure(base + 4)?;
                    self.set(base + 2, v);
                    self.set(base + 3, f);
                    self.set(base, g);
                    self.set(base + 1, Value::UNDEFINED);
                    self.sp = base + 4;
                    return self.call(base, 2, Continuation::PushResult, next);
                }
            }
            Op::IsNullish => {
                let a = self.pop();
                self.push(Value::bool(a.is_nullish()));
            }
            Op::IsUndefined => {
                let a = self.pop();
                self.push(Value::bool(a.is_undefined()));
            }
            Op::Concat => {
                let n = self.imm_u16(1);
                let mut acc = self.heap.atom_string(crate::heap::atoms::EMPTY)?;
                for i in 0..n {
                    let v = self.get(self.sp - n + i);
                    let s = conv::to_string(self.heap, v)?;
                    acc = self.heap.concat(acc, s)?;
                }
                self.sp -= n;
                self.push(Value::string(acc));
            }

            // ---- control flow ----
            Op::Jump => {
                self.pc = self.jump_target(next);
                if self.pc < next {
                    self.safepoint();
                }
                return Ok(None);
            }
            Op::JumpIfFalse | Op::JumpIfTrue => {
                let v = self.pop();
                let t = conv::to_boolean(self.heap, v);
                if t == (op == Op::JumpIfTrue) {
                    self.pc = self.jump_target(next);
                    if self.pc < next {
                        self.safepoint();
                    }
                } else {
                    self.pc = next;
                }
                return Ok(None);
            }
            Op::JumpIfFalse8 | Op::JumpIfTrue8 => {
                let v = self.pop();
                let t = conv::to_boolean(self.heap, v);
                let offset = self.byte(self.pc + 1) as i8;
                self.pc = next;
                if t == (op == Op::JumpIfTrue8) {
                    self.pc = (next as i64 + offset as i64) as u32;
                    if self.pc < next {
                        self.safepoint();
                    }
                }
                return Ok(None);
            }
            Op::Throw => {
                let v = self.pop();
                return Err(Fault::throw(v));
            }
            Op::Return => {
                let v = self.pop();
                return self.do_return(v);
            }
            Op::ReturnUndef => return self.do_return(Value::UNDEFINED),
            Op::Call => {
                let argc = self.imm_u16(1);
                let base = self.sp - 2 - argc;
                return self.call(base, argc, Continuation::PushResult, next);
            }
            Op::Debugger | Op::Nop => {}
            Op::PrivateSymbol
            | Op::GetPrivateField
            | Op::PutPrivateField
            | Op::DefinePrivateField
            | Op::DefinePrivateAccessor
            | Op::PrivateIn
            | Op::GetSuper
            | Op::GetSuperField
            | Op::PutSuperField
            | Op::GetSuperElem
            | Op::PutSuperElem
            | Op::SetHomeObject
            | Op::DefineClass
            | Op::DefineClassMethod
            | Op::DefineClassMethodElem
            | Op::DefineClassField
            | Op::SuperCall
            | Op::SuperCallSpread
            | Op::CheckCtor
            | Op::CheckThis
            | Op::CheckThisUninit
            | Op::CheckCtorReturn
            | Op::ThrowError => return self.class_op(op, next),
            Op::GeneratorStart => return self.generator_start(next),
            Op::Yield => return self.yield_value(next, false),
            Op::YieldStar => return self.yield_value(next, true),
            Op::AsyncStart => self.async_start()?,
            Op::Await => return self.await_value(next),
            Op::AsyncReturn => return self.async_finish(false),
            Op::AsyncReject => return self.async_finish(true),
            #[cfg(feature = "lazy")]
            Op::Lazy => return self.lazy(),
            Op::NameGet
            | Op::NamePut
            | Op::NameSet
            | Op::NameInit
            | Op::NameTypeof
            | Op::EnterScope
            | Op::LeaveScope
            | Op::Label => {
                return Err(Fault::error(
                    ErrorKind::InternalError,
                    crate::text::cat!("bad bytecode: phase-1 op ", crate::detail::op_name(op)),
                ));
            }
            _ => {
                return Err(Fault::error(
                    ErrorKind::InternalError,
                    crate::text::cat!("not supported yet: ", crate::detail::op_name(op)),
                ));
            }
        }
        self.pc = next;
        Ok(None)
    }

    /// The short forms (D2443), out of line: as arms of `exec` they grew its code,
    /// which costs every instruction through `step` in the wasm build.
    #[inline(never)]
    fn exec_short(&mut self, op: Op, next: u32) -> Step {
        let i = self.operand(op);
        match op.wide() {
            Op::GetLoc => {
                let v = self.get(self.lp + i);
                self.push(v);
            }
            Op::PutLoc => {
                let v = self.pop();
                self.set(self.lp + i, v);
            }
            Op::SetLoc => {
                let v = self.top(0);
                self.set(self.lp + i, v);
            }
            Op::GetArg => {
                let v = self.get(self.bp + 2 + i);
                self.push(v);
            }
            Op::PushI8 => self.push(Value::int(i as i32)),
            Op::PushConst => {
                let v = self.constant(i);
                self.push(v);
            }
            Op::GetBuiltinCallee8 => {
                let Some(v) = self.prelude_function(i) else {
                    return Err(Fault::error(
                        ErrorKind::InternalError,
                        String::from("bad bytecode: get_builtin out of range"),
                    ));
                };
                self.push(v);
                self.push(Value::UNDEFINED);
            }
            Op::Call => {
                let base = self.sp - 2 - i;
                return self.call(base, i, Continuation::PushResult, next);
            }
            Op::CallIntrinsic => return self.call_intrinsic(i as u16, arity(i), next),
            Op::Jump | Op::JumpIfFalse | Op::JumpIfTrue => {
                let taken = match op.wide() {
                    Op::Jump => true,
                    o => {
                        let v = self.pop();
                        conv::to_boolean(self.heap, v) == (o == Op::JumpIfTrue)
                    }
                };
                self.pc = next;
                if taken {
                    self.pc = (next as i64 + i as i32 as i64) as u32;
                    if self.pc < next {
                        self.safepoint();
                    }
                }
                return Ok(None);
            }
            _ => {
                return Err(Fault::error(
                    ErrorKind::InternalError,
                    crate::text::cat!("bad bytecode: ", crate::detail::op_name(op)),
                ));
            }
        }
        self.pc = next;
        Ok(None)
    }

    // ---------------------------------------------------------------- helpers

    /// `get_template_object site` (GetTemplateObject, 13.2.8.4): `CONSTS[site]` is the
    /// site's template object once created; before that, the index of `n`, then `n`
    /// cooked strings (`undefined` for an invalid escape) and `n` raw strings. The
    /// first evaluation builds the frozen array with its frozen `raw` array and
    /// stores it in the slot, so each site has one object (D1362).
    fn template_object(&mut self, site: u32) -> Result<Value, Fault> {
        let c = self.constant(site);
        if c.as_object().is_some() {
            return Ok(c);
        }
        let bad = || Fault::error(ErrorKind::InternalError, String::from("bad template site"));
        let start = c.as_int().ok_or_else(bad)? as u32;
        let n = self.constant(start).as_int().ok_or_else(bad)? as u32;
        let cooked: alloc::vec::Vec<Value> = (0..n).map(|i| self.constant(start + 1 + i)).collect();
        let raw: alloc::vec::Vec<Value> =
            (0..n).map(|i| self.constant(start + 1 + n + i)).collect();
        let proto = intrinsic_obj(self.heap, Intrinsic::ArrayProto);
        let raw_obj = self.heap.new_array_from_values(Some(proto), &raw)?;
        crate::builtins::object::set_integrity_level(self.heap, raw_obj, true)?;
        let t = self.heap.new_array_from_values(Some(proto), &cooked)?;
        let d = PropertyDescriptor::data(Value::object(raw_obj), PropFlags::NONE);
        self.heap
            .define_own_property(t, crate::heap::atoms::RAW, &d)?;
        crate::builtins::object::set_integrity_level(self.heap, t, true)?;
        let v = Value::object(t);
        self.heap.value_array_set(self.consts, site, v);
        Ok(v)
    }

    /// Creates the closure for child template `child` of the current function.
    fn closure(&mut self, child: u32) -> Result<Value, Fault> {
        let t = template::child(self.heap, self.template, child).expect("child template");
        let a = self.heap.arena();
        let ncap = a.get(t.0, FuncTemplate::CAPTURES) as u32;
        let f = alloc_closure_of(self.heap, t)?;
        for i in 0..ncap {
            let d = template::capture_desc(self.heap, t.0, i);
            // (Bits 16-30 are a lazy template's capture attributes, D2990.)
            let b = if d & 0x8000_0000 != 0 {
                self.capture(d & 0xFFFF)
            } else {
                self.get(self.lp + (d & 0xFFFF))
            };
            self.heap.set_object_slot(f, i, b);
        }
        function_properties(self.heap, f, t.0)?;
        Ok(Value::object(f))
    }

    /// ToPropertyKey of a primitive key. Object keys never get here: the
    /// `primitive_operands` slow path converts them first.
    fn property_key(&mut self, _o: Value, k: Value) -> Result<Atom, Fault> {
        debug_assert!(k.as_object().is_none());
        conv::to_property_key(self.heap, k)
    }

    /// The slow path for object operands (D1280): if an operand of `op` that must be a
    /// primitive is an object, call `%ToPrimitive(operand, hint)` with a `Replace`
    /// continuation that writes the result into the operand's slot and re-executes this
    /// instruction. Operands are converted left to right, as the spec evaluates them.
    fn primitive_operands(&mut self, op: Op) -> Option<Step> {
        use crate::heap::atoms::{DEFAULT, NUMBER, STRING};
        let checks: &[(u32, Atom)] = match op {
            Op::Add => &[(1, DEFAULT), (0, DEFAULT)],
            Op::Sub
            | Op::Mul
            | Op::Div
            | Op::Mod
            | Op::Pow
            | Op::Shl
            | Op::Sar
            | Op::Shr
            | Op::BAnd
            | Op::BOr
            | Op::BXor
            | Op::Lt
            | Op::Gt
            | Op::Le
            | Op::Ge => &[(1, NUMBER), (0, NUMBER)],
            Op::Neg | Op::Plus | Op::BNot | Op::Inc | Op::Dec | Op::ToNumeric => &[(0, NUMBER)],
            Op::GetElem | Op::GetElem2 | Op::GetElemMethod
                if self.top(1).is_nullish() && self.top(0).as_object().is_some() =>
            {
                // RequireObjectCoercible(base) comes before ToPropertyKey(key) (qjs's
                // messages: the key is not converted).
                let msg = if op == Op::GetElem2 {
                    "value has no property"
                } else if self.top(1).is_undefined() {
                    "cannot read property of undefined"
                } else {
                    "cannot read property of null"
                };
                return Some(type_error(msg));
            }
            Op::ToString
            | Op::ToPropKey
            | Op::GetElem
            | Op::GetElem2
            | Op::GetElemMethod
            | Op::Delete
            | Op::GetSuperElem => &[(0, STRING)],
            Op::DefineElem
            | Op::DefineClassMethodElem
            | Op::DefineClassField
            | Op::PutSuperElem
            | Op::DefineMethodElem
            | Op::DefineGetterElem
            | Op::DefineSetterElem
            | Op::In
            | Op::SetFunctionName => &[(1, STRING)],
            Op::Eq | Op::Ne => {
                // Only an object compared with a non-nullish primitive is converted.
                let (a, b) = (self.top(1), self.top(0));
                let prim = |v: Value| v.as_object().is_none() && !v.is_nullish();
                let k = if a.as_object().is_some() && prim(b) {
                    1
                } else if b.as_object().is_some() && prim(a) {
                    0
                } else {
                    return None;
                };
                return Some(self.call_to_primitive(self.sp - 1 - k, DEFAULT));
            }
            Op::PutElem | Op::PutField => {
                // A typed array element takes ToNumber of the value, which may run user
                // code (or throw for a symbol): convert it first (0039).
                let key_slot = if op == Op::PutElem {
                    Some(self.sp - 2)
                } else {
                    None
                };
                let (o, v) = (
                    self.get(self.sp - 1 - (op == Op::PutElem) as u32 - 1),
                    self.top(0),
                );
                if let Some(k) = key_slot
                    && self.get(k).as_object().is_some()
                {
                    return Some(self.call_to_primitive(k, STRING));
                }
                // A BigInt typed array takes ToBigInt instead (0046): anything but a
                // BigInt, which also makes a BigInt into a Number array throw.
                if let Some(t) = o
                    .as_object()
                    .filter(|&t| self.heap.is_typed_array(t) && !self.heap.ta_immutable(t))
                    && self.heap.element_of(self.heap.ta_kind(t), v).is_none()
                    && self.typed_array_key(op)
                {
                    let id = if self.heap.ta_kind(t).is_bigint() {
                        intrinsics::TO_BIG_INT
                    } else {
                        intrinsics::TO_NUMBER
                    };
                    return Some(self.call_converter(id, self.sp - 1));
                }
                return None;
            }
            Op::Concat => {
                let n = self.imm_u16(1);
                return (0..n)
                    .map(|i| self.sp - n + i)
                    .find(|&slot| self.get(slot).as_object().is_some())
                    .map(|slot| self.call_to_primitive(slot, STRING));
            }
            _ => return None,
        };
        for &(k, hint) in checks {
            let slot = self.sp - 1 - k;
            if self.get(slot).as_object().is_some() {
                if k == 0 && checks.len() == 2 && hint == NUMBER && !self.is_relational(op) {
                    // ApplyStringOrNumericBinaryOperator: ToNumeric(lhs) completes, and
                    // throws for a Symbol, before the rhs is converted.
                    let lhs = self.top(1);
                    if lhs.as_symbol().is_some() {
                        return Some(conv::to_number(self.heap, lhs).map(|_| None));
                    }
                }
                return Some(self.call_to_primitive(slot, hint));
            }
        }
        None
    }

    /// `<`, `>`, `<=`, `>=`: both operands become primitives before either is a number.
    fn is_relational(&self, op: Op) -> bool {
        matches!(op, Op::Lt | Op::Gt | Op::Le | Op::Ge)
    }

    /// Calls `%ToPrimitive(stack[slot], hint)`; its result replaces `stack[slot]` and the
    /// current instruction runs again.
    fn call_to_primitive(&mut self, slot: u32, hint: Atom) -> Step {
        let f = realm::installed_intrinsic(self.heap, intrinsics::TO_PRIMITIVE);
        if f.is_undefined() {
            return type_error("cannot convert an object to a primitive (no prelude)");
        }
        let v = self.get(slot);
        let hint = Value::string(self.heap.atom_string(hint)?);
        let base = self.sp;
        self.ensure(base + 4)?;
        self.push(f);
        self.push(Value::UNDEFINED);
        self.push(v);
        self.push(hint);
        let pc = self.pc;
        self.call(base, 2, Continuation::Retry(slot), pc)
            .inspect_err(|_| self.sp = base)
    }

    /// Whether the key of a PutElem/PutField (already a primitive) is a canonical
    /// numeric string, so a typed array target treats it as an element.
    fn typed_array_key(&mut self, op: Op) -> bool {
        let key = if op == Op::PutElem {
            match conv::to_property_key(self.heap, self.top(1)) {
                Ok(k) => k,
                Err(_) => return false,
            }
        } else {
            self.imm_atom()
        };
        self.heap.canonical_numeric_index(key).is_some()
    }

    /// Calls the prelude's `%ToNumber(stack[slot])` (or another one-argument conversion
    /// `id`); its result replaces `stack[slot]` and the current instruction runs again.
    fn call_converter(&mut self, id: u16, slot: u32) -> Step {
        let f = realm::installed_intrinsic(self.heap, id);
        if f.is_undefined() {
            return type_error("cannot convert an object to a number (no prelude)");
        }
        let v = self.get(slot);
        let base = self.sp;
        self.ensure(base + 3)?;
        self.push(f);
        self.push(Value::UNDEFINED);
        self.push(v);
        let pc = self.pc;
        self.call(base, 1, Continuation::Retry(slot), pc)
            .inspect_err(|_| self.sp = base)
    }

    /// Calls a getter whose result goes into `stack[slot]`; resumes at `self.pc`.
    fn call_getter_into(&mut self, getter: Value, this: Value, slot: u32) -> Step {
        let base = self.sp;
        self.ensure(base + 2)?;
        self.push(getter);
        self.push(this);
        let pc = self.pc;
        self.call(base, 0, Continuation::Replace(slot), pc)
            .inspect_err(|_| self.sp = base)
    }

    fn nullish_base<T>(&self, o: Value, key: Atom, verb: &str) -> Result<T, Fault> {
        let what = if o.is_null() { "null" } else { "undefined" };
        type_error(&crate::text::cat!(
            "cannot ",
            verb,
            " property '",
            conv::describe_key(self.heap, key),
            "' of ",
            what
        ))
    }

    /// What `o[key]` is: a value, a getter to call, or a proxy's `get` trap.
    fn lookup(&mut self, o: Value, key: Atom) -> Result<Got, Fault> {
        match o.kind() {
            Kind::Object => self.lookup_object(o.as_object().unwrap(), key),
            Kind::String => {
                let s = o.as_string().unwrap();
                let len = self.heap.str_len(s);
                if key == crate::heap::atoms::LENGTH {
                    return Ok(Got::Value(Value::int(len as i32)));
                }
                if let Some(i) = key.as_int()
                    && i < len
                {
                    let u = self.heap.char_code_at(s, i).unwrap();
                    let c = self.heap.alloc_utf16(&[u])?;
                    return Ok(Got::Value(Value::string(c)));
                }
                self.lookup_primitive(o, key)
            }
            Kind::Undefined | Kind::Null => self.nullish_base(o, key, "read"),
            _ => self.lookup_primitive(o, key),
        }
    }

    /// `o[key]` for a primitive `o`: through its realm prototype (a getter gets the
    /// primitive as `this`).
    fn lookup_primitive(&mut self, o: Value, key: Atom) -> Result<Got, Fault> {
        let Some(proto) = realm::primitive_proto(self.heap, o) else {
            return Ok(Got::Value(Value::UNDEFINED));
        };
        self.lookup_object(proto, key)
    }

    /// [`Exec::lookup`] on the chain of an object.
    #[inline]
    fn lookup_object(&mut self, o: Obj, key: Atom) -> Result<Got, Fault> {
        let l = self.heap.lookup_property(o, key);
        self.got_of(l, key)
    }

    /// [`Exec::lookup`] for an access site with inline cache `ic`: an own data property
    /// of an object receiver fills the cache (D1620).
    #[inline]
    fn lookup_ic(&mut self, o: Value, key: Atom, ic: u32) -> Result<Got, Fault> {
        if let Some(v) = ic::mega_get(self.heap, o, key) {
            return Ok(Got::Value(v));
        }
        let start = match o.as_object() {
            Some(obj) => obj,
            None => match realm::primitive_proto(self.heap, o) {
                // A string's `length` and indexes are not on its prototype.
                Some(p)
                    if !(o.as_string().is_some()
                        && (key == crate::heap::atoms::LENGTH || key.as_int().is_some())) =>
                {
                    p
                }
                _ => return self.lookup(o, key),
            },
        };
        let l = self.heap.lookup_property(start, key);
        if let Lookup::Found(h, Property::Data { value, .. }) = l
            && !value.is_hole()
        {
            if Value::object(h) == o {
                self.ic_own_fill(h, key, ic);
            } else {
                self.ic_proto_fill(o, key, h, ic);
            }
        }
        self.got_of(l, key)
    }

    /// The [`Got`] of a heap lookup.
    #[inline]
    fn got_of(&mut self, l: Lookup, key: Atom) -> Result<Got, Fault> {
        Ok(match l {
            // A BigInt typed array element needs its allocation.
            Lookup::Found(h, Property::Data { value, .. }) if value.is_hole() => {
                Got::Value(self.heap.materialize_value(h, key, value)?)
            }
            Lookup::Found(_, Property::Data { value, .. }) => Got::Value(value),
            Lookup::Found(_, Property::Accessor { getter, .. }) if !getter.is_undefined() => {
                Got::Getter(getter)
            }
            Lookup::Found(..) | Lookup::Absent => Got::Value(Value::UNDEFINED),
            Lookup::Proxy(p) => Got::Proxy(p),
        })
    }

    /// Calls `%ProxyGet(p, key, receiver)`, the result going where `cont` says; `pc`
    /// is already the instruction to resume.
    fn call_proxy_get(&mut self, p: Obj, key: Atom, receiver: Value, cont: Continuation) -> Step {
        let k = self.heap.atom_to_value(key)?;
        self.call_prelude(proxy::PROXY_GET, &[Value::object(p), k, receiver], cont)
    }

    /// Replaces the object at `sp - 1` with `o[key]`, calling a getter if needed. `pc` is
    /// already the next instruction.
    fn get_property(&mut self, o: Value, key: Atom) -> Step {
        let got = self.lookup(o, key)?;
        self.get_property_got(o, key, got)
    }

    /// [`Exec::get_property`] with the lookup done.
    fn get_property_got(&mut self, o: Value, key: Atom, got: Got) -> Step {
        match got {
            Got::Value(v) => {
                self.set(self.sp - 1, v);
                Ok(None)
            }
            Got::Getter(getter) => {
                self.sp -= 1;
                self.call_accessor(getter, o, None)
            }
            Got::Proxy(p) => {
                self.sp -= 1;
                self.call_proxy_get(p, key, o, Continuation::PushResult)
            }
        }
    }

    /// Calls a getter (`arg = None`, result pushed) or a setter (result dropped). `pc`
    /// must already be the instruction to resume.
    fn call_accessor(&mut self, f: Value, this: Value, arg: Option<Value>) -> Step {
        let base = self.sp;
        self.ensure(base + 3)?;
        self.push(f);
        self.push(this);
        let (argc, cont) = match arg {
            None => (0, Continuation::PushResult),
            Some(v) => {
                self.push(v);
                (1, Continuation::DiscardResult)
            }
        };
        let ret = self.pc;
        self.call(base, argc, cont, ret)
            .inspect_err(|_| self.sp = base)
    }

    /// [[Set]] with receiver `o` for `o v` (the top `n` operands are consumed).
    /// ArraySetLength's conversions of a new `length` that is not a Number (10.4.2.4):
    /// the Number, a RangeError, or `None` for an object, whose conversions run user code
    /// (`%ArraySetLength`).
    pub(super) fn array_length_value(&mut self, v: Value) -> Result<Option<Value>, Fault> {
        if v.is_number() {
            return Ok(Some(v));
        }
        if v.as_object().is_some() {
            return Ok(None);
        }
        // A primitive converts without user code (a Symbol or BigInt throws).
        let number = conv::to_number(self.heap, v)?;
        if crate::builtins::number::to_uint32(number) as f64 != number {
            return Err(Fault::error(
                ErrorKind::RangeError,
                String::from("invalid array length"),
            ));
        }
        Ok(Some(Value::number(number)))
    }

    fn set_property(&mut self, o: Obj, key: Atom, v: Value, n: u32) -> Step {
        if self.heap.set_own_data_value(o, key, v) {
            self.sp -= n;
            return Ok(None);
        }
        // A typed array over an immutable buffer: [[Set]] of any numeric key fails before
        // the value is converted (D1970).
        if self.heap.is_typed_array(o)
            && self.heap.ta_immutable(o)
            && self.heap.canonical_numeric_index(key).is_some()
        {
            return Err(Fault::error(
                ErrorKind::TypeError,
                crate::text::cat!("'", conv::describe_key(self.heap, key), "' is read-only"),
            ));
        }
        // A typed array element whose value needs ToNumber / ToBigInt (user code): the
        // conversion and the store in the builtin language (0090).
        if self.heap.is_typed_array(o)
            && let Some(i) = self.heap.canonical_numeric_index(key)
            && self.heap.element_of(self.heap.ta_kind(o), v).is_none()
        {
            self.sp -= n;
            // -0 is not a valid integer index.
            let i = if i == 0.0 && i.is_sign_negative() {
                -1.0
            } else {
                i
            };
            let args = [Value::object(o), Value::number(i), v];
            return self.call_prelude(
                intrinsics::TYPED_ARRAY_SET_ELEMENT_SLOW,
                &args,
                Continuation::DiscardResult,
            );
        }
        let found = match self.heap.lookup_property(o, key) {
            Lookup::Found(h, p) => Some((h, p)),
            Lookup::Absent => None,
            Lookup::Proxy(p) => {
                // Strict mode: a false from the trap is a TypeError.
                self.sp -= n;
                let k = self.heap.atom_to_value(key)?;
                let args = [Value::object(p), k, v, Value::object(o)];
                return self.call_prelude(
                    proxy::PROXY_SET_OR_THROW,
                    &args,
                    Continuation::DiscardResult,
                );
            }
        };
        let readonly = |heap: &Heap| {
            Err(Fault::error(
                ErrorKind::TypeError,
                crate::text::cat!("'", conv::describe_key(heap, key), "' is read-only"),
            ))
        };
        match found {
            Some((_, Property::Accessor { setter, .. })) => {
                if setter.is_undefined() {
                    // qjs's message (without the key).
                    return type_error("no setter for property");
                }
                self.sp -= n;
                return self.call_accessor(setter, Value::object(o), Some(v));
            }
            Some((holder, Property::Data { flags, .. })) => {
                // The override fix (D2304): see `ordinary_set`.
                if !flags.writable() && (holder == o || !self.heap.frozen(holder)) {
                    return readonly(self.heap);
                }
                if holder == o {
                    let mut v = v;
                    if key == crate::heap::atoms::LENGTH
                        && self.heap.object_class(o) == ObjectClass::Array
                    {
                        match self.array_length_value(v)? {
                            Some(len) => v = len,
                            None => {
                                self.sp -= n;
                                let args = [Value::object(o), v, Value::TRUE];
                                return self.call_prelude(
                                    intrinsics::ARRAY_SET_LENGTH,
                                    &args,
                                    Continuation::DiscardResult,
                                );
                            }
                        }
                    }
                    let d = PropertyDescriptor {
                        value: Some(v),
                        ..Default::default()
                    };
                    if !self.heap.define_own_property(o, key, &d)? {
                        return readonly(self.heap);
                    }
                } else if !self.heap.create_data_property(o, key, v)? {
                    return type_error("object is not extensible");
                }
            }
            // A typed array on the chain without that element: nothing is written.
            None if self.heap.typed_array_set_stops(o, key) => {}
            None => {
                if !self.heap.create_data_property(o, key, v)? {
                    return type_error("object is not extensible");
                }
            }
        }
        self.sp -= n;
        Ok(None)
    }

    /// `o[key] = v` for a primitive `o` (the top `n` operands are consumed): [[Set]] on
    /// its wrapper with `o` as the receiver, which strict mode makes a TypeError unless
    /// a setter or a proxy's `set` trap on the prototype chain takes it.
    fn put_on_primitive(&mut self, o: Value, key: Atom, v: Value, n: u32, next: u32) -> Step {
        if o.is_nullish() {
            return self.nullish_base(o, key, "set");
        }
        let own_string_key = o.as_string().is_some_and(|s| {
            key == crate::heap::atoms::LENGTH
                || key.as_int().is_some_and(|i| i < self.heap.str_len(s))
        });
        if !own_string_key && let Some(proto) = realm::primitive_proto(self.heap, o) {
            match self.heap.lookup_property(proto, key) {
                Lookup::Found(_, Property::Accessor { setter, .. }) if !setter.is_undefined() => {
                    self.sp -= n;
                    self.pc = next;
                    return self.call_accessor(setter, o, Some(v));
                }
                Lookup::Proxy(p) => {
                    self.sp -= n;
                    self.pc = next;
                    let k = self.heap.atom_to_value(key)?;
                    let args = [Value::object(p), k, v, o];
                    return self.call_prelude(
                        proxy::PROXY_SET_OR_THROW,
                        &args,
                        Continuation::DiscardResult,
                    );
                }
                _ => {}
            }
        }
        // A string's own `length` and indexes are read-only; anything else would be
        // created on a temporary wrapper object, which strict mode rejects.
        if let Some(s) = o.as_string()
            && (key == crate::heap::atoms::LENGTH
                || key.as_int().is_some_and(|i| i < self.heap.str_len(s)))
        {
            return type_error(&crate::text::cat!(
                "'",
                conv::describe_key(self.heap, key),
                "' is read-only"
            ));
        }
        type_error("not an object")
    }

    /// The value of a global lexical binding (`None` if there is none); a ReferenceError
    /// while it is uninitialized (TDZ).
    fn global_lexical(&self, key: Atom) -> Result<Option<Value>, Fault> {
        let lex = realm::global_lex(self.heap);
        match self.heap.get_own_property(lex, key) {
            Some(Property::Data { value, .. }) if value.is_uninitialized() => Err(Fault::error(
                ErrorKind::ReferenceError,
                crate::text::cat!(self.heap.atom_name(key), " is not initialized"),
            )),
            Some(p) => Ok(p.value()),
            None => Ok(None),
        }
    }

    /// GlobalDeclarationInstantiation for the declaration list in constant `idx`, with
    /// QuickJS' checks (`JS_CheckDefineGlobalVar`): all checks first, then the bindings.
    fn global_decls(&mut self, idx: u32) -> Result<(), Fault> {
        let n = self.heap.value_array_len(self.consts);
        let consts: alloc::vec::Vec<Value> = (0..n)
            .map(|i| self.heap.value_array_get(self.consts, i))
            .collect();
        let decls = GlobalDecls::decode(&consts, idx as usize).ok_or_else(|| {
            Fault::error(ErrorKind::InternalError, String::from("bad global_decls"))
        })?;
        let g = intrinsic_obj(self.heap, Intrinsic::Global);
        let lex = realm::global_lex(self.heap);
        let redeclaration = |heap: &Heap, k: Atom| {
            Err(Fault::error(
                ErrorKind::SyntaxError,
                crate::text::cat!("redeclaration of '", heap.atom_name(k), "'"),
            ))
        };
        for &k in decls.lets.iter().chain(&decls.consts) {
            let own = self.heap.get_own_property(g, k);
            if own.is_some_and(|p| !p.flags().configurable()) || self.heap.has_own_property(lex, k)
            {
                return redeclaration(self.heap, k);
            }
        }
        for (i, &k) in decls.vars.iter().chain(&decls.funcs).enumerate() {
            let own = self.heap.get_own_property(g, k);
            let ok = match own {
                None => self.heap.is_extensible(g),
                // CanDeclareGlobalFunction: a non-configurable property must be a
                // writable, enumerable data property.
                Some(p) if i >= decls.vars.len() => {
                    let f = p.flags();
                    f.configurable() || (p.value().is_some() && f.writable() && f.enumerable())
                }
                Some(_) => true,
            };
            if !ok {
                return Err(Fault::error(
                    ErrorKind::TypeError,
                    crate::text::cat!("cannot define variable '", self.heap.atom_name(k), "'"),
                ));
            }
            if self.heap.has_own_property(lex, k) {
                return redeclaration(self.heap, k);
            }
        }
        for (i, &k) in decls.vars.iter().chain(&decls.funcs).enumerate() {
            let own = self.heap.get_own_property(g, k);
            // CreateGlobalFunctionBinding redefines a configurable property; the
            // function's value is stored by the hoisted initialization.
            let define = match own {
                None => true,
                Some(p) => i >= decls.vars.len() && p.flags().configurable(),
            };
            if define {
                let d = PropertyDescriptor::data(
                    Value::UNDEFINED,
                    PropFlags::WRITABLE | PropFlags::ENUMERABLE,
                );
                self.heap.define_own_property(g, k, &d)?;
            }
        }
        for (list, writable) in [(&decls.lets, true), (&decls.consts, false)] {
            for &k in list {
                realm::declare_global_lexical(self.heap, k, writable)?;
            }
        }
        Ok(())
    }

    /// `special_object kind`.
    fn special_object(&mut self, kind: u8) -> Result<Value, Fault> {
        Ok(match kind {
            special::THIS => self.get(self.bp + 1),
            special::NEW_TARGET => self.header(frame::NEW_TARGET),
            special::ACTIVE_FUNCTION => self.get(self.bp),
            special::HOME_OBJECT => {
                let f = self.get(self.bp).as_object().expect("frame callee");
                match self.home_slot(f) {
                    Some(slot) => self.heap.object_slot(f, slot),
                    None => Value::UNDEFINED,
                }
            }
            special::ARGUMENTS => {
                // An unmapped (strict) arguments object.
                let argc = self.header_int(frame::ARGC);
                let proto = intrinsic_obj(self.heap, Intrinsic::ObjectProto);
                let a = self
                    .heap
                    .alloc_object(ObjectClass::Arguments, Some(proto), 0)?;
                for i in 0..argc {
                    let v = self.get(self.bp + 2 + i);
                    self.heap
                        .create_data_property(a, Atom::int(i).unwrap(), v)?;
                }
                realm::define_hidden(
                    self.heap,
                    a,
                    crate::heap::atoms::LENGTH,
                    Value::int(argc as i32),
                )?;
                // CreateUnmappedArgumentsObject: @@iterator is %Array.prototype.values%.
                let values =
                    realm::installed_intrinsic(self.heap, intrinsics::ARRAY_PROTOTYPE_VALUES);
                if !values.is_undefined() {
                    realm::define_hidden(self.heap, a, crate::heap::atoms::SYM_ITERATOR, values)?;
                }
                // ... and `callee` is %ThrowTypeError% as getter and setter.
                let thrower = realm::installed_intrinsic(self.heap, intrinsics::THROW_TYPE_ERROR);
                if !thrower.is_undefined() {
                    let d = crate::heap::PropertyDescriptor::accessor(
                        thrower,
                        thrower,
                        crate::heap::PropFlags::NONE,
                    );
                    self.heap
                        .define_own_property(a, crate::heap::atoms::CALLEE, &d)?;
                }
                Value::object(a)
            }
            _ => {
                return Err(Fault::error(
                    ErrorKind::InternalError,
                    crate::text::cat!("not supported yet: special_object ", kind),
                ));
            }
        })
    }

    /// `{...src}` (CopyDataProperties without excluded keys). Like QuickJS, primitive
    /// sources contribute nothing.
    /// Object spread of plain data properties. Returns false, having copied nothing,
    /// when the source needs the general CopyDataProperties (an enumerable getter, a
    /// string's indexes).
    fn copy_data_props(&mut self, o: Obj, src: Value) -> Result<bool, Fault> {
        if src.as_string().is_some_and(|s| self.heap.str_len(s) > 0) {
            return Ok(false);
        }
        let Some(s) = src.as_object() else {
            return Ok(true);
        };
        if self.heap.is_proxy(s) {
            return Ok(false);
        }
        let keys = self.heap.own_keys(s);
        let getter = keys.iter().any(|&k| {
            matches!(self.heap.get_own_property(s, k),
                Some(Property::Accessor { flags, .. }) if flags.enumerable())
        });
        if getter {
            return Ok(false);
        }
        for k in keys {
            if let Some(Property::Data { value, flags }) = self.heap.get_own_property_alloc(s, k)?
                && flags.enumerable()
            {
                self.heap.create_data_property(o, k, value)?;
            }
        }
        Ok(true)
    }

    fn rest_props(&mut self, src: Value, excluded: Value) -> Result<Option<Value>, Fault> {
        let (Some(s), Some(ex)) = (src.as_object(), excluded.as_object()) else {
            return Ok(None);
        };
        if self.heap.is_proxy(s) || self.heap.object_class(ex) != ObjectClass::Array {
            return Ok(None);
        }
        let mut skip = alloc::vec::Vec::new();
        for i in 0..self.heap.array_length(ex) {
            let Some(e) = self
                .heap
                .get_element_fast(ex, i)
                .and_then(|e| e.as_string())
            else {
                return Ok(None);
            };
            if let Some(k) = match self.heap.str_array_index(e) {
                Some(index) => Atom::int(index),
                None => self.heap.find_string_atom_of(e),
            } {
                skip.push(k);
            }
        }
        let keys = self.heap.own_keys(s);
        if keys.iter().any(|&k| {
            matches!(self.heap.get_own_property(s, k),
                Some(Property::Accessor { flags, .. }) if flags.enumerable())
        }) {
            return Ok(None);
        }
        let proto = intrinsic_obj(self.heap, Intrinsic::ObjectProto);
        let o = self.heap.new_object(Some(proto))?;
        for k in keys {
            if !skip.contains(&k)
                && let Some(Property::Data { value, flags }) =
                    self.heap.get_own_property_alloc(s, k)?
                && flags.enumerable()
            {
                self.heap.create_data_property(o, k, value)?;
            }
        }
        Ok(Some(Value::object(o)))
    }

    /// The `delete` operator (strict: failure is a TypeError).
    fn delete(&mut self, o: Value, k: Value) -> Result<bool, Fault> {
        if o.is_nullish() {
            return type_error("cannot convert to object");
        }
        let key = self.property_key(o, k)?;
        let deleted = match (o.as_object(), o.as_string()) {
            (Some(obj), _) => self.heap.delete_own_property(obj, key)?,
            // A string's `length` and its index properties are not configurable.
            (None, Some(s)) => {
                !(key == crate::heap::atoms::LENGTH
                    || key.as_int().is_some_and(|i| i < self.heap.str_len(s)))
            }
            (None, None) => true,
        };
        if !deleted {
            return type_error("could not delete property");
        }
        Ok(true)
    }

    /// `for_in_start`: a snapshot of the keys a for-in visits, in an internal
    /// `ValueArray` `[object, next index, key0, key1, ...]` (only ever on the operand
    /// stack). Keys: the enumerable string keys of the object and then its prototypes, in
    /// [[OwnPropertyKeys]] order per object; a key seen once (enumerable or not) shadows
    /// the same key further up the chain. Symbols are skipped. `null`/`undefined` give
    /// no keys; a string, its indexes.
    fn for_in_start(&mut self, v: Value) -> Result<Value, Fault> {
        let mut keys: alloc::vec::Vec<Value> = alloc::vec::Vec::new();
        if let Some(s) = v.as_string() {
            for i in 0..self.heap.str_len(s) {
                keys.push(self.heap.atom_to_value(Atom::int(i).unwrap())?);
            }
        } else if let Some(o) = v.as_object() {
            // Objects already visited: their keys (enumerable or not) shadow the same
            // keys further up. Checked by lookup rather than kept in a set (0043: a set
            // type costs code size, and chains are short).
            let mut below: alloc::vec::Vec<Obj> = alloc::vec::Vec::new();
            let mut cur = Some(o);
            while let Some(c) = cur {
                for k in self.heap.own_keys(c) {
                    if k.is_symbol() || below.iter().any(|&b| self.heap.has_own_property(b, k)) {
                        continue;
                    }
                    if self
                        .heap
                        .get_own_property(c, k)
                        .is_some_and(|p| p.flags().enumerable())
                    {
                        keys.push(self.heap.atom_to_value(k)?);
                    }
                }
                below.push(c);
                cur = self.heap.get_prototype_of(c);
            }
        }
        let n = keys.len() as u32;
        let arr = self.heap.alloc_value_array(n + 2)?;
        let target = if v.as_object().is_some() {
            v
        } else {
            Value::UNDEFINED
        };
        self.heap.value_array_set(arr, 0, target);
        self.heap.value_array_set(arr, 1, Value::int(0));
        for (i, k) in keys.into_iter().enumerate() {
            self.heap.value_array_set(arr, 2 + i as u32, k);
        }
        self.heap
            .arena_mut()
            .set(arr, crate::heap::cells::ValueArray::LEN, n + 2);
        Ok(Value::internal(arr))
    }

    /// `for_in_next`: the next snapshot key that is still present on the object or its
    /// prototype chain (deleted keys are skipped), or `(undefined, true)` at the end.
    fn for_in_next(&mut self, it: Value) -> Result<(Value, bool), Fault> {
        let arr = it.as_internal().expect("for-in iterator");
        let len = self.heap.value_array_len(arr);
        let target = self.heap.value_array_get(arr, 0).as_object();
        loop {
            let idx = self.heap.value_array_get(arr, 1).as_int().unwrap_or(0) as u32;
            if 2 + idx >= len {
                return Ok((Value::UNDEFINED, true));
            }
            self.heap
                .value_array_set(arr, 1, Value::int(idx as i32 + 1));
            let key = self.heap.value_array_get(arr, 2 + idx);
            let Some(o) = target else {
                return Ok((key, false));
            };
            let atom = self.heap.intern(key.as_string().expect("string key"))?;
            if self.heap.has_property(o, atom) {
                return Ok((key, false));
            }
        }
    }

    /// A method, getter or setter of an object literal: DefinePropertyOrThrow with
    /// enumerable and configurable set (methods are also writable). A getter or setter
    /// alone keeps the other half of an existing accessor.
    fn define_literal_member(&mut self, o: Obj, key: Atom, f: Value, op: Op) -> Result<(), Fault> {
        let d = match op {
            Op::DefineMethod | Op::DefineMethodElem => PropertyDescriptor::data(f, PropFlags::ALL),
            Op::DefineGetter | Op::DefineGetterElem => PropertyDescriptor {
                get: Some(f),
                enumerable: Some(true),
                configurable: Some(true),
                ..Default::default()
            },
            _ => PropertyDescriptor {
                set: Some(f),
                enumerable: Some(true),
                configurable: Some(true),
                ..Default::default()
            },
        };
        if !self.heap.define_own_property(o, key, &d)? {
            return type_error("could not define property");
        }
        self.set_home(f, Value::object(o));
        Ok(())
    }

    /// `[...src]` without the iterator protocol where that is not observable: a string
    /// (by code points) or an array without holes, while their iteration is the
    /// original @@iterator and `next` (D1351, D1346). Returns false, with nothing
    /// changed, otherwise; the caller then runs %ArraySpread. Without a prelude every
    /// array and string is spread here (holes as undefined) and other values are a
    /// TypeError.
    fn array_spread(&mut self, arr: Obj, src: Value) -> Result<bool, Fault> {
        let mut vals = alloc::vec::Vec::new();
        let has_prelude =
            !realm::installed_intrinsic(self.heap, intrinsics::ARRAY_SPREAD).is_undefined();
        if !has_prelude && src.as_string().is_none() && src.as_object().is_none() {
            return type_error("value is not iterable");
        }
        if has_prelude && !self.iteration_unmodified(src) {
            return Ok(false);
        }
        if let Some(s) = src.as_string() {
            let units = self.heap.str_to_utf16(s);
            let mut i = 0;
            while i < units.len() {
                let pair = (0xD800..0xDC00).contains(&units[i])
                    && units
                        .get(i + 1)
                        .is_some_and(|u| (0xDC00..0xE000).contains(u));
                let n = if pair { 2 } else { 1 };
                vals.push(Value::string(self.heap.alloc_utf16(&units[i..i + n])?));
                i += n;
            }
        } else if let Some(a) = src
            .as_object()
            .filter(|&a| self.heap.object_class(a) == ObjectClass::Array)
        {
            for i in 0..self.heap.array_length(a) {
                match self.heap.get_element_fast(a, i) {
                    Some(v) => vals.push(v),
                    None if has_prelude => return Ok(false),
                    None => vals.push(Value::UNDEFINED),
                }
            }
        } else if has_prelude {
            return Ok(false);
        } else {
            return type_error("value is not iterable");
        }
        for v in vals {
            let len = self.heap.array_length(arr);
            if !self.heap.set_element_fast(arr, len, v)? {
                let k = self.heap.atom_from_index(len)?;
                self.heap.create_data_property(arr, k, v)?;
            }
        }
        Ok(true)
    }

    /// Whether iterating `v` (an Array or a string) would run the original @@iterator
    /// and `next` of its kind, as found without side effects (data properties only):
    /// then reading it directly is the same as iterating it. False for anything else.
    pub(super) fn iteration_unmodified(&self, v: Value) -> bool {
        use crate::heap::atoms::{NEXT, SYM_ITERATOR};
        let data = |o: Obj, key: Atom| match self.heap.find_property(o, key) {
            Some((_, Property::Data { value, .. })) => Some(value),
            _ => None,
        };
        let installed = |id| realm::installed_intrinsic(self.heap, id);
        if v.as_string().is_some() {
            let sp = intrinsic_obj(self.heap, Intrinsic::StringProto);
            let proto = intrinsic_obj(self.heap, Intrinsic::StringIteratorProto);
            return data(sp, SYM_ITERATOR)
                == Some(installed(intrinsics::STRING_PROTOTYPE_ITERATOR))
                && data(proto, NEXT) == Some(installed(intrinsics::STRING_ITERATOR_NEXT));
        }
        match v.as_object() {
            Some(a) if self.heap.object_class(a) == ObjectClass::Array => {
                let proto = intrinsic_obj(self.heap, Intrinsic::ArrayIteratorProto);
                data(a, SYM_ITERATOR) == Some(installed(intrinsics::ARRAY_PROTOTYPE_VALUES))
                    && data(proto, NEXT) == Some(installed(intrinsics::ARRAY_ITERATOR_NEXT))
            }
            _ => false,
        }
    }

    /// Replaces the array at the top of the stack by its elements; returns their count.
    /// The compiler builds the array, so it is a plain array (holes read as undefined).
    fn spread_args(&mut self, args: Value) -> Result<u32, Fault> {
        let a = args.as_object().expect("spread arguments array");
        let n = self.heap.array_length(a);
        self.sp -= 1;
        self.ensure(self.sp + n)?;
        for i in 0..n {
            let v = match self.heap.get_element_fast(a, i) {
                Some(v) => v,
                None => {
                    let key = self.heap.atom_from_index(i)?;
                    self.heap
                        .find_property(a, key)
                        .and_then(|(_, p)| p.value())
                        .unwrap_or(Value::UNDEFINED)
                }
            };
            self.push(v);
        }
        Ok(n)
    }

    /// SetFunctionName(f, key, prefix): `name` is the key (`[description]` for symbols),
    /// after `get ` / `set ` for accessors.
    fn set_function_name(&mut self, f: Value, key: Atom, prefix: u8) -> Result<(), Fault> {
        let Some(fo) = f.as_object() else {
            return Ok(());
        };
        let base = if key.is_symbol() {
            let sym = self.heap.atom_symbol(key).unwrap();
            match self.heap.symbol_description(sym).as_string() {
                Some(d) => crate::text::cat!("[", self.heap.str_to_string(d), "]"),
                None => String::new(),
            }
        } else {
            self.heap.atom_name(key)
        };
        let name = match prefix {
            1 => crate::text::cat!("get ", base),
            2 => crate::text::cat!("set ", base),
            _ => base,
        };
        let s = self.heap.alloc_str(&name)?;
        let d = PropertyDescriptor::data(Value::string(s), PropFlags::CONFIGURABLE);
        self.heap
            .define_own_property(fo, crate::heap::atoms::NAME, &d)?;
        Ok(())
    }

    /// IsConstructor: ordinary (non-async, non-generator) functions and class
    /// constructors.
    fn is_constructor(&self, fo: Obj) -> bool {
        use crate::heap::cells::flags::{OBJ_BUILTIN, OBJ_BUILTIN_CTOR};
        if self.heap.is_proxy(fo) {
            return self.proxy_is_constructor(fo);
        }
        if self.heap.object_class(fo) == ObjectClass::BoundFunction {
            let target = self.heap.object_slot(fo, 0).as_object().expect("target");
            return self.is_constructor(target);
        }
        // Builtins are constructors only if installed as one (D1310).
        let flags = self.heap.cell_flags(fo.offset());
        if flags & OBJ_BUILTIN != 0 {
            return flags & OBJ_BUILTIN_CTOR != 0;
        }
        self.heap.object_class(fo) == ObjectClass::Function && {
            let t = self.heap.closure_template(fo);
            let flags = u32::from(self.heap.arena().get::<u16>(t, FuncTemplate::FLAGS));
            flags & (func_flags::ASYNC | func_flags::GENERATOR) == 0
                && matches!(
                    func_flags::kind(flags),
                    func_flags::KIND_NORMAL
                        | func_flags::KIND_CLASS_CTOR
                        | func_flags::KIND_DERIVED_CTOR
                )
        }
    }

    fn is_derived_ctor(&self, fo: Obj) -> bool {
        self.heap.object_class(fo) == ObjectClass::Function && {
            let t = self.template_of(fo);
            func_flags::kind(u32::from(
                self.heap.arena().get::<u16>(t, FuncTemplate::FLAGS),
            )) == func_flags::KIND_DERIVED_CTOR
        }
    }

    /// QuickJS's `<name> is not a constructor` (just `not a constructor` without a
    /// `name` string; an empty name gives ` is not a constructor`, as in qjs).
    fn not_a_constructor_message(&self, f: Value) -> String {
        let name = f
            .as_object()
            .and_then(|fo| self.heap.get_own_property(fo, crate::heap::atoms::NAME))
            .and_then(|p| p.value())
            .and_then(|v| v.as_string())
            .map(|s| self.heap.str_to_string(s));
        match name {
            Some(n) => crate::text::cat!(n, " is not a constructor"),
            None => String::from("not a constructor"),
        }
    }

    /// `new f(args)`: `f a1 .. an` on the stack.
    fn construct(&mut self, argc: u32, next: u32) -> Step {
        self.construct_with(argc, next, None)
    }

    /// [[Construct]] of `f a1 .. an` with an explicit new.target (Reflect.construct);
    /// `None` means new.target is `f`.
    pub(super) fn construct_with(
        &mut self,
        mut argc: u32,
        next: u32,
        mut new_target: Option<Value>,
    ) -> Step {
        let base = self.sp - 1 - argc;
        let mut f = self.get(base);
        let Some(mut fo) = f.as_object() else {
            return type_error("not a function");
        };
        if !self.is_constructor(fo) {
            return type_error(&self.not_a_constructor_message(f));
        }
        // A bound function constructs its target (new.target is the target too, as
        // the spec has it when new.target is the bound function itself).
        while self.heap.object_class(fo) == ObjectClass::BoundFunction {
            let bound = f;
            // `f a1 .. an` gets a `this` slot for unwrap_bound, which construct fills
            // again below.
            self.ensure(self.sp + 1)?;
            for i in (0..argc).rev() {
                let v = self.get(base + 1 + i);
                self.set(base + 2 + i, v);
            }
            self.sp += 1;
            argc = self.unwrap_bound(base, argc, fo)?;
            for i in 0..argc {
                let v = self.get(base + 2 + i);
                self.set(base + 1 + i, v);
            }
            self.sp -= 1;
            f = self.get(base);
            fo = f.as_object().expect("bound target");
            // A new.target that is the bound function itself becomes its target.
            if new_target == Some(bound) {
                new_target = Some(f);
            }
        }
        if self.heap.is_proxy(fo) {
            let nt = new_target.unwrap_or(f);
            return self.construct_through(proxy::PROXY_CONSTRUCT, base, argc, nt, next);
        }
        let Some(this) = self.construct_this(fo, new_target)? else {
            // A `prototype` getter or a proxy's `get` trap: GetPrototypeFromConstructor
            // runs user code, in %OrdinaryConstructSlow.
            let nt = new_target.unwrap_or(f);
            return self.construct_through(proxy::ORDINARY_CONSTRUCT_SLOW, base, argc, nt, next);
        };
        if new_target.is_none() && self.try_construct_entry(fo, base, argc, next)? {
            return Ok(None);
        }
        // Make room for `this` below the arguments.
        self.ensure(self.sp + 1)?;
        self.insert_this(base, argc, this);
        self.construct_call(base, argc, new_target, next)
    }

    /// `new f(a1 .. an)` (`f a1 .. an` at `base`) with new.target f, by the `new` entry
    /// of a builtin constructor (0236): true when it answered.
    fn try_construct_entry(
        &mut self,
        fo: Obj,
        base: u32,
        argc: u32,
        next: u32,
    ) -> Result<bool, Fault> {
        if !self.is_class(fo, ObjectClass::Function) {
            return Ok(false);
        }
        let t = self.template_of(fo);
        let flags = u32::from(self.heap.arena().get::<u16>(t, FuncTemplate::FLAGS));
        Ok(flags & self.slow_call_flags & func_flags::FAST != 0
            && self.try_fast_new(t, base, argc, next)?)
    }

    /// The `this` that [[Construct]] of the function `fo` (not a proxy or bound
    /// function) passes: a new ordinary object whose prototype is new.target's
    /// `prototype` (`None`: reading it runs user code). A builtin constructor makes its
    /// own object (GetPrototypeFromConstructor in the builtin, which may run a
    /// `prototype` getter): `this` is not read (0039). A derived class constructor gets
    /// its `this` from `super()`. Both get undefined.
    fn construct_this(
        &mut self,
        fo: Obj,
        new_target: Option<Value>,
    ) -> Result<Option<Value>, Fault> {
        let builtin =
            self.heap.cell_flags(fo.offset()) & crate::heap::cells::flags::OBJ_BUILTIN != 0;
        if builtin || self.is_derived_ctor(fo) {
            return Ok(Some(Value::UNDEFINED));
        }
        let proto_source = new_target.and_then(|v| v.as_object()).unwrap_or(fo);
        let proto = match self
            .heap
            .lookup_property(proto_source, crate::heap::atoms::PROTOTYPE)
        {
            Lookup::Found(_, Property::Data { value, .. }) => value.as_object(),
            Lookup::Absent => None,
            Lookup::Found(..) | Lookup::Proxy(_) => return Ok(None),
        };
        let proto = proto.unwrap_or_else(|| intrinsic_obj(self.heap, Intrinsic::ObjectProto));
        Ok(Some(Value::object(self.heap.new_object(Some(proto))?)))
    }

    /// `rest index`: the arguments from `index` on, in a new Array.
    fn rest_array(&mut self) -> Result<Value, Fault> {
        let index = self.imm_u16(1);
        let argc = self.header_int(frame::ARGC);
        let mut vals = alloc::vec::Vec::new();
        for i in index..argc.max(index) {
            vals.push(self.get(self.bp + 2 + i));
        }
        let proto = intrinsic_obj(self.heap, Intrinsic::ArrayProto);
        Ok(Value::object(
            self.heap.new_array_from_values(Some(proto), &vals)?,
        ))
    }

    /// `f a1 .. an` at `base` becomes `f this a1 .. an` (room checked by the caller).
    fn insert_this(&mut self, base: u32, argc: u32, this: Value) {
        for i in (0..argc).rev() {
            let v = self.get(base + 1 + i);
            self.set(base + 2 + i, v);
        }
        self.set(base + 1, this);
        self.sp += 1;
    }

    /// [[Construct]] of the ordinary function at `stack[base]` with `this` and the
    /// arguments above it (`f this a1 .. an`) and new.target (`None`: f).
    pub(super) fn construct_call(
        &mut self,
        base: u32,
        argc: u32,
        new_target: Option<Value>,
        next: u32,
    ) -> Step {
        let f = self.get(base);
        // A bytecode frame is pushed flagged and with its new.target, as `call` would
        // push it otherwise (a `@fast` entry never takes a construct).
        if let Some(fo) = f.as_object()
            && self.heap.object_class(fo) == ObjectClass::Function
        {
            let t = self.template_of(fo);
            let flags = u32::from(self.heap.arena().get::<u16>(t, FuncTemplate::FLAGS));
            if flags & self.slow_call_flags & func_flags::NATIVE == 0 {
                if flags & func_flags::SLOPPY_THIS != 0 {
                    self.bind_sloppy_this(base + 1)?;
                }
                let th = hot::template_head(self.heap.arena().bytes(), t);
                self.ensure(base + 2 + argc.max(th.params) + HEADER + th.locals + th.stack)?;
                let nt = new_target.unwrap_or(f);
                let cont = Continuation::ConstructResult;
                self.push_frame_head(t, &th, base, argc, cont, next, Some(nt));
                return Ok(None);
            }
        }
        // A native frame: an explicit new.target is held in a handle across `call`,
        // whose safepoint may move it; without one it is the callee, read back from the
        // frame.
        let held = match new_target {
            Some(v) => Some(self.heap.new_handle(v)?),
            None => None,
        };
        let r = self.call(base, argc, Continuation::ConstructResult, next);
        let nt = held.map(|h| {
            let v = self.heap.get_handle(h);
            self.heap.drop_handle(h);
            v
        });
        let r = r?;
        let flags = self.header_int(frame::FLAGS) as i32 | frame::FLAG_CONSTRUCT;
        self.set(self.hp + frame::FLAGS, Value::int(flags));
        let f = nt.unwrap_or_else(|| self.get(self.bp));
        self.set(self.hp + frame::NEW_TARGET, f);
        Ok(r)
    }

    /// Whether `v instanceof f` needs the builtin InstanceofOperator: `f` has a
    /// Symbol.hasInstance other than Function.prototype's (a getter, a user method, a
    /// non-callable value), or its `prototype` is an accessor. The common case (a plain
    /// function inheriting the default) stays in [`Exec::instance_of`].
    fn instanceof_needs_builtin(&self, f: Value) -> bool {
        use crate::heap::atoms::{PROTOTYPE, SYM_HAS_INSTANCE};
        let Some(fo) = f.as_object() else {
            return false;
        };
        let default = self.heap.get_own_property(
            intrinsic_obj(self.heap, Intrinsic::FunctionProto),
            SYM_HAS_INSTANCE,
        );
        let found = self
            .heap
            .find_property(fo, SYM_HAS_INSTANCE)
            .map(|(_, p)| p);
        if found != default {
            return true;
        }
        // A bound function defers to its target's InstanceofOperator.
        if self.heap.object_class(fo) == ObjectClass::BoundFunction {
            return self.instanceof_needs_builtin(self.heap.object_slot(fo, 0));
        }
        matches!(
            self.heap.find_property(fo, PROTOTYPE),
            Some((_, Property::Accessor { .. }))
        )
    }

    /// OrdinaryHasInstance, for functions with the default Symbol.hasInstance. `None`
    /// (only if `can_defer`) when a proxy's getPrototypeOf trap must answer: the caller
    /// runs %InstanceofOperator.
    fn instance_of(&mut self, v: Value, f: Value, can_defer: bool) -> Result<Option<bool>, Fault> {
        let Some(fo) = f.as_object().filter(|&o| self.heap.is_callable(o)) else {
            return type_error("invalid 'instanceof' right operand");
        };
        let Some(mut o) = v.as_object() else {
            return Ok(Some(false));
        };
        if self.heap.object_class(fo) == ObjectClass::BoundFunction {
            let target = self.heap.object_slot(fo, 0);
            return self.instance_of(v, target, can_defer);
        }
        let proto = match self.heap.find_property(fo, crate::heap::atoms::PROTOTYPE) {
            Some((_, Property::Data { value, .. })) => value,
            None => Value::UNDEFINED,
            Some(_) => return type_error("not supported yet: getter for 'prototype'"),
        };
        let Some(p) = proto.as_object() else {
            return type_error("operand 'prototype' property is not an object");
        };
        loop {
            if can_defer && self.heap.is_proxy(o) {
                return Ok(None);
            }
            match self.heap.get_prototype_of(o) {
                Some(q) if q == p => return Ok(Some(true)),
                Some(q) => o = q,
                None => return Ok(Some(false)),
            }
        }
    }

    /// Whether `o` or an object on its prototype chain is a proxy.
    fn chain_has_proxy(&self, o: Obj) -> bool {
        let mut c = Some(o);
        while let Some(x) = c {
            if self.heap.is_proxy(x) {
                return true;
            }
            c = self.heap.get_prototype_of(x);
        }
        false
    }
}

// ------------------------------------------------------------------ host calls

/// Adds a pending host call and returns its id.
fn add_host_call(
    heap: &mut Heap,
    fn_id: u32,
    args: u32,
    this: Value,
    promise: Value,
) -> Result<u32, HeapError> {
    let i = state::interp(heap);
    let id = heap.arena().get(i, Interp::NEXT_CALL_ID);
    let c = heap.alloc_cell(CellKind::HostCall, HostCall::LAYOUT.min_size)?;
    let table = state::host_calls(heap);
    let len = heap.value_array_len(table);
    let table2 = heap.value_array_reserve(table, len + 1)?;
    let a = heap.arena_mut();
    a.set(c, HostCall::ID, id);
    a.set(c, HostCall::FN, fn_id);
    a.set(c, HostCall::ARGS, Ref(args));
    a.set(c, HostCall::THIS, this);
    a.set(c, HostCall::PROMISE, promise);
    a.set(0, Roots::HOST_CALLS, Ref(table2));
    a.set(i, Interp::NEXT_CALL_ID, id.wrapping_add(1).max(1));
    heap.value_array_set(table2, len, Value::internal(c));
    heap.arena_mut().set(table2, ValueArray::LEN, len + 1);
    Ok(id)
}

/// Adds an async host call with the embedder's `id` and `promise` (an open call of a
/// dump, 0171, D2890). The id counter stays as it is, or moves past `id`.
pub(crate) fn add_loaded_host_call(
    heap: &mut Heap,
    id: u32,
    fn_id: u32,
    args: u32,
    promise: Value,
) -> Result<(), HeapError> {
    let i = state::interp(heap);
    let next = heap.arena().get(i, Interp::NEXT_CALL_ID);
    heap.arena_mut().set(i, Interp::NEXT_CALL_ID, id);
    let r = add_host_call(heap, fn_id, args, Value::UNDEFINED, promise);
    let i = state::interp(heap);
    let next = if next > id {
        next
    } else {
        id.wrapping_add(1).max(1)
    };
    heap.arena_mut().set(i, Interp::NEXT_CALL_ID, next);
    r.map(|_| ())
}

/// Whether an async host call is pending (its promise unsettled), D1374.
pub(crate) fn pending_async_calls(heap: &Heap) -> bool {
    let table = state::host_calls(heap);
    (0..heap.value_array_len(table)).any(|i| {
        let c = heap.value_array_get(table, i).as_internal().unwrap();
        heap.arena()
            .get::<Value>(c, HostCall::PROMISE)
            .as_object()
            .is_some()
    })
}

/// The `HostCall` cell with `id`, if pending.
pub(crate) fn host_call_cell(heap: &Heap, id: u32) -> Option<u32> {
    let table = state::host_calls(heap);
    (0..heap.value_array_len(table))
        .map(|i| heap.value_array_get(table, i).as_internal().unwrap())
        .find(|&c| heap.arena().get(c, HostCall::ID) == id)
}

/// Removes a pending host call.
pub(crate) fn remove_host_call(heap: &mut Heap, id: u32) {
    let table = state::host_calls(heap);
    let len = heap.value_array_len(table);
    let Some(pos) = (0..len).find(|&i| {
        let c = heap.value_array_get(table, i).as_internal().unwrap();
        heap.arena().get(c, HostCall::ID) == id
    }) else {
        return;
    };
    for i in pos + 1..len {
        let v = heap.value_array_get(table, i);
        heap.value_array_set(table, i - 1, v);
    }
    heap.value_array_set(table, len - 1, Value::ZERO);
    heap.arena_mut().set(table, ValueArray::LEN, len - 1);
}

/// The parked call's continuation and whether it was the entry callee.
pub(crate) fn parked(heap: &Heap) -> (Continuation, bool) {
    let f = heap.arena().get(state::interp(heap), Interp::FLAGS);
    (
        Continuation::from_flags((f & !PARK_ENTRY) as i32),
        f & PARK_ENTRY != 0,
    )
}

/// The ReferenceError for a global that doesn't exist. The slim build names the globals
/// of the families it leaves out (D2411), and builds without `embed` those of the
/// embed-only builtins (D3175), so that a script (or the model that wrote it) sees why,
/// instead of a typo-like message.
#[cold]
#[inline(never)]
fn undefined_global(heap: &Heap, key: Atom) -> Fault {
    let name = heap.atom_name(key);
    if (!crate::FULL && realm::ABSENT_GLOBALS.contains(&name.as_str()))
        || (!crate::EMBED && realm::EMBED_GLOBALS.contains(&name.as_str()))
    {
        return Fault::error(
            ErrorKind::ReferenceError,
            crate::text::cat!("'", name, "' is not available in this build"),
        );
    }
    Fault::error(
        ErrorKind::ReferenceError,
        crate::text::cat!("'", name, "' is not defined"),
    )
}

/// A closure of template `t` (its [[Prototype]] by the ASYNC/GENERATOR bits, for every
/// kind, D1420; a `super` method gets its home object slot after the captures), with
/// its capture slots still to fill.
pub(crate) fn alloc_closure_of(heap: &mut Heap, t: Ref) -> Result<Obj, Fault> {
    let a = heap.arena();
    let ncap = a.get(t.0, FuncTemplate::CAPTURES) as u32;
    let flags = u32::from(a.get::<u16>(t.0, FuncTemplate::FLAGS));
    let proto = intrinsic_obj(
        heap,
        match (
            flags & func_flags::ASYNC != 0,
            flags & func_flags::GENERATOR != 0,
        ) {
            (false, false) => Intrinsic::FunctionProto,
            (true, false) => Intrinsic::AsyncFunctionProto,
            (false, true) => Intrinsic::GeneratorFunctionProto,
            (true, true) => Intrinsic::AsyncGeneratorFunctionProto,
        },
    );
    let home = u32::from(flags & func_flags::HOME != 0);
    Ok(heap.alloc_closure(Some(proto), t.0, ncap + home)?)
}

/// `length`, `name` and (for ordinary functions) `prototype` of a new closure.
pub(crate) fn function_properties(heap: &mut Heap, f: Obj, t: u32) -> Result<(), Fault> {
    use crate::heap::atoms;
    let a = heap.arena();
    let length = a.get(t, FuncTemplate::LENGTH);
    let name = Atom::from_id(a.get(t, FuncTemplate::NAME));
    let flags = u32::from(a.get::<u16>(t, FuncTemplate::FLAGS));
    if flags & func_flags::PRIVILEGED != 0 {
        // The prelude: `%Install*` names the builtins user code can see (D1433).
        return Ok(());
    }
    // `length` and `name` come from the template until changed (D2445); stored only
    // for a name that is not a string atom (an integer key's).
    if !heap.set_lazy_fn_props(f) {
        let ro = PropFlags::CONFIGURABLE;
        let d = PropertyDescriptor::data(Value::int(length as i32), ro);
        heap.define_own_property(f, atoms::LENGTH, &d)?;
        let name = heap.atom_string(name)?;
        let d = PropertyDescriptor::data(Value::string(name), ro);
        heap.define_own_property(f, atoms::NAME, &d)?;
    }
    if func_flags::kind(flags) == func_flags::KIND_NORMAL
        && flags & (func_flags::ASYNC | func_flags::GENERATOR) == 0
    {
        let op = intrinsic_obj(heap, Intrinsic::ObjectProto);
        let proto = heap.new_object(Some(op))?;
        realm::define_hidden(heap, proto, atoms::CONSTRUCTOR, Value::object(f))?;
        let d = PropertyDescriptor::data(Value::object(proto), PropFlags::WRITABLE);
        heap.define_own_property(f, atoms::PROTOTYPE, &d)?;
    } else if flags & func_flags::GENERATOR != 0 {
        // A generator function's `prototype` is the prototype of its generator
        // objects (no `constructor`), D1375; async ones inherit from
        // %AsyncGeneratorPrototype% (D1420).
        let gp = intrinsic_obj(
            heap,
            if flags & func_flags::ASYNC != 0 {
                Intrinsic::AsyncGeneratorProto
            } else {
                Intrinsic::GeneratorProto
            },
        );
        let proto = heap.new_object(Some(gp))?;
        let d = PropertyDescriptor::data(Value::object(proto), PropFlags::WRITABLE);
        heap.define_own_property(f, atoms::PROTOTYPE, &d)?;
    }
    Ok(())
}
