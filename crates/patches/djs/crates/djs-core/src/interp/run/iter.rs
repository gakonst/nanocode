//! Iteration without iterator result objects (0080, D1800).
//!
//! A step of the iterator protocol calls `next` and reads `done` and `value` from the
//! result. When `next` is the original `%ArrayIteratorPrototype%.next` (or the String
//! Iterator's) and the iterator is one of its objects, that result would be a fresh
//! ordinary object with own data properties `value` and `done`: reading them back runs
//! no user code, so producing the value directly is indistinguishable. Only `next`
//! itself matters here, not how the iterator was obtained: the protocol calls the
//! `next` value read once at the start, which is the one checked on every step.
//!
//! The fast step covers the cases where the original `next` runs no user code either:
//! a present dense element of an Array (holes and anything beyond take the real `next`,
//! whose `Get` may reach the prototype chain or a getter; a keys iterator, which reads no
//! element, takes every index below the length), an in-bounds Number element
//! of a typed array, and a code point of a string. The last step (`done`) also takes
//! the real `next`, which finishes the iterator and allocates the one final result.

use super::intrinsics::{
    ARRAY_ITERATOR_NEXT, ARRAY_PROTOTYPE_VALUES, STRING_ITERATOR_NEXT, STRING_PROTOTYPE_ITERATOR,
    id,
};
use super::*;
use crate::builtins::dense::index;
use crate::builtins::{collection, string};
use crate::heap::cells::Object;
use crate::heap::{Lookup, atoms};
use crate::value::Str;

/// The kind slot of a String Iterator (D1346).
const STRING_KIND: i32 = 3;

const GET_ITERATOR: u16 = id("GetIterator");
const ITERATOR_CLOSE: u16 = id("IteratorClose");
const MAP_ITERATOR_NEXT: u16 = id("MapIteratorPrototypeNext");
const SET_ITERATOR_NEXT: u16 = id("SetIteratorPrototypeNext");

impl Exec<'_> {
    /// `%GetIterator(obj)` and `%IteratorClose(it)` of for-of and array destructuring
    /// without a frame, where they run no user code (D1800): GetIterator of an Array or
    /// a string whose @@iterator is the original (a data property, as D1351 checks), and
    /// IteratorClose of an Array or String Iterator without a `return` method on its
    /// chain. `None`, with nothing changed, otherwise.
    pub(super) fn iter_intrinsic_fast(
        &mut self,
        id: u16,
        base: u32,
        argc: u32,
    ) -> Result<Option<Value>, Fault> {
        if argc != 1 {
            return Ok(None);
        }
        let a0 = self.get(base);
        match id {
            GET_ITERATOR => {
                let (original, proto, target, kind) = if a0.as_string().is_some() {
                    (
                        STRING_PROTOTYPE_ITERATOR,
                        Intrinsic::StringIteratorProto,
                        a0,
                        STRING_KIND,
                    )
                } else {
                    match a0.as_object() {
                        Some(o) if self.heap.object_class(o) == ObjectClass::Array => {
                            (ARRAY_PROTOTYPE_VALUES, Intrinsic::ArrayIteratorProto, a0, 1)
                        }
                        _ => return Ok(None),
                    }
                };
                let Some(method) = self.iterator_method(a0) else {
                    return Ok(None);
                };
                if method != realm::installed_intrinsic(self.heap, original) {
                    return Ok(None);
                }
                let proto = intrinsic_obj(self.heap, proto);
                let it = self.heap.alloc_object(
                    ObjectClass::ArrayIterator,
                    Some(proto),
                    ObjectClass::ArrayIterator.slots(),
                )?;
                self.heap.set_object_slot(it, 0, target);
                self.heap.set_object_slot(it, 1, Value::int(0));
                self.heap.set_object_slot(it, 2, Value::int(kind));
                Ok(Some(Value::object(it)))
            }
            ITERATOR_CLOSE => {
                let Some(it) = a0
                    .as_object()
                    .filter(|&o| self.heap.object_class(o) == ObjectClass::ArrayIterator)
                else {
                    return Ok(None);
                };
                if self.lacks_return(it) {
                    return Ok(Some(Value::UNDEFINED));
                }
                match self.heap.lookup_property(it, atoms::RETURN) {
                    Lookup::Absent => Ok(Some(Value::UNDEFINED)),
                    Lookup::Found(_, Property::Data { value, .. }) if value.is_nullish() => {
                        Ok(Some(Value::UNDEFINED))
                    }
                    _ => Ok(None),
                }
            }
            _ => Ok(None),
        }
    }

    /// The next step of iterator `it` with `next` method `f`, if it can skip the
    /// protocol (module docs); the iterator has then advanced (or finished). `Slow`,
    /// with nothing changed, when the caller must call `next`.
    pub(super) fn iter_fast_next(&mut self, it: Value, f: Value) -> Result<FastStep, Fault> {
        // A Map or Set Iterator with its original `next`: that never runs user code.
        if let Some(o) = it.as_object()
            && self.heap.object_class(o) == ObjectClass::CollectionIterator
        {
            let original = if self.heap.object_slot(o, 3).as_int() == Some(1) {
                SET_ITERATOR_NEXT
            } else {
                MAP_ITERATOR_NEXT
            };
            if f != realm::installed_intrinsic(self.heap, original) {
                return Ok(FastStep::Slow);
            }
            return Ok(match collection::iterator_step_value(self.heap, o)? {
                Some(v) => FastStep::Value(v),
                None => FastStep::Done,
            });
        }
        Ok(match self.iter_plan(it, f) {
            Plan::Slow => FastStep::Slow,
            Plan::Done(it) => {
                self.heap.set_object_slot(it, 0, Value::UNDEFINED);
                FastStep::Done
            }
            Plan::Str(it, s, i) => {
                let s = self.heap.contiguous(s)?;
                let n = match string::code_point_at(self.heap, s, i) {
                    Some(c) if c >= 65536.0 => 2,
                    _ => 1,
                };
                let cp = string::substring(self.heap, s, i, i + n)?;
                self.heap
                    .set_object_slot(it, 1, conv::number((i + n) as f64));
                FastStep::Value(Value::string(cp))
            }
            Plan::Elem(it, v, i, kind) => {
                let r = match kind {
                    0 => conv::number(i as f64),
                    1 => v,
                    _ => {
                        let proto = intrinsic_obj(self.heap, Intrinsic::ArrayProto);
                        let pair = [conv::number(i as f64), v];
                        Value::object(self.heap.new_array_from_values(Some(proto), &pair)?)
                    }
                };
                self.heap
                    .set_object_slot(it, 1, conv::number(i as f64 + 1.0));
                FastStep::Value(r)
            }
        })
    }

    #[cfg(test)]
    /// `fast_step` for `iter_next`: an Array Iterator of kind values at a present
    /// dense element (for-of over an array); false, with nothing changed, otherwise.
    #[inline(never)]
    pub(super) fn fast_iter_next(&mut self) -> bool {
        let Plan::Elem(it, v, i, 1) = self.iter_plan(self.top(1), self.top(0)) else {
            return false;
        };
        self.heap
            .set_object_slot(it, 1, conv::number(i as f64 + 1.0));
        self.sp -= 1;
        self.set(self.sp - 1, v);
        self.fast_jump(Op::IterNext, true);
        true
    }

    /// `fast_iter_next`'s step on explicit operands (the tier-0 loop, D1930): advances
    /// the Array Iterator `it` and returns the element, or `None` with nothing changed.
    pub(super) fn iter_next_hit(&mut self, it: Value, f: Value) -> Option<Value> {
        let Plan::Elem(it, v, i, 1) = self.iter_plan(it, f) else {
            return None;
        };
        self.heap
            .set_object_slot(it, 1, conv::number(i as f64 + 1.0));
        Some(v)
    }

    /// The element `iter_next_hit` would return, without advancing.
    pub(super) fn iter_next_peek(&self, it: Value, f: Value) -> Option<Value> {
        match self.iter_plan(it, f) {
            Plan::Elem(_, v, _, 1) => Some(v),
            _ => None,
        }
    }

    /// Whether `iter_fast_next` would produce a value (`%IteratorFastReady`).
    pub(super) fn iter_fast_ready(&self, it: Value, f: Value) -> bool {
        matches!(self.iter_plan(it, f), Plan::Str(..) | Plan::Elem(..))
    }

    /// `%IteratorFastDrain(it, next, arr, n)` (D2305): the steps `iter_fast_next` would
    /// take one by one for a keys or values Array Iterator over an Array or typed array,
    /// each stored as CreateDataProperty of `arr` at `n`, `n + 1`, ..., in one call, up to
    /// the iterator's end or the first element the real `next` must read (a hole). Stops
    /// early where the store is not a plain dense one (`create_element_fast`), and does
    /// nothing when `arr` is the iterated array (each step re-reads its length). Returns
    /// the next `n`. No user code runs in between, so the batch is unobservable; the
    /// iterator advances by what was stored, after the stores (an allocation failure in
    /// between leaves it where it was, and the retry stores the same values again).
    pub(super) fn iter_fast_drain(
        &mut self,
        it: Value,
        f: Value,
        arr: Value,
        n: f64,
    ) -> Result<f64, Fault> {
        let Plan::Elem(it, _, i, kind @ 0..=1) = self.iter_plan(it, f) else {
            return Ok(n);
        };
        let target = self.heap.object_slot(it, 0).as_object();
        let (Some(arr), Some(o), Some(n0)) = (arr.as_object(), target, index(n)) else {
            return Ok(n);
        };
        if arr == o || self.heap.object_class(arr) != ObjectClass::Array {
            return Ok(n);
        }
        let len = match self.heap.object_class(o) {
            ObjectClass::Array => self.heap.array_length(o),
            _ => self.heap.ta_length(o).unwrap_or(0),
        };
        // Below 2^32 - 1, the largest array index.
        let len = len.min(i.saturating_add(u32::MAX - 1 - n0));
        let mut k = i;
        while k < len {
            let v = if kind == 0 {
                conv::number(f64::from(k))
            } else {
                match self.heap.get_element_fast(o, k) {
                    Some(v) => v,
                    None => break,
                }
            };
            if !self.heap.create_element_fast(arr, n0 + (k - i), v)? {
                break;
            }
            k += 1;
        }
        self.heap.set_object_slot(it, 1, conv::number(f64::from(k)));
        // One unit per four elements, as the dense leaves charge.
        self.fuel -= i64::from((k - i) / 4);
        Ok(f64::from(n0 + (k - i)))
    }

    /// What the next step of `it` with `next` method `f` does, without doing it.
    fn iter_plan(&self, it: Value, f: Value) -> Plan {
        if let Some(p) = self.array_values_plan(it, f) {
            return p;
        }
        let Some(it) = it.as_object() else {
            return Plan::Slow;
        };
        if self.heap.object_class(it) != ObjectClass::ArrayIterator || f.as_object().is_none() {
            return Plan::Slow;
        }
        let target = self.heap.object_slot(it, 0);
        let (Some(index), Some(kind)) = (
            self.heap.object_slot(it, 1).as_number(),
            self.heap.object_slot(it, 2).as_int(),
        ) else {
            return Plan::Slow;
        };
        let original = if kind == STRING_KIND {
            STRING_ITERATOR_NEXT
        } else {
            ARRAY_ITERATOR_NEXT
        };
        // Index slots stay below 2^32 for the targets handled here.
        if f != realm::installed_intrinsic(self.heap, original) || index >= u32::MAX as f64 {
            return Plan::Slow;
        }
        if target.is_undefined() {
            return Plan::Done(it);
        }
        let i = index as u32;
        if kind == STRING_KIND {
            return match target.as_string() {
                Some(s) if i >= self.heap.str_len(s) => Plan::Done(it),
                Some(s) => Plan::Str(it, s, i),
                None => Plan::Slow,
            };
        }
        let Some(o) = target.as_object() else {
            return Plan::Slow;
        };
        let len = match self.heap.object_class(o) {
            ObjectClass::Array => self.heap.array_length(o),
            // Detached or out of bounds: the real `next` throws.
            ObjectClass::TypedArray => match self.heap.ta_length(o) {
                Some(n) => n,
                None => return Plan::Slow,
            },
            _ => return Plan::Slow,
        };
        if i >= len {
            return Plan::Done(it);
        }
        // A keys iterator never reads the element (23.1.5.1 step 6.c.ii), so holes don't
        // matter to it: `[...Array(n).keys()]` (D2305).
        if kind == 0 {
            return Plan::Elem(it, Value::UNDEFINED, i, 0);
        }
        // A hole or a non-dense element may reach a getter or the prototype chain, and
        // a BigInt element allocates: the real `next` reads those.
        match self.heap.get_element_fast(o, i) {
            Some(v) if (0..=2).contains(&kind) => Plan::Elem(it, v, i, kind),
            _ => Plan::Slow,
        }
    }

    /// CreateIterResultObject(undefined, true).
    pub(super) fn iter_done_hit(&mut self, it: Value, f: Value) -> Option<Value> {
        let Plan::Done(it) = self.iter_plan(it, f) else {
            return None;
        };
        let r = self.done_result().ok()?;
        self.heap.set_object_slot(it, 0, Value::UNDEFINED);
        Some(r)
    }

    pub(super) fn done_result(&mut self) -> Result<Value, Fault> {
        let proto = intrinsic_obj(self.heap, Intrinsic::ObjectProto);
        let o = self
            .heap
            .new_iter_result(Some(proto), Value::UNDEFINED, true)?;
        Ok(Value::object(o))
    }

    #[inline(always)]
    fn array_values_plan(&self, it: Value, f: Value) -> Option<Plan> {
        let o = it.as_object()?;
        if !self.is_class(o, ObjectClass::ArrayIterator) {
            return None;
        }
        let slot = |o: Obj, k: u32| {
            self.heap
                .arena()
                .read::<Value>(Object::SLOTS.at(o.offset(), k))
        };
        let (i, kind, t) = (
            slot(o, 1).as_int()?,
            slot(o, 2).as_int(),
            slot(o, 0).as_object()?,
        );
        if kind != Some(1)
            || !self.is_class(t, ObjectClass::Array)
            || f != realm::installed_intrinsic(self.heap, ARRAY_ITERATOR_NEXT)
        {
            return None;
        }
        let i = u32::try_from(i).ok()?;
        let v = self.heap.get_element_fast(t, i)?;
        Some(Plan::Elem(o, v, i, 1))
    }
}

/// [`Exec::iter_plan`]'s answer: the iterator (and what the step reads).
enum Plan {
    Slow,
    Done(Obj),
    /// A String Iterator at code unit `i` of a string.
    Str(Obj, Str, u32),
    /// An Array Iterator of kind 0-2 at index `i`, whose element is the value.
    Elem(Obj, Value, u32, i32),
}

/// The outcome of [`Exec::iter_fast_next`].
pub(super) enum FastStep {
    /// The next value: no result object needed.
    Value(Value),
    /// The iterator is done (and finished).
    Done,
    /// Call `next`.
    Slow,
}
