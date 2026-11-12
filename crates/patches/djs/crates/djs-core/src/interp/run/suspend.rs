//! Suspendable frames: generators and async functions (D1375, docs/compiler.md 7.7).
//!
//! A suspension copies the current frame's slice of the value stack, `stack[bp..sp)`
//! (callee, `this`, arguments, header, locals, operand stack), into an internal
//! `ValueArray` held by a Generator object (`ObjectClass::Generator`), with the pc to
//! resume at and `hp - bp`. A frame holds no absolute stack positions except its
//! header's links to its caller, so resuming copies the slice back at the top of the
//! stack and rewrites those links for the new caller (the resumer, with `PushResult`).
//!
//! Generator object slots:
//!
//! ```text
//! 0  state    int32: START (suspended before the body), SUSPENDED (at a yield or
//!             await), EXECUTING, COMPLETED
//! 1  frame    internal ValueArray while suspended, else undefined
//! 2  pc       int32: where to resume
//! 3  hp       int32: hp - bp of the frame
//! 4  promise  the result promise of an async function; undefined for generators
//! 5  kind     int32: GENERATOR, ASYNC or ASYNC_GENERATOR
//! 6  queue    an async generator's requests (internal ValueArray of Records), D1421
//! ```
//!
//! An `await` suspends with state AWAITING (resumed by a promise job, pushing the
//! value); a `yield` with SUSPENDED (resumed by a request, pushing `value isReturn`).
//! Async generators use both: their resumer (builtins/asyncgen.ts) tells them apart by
//! the state after the frame hands control back.
//!
//! `await` and the end of an async function return the result promise, so whoever ran
//! the frame (the caller the first time, the job that resumed it later) gets it. Where
//! that needs no user code (D3130), the op subscribes or settles itself; else it replaces
//! the frame with a call of a builtin (`%AsyncAwait`, `%AsyncFunctionReturn`) with the
//! frame's own continuation: a tail call.

use super::*;

pub(crate) const START: i32 = 0;
pub(crate) const SUSPENDED: i32 = 1;
pub(crate) const EXECUTING: i32 = 2;
pub(crate) const COMPLETED: i32 = 3;
pub(crate) const AWAITING: i32 = 4;
/// Suspended at a `yield_star`: the resumer passes the value on unwrapped. (5 is an
/// async generator's awaiting-return, kept by builtins/asyncgen.ts.)
pub(crate) const SUSPENDED_RAW: i32 = 6;

pub(crate) const GENERATOR: i32 = 0;
pub(crate) const ASYNC: i32 = 1;
pub(crate) const ASYNC_GENERATOR: i32 = 2;

const STATE: u32 = 0;
const FRAME: u32 = 1;
const PC: u32 = 2;
const HP: u32 = 3;
const PROMISE: u32 = 4;
const KIND: u32 = 5;
pub(crate) const QUEUE: u32 = 6;

const GENERATOR_NEXT: u16 = intrinsics::id("GeneratorPrototypeNext");
const ASYNC_AWAIT: u16 = intrinsics::id("AsyncAwait");
const ASYNC_FUNCTION_RETURN: u16 = intrinsics::id("AsyncFunctionReturn");

/// `v` as a Generator object of `kind`.
pub(crate) fn as_generator(heap: &Heap, v: Value, kind: i32) -> Option<Obj> {
    v.as_object().filter(|&o| {
        heap.object_class(o) == ObjectClass::Generator
            && heap.object_slot(o, KIND).as_int() == Some(kind)
    })
}

/// `v` as a Generator object of any of `kinds`.
fn as_generator_of(heap: &Heap, v: Value, kinds: &[i32]) -> Option<Obj> {
    kinds.iter().find_map(|&k| as_generator(heap, v, k))
}

pub(crate) fn state_of(heap: &Heap, g: Obj) -> i32 {
    heap.object_slot(g, STATE).as_int().unwrap_or(COMPLETED)
}

pub(crate) fn set_state(heap: &mut Heap, g: Obj, s: i32) {
    heap.set_object_slot(g, STATE, Value::int(s));
    if s == COMPLETED {
        // A completed activation keeps nothing alive.
        heap.set_object_slot(g, FRAME, Value::UNDEFINED);
    }
}

pub(crate) fn promise_of(heap: &Heap, a: Obj) -> Value {
    heap.object_slot(a, PROMISE)
}

fn alloc(heap: &mut Heap, proto: Option<Obj>, kind: i32) -> Result<Obj, HeapError> {
    let g = heap.alloc_object(
        ObjectClass::Generator,
        proto,
        ObjectClass::Generator.slots(),
    )?;
    for (slot, v) in [
        (STATE, Value::int(EXECUTING)),
        (FRAME, Value::UNDEFINED),
        (PC, Value::int(0)),
        (HP, Value::int(0)),
        (PROMISE, Value::UNDEFINED),
        (KIND, Value::int(kind)),
        (QUEUE, Value::UNDEFINED),
    ] {
        heap.set_object_slot(g, slot, v);
    }
    Ok(g)
}

/// Appends a request Record to async generator `g`'s queue (D1421).
pub(crate) fn enqueue_request(heap: &mut Heap, g: Obj, request: Value) -> Result<(), HeapError> {
    let q = heap.object_slot(g, QUEUE).as_internal().unwrap_or(0);
    let len = if q == 0 { 0 } else { heap.value_array_len(q) };
    let q2 = heap.value_array_reserve(q, len + 1)?;
    heap.value_array_set(q2, len, request);
    heap.arena_mut().set(q2, ValueArray::LEN, len + 1);
    heap.set_object_slot(g, QUEUE, Value::internal(q2));
    Ok(())
}

/// The oldest request of `g`, or undefined.
pub(crate) fn first_request(heap: &Heap, g: Obj) -> Value {
    match heap.object_slot(g, QUEUE).as_internal() {
        Some(q) if heap.value_array_len(q) > 0 => heap.value_array_get(q, 0),
        _ => Value::UNDEFINED,
    }
}

/// Removes and returns the oldest request of `g` (queues stay short: a shift).
pub(crate) fn dequeue_request(heap: &mut Heap, g: Obj) -> Value {
    let Some(q) = heap.object_slot(g, QUEUE).as_internal() else {
        return Value::UNDEFINED;
    };
    let len = heap.value_array_len(q);
    if len == 0 {
        return Value::UNDEFINED;
    }
    let first = heap.value_array_get(q, 0);
    for i in 1..len {
        let v = heap.value_array_get(q, i);
        heap.value_array_set(q, i - 1, v);
    }
    heap.value_array_set(q, len - 1, Value::UNDEFINED);
    heap.arena_mut().set(q, ValueArray::LEN, len - 1);
    first
}

/// A suspended activation as the dumper reads it (0172).
#[cfg(feature = "dump")]
pub(crate) struct Suspended {
    pub kind: i32,
    pub state: i32,
    /// The saved frame (`suspend_frame`), empty unless suspended.
    pub frame: alloc::vec::Vec<Value>,
    pub pc: u32,
    /// The header's position in `frame`.
    pub hp: u32,
    /// An async function's result promise.
    pub promise: Value,
    /// An async generator's request Records.
    pub queue: alloc::vec::Vec<Value>,
}

/// Reads activation `g`.
#[cfg(feature = "dump")]
pub(crate) fn read_activation(heap: &Heap, g: Obj) -> Suspended {
    let int = |slot| heap.object_slot(g, slot).as_int().unwrap_or(0);
    let list = |v: Value| -> alloc::vec::Vec<Value> {
        match v.as_internal() {
            Some(a) => (0..heap.value_array_len(a))
                .map(|i| heap.value_array_get(a, i))
                .collect(),
            None => alloc::vec::Vec::new(),
        }
    };
    Suspended {
        kind: int(KIND),
        state: int(STATE),
        frame: list(heap.object_slot(g, FRAME)),
        pc: int(PC) as u32,
        hp: int(HP) as u32,
        promise: heap.object_slot(g, PROMISE),
        queue: list(heap.object_slot(g, QUEUE)),
    }
}

pub(super) fn plain_generator(heap: &Heap, it: Value, f: Value) -> Option<Obj> {
    let g = as_generator(heap, it, GENERATOR)?;
    let ready = matches!(state_of(heap, g), START | SUSPENDED);
    (ready && f == realm::installed_intrinsic(heap, GENERATOR_NEXT)).then_some(g)
}

impl Exec<'_> {
    fn gen_step_iterator(&mut self, at: u32) -> (bool, Option<Obj>) {
        self.pc = at;
        let loc = self.heap.arena().read::<u8>(self.code + at) == Op::IterNextLoc as u8;
        let it = if loc {
            self.get(self.lp + self.imm_u16(1))
        } else {
            self.top(1)
        };
        (loc, as_generator(self.heap, it, GENERATOR))
    }

    pub(super) fn gen_step_return(&mut self, v: Value) -> Step {
        let at = self.pc;
        let (loc, g) = self.gen_step_iterator(at);
        let (r, jump) = match g.map(|g| (g, state_of(self.heap, g))) {
            Some((_, SUSPENDED)) => (v, true),
            Some((g, EXECUTING)) => {
                set_state(self.heap, g, COMPLETED);
                let proto = intrinsic_obj(self.heap, Intrinsic::ObjectProto);
                let keys = [crate::heap::atoms::VALUE, crate::heap::atoms::DONE];
                let o = self
                    .heap
                    .new_object_from_entries(Some(proto), &keys, &[v, Value::TRUE])?;
                (Value::object(o), false)
            }
            _ => (v, false),
        };
        let op = if loc { Op::IterNextLoc } else { Op::IterNext };
        let next = at + OPS[op as usize].size as u32;
        self.pc = if jump {
            (next as i64 + self.imm_i32(if loc { 3 } else { 1 }) as i64) as u32
        } else {
            next
        };
        if loc {
            self.push(r);
        } else {
            self.sp -= 1;
            self.set(self.sp - 1, r);
        }
        self.safepoint();
        Ok(None)
    }

    pub(super) fn gen_step_abort(&mut self, at: u32) {
        if let (_, Some(g)) = self.gen_step_iterator(at)
            && state_of(self.heap, g) == EXECUTING
        {
            set_state(self.heap, g, COMPLETED);
        }
    }

    /// The Generator object in local `loc` of the current frame.
    fn local_generator(&self, loc: u32, kinds: &[i32]) -> Result<Obj, Fault> {
        as_generator_of(self.heap, self.get(self.lp + loc), kinds).ok_or_else(|| {
            Fault::error(
                ErrorKind::InternalError,
                String::from("bad bytecode: no generator state"),
            )
        })
    }

    /// Saves the frame `stack[bp..sp)` into `g`, to resume at `pc`.
    fn suspend_frame(&mut self, g: Obj, state: i32, pc: u32) -> Result<(), Fault> {
        let len = self.sp - self.bp;
        let arr = match self.heap.object_slot(g, FRAME).as_internal() {
            Some(a) if self.heap.value_array_capacity(a) >= len => a,
            _ => self.heap.alloc_value_array(len)?,
        };
        for i in 0..len {
            let v = self.get(self.bp + i);
            self.heap.value_array_set(arr, i, v);
        }
        self.heap.arena_mut().set(arr, ValueArray::LEN, len);
        self.fuel -= (len / 8) as i64;
        for (slot, v) in [
            (STATE, Value::int(state)),
            (FRAME, Value::internal(arr)),
            (PC, Value::int(pc as i32)),
            (HP, Value::int((self.hp - self.bp) as i32)),
        ] {
            self.heap.set_object_slot(g, slot, v);
        }
        Ok(())
    }

    /// Replaces the current frame by a call of the installed builtin `id` with `args`,
    /// returning to the frame's own continuation.
    fn tail_call_builtin(&mut self, id: u16, args: [Value; 2]) -> Step {
        let f = realm::installed_intrinsic(self.heap, id);
        let cont = Continuation::from_flags(self.header_int(frame::FLAGS) as i32);
        let ret_pc = self.header_int(frame::RET_PC);
        let base = self.bp;
        if !cont.is_bottom() {
            let (prev_bp, prev_hp) = (
                self.header_int(frame::PREV_BP),
                self.header_int(frame::PREV_HP),
            );
            self.bp = prev_bp;
            self.hp = prev_hp;
        }
        self.sp = base;
        self.push(f);
        self.push(Value::UNDEFINED);
        for a in args {
            self.push(a);
        }
        self.call(base, 2, cont, ret_pc)
    }

    /// `generator_start loc`.
    pub(super) fn generator_start(&mut self, next: u32) -> Step {
        let loc = self.imm_u16(1);
        // An async generator if the function is also ASYNC (D1421). The prototype: the
        // function's own `prototype` if it is an object (a data property of generator
        // functions), else %GeneratorPrototype% / %AsyncGeneratorPrototype%.
        let callee = self.get(self.bp).as_object().expect("frame callee");
        let flags = u32::from(
            self.heap
                .arena()
                .get::<u16>(self.template, FuncTemplate::FLAGS),
        );
        let (kind, fallback) = if flags & func_flags::ASYNC != 0 {
            (ASYNC_GENERATOR, Intrinsic::AsyncGeneratorProto)
        } else {
            (GENERATOR, Intrinsic::GeneratorProto)
        };
        let proto = self
            .heap
            .get_own_property(callee, crate::heap::atoms::PROTOTYPE)
            .and_then(|p| p.value())
            .and_then(|v| v.as_object())
            .unwrap_or_else(|| intrinsic_obj(self.heap, fallback));
        let g = alloc(self.heap, Some(proto), kind)?;
        self.set(self.lp + loc, Value::object(g));
        self.suspend_frame(g, START, next)?;
        self.do_return(Value::object(g))
    }

    /// `yield loc`.
    pub(super) fn yield_value(&mut self, next: u32, raw: bool) -> Step {
        let kinds: &[i32] = if raw {
            &[GENERATOR]
        } else {
            &[GENERATOR, ASYNC_GENERATOR]
        };
        let g = self.local_generator(self.imm_u16(1), kinds)?;
        let v = self.pop();
        let state = if raw { SUSPENDED_RAW } else { SUSPENDED };
        self.suspend_frame(g, state, next)?;
        self.do_return(v)
    }

    /// `async_start loc`: the result promise. The state object comes at the first `await`
    /// (`async_state`, D3157): until then local `loc` holds the promise, so a call that
    /// never awaits allocates just that.
    pub(super) fn async_start(&mut self) -> Result<(), Fault> {
        let loc = self.imm_u16(1);
        let proto = intrinsic_obj(self.heap, Intrinsic::PromiseProto);
        let p = promise::alloc(self.heap, Some(proto))?;
        self.set(self.lp + loc, Value::object(p));
        Ok(())
    }

    /// The async state in local `loc`, made now if the local still holds the result
    /// promise (D3157); an async generator's is made by `generator_start`.
    fn async_state(&mut self, loc: u32) -> Result<Obj, Fault> {
        if let Some(p) = promise::as_promise(self.heap, self.get(self.lp + loc)) {
            let a = alloc(self.heap, None, ASYNC)?;
            self.heap.set_object_slot(a, PROMISE, Value::object(p));
            self.set(self.lp + loc, Value::object(a));
            return Ok(a);
        }
        self.local_generator(loc, &[ASYNC, ASYNC_GENERATOR])
    }

    /// `await loc`. Await ( v ) runs no user code when `v` is a Promise of %Promise% or a
    /// value that fulfills a new one without a `then` call (D3130): then the frame
    /// subscribes here, as `%AsyncAwait` would, without the intermediate promise (which
    /// nothing else could reach), and returns as `%AsyncAwait` returns.
    pub(super) fn await_value(&mut self, next: u32) -> Step {
        let a = self.async_state(self.imm_u16(1))?;
        let v = self.pop();
        let p = promise::plain_promise(self.heap, v);
        if p.is_none() && !promise::plain_fulfillment(self.heap, v) {
            self.suspend_frame(a, AWAITING, next)?;
            return self.tail_call_builtin(ASYNC_AWAIT, [Value::object(a), v]);
        }
        self.suspend_frame(a, AWAITING, next)?;
        let h = Value::object(a);
        let r = match p {
            Some(p) => promise::perform_then(self.heap, p, h, h, Value::UNDEFINED),
            None => {
                let job = realm::installed_intrinsic(self.heap, promise::PROMISE_REACTION_JOB);
                promise::enqueue(self.heap, job, [Value::UNDEFINED, h, v, Value::TRUE])
            }
        };
        if let Err(e) = r {
            // Out of memory: the frame goes on as if it had not suspended.
            self.heap.set_object_slot(a, STATE, Value::int(EXECUTING));
            self.heap.set_object_slot(a, FRAME, Value::UNDEFINED);
            return Err(e.into());
        }
        // An async generator's resumer (AsyncGeneratorStep) only looks at its state.
        let result = if self.heap.object_slot(a, KIND).as_int() == Some(ASYNC) {
            promise_of(self.heap, a)
        } else {
            Value::UNDEFINED
        };
        self.do_return(result)
    }

    /// `async_return loc` / `async_reject loc`: settles the result promise and returns it.
    /// A value whose resolution may call user code (a `then`) goes to
    /// `%AsyncFunctionReturn` instead (D3130).
    pub(super) fn async_finish(&mut self, reject: bool) -> Step {
        let loc = self.imm_u16(1);
        let v = self.pop();
        if !reject && !promise::plain_fulfillment(self.heap, v) {
            let a = self.async_state(loc)?;
            return self.tail_call_builtin(ASYNC_FUNCTION_RETURN, [Value::object(a), v]);
        }
        // The state, if the function awaited (D3157), else the promise itself.
        let p = match promise::as_promise(self.heap, self.get(self.lp + loc)) {
            Some(p) => p,
            None => {
                let a = self.local_generator(loc, &[ASYNC])?;
                set_state(self.heap, a, COMPLETED);
                promise_of(self.heap, a)
                    .as_object()
                    .expect("an async function's promise")
            }
        };
        promise::settle(self.heap, p, !reject, v)?;
        self.do_return(Value::object(p))
    }

    /// `%GeneratorResume(g, value, mode)`: the arguments are popped (`sp` = their base);
    /// pushes the saved frame of `g` with `cont` back to `next` (`PushResult`, or `Job`
    /// for a job's bottom frame).
    pub(super) fn generator_resume(
        &mut self,
        g: Value,
        value: Value,
        mode: Value,
        next: u32,
        cont: Continuation,
    ) -> Step {
        let g = g
            .as_object()
            .filter(|&o| self.heap.object_class(o) == ObjectClass::Generator);
        let (Some(g), Some(mode)) = (g, mode.as_int()) else {
            return intrinsics::bad_argument(
                const { crate::bytecode::intrinsics::const_id("GeneratorResume") },
            );
        };
        let state = state_of(self.heap, g);
        let saved = self.heap.object_slot(g, FRAME).as_internal();
        let (Some(arr), START | SUSPENDED | AWAITING | SUSPENDED_RAW) = (saved, state) else {
            return intrinsics::bad_argument(
                const { crate::bytecode::intrinsics::const_id("GeneratorResume") },
            );
        };
        let pc = self.heap.object_slot(g, PC).as_int().unwrap_or(0) as u32;
        let f = self
            .heap
            .value_array_get(arr, 0)
            .as_object()
            .expect("callee");
        let t = self.template_of(f);
        if state != START && mode == 1 {
            // Thrown at the suspension point: without a handler there, it leaves the
            // activation at once, as if thrown by this intrinsic.
            let caught = HandlerTable::new(crate::bytecode::template::handlers(self.heap, t))
                .ok()
                .and_then(|h| h.find(pc - 1))
                .is_some();
            if !caught {
                return Err(Fault::throw(value));
            }
        }
        let hp_off = self.heap.object_slot(g, HP).as_int().unwrap_or(0) as u32;
        let len = self.heap.value_array_len(arr);
        let base = self.sp;
        // Room for the frame, its operand stack and the resume values.
        let top = base
            + hp_off
            + HEADER
            + self.heap.arena().get(t, FuncTemplate::LOCALS) as u32
            + self.heap.arena().get(t, FuncTemplate::STACK) as u32;
        self.ensure(top.max(base + len + 2))?;
        for i in 0..len {
            let v = self.heap.value_array_get(arr, i);
            self.set(base + i, v);
        }
        self.fuel -= (len / 8) as i64;
        let hp = base + hp_off;
        let bottom = cont.is_bottom();
        let header = [
            (
                frame::PREV_BP,
                Value::int(if bottom { 0 } else { self.bp as i32 }),
            ),
            (
                frame::PREV_HP,
                Value::int(if bottom { 0 } else { self.hp as i32 }),
            ),
            (frame::RET_PC, Value::int(next as i32)),
            (frame::FLAGS, Value::int(cont.to_flags())),
        ];
        for (k, v) in header {
            self.set(hp + k, v);
        }
        self.heap.set_object_slot(g, STATE, Value::int(EXECUTING));
        self.bp = base;
        self.hp = hp;
        self.sp = base + len;
        self.pc = pc;
        self.enter_template(t);
        match (state, mode) {
            (START, _) => {}
            (_, 1) => {
                let st = self.throw_value(value, pc - 1);
                if st.is_none() {
                    self.safepoint();
                }
                return Ok(st);
            }
            (AWAITING, _) => self.push(value),
            _ => {
                self.push(value);
                self.push(Value::bool(mode == 2));
            }
        }
        self.safepoint();
        Ok(None)
    }
}
