//! Promises and the job queue in Rust (D1370-D1373): the parts that never run user code.
//! `builtins/promise.ts` has the rest (resolving functions, thenables, `then`, the
//! combinators) and calls these through leaf intrinsics.
//!
//! A Promise object (`ObjectClass::Promise`) has four slots:
//!
//! ```text
//! 0  state      int32: PENDING, FULFILLED, REJECTED
//! 1  result     the value or reason
//! 2  reactions  internal ValueArray of [capability, onFulfilled, onRejected] triples
//!               while pending, else undefined
//! 3  flags      int32: FLAG_HANDLED ([[PromiseIsHandled]]), FLAG_RESOLVED
//! ```
//!
//! A *capability* is `undefined` (nothing to resolve: `await`), a Promise (an internal
//! capability of `%Promise%`, resolved directly, with `FLAG_RESOLVED` as its
//! `[[AlreadyResolved]]`), or a Record `[promise, resolve, reject]` (any other
//! constructor). Reactions run as jobs of the installed `%PromiseReactionJob`.
//!
//! The job queue is `Roots::JOBS`, a `ValueArray` of `JOB_SIZE`-value entries `[f, a0,
//! a1, a2, a3]` from `Interp::JOB_HEAD` to its `LEN`: a job calls `f(a0, a1, a2, a3)`
//! with `this` undefined in a frame of its own.

use alloc::vec::Vec;

use super::intrinsics::{PROMISE_CONSTRUCTOR, id, new_builtin_closure};
use crate::builtins::fast::Fast;
use crate::heap::cells::{Interp, Roots, ValueArray};
use crate::heap::{Heap, HeapError, Lookup, ObjectClass, Property, Ref, atoms};
use crate::interp::realm;
use crate::interp::state;
use crate::value::{Obj, Value};

pub(crate) const PENDING: i32 = 0;
pub(crate) const FULFILLED: i32 = 1;
pub(crate) const REJECTED: i32 = 2;

/// `[[PromiseIsHandled]]`.
pub(crate) const FLAG_HANDLED: i32 = 1;
/// `[[AlreadyResolved]]` of an internal capability.
pub(crate) const FLAG_RESOLVED: i32 = 2;

const STATE: u32 = 0;
const RESULT: u32 = 1;
const REACTIONS: u32 = 2;
const FLAGS: u32 = 3;

/// Values per job entry: the function and four arguments.
pub(crate) const JOB_SIZE: u32 = 5;

pub(super) const PROMISE_REACTION_JOB: u16 = id("PromiseReactionJob");
const HOST_CALL_SETTLE: u16 = id("HostCallSettle");
const PROMISE_STATIC_RESOLVE: u16 = id("PromiseStaticResolve");
const PROMISE_PROTOTYPE_THEN: u16 = id("PromisePrototypeThen");
const PROMISE_SPECIES: u16 = id("PromiseSpecies");
const PROMISE_ALL_RESOLVE_ELEMENT: u16 = id("PromiseAllResolveElement");
const PROMISE_ALL_SETTLED_RESOLVE_ELEMENT: u16 = id("PromiseAllSettledResolveElement");
const PROMISE_ALL_SETTLED_REJECT_ELEMENT: u16 = id("PromiseAllSettledRejectElement");
const PROMISE_ANY_REJECT_ELEMENT: u16 = id("PromiseAnyRejectElement");

/// A new pending promise.
pub(crate) fn alloc(heap: &mut Heap, proto: Option<Obj>) -> Result<Obj, HeapError> {
    let p = heap.alloc_object(ObjectClass::Promise, proto, ObjectClass::Promise.slots())?;
    heap.set_object_slot(p, STATE, Value::int(PENDING));
    heap.set_object_slot(p, RESULT, Value::UNDEFINED);
    heap.set_object_slot(p, REACTIONS, Value::UNDEFINED);
    heap.set_object_slot(p, FLAGS, Value::int(0));
    Ok(p)
}

/// `v` as a Promise object.
pub(crate) fn as_promise(heap: &Heap, v: Value) -> Option<Obj> {
    v.as_object()
        .filter(|&o| heap.object_class(o) == ObjectClass::Promise)
}

pub(crate) fn state_of(heap: &Heap, p: Obj) -> i32 {
    heap.object_slot(p, STATE).as_int().unwrap_or(PENDING)
}

pub(crate) fn result_of(heap: &Heap, p: Obj) -> Value {
    heap.object_slot(p, RESULT)
}

pub(crate) fn flags(heap: &Heap, p: Obj) -> i32 {
    heap.object_slot(p, FLAGS).as_int().unwrap_or(0)
}

pub(crate) fn set_flags(heap: &mut Heap, p: Obj, f: i32) {
    heap.set_object_slot(p, FLAGS, Value::int(f));
}

/// Whether ResolvePromise ( promise, v ) fulfills with `v` without running user code
/// (D3130): `v` is no object, or its `then` is absent or a data property that is not
/// callable, found without meeting a Proxy or a getter. (The caller's promise is never
/// `v`: no self-resolution.)
pub(crate) fn plain_fulfillment(heap: &Heap, v: Value) -> bool {
    let Some(o) = v.as_object() else {
        return true;
    };
    match plain_get(heap, o, atoms::THEN) {
        Some(Some(f)) => !f.as_object().is_some_and(|f| heap.is_callable(f)),
        Some(None) => true,
        None => false,
    }
}

/// [[Get]] of `key` from `o` if no user code runs: `Some(None)` when absent, `None` for a
/// getter or a Proxy on the way.
#[inline(never)]
fn plain_get(heap: &Heap, o: Obj, key: crate::heap::Atom) -> Option<Option<Value>> {
    match heap.lookup_property(o, key) {
        Lookup::Absent => Some(None),
        Lookup::Found(_, Property::Data { value, .. }) => Some(Some(value)),
        _ => None,
    }
}

/// PromiseResolve ( %Promise%, v ) when it is `v` itself without running user code
/// (D3130): a Promise whose `constructor`, a data property found without a Proxy or a
/// getter on the way, is %Promise%.
pub(crate) fn plain_promise(heap: &Heap, v: Value) -> Option<Obj> {
    let p = as_promise(heap, v)?;
    let ctor = realm::installed_intrinsic(heap, PROMISE_CONSTRUCTOR);
    (plain_get(heap, p, atoms::CONSTRUCTOR) == Some(Some(ctor))).then_some(p)
}

/// Whether Invoke ( p, "then" ) on a Promise `p` whose lookups start at `o` (`p`, or
/// %Promise.prototype% for a new one) calls the original `then`, whose
/// SpeciesConstructor is %Promise%, all without running user code: `then` and
/// `constructor` are data properties found without a Proxy or a getter, and %Promise%'s
/// own @@species is the original getter.
fn plain_then(heap: &Heap, o: Obj) -> bool {
    let then = realm::installed_intrinsic(heap, PROMISE_PROTOTYPE_THEN);
    plain_species(heap, o)
        && (frozen_plain(heap, o) || plain_get(heap, o, atoms::THEN) == Some(Some(then)))
}

/// Whether the builtins are frozen (D2304), so that %Promise.prototype% and %Promise% are
/// as the base made them, which is plain, and lookups from `o` reach them unchanged: `o`
/// is %Promise.prototype%, or has it as prototype and no own `then` or `constructor`.
fn frozen_plain(heap: &Heap, o: Obj) -> bool {
    let ctor = realm::installed_intrinsic(heap, PROMISE_CONSTRUCTOR);
    let proto = realm::intrinsic_obj(heap, realm::Intrinsic::PromiseProto);
    heap.frozen(proto)
        && ctor.as_object().is_some_and(|c| heap.frozen(c))
        && (o == proto
            || (heap.get_prototype_of(o) == Some(proto)
                && !heap.has_own_property(o, atoms::THEN)
                && !heap.has_own_property(o, atoms::CONSTRUCTOR)))
}

/// Whether SpeciesConstructor ( o, %Promise% ) is %Promise% without running user code:
/// `constructor` is %Promise% as a data property found without a Proxy or a getter, and
/// %Promise%'s own @@species is the original getter.
fn plain_species(heap: &Heap, o: Obj) -> bool {
    if frozen_plain(heap, o) {
        return true;
    }
    let ctor = realm::installed_intrinsic(heap, PROMISE_CONSTRUCTOR);
    if plain_get(heap, o, atoms::CONSTRUCTOR) != Some(Some(ctor)) {
        return false;
    }
    let species = realm::installed_intrinsic(heap, PROMISE_SPECIES);
    ctor.as_object().is_some_and(|c| {
        matches!(heap.get_own_property(c, atoms::SYM_SPECIES),
            Some(Property::Accessor { getter, .. }) if getter == species)
    })
}

/// `ResolveWithData ( data, v )` (and the resolve functions, whose `this` is the data),
/// a `@fast` entry (D3156): for a Promise `data` (its own resolving functions or an
/// internal capability) when the resolution runs no user code. Declines otherwise.
pub(crate) fn fast_resolve(heap: &mut Heap, data: Value, v: Value) -> Fast {
    let Some(p) = as_promise(heap, data) else {
        return Ok(None);
    };
    let f = flags(heap, p);
    if f & FLAG_RESOLVED == 0 {
        if v == data || !plain_fulfillment(heap, v) {
            return Ok(None);
        }
        set_flags(heap, p, f | FLAG_RESOLVED);
        settle(heap, p, true, v)?;
    }
    Ok(Some(Value::UNDEFINED))
}

/// `RejectWithData ( data, reason )` (and the reject functions), a `@fast` entry
/// (D3156) for a Promise `data`.
pub(crate) fn fast_reject(heap: &mut Heap, data: Value, reason: Value) -> Fast {
    let Some(p) = as_promise(heap, data) else {
        return Ok(None);
    };
    let f = flags(heap, p);
    if f & FLAG_RESOLVED == 0 {
        set_flags(heap, p, f | FLAG_RESOLVED);
        settle(heap, p, false, reason)?;
    }
    Ok(Some(Value::UNDEFINED))
}

/// `Promise.resolve ( x )` on %Promise%, a `@fast` entry (D3156): `x` itself for a
/// `plain_promise`, else a new promise fulfilled with a `plain_fulfillment` value.
pub(crate) fn fast_static_resolve(heap: &mut Heap, this: Value, args: &[Value]) -> Fast {
    let x = args.first().copied().unwrap_or(Value::UNDEFINED);
    if this != realm::installed_intrinsic(heap, PROMISE_CONSTRUCTOR) {
        return Ok(None);
    }
    if plain_promise(heap, x).is_some() {
        return Ok(Some(x));
    }
    if as_promise(heap, x).is_some() || !plain_fulfillment(heap, x) {
        return Ok(None);
    }
    let proto = realm::intrinsic_obj(heap, realm::Intrinsic::PromiseProto);
    let p = alloc(heap, Some(proto))?;
    set_flags(heap, p, FLAG_RESOLVED);
    settle(heap, p, true, x)?;
    Ok(Some(Value::object(p)))
}

/// `Promise.prototype.then`, a `@fast` entry (D3156): on a Promise whose
/// SpeciesConstructor is %Promise% without user code, the derived promise is a new one
/// of %Promise% (NewPromiseCapability), subscribed with PerformPromiseThen.
pub(crate) fn fast_then(heap: &mut Heap, this: Value, args: &[Value]) -> Fast {
    let Some(p) = as_promise(heap, this).filter(|&p| plain_species(heap, p)) else {
        return Ok(None);
    };
    let arg = |i: usize| args.get(i).copied().unwrap_or(Value::UNDEFINED);
    let proto = realm::intrinsic_obj(heap, realm::Intrinsic::PromiseProto);
    let derived = alloc(heap, Some(proto))?;
    perform_then(heap, p, arg(0), arg(1), Value::object(derived))?;
    Ok(Some(Value::object(derived)))
}

/// One element `x` of a combinator's loop (D3131, `%PromiseCombinatorRun` in
/// builtins/intrinsics.d.ts) without its builtin calls, leaving the heap as they
/// would (the values slot, the element Record and functions, `then`'s derived promise),
/// when none of them could run user code. False, with nothing changed, otherwise.
fn combinator_step(
    heap: &mut Heap,
    fixed: Obj,
    kind: i32,
    index: f64,
    x: Value,
) -> Result<bool, HeapError> {
    let [c, resolve, cap, values, remaining, resolve_fn, reject_fn] =
        core::array::from_fn(|i| heap.object_slot(fixed, i as u32));
    if c != realm::installed_intrinsic(heap, PROMISE_CONSTRUCTOR)
        || resolve != realm::installed_intrinsic(heap, PROMISE_STATIC_RESOLVE)
        || !(0..=3).contains(&kind)
    {
        return Ok(false);
    }
    let proto = realm::intrinsic_obj(heap, realm::Intrinsic::PromiseProto);
    let existing = plain_promise(heap, x);
    if (existing.is_none() && !plain_fulfillment(heap, x))
        || !plain_then(heap, existing.unwrap_or(proto))
    {
        return Ok(false);
    }
    let (Some(values), Some(remaining)) = (values.as_object(), remaining.as_object()) else {
        return Ok(false);
    };
    // CreateDataPropertyOrThrow(values, index, undefined): an append to the loop's own
    // array, which nothing else has seen yet.
    if kind != 3
        && (heap.array_length(values) as f64 != index
            || !heap.set_element_fast(values, index as u32, Value::UNDEFINED)?)
    {
        return Ok(false);
    }
    // PromiseResolve ( C, x ): x itself, or a new promise fulfilled with x.
    let p = match existing {
        Some(p) => p,
        None => {
            let p = alloc(heap, Some(proto))?;
            set_flags(heap, p, FLAG_RESOLVED);
            settle(heap, p, true, x)?;
            p
        }
    };
    let (on_f, on_r) = if kind == 3 {
        (resolve_fn, reject_fn)
    } else {
        // NewElementRecord, and remainingElementsCount + 1.
        let r = heap.alloc_object(ObjectClass::Record, None, 6)?;
        let index = super::conv::number(index);
        let slots = [
            Value::FALSE,
            index,
            Value::object(values),
            cap,
            Value::object(remaining),
        ];
        for (i, v) in slots.into_iter().chain([Value::UNDEFINED]).enumerate() {
            heap.set_object_slot(r, i as u32, v);
        }
        let n = heap.object_slot(remaining, 0).as_number().unwrap_or(0.0);
        heap.set_object_slot(remaining, 0, super::conv::number(n + 1.0));
        let mut closure = |k| -> Result<Value, HeapError> {
            let f = new_builtin_closure(heap, k, Value::object(r), 1)?;
            Ok(Value::object(f.expect("a builtin closure kind")))
        };
        match kind {
            0 => (closure(3)?, reject_fn),
            1 => (closure(4)?, closure(5)?),
            _ => (resolve_fn, closure(6)?),
        }
    };
    // Promise.prototype.then: NewPromiseCapability ( %Promise% ), PerformPromiseThen.
    let derived = alloc(heap, Some(proto))?;
    perform_then(heap, p, on_f, on_r, Value::object(derived))?;
    Ok(true)
}

/// `%PromiseReactionJob(cap, handler, x, _)` with an element function of Promise.all,
/// allSettled or any as the handler (D3131, D3158): SettleElement and the resolution of
/// `cap` with the handler's result (undefined) done here, when neither runs user code:
/// `cap` and the combinator's capability are Promises of %Promise% (or `cap` is
/// undefined), the last element's values fulfill the capability without a `then` call,
/// and the last element of `any` (which makes an AggregateError) is left to the builtin.
/// False, with nothing changed, otherwise.
pub(super) fn all_element_job(
    heap: &mut Heap,
    cap: Value,
    handler: Value,
    x: Value,
) -> Result<bool, HeapError> {
    let Some(f) = handler.as_object() else {
        return Ok(false);
    };
    if heap.object_class(f) != ObjectClass::BuiltinClosure
        || !(cap.is_undefined() || as_promise(heap, cap).is_some())
    {
        return Ok(false);
    }
    let id = heap
        .object_slot(f, 0)
        .as_int()
        .and_then(|i| u16::try_from(i).ok());
    // The settled record's status atom (allSettled), and whether this is `any`.
    let (settled, any) = match id {
        Some(PROMISE_ALL_RESOLVE_ELEMENT) => (None, false),
        Some(PROMISE_ALL_SETTLED_RESOLVE_ELEMENT) => (Some(atoms::FULFILLED), false),
        Some(PROMISE_ALL_SETTLED_REJECT_ELEMENT) => (Some(atoms::REJECTED), false),
        Some(PROMISE_ANY_REJECT_ELEMENT) => (None, true),
        _ => return Ok(false),
    };
    // The element Record (builtins/promise.ts NewElementRecord): 0 called, 1 index,
    // 2 values, 3 capability, 4 remaining, 5 keys.
    let r = heap
        .object_slot(f, 1)
        .as_object()
        .expect("an element Record");
    let [called, index, values, capability, remaining, keys] =
        core::array::from_fn(|i| heap.object_slot(r, i as u32));
    if called == Value::TRUE {
        // Nothing to store: only `cap` resolves.
        return resolve_derived(heap, cap).map(|()| true);
    }
    let (Some(values), Some(remaining), Some(index)) =
        (values.as_object(), remaining.as_object(), index.as_int())
    else {
        return Ok(false);
    };
    let left = heap.object_slot(remaining, 0).as_number().unwrap_or(0.0) - 1.0;
    let last = left == 0.0;
    let cap_promise = as_promise(heap, capability);
    if last
        && (any
            || !keys.is_undefined()
            || cap_promise.is_none_or(|p| {
                flags(heap, p) & FLAG_RESOLVED == 0
                    && !plain_fulfillment(heap, Value::object(values))
            }))
    {
        return Ok(false);
    }
    if index < 0 || index as u32 >= heap.array_length(values) {
        return Ok(false);
    }
    // allSettled stores { status, value | reason } (SettledRecord).
    let x = match settled {
        Some(status) => {
            let key = if status == atoms::FULFILLED {
                atoms::VALUE
            } else {
                atoms::REASON
            };
            let s = heap.atom_str_value(status).expect("a string atom");
            let proto = realm::intrinsic_obj(heap, realm::Intrinsic::ObjectProto);
            let keys = [atoms::STATUS, key];
            Value::object(heap.new_object_from_entries(
                Some(proto),
                &keys,
                &[Value::string(s), x],
            )?)
        }
        None => x,
    };
    // CreateDataPropertyOrThrow(values, index, x): values is the combinator's own array,
    // which nothing else sees before the last element resolves with it.
    if !heap.set_element_fast(values, index as u32, x)? {
        return Ok(false);
    }
    heap.set_object_slot(r, 0, Value::TRUE);
    heap.set_object_slot(remaining, 0, super::conv::number(left));
    if let Some(p) = cap_promise.filter(|_| last) {
        // ResolveCapability ( capability, values ) of an internal capability.
        let f = flags(heap, p);
        set_flags(heap, p, f | FLAG_RESOLVED);
        if f & FLAG_RESOLVED == 0 {
            settle(heap, p, true, Value::object(values))?;
        }
    }
    resolve_derived(heap, cap).map(|()| true)
}

/// ResolveCapability ( cap, undefined ) of a reaction's internal capability (or none).
fn resolve_derived(heap: &mut Heap, cap: Value) -> Result<(), HeapError> {
    if let Some(p) = as_promise(heap, cap) {
        let f = flags(heap, p);
        set_flags(heap, p, f | FLAG_RESOLVED);
        if f & FLAG_RESOLVED == 0 {
            settle(heap, p, true, Value::UNDEFINED)?;
        }
    }
    Ok(())
}

impl super::Exec<'_> {
    /// `%PromiseCombinatorRun(fixed, kind, index, it, next)`: `combinator_step` for each
    /// element the Array iterator `it` gives without user code, while the step takes it;
    /// the next index. Each step reads the element, does the element's steps, then
    /// advances the iterator: the order does not show, as neither runs user code.
    #[inline(never)]
    pub(super) fn promise_combinator_run(
        &mut self,
        fixed: Obj,
        kind: i32,
        mut index: f64,
        it: Value,
        next: Value,
    ) -> Result<f64, HeapError> {
        while let Some(x) = self.iter_next_peek(it, next) {
            if !combinator_step(self.heap, fixed, kind, index, x)? {
                break;
            }
            self.iter_next_hit(it, next);
            index += 1.0;
            self.fuel -= 1;
        }
        Ok(index)
    }
}

// ---------------------------------------------------------------- the job queue

fn jobs(heap: &Heap) -> u32 {
    heap.arena().get::<Ref>(0, Roots::JOBS).0
}

/// Room for `n` more jobs; returns the queue cell.
fn reserve_jobs(heap: &mut Heap, n: u32) -> Result<u32, HeapError> {
    let q = jobs(heap);
    let i = state::interp(heap);
    if q != 0 {
        // Move the live entries to the front before growing.
        let head = heap.arena().get(i, Interp::JOB_HEAD);
        let len = heap.value_array_len(q);
        if head > 0 && len + n * JOB_SIZE > heap.value_array_capacity(q) {
            for k in head..len {
                let v = heap.value_array_get(q, k);
                heap.value_array_set(q, k - head, v);
            }
            for k in len - head..len {
                heap.value_array_set(q, k, Value::UNDEFINED);
            }
            heap.arena_mut().set(q, ValueArray::LEN, len - head);
            heap.arena_mut().set(i, Interp::JOB_HEAD, 0);
        }
    }
    let len = if q == 0 { 0 } else { heap.value_array_len(q) };
    let q2 = heap.value_array_reserve(q, len + n * JOB_SIZE)?;
    if q2 != q {
        heap.arena_mut().set(0, Roots::JOBS, Ref(q2));
    }
    Ok(q2)
}

/// Appends a job (room reserved with `reserve_jobs`).
fn push_job(heap: &mut Heap, q: u32, f: Value, args: [Value; 4]) {
    let len = heap.value_array_len(q);
    heap.value_array_set(q, len, f);
    for (k, a) in args.into_iter().enumerate() {
        heap.value_array_set(q, len + 1 + k as u32, a);
    }
    heap.arena_mut().set(q, ValueArray::LEN, len + JOB_SIZE);
}

/// Queues the job `f(args)` (HostEnqueuePromiseJob).
pub(crate) fn enqueue(heap: &mut Heap, f: Value, args: [Value; 4]) -> Result<(), HeapError> {
    let q = reserve_jobs(heap, 1)?;
    push_job(heap, q, f, args);
    Ok(())
}

pub(crate) fn has_jobs(heap: &Heap) -> bool {
    let q = jobs(heap);
    q != 0 && heap.arena().get(state::interp(heap), Interp::JOB_HEAD) < heap.value_array_len(q)
}

/// Whether nothing is left to run or to report: no queued job and no promise rejected
/// without a handler (`Vm::quiet`).
pub(crate) fn quiet(heap: &Heap) -> bool {
    let r = heap
        .arena()
        .get::<Ref>(state::interp(heap), Interp::REJECTIONS)
        .0;
    !has_jobs(heap) && (r == 0 || heap.value_array_len(r) == 0)
}

/// Takes the oldest job: `[f, a0, a1, a2, a3]`.
pub(crate) fn dequeue(heap: &mut Heap) -> Option<[Value; JOB_SIZE as usize]> {
    if !has_jobs(heap) {
        return None;
    }
    let q = jobs(heap);
    let i = state::interp(heap);
    let head = heap.arena().get(i, Interp::JOB_HEAD);
    let mut job = [Value::UNDEFINED; JOB_SIZE as usize];
    for (k, v) in job.iter_mut().enumerate() {
        *v = heap.value_array_get(q, head + k as u32);
        // A taken job must not keep its values alive.
        heap.value_array_set(q, head + k as u32, Value::UNDEFINED);
    }
    let head = head + JOB_SIZE;
    if head >= heap.value_array_len(q) {
        heap.arena_mut().set(q, ValueArray::LEN, 0);
        heap.arena_mut().set(i, Interp::JOB_HEAD, 0);
    } else {
        heap.arena_mut().set(i, Interp::JOB_HEAD, head);
    }
    Some(job)
}

/// Room in the base image's queues (D2067): the job queue (jobs), the host inbox
/// (results) and the unhandled rejections. A VM's first uses need no new cell, so when
/// they come does not move the session's cells, and the queues are emptied, never
/// dropped.
const BASE_JOBS: u32 = 8;
const BASE_INBOX: u32 = 4;
const BASE_REJECTIONS: u32 = 8;

/// Gives the heap its empty queues (the base image's), their slots `undefined` as a
/// taken entry leaves them, so that a used queue and an unused one are the same bytes.
pub(crate) fn preallocate_queues(heap: &mut Heap) -> Result<(), HeapError> {
    let alloc = |heap: &mut Heap, cap: u32| -> Result<u32, HeapError> {
        let q = heap.alloc_value_array(cap)?;
        for k in 0..cap {
            heap.arena_mut()
                .write(ValueArray::VALUES.at(q, k), Value::UNDEFINED);
        }
        Ok(q)
    };
    if jobs(heap) == 0 {
        let q = alloc(heap, BASE_JOBS * JOB_SIZE)?;
        heap.arena_mut().set(0, Roots::JOBS, Ref(q));
    }
    for (field, cap) in [
        (Interp::INBOX, BASE_INBOX * INBOX_SIZE),
        (Interp::REJECTIONS, BASE_REJECTIONS),
    ] {
        if heap.arena().get::<Ref>(state::interp(heap), field).0 == 0 {
            let q = alloc(heap, cap)?;
            let i = state::interp(heap);
            heap.arena_mut().set(i, field, Ref(q));
        }
    }
    Ok(())
}

/// Empties the value array `q` (if any), keeping its cell.
fn empty(heap: &mut Heap, q: u32) {
    if q != 0 {
        for k in 0..heap.value_array_len(q) {
            heap.value_array_set(q, k, Value::UNDEFINED);
        }
        heap.arena_mut().set(q, ValueArray::LEN, 0);
    }
}

/// Drops every queued job (`Vm::abort`).
pub(crate) fn clear_jobs(heap: &mut Heap) {
    empty(heap, jobs(heap));
    let i = state::interp(heap);
    heap.arena_mut().set(i, Interp::JOB_HEAD, 0);
}

// ---------------------------------------------------------------- settling and then

/// FulfillPromise / RejectPromise ( promise, value ), 27.2.1.4 and 27.2.1.7, with
/// TriggerPromiseReactions: every reaction becomes a job. A promise that is not pending
/// stays as it is.
pub(crate) fn settle(heap: &mut Heap, p: Obj, fulfilled: bool, v: Value) -> Result<(), HeapError> {
    if state_of(heap, p) != PENDING {
        return Ok(());
    }
    let reactions = heap.object_slot(p, REACTIONS).as_internal();
    let n = reactions.map_or(0, |r| heap.value_array_len(r) / 3);
    // Reserve first, so running out of memory leaves the promise pending.
    let q = if n > 0 { reserve_jobs(heap, n)? } else { 0 };
    if !fulfilled && flags(heap, p) & FLAG_HANDLED == 0 {
        track_rejection(heap, p)?;
    }
    let job = realm::installed_intrinsic(heap, PROMISE_REACTION_JOB);
    if let Some(r) = reactions {
        for k in 0..n {
            let cap = heap.value_array_get(r, 3 * k);
            let handler = heap.value_array_get(r, 3 * k + if fulfilled { 1 } else { 2 });
            push_job(heap, q, job, [cap, handler, v, Value::bool(fulfilled)]);
        }
    }
    heap.set_object_slot(
        p,
        STATE,
        Value::int(if fulfilled { FULFILLED } else { REJECTED }),
    );
    heap.set_object_slot(p, RESULT, v);
    heap.set_object_slot(p, REACTIONS, Value::UNDEFINED);
    Ok(())
}

/// PerformPromiseThen ( promise, onFulfilled, onRejected, resultCapability ), 27.2.5.4.1
/// (the result is the capability's promise, which the caller has). Handlers that are
/// not callable are passed as `undefined` ("empty").
pub(crate) fn perform_then(
    heap: &mut Heap,
    p: Obj,
    on_fulfilled: Value,
    on_rejected: Value,
    cap: Value,
) -> Result<(), HeapError> {
    // An async function's state is the handler of an await (D1375).
    let callable = |v: Value| {
        if v.as_object().is_some_and(|o| {
            heap.is_callable(o)
                || super::suspend::as_generator(heap, v, super::suspend::ASYNC).is_some()
                || super::suspend::as_generator(heap, v, super::suspend::ASYNC_GENERATOR).is_some()
        }) {
            v
        } else {
            Value::UNDEFINED
        }
    };
    let (on_f, on_r) = (callable(on_fulfilled), callable(on_rejected));
    let f = flags(heap, p);
    match state_of(heap, p) {
        PENDING => {
            let r = heap.object_slot(p, REACTIONS).as_internal().unwrap_or(0);
            let len = if r == 0 { 0 } else { heap.value_array_len(r) };
            let r2 = heap.value_array_reserve(r, len + 3)?;
            heap.value_array_set(r2, len, cap);
            heap.value_array_set(r2, len + 1, on_f);
            heap.value_array_set(r2, len + 2, on_r);
            heap.arena_mut().set(r2, ValueArray::LEN, len + 3);
            heap.set_object_slot(p, REACTIONS, Value::internal(r2));
        }
        s => {
            let fulfilled = s == FULFILLED;
            let job = realm::installed_intrinsic(heap, PROMISE_REACTION_JOB);
            let handler = if fulfilled { on_f } else { on_r };
            let v = result_of(heap, p);
            enqueue(heap, job, [cap, handler, v, Value::bool(fulfilled)])?;
            if !fulfilled && f & FLAG_HANDLED == 0 {
                untrack_rejection(heap, p, false);
            }
        }
    }
    set_flags(heap, p, f | FLAG_HANDLED);
    Ok(())
}

// ---------------------------------------------------------------- the host inbox

/// Values per inbox entry: the promise, ok (bool), the value or reason, and the clock
/// (`undefined`: leave it).
pub(crate) const INBOX_SIZE: u32 = 4;

/// Appends a completed async host call to the host inbox (D1670). The run loop applies it
/// as its own macrotask once the job queue is empty, so where a result lands does not
/// depend on when the embedder delivered it.
pub(crate) fn inbox_push(
    heap: &mut Heap,
    p: Obj,
    result: Result<Value, Value>,
    now: Option<f64>,
) -> Result<(), HeapError> {
    let i = state::interp(heap);
    let q = heap.arena().get::<Ref>(i, Interp::INBOX).0;
    if q != 0 {
        // Move the live entries to the front before growing.
        let head = heap.arena().get(i, Interp::INBOX_HEAD);
        let len = heap.value_array_len(q);
        if head > 0 && len + INBOX_SIZE > heap.value_array_capacity(q) {
            for k in head..len {
                let v = heap.value_array_get(q, k);
                heap.value_array_set(q, k - head, v);
            }
            for k in len - head..len {
                heap.value_array_set(q, k, Value::UNDEFINED);
            }
            heap.arena_mut().set(q, ValueArray::LEN, len - head);
            heap.arena_mut().set(i, Interp::INBOX_HEAD, 0);
        }
    }
    let len = if q == 0 { 0 } else { heap.value_array_len(q) };
    let q2 = heap.value_array_reserve(q, len + INBOX_SIZE)?;
    let (ok, v) = match result {
        Ok(v) => (true, v),
        Err(e) => (false, e),
    };
    let now = now.map_or(Value::UNDEFINED, Value::number);
    for (k, x) in [Value::object(p), Value::bool(ok), v, now]
        .into_iter()
        .enumerate()
    {
        heap.value_array_set(q2, len + k as u32, x);
    }
    heap.arena_mut().set(q2, ValueArray::LEN, len + INBOX_SIZE);
    let i = state::interp(heap);
    heap.arena_mut().set(i, Interp::INBOX, Ref(q2));
    Ok(())
}

/// Takes the oldest inbox entry: `(promise, result, now)`.
fn inbox_take(heap: &mut Heap) -> Option<(Obj, Result<Value, Value>, Option<f64>)> {
    let i = state::interp(heap);
    let q = heap.arena().get::<Ref>(i, Interp::INBOX).0;
    if q == 0 {
        return None;
    }
    let head = heap.arena().get(i, Interp::INBOX_HEAD);
    if head >= heap.value_array_len(q) {
        return None;
    }
    let mut e = [Value::UNDEFINED; INBOX_SIZE as usize];
    for (k, x) in e.iter_mut().enumerate() {
        *x = heap.value_array_get(q, head + k as u32);
        // A taken entry must not keep its values alive.
        heap.value_array_set(q, head + k as u32, Value::UNDEFINED);
    }
    let head = head + INBOX_SIZE;
    if head >= heap.value_array_len(q) {
        heap.arena_mut().set(q, ValueArray::LEN, 0);
        heap.arena_mut().set(i, Interp::INBOX_HEAD, 0);
    } else {
        heap.arena_mut().set(i, Interp::INBOX_HEAD, head);
    }
    let p = e[0].as_object().expect("an inbox entry's promise");
    let result = if e[1] == Value::TRUE {
        Ok(e[2])
    } else {
        Err(e[2])
    };
    Some((p, result, e[3].as_number()))
}

/// Applies the oldest inbox entry, if any: sets the clock it carries and queues the job
/// that settles its promise (D1374: a job, so that a thenable result is adopted the way a
/// resolve function would). The job slot is reserved first, so running out of memory
/// leaves the entry in the inbox.
pub(crate) fn inbox_apply(heap: &mut Heap) -> Result<bool, HeapError> {
    if !has_inbox(heap) {
        return Ok(false);
    }
    let q = reserve_jobs(heap, 1)?;
    let (p, result, now) = inbox_take(heap).expect("checked above");
    if let Some(ms) = now {
        realm::set_intrinsic(heap, realm::Intrinsic::Clock, Value::number(ms));
    }
    let job = realm::installed_intrinsic(heap, HOST_CALL_SETTLE);
    let (ok, v) = match result {
        Ok(v) => (true, v),
        Err(e) => (false, e),
    };
    push_job(
        heap,
        q,
        job,
        [Value::object(p), Value::bool(ok), v, Value::UNDEFINED],
    );
    Ok(true)
}

/// Whether the host inbox holds results.
pub(crate) fn has_inbox(heap: &Heap) -> bool {
    let i = state::interp(heap);
    let q = heap.arena().get::<Ref>(i, Interp::INBOX).0;
    q != 0 && heap.arena().get(i, Interp::INBOX_HEAD) < heap.value_array_len(q)
}

/// Drops the host inbox (`Vm::abort`).
pub(crate) fn clear_inbox(heap: &mut Heap) {
    let i = state::interp(heap);
    let q = heap.arena().get::<Ref>(i, Interp::INBOX).0;
    empty(heap, q);
    heap.arena_mut().set(i, Interp::INBOX_HEAD, 0);
}

// ---------------------------------------------------------------- rejection tracking

/// HostPromiseRejectionTracker ( promise, "reject" ): remembered until handled or taken.
fn track_rejection(heap: &mut Heap, p: Obj) -> Result<(), HeapError> {
    let i = state::interp(heap);
    let r = heap.arena().get::<Ref>(i, Interp::REJECTIONS).0;
    let len = if r == 0 { 0 } else { heap.value_array_len(r) };
    let r2 = heap.value_array_reserve(r, len + 1)?;
    heap.value_array_set(r2, len, Value::object(p));
    heap.arena_mut().set(r2, ValueArray::LEN, len + 1);
    heap.arena_mut().set(i, Interp::REJECTIONS, Ref(r2));
    Ok(())
}

/// HostPromiseRejectionTracker ( promise, "handle" ).
/// Sets [[PromiseIsHandled]] as a `then` would, without adding a reaction: the embedder
/// takes care of the outcome (D1423).
pub(crate) fn mark_handled(heap: &mut Heap, p: Obj) {
    let f = flags(heap, p);
    if f & FLAG_HANDLED == 0 {
        if state_of(heap, p) == REJECTED {
            // The embedder consumes the outcome itself: withdrawn even when reported.
            untrack_rejection(heap, p, true);
        }
        set_flags(heap, p, f | FLAG_HANDLED);
    }
}

/// HostPromiseRejectionTracker ( promise, "handle" ). A JS handler added after a
/// microtask checkpoint does not withdraw a rejection the checkpoint reported (D1670);
/// the embedder (`reported`) can.
fn untrack_rejection(heap: &mut Heap, p: Obj, reported: bool) {
    let i = state::interp(heap);
    let r = heap.arena().get::<Ref>(i, Interp::REJECTIONS).0;
    if r == 0 {
        return;
    }
    let len = heap.value_array_len(r);
    let done = heap.arena().get(i, Interp::REPORTED).min(len);
    let from = if reported { 0 } else { done };
    let Some(pos) = (from..len).find(|&k| heap.value_array_get(r, k) == Value::object(p)) else {
        return;
    };
    if pos < done {
        heap.arena_mut().set(i, Interp::REPORTED, done - 1);
    }
    for k in pos + 1..len {
        let v = heap.value_array_get(r, k);
        heap.value_array_set(r, k - 1, v);
    }
    heap.value_array_set(r, len - 1, Value::UNDEFINED);
    heap.arena_mut().set(r, ValueArray::LEN, len - 1);
}

/// The rejected promises that have no handler, oldest first; forgets them.
pub(crate) fn take_rejections(heap: &mut Heap) -> Vec<Obj> {
    let i = state::interp(heap);
    let r = heap.arena().get::<Ref>(i, Interp::REJECTIONS).0;
    if r == 0 {
        return Vec::new();
    }
    let out = (0..heap.value_array_len(r))
        .filter_map(|k| heap.value_array_get(r, k).as_object())
        .collect();
    empty(heap, r);
    heap.arena_mut().set(i, Interp::REPORTED, 0);
    out
}

/// A microtask checkpoint (the job queue ran empty, D1670): the rejections that are still
/// unhandled now count as reported, whether the embedder takes them now or after more
/// macrotasks ran (HTML's "notify about rejected promises").
pub(crate) fn rejection_checkpoint(heap: &mut Heap) {
    let i = state::interp(heap);
    let r = heap.arena().get::<Ref>(i, Interp::REJECTIONS).0;
    let len = if r == 0 { 0 } else { heap.value_array_len(r) };
    heap.arena_mut().set(i, Interp::REPORTED, len);
}

/// Forgets the tracked rejections (`Vm::abort`).
pub(crate) fn clear_rejections(heap: &mut Heap) {
    let i = state::interp(heap);
    let r = heap.arena().get::<Ref>(i, Interp::REJECTIONS).0;
    empty(heap, r);
    heap.arena_mut().set(i, Interp::REPORTED, 0);
}
