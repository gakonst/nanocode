//! Array leaf intrinsics: the primitive sort (ARCH "The leaf rule": `sort` on
//! primitives without a comparator stays leaf).
use crate::PushOut;

use alloc::vec::Vec;

use crate::builtins::string;
use crate::heap::{Heap, ObjectClass, StrView};
use crate::interp::conv::{self, Fault};
use crate::value::{Kind, Obj, Str, Value};

/// `%SortPrimitives(list)`: sorts a dense array of non-symbol primitives by
/// SortCompare without a comparator (undefined last, then by ToString, stable). Returns
/// false without touching the list if an element is an object, a symbol or a hole.
pub(crate) fn sort_primitives(heap: &mut Heap, list: Obj) -> Result<bool, Fault> {
    if heap.object_class(list) != ObjectClass::Array {
        return Ok(false);
    }
    let n = heap.array_length(list);
    let mut values = crate::buf::vec_with_capacity(n as usize);
    for i in 0..n {
        match heap.get_element_fast(list, i) {
            Some(v) if !matches!(v.kind(), Kind::Object | Kind::Symbol) => values.push_out(v),
            _ => return Ok(false),
        }
    }
    // The strings in one piece first (flattening allocates), then their views, so
    // that a comparison reads the units in place (no copy per element).
    let mut keyed: Vec<(Str, Value)> = crate::buf::vec_with_capacity(values.len());
    let mut undefined = 0;
    for v in values {
        if v.is_undefined() {
            undefined += 1;
            continue;
        }
        let s = conv::to_string(heap, v)?;
        keyed.push_out((heap.contiguous(s)?, v));
    }
    let mut views: Vec<(StrView, Value)> = crate::buf::vec_with_capacity(keyed.len());
    for &(s, v) in &keyed {
        views.push_out((heap.str_view(s).unwrap_or(StrView::Latin1(&[])), v));
    }
    // A stable sort by code units, as IsLessThan compares strings.
    crate::sort::sort_by(&mut views, |a, b| string::view_cmp(a.0, b.0).is_lt());
    let keyed: Vec<Value> = views.into_iter().map(|(_, v)| v).collect();
    let mut i = 0;
    for v in keyed {
        heap.set_element_fast(list, i, v)?;
        i += 1;
    }
    for _ in 0..undefined {
        heap.set_element_fast(list, i, Value::UNDEFINED)?;
        i += 1;
    }
    Ok(true)
}

/// The comparator sort's working state (D3125): an internal Array made by
/// [`sort_start`], read and written only by these leaves, so that the merge work between
/// two calls of the comparator is one leaf call instead of a dozen resume points of the
/// builtin. Being on the heap, it survives a suspension in the comparator. Its dense
/// store holds the bottom-up merge's position (the run width, the start of the pair of
/// runs being merged, the read positions in the left and the right run, and flags), the
/// list, then two regions of `m` values: the source and the target of the current pass
/// (which one is which flips each pass). The write position is implied: `i + j - mid`.
mod sort_state {
    pub const WIDTH: u32 = 0;
    pub const LO: u32 = 1;
    pub const I: u32 = 2;
    pub const J: u32 = 3;
    /// [`FLIPPED`] | [`PENDING`].
    pub const FLAGS: u32 = 4;
    pub const LIST: u32 = 5;
    /// The pending comparison's left and right values (`src[I]`, `src[J]`).
    pub const X: u32 = 6;
    pub const HEAD: u32 = 8;
    /// The source of this pass is the second region.
    pub const FLIPPED: i32 = 1;
    /// A comparison of `src[I]` and `src[J]` waits for its result.
    pub const PENDING: i32 = 2;
}

/// `%SortStart(list)`: the state of a merge sort of the dense array `list` by
/// SortCompare: its undefined elements go to the end of `list` now (they sort last and
/// are never compared), the others into the state.
pub(crate) fn sort_start(heap: &mut Heap, list: Obj) -> Result<Obj, Fault> {
    use sort_state::*;
    let n = heap.array_length(list);
    let mut vals: Vec<Value> = crate::buf::vec_with_capacity(HEAD as usize);
    vals.resize(HEAD as usize, Value::int(0));
    for i in 0..n {
        let v = heap.get_element_fast(list, i).unwrap_or(Value::UNDEFINED);
        if !v.is_undefined() {
            vals.push_out(v);
        }
    }
    let m = vals.len() as u32 - HEAD;
    for i in m..n {
        heap.set_element_fast(list, i, Value::UNDEFINED)?;
    }
    vals.resize((HEAD + 2 * m) as usize, Value::UNDEFINED);
    vals[WIDTH as usize] = Value::int(1);
    vals[J as usize] = Value::int(m.min(1) as i32);
    vals[LIST as usize] = Value::object(list);
    Ok(heap.new_array_from_values(None, &vals)?)
}

/// `%SortStep(st, v)`: places the pending comparison's element by its result `v` (the
/// left one unless `v > 0`, so equal elements keep their order), then merges on to the
/// next comparison: true when there is one ([`sort_operand`] reads its two values), false
/// when the list is sorted (written back into it).
pub(crate) fn sort_step(heap: &mut Heap, st: Obj, v: f64) -> Result<bool, Fault> {
    use sort_state::*;
    let el = heap.dense_store(st);
    let m = (heap.value_array_len(el) - HEAD) / 2;
    let int = |heap: &Heap, slot: u32| heap.value_array_get(el, slot).as_int().unwrap_or(0);
    let (mut w, mut lo) = (int(heap, WIDTH) as u32, int(heap, LO) as u32);
    let (mut i, mut j, flags) = (int(heap, I) as u32, int(heap, J) as u32, int(heap, FLAGS));
    let (mut src, mut dst) = (HEAD, HEAD + m);
    if flags & FLIPPED != 0 {
        core::mem::swap(&mut src, &mut dst);
    }
    let mut mid = lo.saturating_add(w).min(m);
    let mut hi = lo.saturating_add(w.saturating_mul(2)).min(m);
    // Moves source element `from` to the next target position.
    let mv = |heap: &mut Heap, src: u32, dst: u32, from: u32, k: u32| {
        let x = heap.value_array_get(el, src + from);
        heap.value_array_set(el, dst + k, x);
    };
    if flags & PENDING != 0 {
        let k = i + j - mid;
        if v > 0.0 {
            mv(heap, src, dst, j, k);
            j += 1;
        } else {
            mv(heap, src, dst, i, k);
            i += 1;
        }
        // The common case: the merge goes on with the next comparison.
        if i < mid && j < hi {
            let (slot, at) = if v > 0.0 { (J, j) } else { (I, i) };
            heap.value_array_set(el, slot, Value::int(at as i32));
            let x = heap.value_array_get(el, src + at);
            heap.value_array_set(el, X + slot - I, x);
            return Ok(true);
        }
    }
    loop {
        if i < mid && j < hi {
            let flipped = if src == HEAD { 0 } else { FLIPPED };
            for (slot, x) in [(WIDTH, w), (LO, lo), (I, i), (J, j)] {
                heap.value_array_set(el, slot, Value::int(x as i32));
            }
            heap.value_array_set(el, FLAGS, Value::int(flipped | PENDING));
            for (slot, at) in [(X, i), (X + 1, j)] {
                let x = heap.value_array_get(el, src + at);
                heap.value_array_set(el, slot, x);
            }
            return Ok(true);
        }
        let mut k = i + j - mid;
        for (from, end) in [(i, mid), (j, hi)] {
            for f in from..end {
                mv(heap, src, dst, f, k);
                k += 1;
            }
        }
        lo = hi;
        if lo >= m {
            core::mem::swap(&mut src, &mut dst);
            w = w.saturating_mul(2);
            lo = 0;
            if w >= m {
                break;
            }
        }
        mid = lo.saturating_add(w).min(m);
        hi = lo.saturating_add(w.saturating_mul(2)).min(m);
        (i, j) = (lo, mid);
    }
    if let Some(list) = heap.value_array_get(el, LIST).as_object() {
        for t in 0..m {
            let x = heap.value_array_get(el, src + t);
            heap.set_element_fast(list, t, x)?;
        }
    }
    Ok(false)
}

/// `%SortOperand(st, which)`: the left (0) or right (1) value of the pending comparison.
#[inline]
pub(crate) fn sort_operand(heap: &Heap, st: Obj, which: bool) -> Value {
    heap.value_array_get(heap.dense_store(st), sort_state::X + which as u32)
}

/// `%SortNumbers(list)`: a typed array's default sort (23.2.3.30 with no comparator):
/// numeric, -0 before +0, NaN last; stable.
pub(crate) fn sort_numbers(heap: &mut Heap, list: Obj) -> Result<(), Fault> {
    let n = heap.array_length(list);
    if n > 0
        && heap
            .get_element_fast(list, 0)
            .is_some_and(|v| v.as_bigint().is_some())
    {
        // A BigInt typed array's elements (0046): sorted by value.
        let mut values: Vec<(crate::bigint::BigInt, Value)> = (0..n)
            .filter_map(|i| heap.get_element_fast(list, i))
            .map(|v| (heap.bigint(v), v))
            .collect();
        crate::sort::sort_by(&mut values, |a, b| a.0 < b.0);
        for (i, (_, v)) in values.into_iter().enumerate() {
            heap.set_element_fast(list, i as u32, v)?;
        }
        return Ok(());
    }
    let mut values: Vec<f64> = (0..n)
        .map(|i| {
            heap.get_element_fast(list, i)
                .and_then(|v| v.as_number())
                .unwrap_or(f64::NAN)
        })
        .collect();
    // crate::sort, not core's sorts (D1432).
    crate::sort::sort_by(&mut values, |a, b| match (a.is_nan(), b.is_nan()) {
        (_, true) => !a.is_nan(),
        (true, false) => false,
        _ if a == b && *a == 0.0 => a.is_sign_negative() && !b.is_sign_negative(),
        _ => a < b,
    });
    for (i, x) in values.into_iter().enumerate() {
        heap.set_element_fast(list, i as u32, crate::interp::conv::number(x))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The comparator sort's leaves, driven as SortList drives them: stable, a
    /// permutation even for an inconsistent comparator, the same comparisons as the
    /// bottom-up merge of `crate::sort`, and undefined last without being compared.
    #[test]
    fn sort_leaves_merge_stably() {
        let mut heap = Heap::new().unwrap();
        for n in 0..70i32 {
            for (holes, consistent) in [(false, true), (true, true), (false, false)] {
                let mut vals: Vec<Value> = (0..n)
                    .map(|i| Value::int((i * 7919) % 13 * 100 + i))
                    .collect();
                if holes {
                    for v in vals.iter_mut().step_by(5) {
                        *v = Value::UNDEFINED;
                    }
                }
                let list = heap.new_array_from_values(None, &vals).unwrap();
                let st = sort_start(&mut heap, list).unwrap();
                let (mut v, mut calls, mut seed) = (0.0, 0, n as u32);
                while sort_step(&mut heap, st, v).unwrap() {
                    let x = sort_operand(&heap, st, false).as_int().unwrap();
                    let y = sort_operand(&heap, st, true).as_int().unwrap();
                    v = if consistent {
                        (x / 100 - y / 100) as f64
                    } else {
                        seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
                        [f64::NAN, -1.0, 1.0][(seed >> 16) as usize % 3]
                    };
                    calls += 1;
                }
                let got: Vec<Value> = (0..n as u32)
                    .map(|i| heap.get_element_fast(list, i).unwrap())
                    .collect();
                let mut defined: Vec<Value> =
                    vals.iter().copied().filter(|v| !v.is_undefined()).collect();
                let m = defined.len();
                assert!(got[m..].iter().all(|v| v.is_undefined()), "n {n}");
                if consistent {
                    let mut want_calls = 0;
                    crate::sort::sort_by(&mut defined, |a, b| {
                        want_calls += 1;
                        a.as_int().unwrap() / 100 < b.as_int().unwrap() / 100
                    });
                    assert_eq!(got[..m], defined[..], "n {n}");
                    assert_eq!(calls, want_calls, "n {n}");
                } else {
                    let key = |v: &Value| v.as_int().unwrap();
                    let mut a: Vec<i32> = got[..m].iter().map(key).collect();
                    let mut b: Vec<i32> = defined.iter().map(key).collect();
                    a.sort_unstable();
                    b.sort_unstable();
                    assert_eq!(a, b, "n {n}");
                }
            }
        }
    }
}
