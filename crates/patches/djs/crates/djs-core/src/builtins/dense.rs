//! Element loops of Array builtins over dense arrays in one step (0080 D1801, 0091).
//!
//! A present element in an Array's dense storage is an own, writable, enumerable,
//! configurable data property, so HasProperty and Get of it run no user code and do
//! not reach the prototype chain. When every element a loop would touch is such an
//! element (and the loop's other steps cannot run user code either), the loop runs
//! here; anything else (a hole, an index past the dense storage, another kind of
//! object) declines with nothing changed, or with only plain stores the general loop
//! repeats, and the builtin runs its general loop. The leaf intrinsics of the builtin
//! language (interp/run/dense.rs) and the `@fast` entries (builtins/fast.rs) share
//! these. Each returns the number of elements it handled, for fuel.

use alloc::string::String;
use alloc::vec::Vec;

use crate::builtins::string;
use crate::heap::atom::MAX_INT_ATOM;
use crate::heap::{Heap, ObjectClass, StrView};
use crate::interp::conv::{self, Fault};
use crate::value::{Obj, Str, Value};

/// The search result that declines.
pub(crate) const DECLINE: f64 = -2.0;

/// The dense elements `o[from .. to)` of an Array, if all are present.
pub(crate) fn dense_range(heap: &Heap, o: Obj, from: u32, to: u32) -> Option<Vec<Value>> {
    if heap.object_class(o) != ObjectClass::Array || to > heap.dense_len(o) {
        return None;
    }
    let mut out = crate::buf::vec_with_capacity(to.saturating_sub(from) as usize);
    for i in from..to {
        out.push(heap.get_element_fast(o, i)?);
    }
    Some(out)
}

/// An integral Number in [0, 2^32) as a u32.
pub(crate) fn index(v: f64) -> Option<u32> {
    ((0.0..4294967296.0).contains(&v) && (v as u32) as f64 == v).then_some(v as u32)
}

/// fill's Sets from `k` while each is a plain store (`set_element_fast`: an existing
/// dense element, or an append or hole fill of an extensible Array whose prototypes
/// have no index properties): the index the general [[Set]] loop continues from.
pub(crate) fn fill(heap: &mut Heap, o: Obj, value: Value, k: u32, fin: u32) -> Result<u32, Fault> {
    if heap.object_class(o) != ObjectClass::Array {
        return Ok(k);
    }
    let mut i = k;
    while i < fin && heap.set_element_fast(o, i, value)? {
        i += 1;
    }
    Ok(i)
}

/// The loops of indexOf (mode 0: `from` up to `len`, strict equality), lastIndexOf
/// (1: `from` down to 0) and includes (2: up to `len`, SameValueZero): the index found,
/// -1, or [`DECLINE`]; and the number of elements compared.
pub(crate) fn search(
    heap: &Heap,
    o: Obj,
    search: Value,
    from: u32,
    len: u32,
    mode: i32,
) -> (f64, u32) {
    if heap.object_class(o) != ObjectClass::Array {
        return (DECLINE, 0);
    }
    let same_value_zero = |v: Value| match (v.as_number(), search.as_number()) {
        (Some(a), Some(b)) => a == b || (a.is_nan() && b.is_nan()),
        _ => conv::strict_equals(heap, v, search),
    };
    match mode {
        // indexOf / includes: [from, len) must all be dense up to the match.
        0 | 2 => {
            if len > heap.dense_len(o) {
                return (DECLINE, 0);
            }
            if let Some(s) = search.as_string() {
                let n = heap.str_len(s);
                for i in from..len {
                    let Some(v) = heap.get_element_fast(o, i) else {
                        return (DECLINE, 0);
                    };
                    if let Some(t) = v.as_string()
                        && heap.str_len(t) == n
                        && heap.str_eq(t, s)
                    {
                        return (i as f64, i - from + 1);
                    }
                }
                return (-1.0, len.saturating_sub(from));
            }
            for i in from..len {
                let Some(v) = heap.get_element_fast(o, i) else {
                    return (DECLINE, 0);
                };
                let hit = if mode == 0 {
                    conv::strict_equals(heap, v, search)
                } else {
                    same_value_zero(v)
                };
                if hit {
                    return (i as f64, i - from + 1);
                }
            }
            (-1.0, len.saturating_sub(from))
        }
        // lastIndexOf: from `from` down to 0.
        1 => {
            if from >= heap.dense_len(o) {
                return (DECLINE, 0);
            }
            for i in (0..=from).rev() {
                let Some(v) = heap.get_element_fast(o, i) else {
                    return (DECLINE, 0);
                };
                if conv::strict_equals(heap, v, search) {
                    return (i as f64, from - i + 1);
                }
            }
            (-1.0, from + 1)
        }
        _ => (DECLINE, 0),
    }
}

/// slice's copy: `a[0 .. fin - k) = o[k .. fin)` when every element of `o` in the range
/// is present in dense storage and `a` takes plain appends. False, with nothing
/// changed, otherwise; after a partial copy (the heap cap) the general loop repeats the
/// same stores into the fresh `a`.
pub(crate) fn slice(heap: &mut Heap, o: Obj, k: u32, fin: u32, a: Obj) -> Result<bool, Fault> {
    if fin < k || heap.object_class(a) != ObjectClass::Array {
        return Ok(false);
    }
    let Some(values) = dense_range(heap, o, k, fin) else {
        return Ok(false);
    };
    for (n, v) in values.into_iter().enumerate() {
        let n = n as u32;
        // CreateDataPropertyOrThrow: the first store checks what makes all of them plain
        // appends (an extensible Array without index properties) and declines with
        // nothing changed; that does not change in between.
        if !heap.create_element_fast(a, n, v)? {
            if n == 0 {
                return Ok(false);
            }
            let key = heap.atom_from_index(n)?;
            heap.create_data_property(a, key, v)?;
        }
    }
    Ok(true)
}

/// push(v) as a plain append: the new length, or `None` with nothing changed.
pub(crate) fn push(heap: &mut Heap, o: Obj, v: Value) -> Result<Option<u32>, Fault> {
    if heap.object_class(o) != ObjectClass::Array {
        return Ok(None);
    }
    if let Some(n) = heap.array_append_fast(o, v)? {
        return Ok(Some(n));
    }
    let len = heap.array_length(o);
    if len >= MAX_INT_ATOM || !heap.set_element_fast(o, len, v)? {
        return Ok(None);
    }
    Ok(Some(len + 1))
}

/// join over o[0 .. len) when every element is present in dense storage and a
/// primitive other than a symbol: the string, or `None`.
pub(crate) fn join(heap: &mut Heap, o: Obj, len: u32, sep: Str) -> Result<Option<Str>, Fault> {
    // Flat one-byte strings (lines, words): straight from the cells.
    if let Some(s) = heap.join_flat_latin1(o, len, sep)? {
        return Ok(Some(s));
    }
    let Some(values) = dense_range(heap, o, 0, len) else {
        return Ok(None);
    };
    // Symbols throw and objects run user code: the general loop does those.
    if values
        .iter()
        .any(|v| v.as_object().is_some() || v.as_symbol().is_some())
    {
        return Ok(None);
    }
    // Strings and holes only (the common case): the result written once from the
    // pieces (0110: each element went through its own Vec of units before).
    if values
        .iter()
        .all(|v| v.as_string().is_some() || v.is_nullish())
    {
        let sep_len = heap.str_len(sep);
        let mut pieces: Vec<(Str, u32, u32)> = crate::buf::vec_with_capacity(2 * values.len());
        for (i, v) in values.iter().enumerate() {
            if i > 0 && sep_len > 0 {
                pieces.push((sep, 0, sep_len));
            }
            if let Some(s) = v.as_string() {
                pieces.push((s, 0, heap.str_len(s)));
            }
        }
        return Ok(Some(string::join(heap, &mut pieces)?));
    }
    let sep = units_of(heap, sep)?;
    let mut out: Vec<u16> = Vec::new();
    let mut buf = String::new();
    for (i, v) in values.into_iter().enumerate() {
        if i > 0 {
            out.extend_from_slice(&sep);
        }
        if let Some(s) = v.as_string() {
            out.extend(units_of(heap, s)?);
        } else if let Some(n) = v.as_int() {
            buf.clear();
            crate::text::cat_to!(&mut buf, n);
            out.extend(buf.bytes().map(u16::from));
        } else if !v.is_nullish() {
            let s = conv::to_string(heap, v)?;
            out.extend(units_of(heap, s)?);
        }
    }
    let s = if out.iter().all(|&u| u < 256) {
        let bytes: Vec<u8> = out.iter().map(|&u| u as u8).collect();
        heap.alloc_latin1(&bytes)?
    } else {
        heap.alloc_utf16(&out)?
    };
    Ok(Some(s))
}

/// The code units of `s`.
fn units_of(heap: &mut Heap, s: Str) -> Result<Vec<u16>, Fault> {
    Ok(match heap.str_view_flat(s)? {
        StrView::Latin1(b) => b.iter().map(|&c| u16::from(c)).collect(),
        view @ StrView::Utf16(_) => view.units().collect(),
    })
}
