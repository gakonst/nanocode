//! The leaf intrinsics over dense arrays of the builtin language (0080, D1801): fuel and
//! argument checks around builtins/dense.rs, plus Object.values/entries and typed
//! arrays from dense arrays.
use crate::PushOut;

use alloc::vec::Vec;

use super::*;
use crate::builtins::dense::{self, dense_range, index};
use crate::value::Str;

impl Exec<'_> {
    /// Fuel for a loop over `n` elements done in one leaf call: one unit per four
    /// elements (the leaves run to completion, so they pay afterwards, as D1561's do).
    fn charge_elements(&mut self, n: u32) {
        self.fuel -= i64::from(n / 4);
    }

    /// `%ArrayFillDense(o, value, k, fin)`: the index the general loop continues from.
    pub(super) fn array_fill_dense(
        &mut self,
        o: Obj,
        value: Value,
        k: f64,
        fin: f64,
    ) -> Result<f64, Fault> {
        let (Some(k), Some(fin)) = (index(k), index(fin)) else {
            return Ok(k);
        };
        let i = dense::fill(self.heap, o, value, k, fin)?;
        self.charge_elements(i - k);
        Ok(i as f64)
    }

    /// `%ArraySearchDense(o, search, from, len, mode)` (builtins/intrinsics.d.ts).
    pub(super) fn array_search_dense(
        &mut self,
        o: Obj,
        search: Value,
        from: f64,
        len: f64,
        mode: i32,
    ) -> f64 {
        let (Some(from), Some(len)) = (index(from), index(len)) else {
            return dense::DECLINE;
        };
        let (r, scanned) = dense::search(self.heap, o, search, from, len, mode);
        self.charge_elements(scanned);
        r
    }

    /// `%ArraySliceDense(o, k, fin, a)`.
    pub(super) fn array_slice_dense(
        &mut self,
        o: Obj,
        k: f64,
        fin: f64,
        a: Obj,
    ) -> Result<bool, Fault> {
        let (Some(k), Some(fin)) = (index(k), index(fin)) else {
            return Ok(false);
        };
        let done = dense::slice(self.heap, o, k, fin, a)?;
        if done {
            self.charge_elements(fin - k);
        }
        Ok(done)
    }

    /// `%EnumerableOwnDataProperties(o, kind)`: Object.values (kind 1) / entries (2)
    /// in one step when no enumerable string-keyed property of `o` is an accessor and
    /// `o` has no exotic [[GetOwnProperty]] that could run code (a Proxy): then the
    /// per-key Get runs no user code and the enumerable checks cannot change in
    /// between. Undefined, for the general loop, otherwise.
    pub(super) fn enumerable_own_data(&mut self, o: Obj, kind: i32) -> Result<Value, Fault> {
        if !matches!(
            self.heap.object_class(o),
            ObjectClass::Ordinary
                | ObjectClass::Array
                | ObjectClass::Function
                | ObjectClass::Error
                | ObjectClass::TypedArray
        ) {
            return Ok(Value::UNDEFINED);
        }
        let keys = self.heap.own_keys(o);
        self.charge_elements(keys.len() as u32);
        let mut props = crate::buf::vec_with_capacity(keys.len());
        for k in keys {
            if k.is_symbol() {
                continue;
            }
            match self.heap.get_own_property_alloc(o, k)? {
                Some(Property::Data { value, flags }) if flags.enumerable() => {
                    props.push_out((k, value))
                }
                Some(Property::Accessor { flags, .. }) if flags.enumerable() => {
                    return Ok(Value::UNDEFINED);
                }
                _ => {}
            }
        }
        let proto = intrinsic_obj(self.heap, Intrinsic::ArrayProto);
        let mut out = crate::buf::vec_with_capacity(props.len());
        for (k, v) in props {
            out.push_out(if kind == 1 {
                v
            } else {
                let key = self.heap.atom_to_value(k)?;
                Value::object(self.heap.new_array_from_values(Some(proto), &[key, v])?)
            });
        }
        Ok(Value::object(
            self.heap.new_array_from_values(Some(proto), &out)?,
        ))
    }

    /// `%TypedArraySetFromDense(o, values)`.
    pub(super) fn typed_array_set_from_dense(&mut self, o: Obj, values: Obj) -> bool {
        if self.heap.object_class(o) != ObjectClass::TypedArray
            || self.heap.object_class(values) != ObjectClass::Array
        {
            return false;
        }
        let n = self.heap.array_length(values);
        if self.heap.ta_length(o).is_none_or(|len| len < n) {
            return false;
        }
        let kind = self.heap.ta_kind(o);
        let Some(elems) = dense_range(self.heap, values, 0, n).and_then(|vals| {
            vals.into_iter()
                .map(|v| self.heap.element_of(kind, v))
                .collect::<Option<Vec<_>>>()
        }) else {
            return false;
        };
        for (i, e) in elems.into_iter().enumerate() {
            self.heap.ta_set(o, i as u32, e);
        }
        self.charge_elements(n);
        true
    }

    /// `%ArrayPushDense(o, v)`. Inline: push's hot path (D2255).
    #[inline]
    pub(super) fn array_push_dense(&mut self, o: Obj, v: Value) -> Result<f64, Fault> {
        Ok(dense::push(self.heap, o, v)?.map_or(-1.0, f64::from))
    }

    /// `%ArrayJoinDense(o, len, sep)`: the joined string, or undefined.
    pub(super) fn array_join_dense(&mut self, o: Obj, len: f64, sep: Str) -> Result<Value, Fault> {
        let Some(len) = index(len) else {
            return Ok(Value::UNDEFINED);
        };
        match dense::join(self.heap, o, len, sep)? {
            Some(s) => {
                self.charge_elements(len);
                Ok(Value::string(s))
            }
            None => Ok(Value::UNDEFINED),
        }
    }
}
