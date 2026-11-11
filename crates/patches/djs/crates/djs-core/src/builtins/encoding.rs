//! TextEncoder and TextDecoder, UTF-8 only (the WHATWG Encoding standard; 0244, D3172):
//! the leaf work of builtins/encoding.ts, which converts the arguments, checks labels and
//! finds the bytes of a BufferSource (a buffer, an offset and a length). Both are built
//! from the transcoders the engine has anyway (D3210): `from_utf8_lossy` (a U+FFFD per
//! maximal invalid subpart, as the Encoding standard's decoder) and `alloc_str` to decode,
//! `str_to_string` (lone surrogates as U+FFFD) and `decode_utf16` to encode.
//!
//! A TextEncoder or TextDecoder is an object of class `TextCodec` with two slots: the
//! flags (int32: [`DECODER`], [`FATAL`], [`IGNORE_BOM`]) and a decoder's stream state
//! (int32: the up to three bytes of an incomplete sequence that `stream: true` held back
//! in bits 0..24, their number in bits 24..26, then [`BOM_SEEN`] and [`DO_NOT_FLUSH`]).

use alloc::string::String;
use alloc::vec::Vec;

use crate::heap::string::is_ascii;
use crate::heap::{Heap, ObjectClass, StrView};
use crate::interp::conv::{Fault, type_error};
use crate::interp::{Intrinsic, realm};
use crate::value::{Obj, Str, Value};

/// Flag bits of a codec's slot 0.
const DECODER: i32 = 1;
const FATAL: i32 = 2;
const IGNORE_BOM: i32 = 4;
/// State bits of a decoder's slot 1 (the spec's BOM seen and do not flush).
const BOM_SEEN: u32 = 1 << 26;
const DO_NOT_FLUSH: u32 = 1 << 27;

/// The leaves of builtins/encoding.ts, by their index from `%TextCodecCreate`: create,
/// flags, IsBufferSource, encode, decode (builtins/intrinsics.d.ts). `None` for an
/// argument of the wrong type.
pub(crate) fn intrinsic(
    heap: &mut Heap,
    k: u16,
    a: [Value; 5],
    fuel: &mut i64,
) -> Result<Option<Value>, Fault> {
    let range = [a[1], a[2], a[3]];
    Ok(Some(match (k, a[0].as_object(), a[0].as_string()) {
        (0, Some(proto), _) => {
            let o = heap.alloc_object(ObjectClass::TextCodec, Some(proto), 2)?;
            heap.set_object_slot(o, 0, a[1]);
            heap.set_object_slot(o, 1, Value::int(0));
            Value::object(o)
        }
        (1, ..) => Value::int(flags(heap, a[0], a[1] == Value::TRUE)?),
        (2, o, _) => Value::bool(o.is_some_and(|o| {
            matches!(
                heap.object_class(o),
                ObjectClass::ArrayBuffer | ObjectClass::TypedArray | ObjectClass::DataView
            )
        })),
        (3, _, Some(s)) => encode(heap, s, range, fuel)?,
        (4, Some(d), _) => decode(heap, d, range, a[4] == Value::TRUE, fuel)?,
        _ => return Ok(None),
    }))
}

/// The flags of `v` if it is a TextDecoder (`decoder`) or a TextEncoder, else WebIDL's
/// TypeError (in node's words).
fn flags(heap: &Heap, v: Value, decoder: bool) -> Result<i32, Fault> {
    if let Some(o) = v.as_object()
        && heap.object_class(o) == ObjectClass::TextCodec
    {
        let f = heap.object_slot(o, 0).as_int().unwrap_or(0);
        if (f & DECODER != 0) == decoder {
            return Ok(f);
        }
    }
    type_error(if decoder {
        "Value of \"this\" must be of type TextDecoder"
    } else {
        "Value of \"this\" must be of type TextEncoder"
    })
}

/// The bytes `offset..offset + length` of the ArrayBuffer `buffer` (`length` negative:
/// to its end); none when it is detached, not a buffer or the range is out of bounds.
fn bytes_of(heap: &Heap, buffer: Value, offset: Value, length: Value) -> &[u8] {
    let data = match buffer.as_object() {
        Some(b) if heap.object_class(b) == ObjectClass::ArrayBuffer => heap.buffer_data(b),
        _ => &[],
    };
    let num = |v: Value| v.as_number().unwrap_or(0.0);
    let start = num(offset) as usize;
    let end = if num(length) < 0.0 {
        data.len()
    } else {
        start.saturating_add(num(length) as usize)
    };
    data.get(start..end).unwrap_or(&[])
}

/// The length of an incomplete (but so far valid) UTF-8 sequence at the end of `b`, 0 if
/// there is none: what `stream: true` holds back for the next call.
fn incomplete_tail(b: &[u8]) -> usize {
    for k in 1..=b.len().min(3) {
        if b[b.len() - k] & 0xC0 != 0x80 {
            return match core::str::from_utf8(&b[b.len() - k..]) {
                Err(e) if e.valid_up_to() == 0 && e.error_len().is_none() => k,
                _ => 0,
            };
        }
    }
    0
}

/// TextDecoder.prototype.decode's steps after the conversions: decodes the bytes (after
/// those the last `stream: true` call held back) as UTF-8. `undefined` for invalid input
/// to a fatal decoder, which then starts over (as node's does).
fn decode(
    heap: &mut Heap,
    d: Obj,
    input: [Value; 3],
    stream: bool,
    fuel: &mut i64,
) -> Result<Value, Fault> {
    let flags = heap.object_slot(d, 0).as_int().unwrap_or(0);
    let mut state = heap.object_slot(d, 1).as_int().unwrap_or(0) as u32;
    if state & DO_NOT_FLUSH == 0 {
        state = 0;
    }
    let src = bytes_of(heap, input[0], input[1], input[2]);
    let held = (state >> 24 & 3) as usize;
    let mut bytes: Vec<u8> = crate::buf::vec_with_capacity(held + src.len());
    bytes.extend((0..held).map(|i| (state >> (8 * i)) as u8));
    bytes.extend_from_slice(src);
    *fuel -= (bytes.len() / 8) as i64;
    let keep = if stream { incomplete_tail(&bytes) } else { 0 };
    let body = &bytes[..bytes.len() - keep];
    let mut next = 0;
    if stream {
        next = DO_NOT_FLUSH | state & BOM_SEEN | (keep as u32) << 24;
        for (i, &b) in bytes[body.len()..].iter().enumerate() {
            next |= (b as u32) << (8 * i);
        }
    }
    // The first code point decides about the BOM: a later U+FEFF is text.
    if !body.is_empty() {
        next |= BOM_SEEN;
    }
    let text = String::from_utf8_lossy(body);
    let failed = flags & FATAL != 0 && matches!(text, alloc::borrow::Cow::Owned(_));
    heap.set_object_slot(d, 1, Value::int(if failed { 0 } else { next as i32 }));
    if failed {
        return Ok(Value::UNDEFINED);
    }
    // U+FEFF is EF BB BF.
    let skip = if flags & IGNORE_BOM == 0
        && state & BOM_SEEN == 0
        && text.as_bytes().starts_with(&[0xEF, 0xBB, 0xBF])
    {
        3
    } else {
        0
    };
    Ok(Value::string(heap.alloc_str(&text[skip..])?))
}

/// TextEncoder.prototype.encode (`dest[0]` undefined): a new Uint8Array of the UTF-8 of
/// `s`, lone surrogates as U+FFFD. encodeInto (`dest` a Uint8Array's buffer, byte offset
/// and byte length): writes the longest prefix of whole code points that fits,
/// `{ read, written }`.
fn encode(heap: &mut Heap, s: Str, dest: [Value; 3], fuel: &mut i64) -> Result<Value, Fault> {
    *fuel -= i64::from(heap.str_len(s) / 8);
    let text = match heap.str_view_flat(s)? {
        // ASCII: the bytes as they are.
        StrView::Latin1(b) if is_ascii(b) => b.to_vec(),
        _ => heap.str_to_string(s).into_bytes(),
    };
    let Some(b) = dest[0].as_object() else {
        let bp = realm::intrinsic_obj(heap, Intrinsic::ArrayBufferProto);
        let p = realm::intrinsic_obj(heap, Intrinsic::Uint8ArrayProto);
        return Ok(Value::object(heap.new_uint8_array(
            &text,
            Some(bp),
            Some(p),
        )?));
    };
    let mut n = bytes_of(heap, dest[0], dest[1], dest[2])
        .len()
        .min(text.len());
    // Back to the start of a code point (a continuation byte is 0b10xxxxxx).
    while n > 0 && n < text.len() && text[n] & 0xC0 == 0x80 {
        n -= 1;
    }
    if n > 0 {
        let at = dest[1].as_number().unwrap_or(0.0) as usize;
        heap.buffer_data_mut(b)[at..at + n].copy_from_slice(&text[..n]);
    }
    // Code units read: one per code point, two for those that take four bytes (a lone
    // surrogate, one unit, became the three bytes of U+FFFD).
    let read = text[..n]
        .iter()
        .map(|&c| usize::from(c & 0xC0 != 0x80) + usize::from(c >= 0xF0))
        .sum();
    let keys = [heap.atom_str("read")?, heap.atom_str("written")?];
    let int = |x: usize| i32::try_from(x).map_or(Value::number(x as f64), Value::int);
    let values = [int(read), int(n)];
    let proto = realm::intrinsic_obj(heap, Intrinsic::ObjectProto);
    Ok(Value::object(heap.new_object_from_entries(
        Some(proto),
        &keys,
        &values,
    )?))
}
