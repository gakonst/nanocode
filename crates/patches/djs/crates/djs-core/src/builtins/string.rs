//! String leaf intrinsics on the heap representation (one-byte and two-byte strings,
//! ropes walked in chunks where that avoids flattening).
use crate::PushOut;

use alloc::vec::Vec;

use crate::heap::{Heap, StrView};
use crate::interp::conv::Fault;
use crate::interp::{Intrinsic, realm};
use crate::value::{Obj, Str, Value};

/// IsLessThan for two strings, as -1, 0 or 1 (code unit order).
pub(crate) fn compare(heap: &Heap, a: Str, b: Str) -> f64 {
    let order = match (heap.str_view(a), heap.str_view(b)) {
        (Some(x), Some(y)) => view_cmp(x, y),
        _ => heap.str_units(a).cmp(heap.str_units(b)),
    };
    match order {
        core::cmp::Ordering::Less => -1.0,
        core::cmp::Ordering::Equal => 0.0,
        core::cmp::Ordering::Greater => 1.0,
    }
}

/// Code unit order of two flat strings: bytes for two one-byte ones (most), units
/// otherwise.
pub(crate) fn view_cmp(a: StrView, b: StrView) -> core::cmp::Ordering {
    match (a, b) {
        (StrView::Latin1(x), StrView::Latin1(y)) => x.cmp(y),
        _ => a.units().cmp(b.units()),
    }
}

/// The code points of `s` as an array of one- or two-unit strings (lone surrogates on
/// their own), as the String iterator yields them.
pub(crate) fn code_points(heap: &mut Heap, s: Str) -> Result<Obj, Fault> {
    let units: Vec<u16> = heap.str_units(s).collect();
    let mut values = Vec::new();
    let mut i = 0;
    while i < units.len() {
        let n = if (0xD800..0xDC00).contains(&units[i])
            && units
                .get(i + 1)
                .is_some_and(|u| (0xDC00..0xE000).contains(u))
        {
            2
        } else {
            1
        };
        values.push_out(Value::string(heap.alloc_utf16(&units[i..i + n])?));
        i += n;
    }
    let proto = realm::intrinsic_obj(heap, Intrinsic::ArrayProto);
    Ok(heap.new_array_from_values(Some(proto), &values)?)
}

/// The code units of `s` (flattening a rope once).
fn units(heap: &mut Heap, s: Str) -> Result<Vec<u16>, Fault> {
    Ok(heap.str_view_flat(s)?.units().collect())
}

/// `s[start .. end)` in code units (the caller clamps the indexes).
pub(crate) fn substring(heap: &mut Heap, s: Str, start: u32, end: u32) -> Result<Str, Fault> {
    let len = heap.str_len(s);
    if start == 0 && end >= len {
        return Ok(s);
    }
    if start >= end {
        return Ok(heap.alloc_latin1(&[])?);
    }
    if let Some(slice) = heap.substring_slice(s, start, end)? {
        return Ok(slice);
    }
    join(heap, &mut [(s, start, end)])
}

/// The concatenation of the pieces `s[start .. end)` as one flat string, written once
/// (no ropes and no intermediate strings). One-byte pieces are copied within the arena;
/// pieces are read like `str_view_flat` reads them (an append rope in place).
pub(crate) fn join(heap: &mut Heap, pieces: &mut [(Str, u32, u32)]) -> Result<Str, Fault> {
    let mut total: u64 = 0;
    let mut two_byte = false;
    for p in pieces.iter_mut() {
        total += u64::from(p.2 - p.1);
        two_byte |= heap.str_is_two_byte(p.0);
    }
    if total > u64::from(crate::heap::string::MAX_LEN) {
        return Err(crate::heap::HeapError::StringTooLong.into());
    }
    let total = total as usize;
    if !two_byte {
        for &(s, _, _) in pieces.iter() {
            // (Flat pieces need nothing; ropes are flattened, append ropes read in place.)
            if heap.str_view(s).is_none() {
                heap.str_view_flat(s)?;
            }
        }
        return Ok(heap.alloc_latin1_pieces(pieces, total as u32)?);
    }
    let mut units: Vec<u16> = crate::buf::vec_with_capacity(total);
    for &(s, a, b) in pieces.iter() {
        let v = heap.str_view_flat(s)?;
        units.extend((a as usize..b as usize).map(|i| v.get(i)));
    }
    Ok(heap.alloc_utf16(&units)?)
}

/// The index of the first `b` in `h`, eight bytes at a time: wasm has no SIMD by default
/// (D016), and a byte loop was most of `includes` (0110). The zero-byte test
/// `(x - 0x01..) & !x & 0x80..` can flag bytes above a real match, never below it, so its
/// lowest flag is exact.
#[inline]
pub(crate) fn find_byte(h: &[u8], b: u8) -> Option<usize> {
    const LO: u64 = 0x0101_0101_0101_0101;
    const HI: u64 = 0x8080_8080_8080_8080;
    let pat = LO * b as u64;
    let (chunks, rest) = h.as_chunks::<8>();
    for (k, c) in chunks.iter().enumerate() {
        let x = u64::from_le_bytes(*c) ^ pat;
        let z = x.wrapping_sub(LO) & !x & HI;
        if z != 0 {
            return Some(8 * k + (z.trailing_zeros() / 8) as usize);
        }
    }
    let at = 8 * chunks.len();
    rest.iter().position(|&c| c == b).map(|k| at + k)
}

/// The first index ≥ `from` of the non-empty `needle` in `h` (one-byte strings). Eight
/// candidate positions at a time, testing the needle's first and last byte together
/// (0116): with the first byte alone, a common letter flagged most positions of a line.
pub(crate) fn find_latin1(h: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    const LO: u64 = 0x0101_0101_0101_0101;
    const HI: u64 = 0x8080_8080_8080_8080;
    let m = needle.len();
    let (&first, rest) = needle.split_first()?;
    if rest.is_empty() {
        return h
            .get(from..)
            .and_then(|t| find_byte(t, first))
            .map(|k| from + k);
    }
    let last = needle[m - 1];
    let mid = &needle[1..m - 1];
    let word = |i: usize| u64::from_le_bytes(h[i..i + 8].try_into().unwrap());
    // `(x - 0x01..) & !x & 0x80..` flags every zero byte (and maybe some above one): a
    // flagged position is only a candidate, checked in full.
    let zero = |x: u64| x.wrapping_sub(LO) & !x & HI;
    let (pf, pl) = (LO * first as u64, LO * last as u64);
    let mut i = from;
    while i + m - 1 + 8 <= h.len() {
        let mut flags = zero(word(i) ^ pf) & zero(word(i + m - 1) ^ pl);
        while flags != 0 {
            let at = i + (flags.trailing_zeros() / 8) as usize;
            if h[at] == first
                && h[at + m - 1] == last
                && h[at + 1..at + m - 1].iter().zip(mid).all(|(a, b)| a == b)
            {
                return Some(at);
            }
            flags &= flags - 1;
        }
        i += 8;
    }
    while i + m <= h.len() {
        if h[i] == first && h[i + m - 1] == last && h[i + 1..i + m - 1] == *mid {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// The first index ≥ `from` (or, with `last`, the last index ≤ `from`) where `search`
/// occurs in `s`, or -1.
pub(crate) fn index_of(
    heap: &mut Heap,
    s: Str,
    search: Str,
    from: u32,
    last: bool,
) -> Result<f64, Fault> {
    // Both flat, then both read in place (0110: no copy of the needle).
    heap.str_view_flat(s)?;
    let search = heap.contiguous(search)?;
    let from = from as usize;
    if !last
        && let (Some(StrView::Latin1(h)), Some(StrView::Latin1(t))) =
            (heap.str_view(s), heap.str_view(search))
    {
        if t.is_empty() {
            return Ok(if from > h.len() { -1.0 } else { from as f64 });
        }
        return Ok(find_latin1(h, t, from).map_or(-1.0, |i| i as f64));
    }
    // The needle on the stack when short (no Rust heap allocation, 0081 F6).
    let m = heap.str_len(search) as usize;
    let mut stack = [0u16; 64];
    let vec: Vec<u16>;
    let needle: &[u16] = if m <= stack.len() {
        let v = heap.str_view(search).expect("flattened");
        for (i, u) in v.units().enumerate() {
            stack[i] = u;
        }
        &stack[..m]
    } else {
        vec = units(heap, search)?;
        &vec
    };
    let hay = heap.str_view_flat(s)?;
    let n = hay.len();
    if m > n {
        return Ok(-1.0);
    }
    let matches = |i: usize| (0..m).all(|j| hay.get(i + j) == needle[j]);
    if last {
        let mut i = from.min(n - m);
        loop {
            if matches(i) {
                return Ok(i as f64);
            }
            if i == 0 {
                return Ok(-1.0);
            }
            i -= 1;
        }
    }
    Ok((from..=n - m)
        .find(|&i| matches(i))
        .map_or(-1.0, |i| i as f64))
}

/// The code point at `i` (a surrogate pair combined), as a Number.
pub(crate) fn code_point_at(heap: &Heap, s: Str, i: u32) -> Option<f64> {
    let hi = heap.char_code_at(s, i)?;
    if (0xD800..0xDC00).contains(&hi)
        && let Some(lo) = heap.char_code_at(s, i + 1)
        && (0xDC00..0xE000).contains(&lo)
    {
        return Some((0x10000 + ((hi as u32 - 0xD800) << 10) + (lo as u32 - 0xDC00)) as f64);
    }
    Some(hi as f64)
}

/// Code points of `u` (lone surrogates as themselves), with their unit positions.
fn code_points_of(u: &[u16]) -> Vec<(usize, u32)> {
    let mut out = crate::buf::vec_with_capacity(u.len());
    let mut i = 0;
    while i < u.len() {
        let c = u[i] as u32;
        if (0xD800..0xDC00).contains(&c)
            && u.get(i + 1).is_some_and(|l| (0xDC00..0xE000).contains(l))
        {
            out.push_out((
                i,
                0x10000 + ((c - 0xD800) << 10) + (u[i + 1] as u32 - 0xDC00),
            ));
            i += 2;
        } else {
            out.push_out((i, c));
            i += 1;
        }
    }
    out
}

fn push_code_point(out: &mut Vec<u16>, c: u32) {
    if c >= 0x10000 {
        let c = c - 0x10000;
        out.push_out(0xD800 + (c >> 10) as u16);
        out.push_out(0xDC00 + (c & 0x3FF) as u16);
    } else {
        out.push_out(c as u16);
    }
}

/// toUpperCase / toLowerCase: the full Unicode mappings (SpecialCasing without
/// language-specific rules) and Final_Sigma for lowercase.
pub(crate) fn to_case(heap: &mut Heap, s: Str, upper: bool) -> Result<Str, Fault> {
    use crate::unicode::case;
    // An ASCII string read in place: unchanged (most lowercase calls), or one buffer for
    // the mapped bytes, on the stack when short.
    if let StrView::Latin1(b) = heap.str_view_flat(s)?
        && crate::heap::string::is_ascii(b)
    {
        let changes = |c: &u8| {
            if upper {
                c.is_ascii_lowercase()
            } else {
                c.is_ascii_uppercase()
            }
        };
        if !b.iter().any(changes) {
            return Ok(s);
        }
        let map = |c: u8| {
            if upper {
                c.to_ascii_uppercase()
            } else {
                c.to_ascii_lowercase()
            }
        };
        let mut stack = [0u8; 64];
        if b.len() <= stack.len() {
            let n = b.len();
            for (d, &c) in stack.iter_mut().zip(b) {
                *d = map(c);
            }
            return Ok(heap.alloc_latin1(&stack[..n])?);
        }
        let mapped: Vec<u8> = b.iter().map(|&c| map(c)).collect();
        return Ok(heap.alloc_latin1(&mapped)?);
    }
    let u = units(heap, s)?;
    if u.iter().all(|&c| c < 0x80) {
        let mapped: Vec<u8> = u
            .iter()
            .map(|&c| {
                let b = c as u8;
                if upper {
                    b.to_ascii_uppercase()
                } else {
                    b.to_ascii_lowercase()
                }
            })
            .collect();
        if mapped.iter().zip(&u).all(|(&m, &c)| m as u16 == c) {
            return Ok(s);
        }
        return Ok(heap.alloc_latin1(&mapped)?);
    }
    let cps = code_points_of(&u);
    let mut out = crate::buf::vec_with_capacity(u.len());
    for (k, &(_, c)) in cps.iter().enumerate() {
        if !upper && c == 0x03A3 {
            // Final_Sigma: preceded by a cased letter and not followed by one (case
            // ignorables skipped on both sides).
            let before = cps[..k]
                .iter()
                .rev()
                .map(|&(_, c)| c)
                .find(|&c| !case::is_case_ignorable(c))
                .is_some_and(case::is_cased);
            let after = cps[k + 1..]
                .iter()
                .map(|&(_, c)| c)
                .find(|&c| !case::is_case_ignorable(c))
                .is_some_and(case::is_cased);
            push_code_point(&mut out, if before && !after { 0x03C2 } else { 0x03C3 });
            continue;
        }
        let m = if upper {
            case::to_upper_full(c)
        } else {
            case::to_lower_full(c)
        };
        for &d in m.as_slice() {
            push_code_point(&mut out, d);
        }
    }
    Ok(heap.alloc_utf16(&out)?)
}

fn is_trim_space(c: u16) -> bool {
    let c = c as u32;
    crate::unicode::is_js_whitespace(c) || crate::unicode::is_js_line_terminator(c)
}

/// trim (where 0), trimStart (1), trimEnd (2).
pub(crate) fn trim(heap: &mut Heap, s: Str, where_: i32) -> Result<Str, Fault> {
    let view = heap.str_view_flat(s)?;
    let n = view.len();
    let mut start = 0;
    let mut end = n;
    if where_ != 2 {
        while start < n && is_trim_space(view.get(start)) {
            start += 1;
        }
    }
    if where_ != 1 {
        while end > start && is_trim_space(view.get(end - 1)) {
            end -= 1;
        }
    }
    substring(heap, s, start as u32, end as u32)
}

/// IsStringWellFormedUnicode, or (with `fix`) toWellFormed's result.
pub(crate) fn well_formed(heap: &mut Heap, s: Str, fix: bool) -> Result<Value, Fault> {
    let u = units(heap, s)?;
    let mut out = Vec::new();
    let mut ok = true;
    for (_, c) in code_points_of(&u) {
        let lone = (0xD800..0xE000).contains(&c);
        ok &= !lone;
        if fix {
            push_code_point(&mut out, if lone { 0xFFFD } else { c });
        }
    }
    if !fix {
        return Ok(Value::bool(ok));
    }
    Ok(Value::string(if ok { s } else { heap.alloc_utf16(&out)? }))
}

/// `s` repeated `n` times, by doubling (ropes keep it O(log n) concatenations).
pub(crate) fn repeat(heap: &mut Heap, s: Str, n: u32) -> Result<Str, Fault> {
    let len = heap.str_len(s) as u64;
    if len * n as u64 > crate::heap::string::MAX_LEN as u64 {
        return Err(crate::heap::HeapError::StringTooLong.into());
    }
    // Short results are written once (no ropes, no intermediate strings).
    if len * n as u64 <= 4096 && n <= 4096 {
        let l = len as u32;
        let mut pieces: Vec<(Str, u32, u32)> = (0..n).map(|_| (s, 0, l)).collect();
        return join(heap, &mut pieces);
    }
    let mut result = heap.alloc_latin1(&[])?;
    let mut part = s;
    let mut k = n;
    while k > 0 {
        if k & 1 == 1 {
            result = heap.concat(result, part)?;
        }
        k >>= 1;
        if k > 0 {
            part = heap.concat(part, part)?;
        }
    }
    Ok(result)
}

/// A string of the code units (`points` false: ToUint16 of each Number) or code points
/// (already validated) in the dense array `list`.
pub(crate) fn from_codes(heap: &mut Heap, list: Obj, points: bool) -> Result<Str, Fault> {
    let n = heap.array_length(list);
    let code = |v: Value| match v.as_int() {
        Some(i) => i as u32,
        None => {
            let x = v.as_number().unwrap_or(0.0);
            if points {
                x as u32
            } else {
                crate::interp::conv::to_int32(x) as u32
            }
        }
    };
    let emit = |out: &mut Units, v: Value| {
        if points {
            let c = code(v);
            if c >= 0x10000 {
                let c = c - 0x10000;
                out.push(0xD800 + (c >> 10) as u16);
                out.push(0xDC00 + (c & 0x3FF) as u16);
                return;
            }
            out.push(c as u16);
        } else {
            out.push(code(v) as u16);
        }
    };
    // A few units (`String.fromCharCode(c)` in a loop) go through a stack buffer: no Rust
    // allocation per call (D1871).
    let mut out = if n <= 32 {
        Units::Small([0; 64], 0)
    } else {
        Units::Big(crate::buf::vec_with_capacity(n as usize))
    };
    match heap.dense_array_values(list) {
        // One pass over the dense store (D1784).
        Some(vs) => vs.for_each(|v| emit(&mut out, v)),
        None => {
            for i in 0..n {
                let v = heap.get_element_fast(list, i).unwrap_or(Value::int(0));
                emit(&mut out, v);
            }
        }
    }
    Ok(heap.alloc_utf16(out.as_slice())?)
}

/// Code units collected on the stack while there are few, else in a Vec.
enum Units {
    Small([u16; 64], usize),
    Big(Vec<u16>),
}

impl Units {
    fn push(&mut self, u: u16) {
        match self {
            Units::Small(buf, n) if *n < buf.len() => {
                buf[*n] = u;
                *n += 1;
            }
            Units::Small(buf, n) => {
                let mut v = buf[..*n].to_vec();
                v.push_out(u);
                *self = Units::Big(v);
            }
            Units::Big(v) => v.push_out(u),
        }
    }

    fn as_slice(&self) -> &[u16] {
        match self {
            Units::Small(buf, n) => &buf[..*n],
            Units::Big(v) => v,
        }
    }
}

/// `normalize(form)` (form 0 NFC, 1 NFD, 2 NFKC, 3 NFKD). With the
/// `unicode-normalization` feature every string normalizes; strings that pass the quick
/// check come back unchanged. Without the tables only strings whose code units are all
/// below U+0300 (for NFD/NFKD/NFKC below U+00A0) are known to be normalized; `None` for
/// anything else (D1760).
pub(crate) fn normalize(heap: &mut Heap, s: Str, form: u8) -> Result<Option<Str>, Fault> {
    // Nothing below U+0300 (NFC) or U+00A0 (other forms) changes: checked in place,
    // without copying the units out.
    let limit = if form == 0 { 0x300 } else { 0xA0 };
    if heap.str_view_flat(s)?.units().all(|u| u < limit) {
        return Ok(Some(s));
    }
    #[cfg(not(feature = "unicode-normalization"))]
    return Ok(None);
    #[cfg(feature = "unicode-normalization")]
    {
        use crate::unicode::normalize::{self as norm, Form};
        let form = match form {
            0 => Form::Nfc,
            1 => Form::Nfd,
            2 => Form::Nfkc,
            _ => Form::Nfkd,
        };
        let u = units(heap, s)?;
        if norm::is_normalized_quick(&u, form) {
            return Ok(Some(s));
        }
        let out = norm::normalize(&u, form, &mut heap.compositions);
        if out == u {
            return Ok(Some(s));
        }
        Ok(Some(crate::builtins::regexp::string_from_units(
            heap, &out,
        )?))
    }
}

/// localeCompare without Intl, as QuickJS computes it: both strings NFC-normalized (with
/// the `unicode-normalization` feature), then the difference of the first differing code
/// points, else -1/0/1 by length.
pub(crate) fn locale_compare(heap: &mut Heap, a: Str, b: Str) -> Result<f64, Fault> {
    // One-byte strings are their own NFC form and have no surrogates: compared in place.
    let (a, b) = (heap.contiguous(a)?, heap.contiguous(b)?);
    if let (Some(StrView::Latin1(x)), Some(StrView::Latin1(y))) =
        (heap.str_view(a), heap.str_view(b))
    {
        if let Some((p, q)) = x.iter().zip(y).find(|(p, q)| p != q) {
            return Ok(f64::from(*p) - f64::from(*q));
        }
        return Ok(match x.len().cmp(&y.len()) {
            core::cmp::Ordering::Less => -1.0,
            core::cmp::Ordering::Equal => 0.0,
            core::cmp::Ordering::Greater => 1.0,
        });
    }
    let (ua, ub) = (units(heap, a)?, units(heap, b)?);
    #[cfg(feature = "unicode-normalization")]
    let (ua, ub) = {
        use crate::unicode::normalize::{self as norm, Form};
        let mut nfc = |u: Vec<u16>| {
            if norm::is_normalized_quick(&u, Form::Nfc) {
                u
            } else {
                norm::normalize(&u, Form::Nfc, &mut heap.compositions)
            }
        };
        (nfc(ua), nfc(ub))
    };
    let (ca, cb) = (code_points_of(&ua), code_points_of(&ub));
    for (x, y) in ca.iter().zip(&cb) {
        if x.1 != y.1 {
            return Ok(x.1 as f64 - y.1 as f64);
        }
    }
    Ok(match ca.len().cmp(&cb.len()) {
        core::cmp::Ordering::Less => -1.0,
        core::cmp::Ordering::Equal => 0.0,
        core::cmp::Ordering::Greater => 1.0,
    })
}

#[cfg(test)]
mod tests {
    use super::{find_byte, find_latin1};

    /// Against the naive searches, on random strings over small alphabets (so that
    /// matches, near misses and bytes with the high bit set all occur) at every
    /// alignment and length around the 8-byte chunks.
    #[test]
    fn word_at_a_time_search_matches_the_naive_one() {
        let mut seed = 0x9E37_79B9_7F4A_7C15u64;
        let mut next = |n: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % n) as u8
        };
        let alphabets: [&[u8]; 3] = [b"ab", b"abc\x80\xff\x00", b"\x01\x02\x7f\x80\x81"];
        for round in 0..8000 {
            let al = alphabets[round % 3];
            let n = next(90) as usize;
            let h: alloc::vec::Vec<u8> =
                (0..n).map(|_| al[next(al.len() as u64) as usize]).collect();
            let b = al[next(al.len() as u64) as usize];
            assert_eq!(
                find_byte(&h, b),
                h.iter().position(|&c| c == b),
                "{h:?} {b}"
            );
            let m = 1 + next(6) as usize;
            let t: alloc::vec::Vec<u8> =
                (0..m).map(|_| al[next(al.len() as u64) as usize]).collect();
            let from = next(n as u64 + 2) as usize;
            let naive = (from..(n + 1).saturating_sub(m)).find(|&i| h[i..i + m] == t[..]);
            assert_eq!(find_latin1(&h, &t, from), naive, "{h:?} {t:?} {from}");
        }
    }
}
