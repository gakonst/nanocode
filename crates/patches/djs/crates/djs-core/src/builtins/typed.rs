//! ArrayBuffer, typed array and DataView leaf intrinsics, and base64/hex (0039), over
//! `heap/typed.rs`.
use crate::PushOut;

use alloc::string::String;
use alloc::vec::Vec;

use crate::heap::cells::Bytes;
use crate::heap::typed::{self, Kind};
use crate::heap::{CellKind, Heap, ObjectClass};
use crate::interp::conv::{Fault, type_error};
use crate::interp::{ErrorKind, Intrinsic, realm};
use crate::value::{Obj, Value};

/// The largest ArrayBuffer (the arena caps it long before).
pub(crate) const MAX_BUFFER: f64 = (1u64 << 31) as f64;

pub(crate) fn range_error<T>(msg: &str) -> Result<T, Fault> {
    Err(Fault::error(ErrorKind::RangeError, msg.into()))
}

/// A zeroed Bytes cell of `len` bytes with room for `cap`.
fn alloc_bytes(heap: &mut Heap, len: u32, cap: u32) -> Result<u32, Fault> {
    let size = Bytes::DATA
        .cell_size(cap)
        .ok_or(crate::heap::HeapError::OutOfMemory)?;
    let b = heap.alloc_cell(CellKind::Bytes, size)?;
    heap.arena_mut().set(b, Bytes::LEN, len);
    let data = heap.arena_mut().slice_mut(Bytes::DATA.at(b, 0), cap);
    data.fill(0);
    Ok(b)
}

/// AllocateArrayBuffer with prototype `proto`; `max` is the maxByteLength of a
/// resizable buffer (or `None`).
pub(crate) fn buffer_create(
    heap: &mut Heap,
    proto: Obj,
    len: f64,
    max: Option<f64>,
) -> Result<Obj, Fault> {
    if !(0.0..=MAX_BUFFER).contains(&len) || max.is_some_and(|m| !(0.0..=MAX_BUFFER).contains(&m)) {
        return range_error("invalid array buffer length");
    }
    if let Some(m) = max
        && len > m
    {
        return range_error("invalid array buffer max length");
    }
    let cap = max.unwrap_or(len) as u32;
    let bytes = alloc_bytes(heap, len as u32, cap)?;
    let o = heap.alloc_object(ObjectClass::ArrayBuffer, Some(proto), 2)?;
    heap.set_object_slot(o, 0, Value::internal(bytes));
    heap.set_object_slot(o, 1, max.map_or(Value::UNDEFINED, Value::number));
    Ok(o)
}

/// A new ArrayBuffer (realm prototype) holding `data`.
pub(crate) fn buffer_from(heap: &mut Heap, data: &[u8]) -> Result<Obj, Fault> {
    let proto = realm::intrinsic_obj(heap, Intrinsic::ArrayBufferProto);
    let b = buffer_create(heap, proto, data.len() as f64, None)?;
    heap.buffer_data_mut(b).copy_from_slice(data);
    Ok(b)
}

pub(crate) fn as_buffer(heap: &Heap, v: Value) -> Result<Obj, Fault> {
    match v.as_object() {
        Some(o) if heap.object_class(o) == ObjectClass::ArrayBuffer => Ok(o),
        _ => type_error("ArrayBuffer object expected"),
    }
}

/// `%BufferInfo(b, field)`: 0 byteLength, 1 maxByteLength, 2 resizable, 3 detached,
/// 4 immutable.
pub(crate) fn buffer_info(heap: &Heap, b: Obj, field: i32) -> Value {
    let detached = heap.buffer_bytes(b).is_none();
    let max = heap.object_slot(b, 1);
    match field {
        0 => Value::int(heap.buffer_len(b) as i32),
        1 => {
            if detached {
                Value::int(0)
            } else if max.is_number() {
                max
            } else {
                Value::int(heap.buffer_len(b) as i32)
            }
        }
        2 => Value::bool(heap.buffer_resizable(b)),
        3 => Value::bool(detached),
        _ => Value::bool(heap.buffer_immutable(b)),
    }
}

/// DetachArrayBuffer: the data goes away; views see length 0. An immutable buffer is a
/// TypeError (D1970).
pub(crate) fn buffer_detach(heap: &mut Heap, b: Obj) -> Result<(), Fault> {
    if heap.buffer_immutable(b) {
        return type_error("ArrayBuffer is immutable");
    }
    heap.detach_buffer(b);
    Ok(())
}

/// Resizes a resizable buffer in place (its capacity is maxByteLength); new bytes are 0.
pub(crate) fn buffer_resize(heap: &mut Heap, b: Obj, len: u32) -> Result<(), Fault> {
    let Some(bytes) = heap.buffer_bytes(b) else {
        return type_error("ArrayBuffer is detached");
    };
    let old = heap.arena().get(bytes, Bytes::LEN);
    if len > old {
        heap.arena_mut()
            .slice_mut(Bytes::DATA.at(bytes, old), len - old)
            .fill(0);
    }
    heap.arena_mut().set(bytes, Bytes::LEN, len);
    Ok(())
}

/// ArrayBufferCopyAndDetach's copy: a new buffer of `len` bytes (resizable up to `max`
/// if given) with the start of `b`'s data; `b` is detached.
pub(crate) fn buffer_transfer(
    heap: &mut Heap,
    b: Obj,
    len: f64,
    max: Option<f64>,
    immutable: bool,
) -> Result<Obj, Fault> {
    let data: Vec<u8> = heap.buffer_data(b).to_vec();
    let proto = realm::intrinsic_obj(heap, Intrinsic::ArrayBufferProto);
    let new = buffer_create(heap, proto, len, max)?;
    let n = data.len().min(len as usize);
    heap.buffer_data_mut(new)[..n].copy_from_slice(&data[..n]);
    if immutable {
        heap.set_buffer_immutable(new);
    }
    buffer_detach(heap, b)?;
    Ok(new)
}

/// Copies `count` bytes between buffers (they may be the same; memmove semantics).
pub(crate) fn buffer_copy(heap: &mut Heap, src: Obj, from: u32, dst: Obj, to: u32, count: u32) {
    if count == 0 {
        return;
    }
    let tmp: Vec<u8> = heap.buffer_data(src)[from as usize..(from + count) as usize].to_vec();
    heap.buffer_data_mut(dst)[to as usize..(to + count) as usize].copy_from_slice(&tmp);
}

pub(crate) fn as_typed_array(heap: &Heap, v: Value) -> Result<Obj, Fault> {
    match v.as_object() {
        Some(o) if heap.object_class(o) == ObjectClass::TypedArray => Ok(o),
        _ => type_error("not a TypedArray"),
    }
}

/// A typed array over `buffer` (`length` -1: length-tracking).
pub(crate) fn typed_array_create(
    heap: &mut Heap,
    proto: Obj,
    kind: Kind,
    buffer: Obj,
    byte_offset: f64,
    length: f64,
) -> Result<Obj, Fault> {
    let o = heap.alloc_object(ObjectClass::TypedArray, Some(proto), 4)?;
    heap.set_object_slot(o, 0, Value::object(buffer));
    heap.set_object_slot(o, 1, Value::int(kind as i32));
    heap.set_object_slot(o, 2, Value::number(byte_offset));
    heap.set_object_slot(o, 3, Value::number(length));
    Ok(o)
}

/// `%TypedArrayField(o, field)`: 0 kind, 1 buffer, 2 byteOffset (0 out of bounds),
/// 3 length (0 out of bounds), 4 byteLength (0 out of bounds), 5 out of bounds or
/// detached, 6 length-tracking.
pub(crate) fn typed_array_field(heap: &Heap, o: Obj, field: i32) -> Value {
    let len = heap.ta_length(o);
    let kind = heap.ta_kind(o);
    match field {
        0 => Value::int(kind as i32),
        1 => heap.object_slot(o, 0),
        2 => Value::number(if len.is_some() {
            heap.ta_byte_offset(o) as f64
        } else {
            0.0
        }),
        3 => Value::number(len.unwrap_or(0) as f64),
        4 => Value::number(len.map_or(0.0, |n| n as f64 * kind.size() as f64)),
        5 => Value::bool(len.is_none()),
        6 => Value::bool(heap.object_slot(o, 3).as_number().is_none_or(|n| n < 0.0)),
        // [[ByteOffset]] itself, also when out of bounds (subarray, 23.2.3.30 step 13).
        _ => Value::number(heap.ta_byte_offset(o) as f64),
    }
}

/// Copies `count` elements from typed array `src` (from index `from`) to `dst` (at
/// `to`), converting between element types; overlapping ranges in one buffer are
/// handled (the source is read first).
pub(crate) fn typed_array_copy(
    heap: &mut Heap,
    src: Obj,
    from: u32,
    dst: Obj,
    to: u32,
    count: u32,
) {
    let (sk, dk) = (heap.ta_kind(src), heap.ta_kind(dst));
    if sk == dk {
        let size = sk.size();
        let (sb, db) = (heap.ta_buffer(src), heap.ta_buffer(dst));
        let so = heap.ta_byte_offset(src) + from * size;
        let d0 = heap.ta_byte_offset(dst) + to * size;
        buffer_copy(heap, sb, so, db, d0, count * size);
        return;
    }
    // Number kinds convert through f64; BigInt64 <-> BigUint64 keep the 64 bits.
    let values: Vec<typed::Elem> = (0..count)
        .map(|i| {
            heap.ta_elem(src, from + i)
                .unwrap_or(typed::Elem::Number(0.0))
        })
        .collect();
    for (i, e) in values.into_iter().enumerate() {
        heap.ta_set(dst, to + i as u32, e);
    }
}

// ---- base64 and hex (the uint8array-base64 proposal, ES2026) ------------------------------

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
const B64URL: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// The bytes of a Uint8Array (`None` if detached or out of bounds).
pub(crate) fn uint8_bytes(heap: &Heap, o: Obj) -> Option<&[u8]> {
    let len = heap.ta_length(o)? as usize;
    let off = heap.ta_byte_offset(o) as usize;
    Some(&heap.buffer_data(heap.ta_buffer(o))[off..off + len])
}

pub(crate) fn to_base64(bytes: &[u8], url: bool, omit_padding: bool) -> String {
    let table = if url { B64URL } else { B64 };
    let mut out: Vec<u8> = crate::buf::vec_with_capacity(bytes.len().div_ceil(3) * 4);
    // Whole groups of three bytes, four digits at a time (a `push` per digit was most of
    // btoa of a long string).
    let (groups, rest) = bytes.as_chunks::<3>();
    for g in groups {
        let n = (g[0] as u32) << 16 | (g[1] as u32) << 8 | g[2] as u32;
        out.extend_from_slice(&[
            table[(n >> 18) as usize & 63],
            table[(n >> 12) as usize & 63],
            table[(n >> 6) as usize & 63],
            table[n as usize & 63],
        ]);
    }
    if !rest.is_empty() {
        let n = (rest[0] as u32) << 16 | (*rest.get(1).unwrap_or(&0) as u32) << 8;
        let chars = [
            table[(n >> 18) as usize & 63],
            table[(n >> 12) as usize & 63],
            table[(n >> 6) as usize & 63],
        ];
        let keep = rest.len() + 1;
        out.extend_from_slice(&chars[..keep]);
        if !omit_padding {
            out.extend(core::iter::repeat_n(b'=', 4 - keep));
        }
    }
    String::from_utf8(out).expect("base64 digits are ASCII")
}

pub(crate) fn to_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push_out(HEX[(b >> 4) as usize] as char);
        out.push_out(HEX[(b & 15) as usize] as char);
    }
    out
}

/// The result of FromBase64 / FromHex: code units read, the bytes, and whether it
/// stopped at an error (a SyntaxError for the caller).
pub(crate) struct Decoded {
    pub read: usize,
    pub bytes: Vec<u8>,
    pub error: bool,
}

/// The value of base64 digit `c` (by range: a search of the alphabet was most of decoding
/// a dump's buffers, 0215).
fn b64_value(c: u16) -> Option<u32> {
    let c = u32::from(c);
    match c {
        0x41..=0x5A => Some(c - 0x41),
        0x61..=0x7A => Some(c - 0x61 + 26),
        0x30..=0x39 => Some(c - 0x30 + 52),
        0x2B => Some(62),
        0x2F => Some(63),
        _ => None,
    }
}

fn is_ascii_ws(c: u16) -> bool {
    matches!(c, 0x09 | 0x0A | 0x0C | 0x0D | 0x20)
}

/// DecodeBase64Chunk: `None` when strict and the unused bits are not zero.
fn decode_chunk(chunk: &[u32], strict: bool, out: &mut Vec<u8>) -> bool {
    let mut n = 0u32;
    for (i, &v) in chunk.iter().enumerate() {
        n |= v << (18 - 6 * i);
    }
    let bytes = [(n >> 16) as u8, (n >> 8) as u8, n as u8];
    match chunk.len() {
        2 => {
            if strict && bytes[1] != 0 {
                return false;
            }
            out.push_out(bytes[0]);
        }
        3 => {
            if strict && bytes[2] != 0 {
                return false;
            }
            out.extend_from_slice(&bytes[..2]);
        }
        _ => out.extend_from_slice(&bytes),
    }
    true
}

/// atob's FromBase64 (the base64 alphabet, loose) of one-byte code units: the leading
/// whole groups of four digits straight from the bytes, the rest (white space, padding,
/// a partial group) through [`from_base64`], which decodes it the same after whole
/// groups. `None` for an invalid string.
pub(crate) fn from_base64_latin1(s: &[u8]) -> Option<Vec<u8>> {
    let mut bytes = crate::buf::vec_with_capacity(s.len() / 4 * 3 + 3);
    let (groups, _) = s.as_chunks::<4>();
    let mut read = 0;
    for g in groups {
        let v = |k: usize| b64_value(u16::from(g[k]));
        let (Some(a), Some(b), Some(c), Some(e)) = (v(0), v(1), v(2), v(3)) else {
            break;
        };
        let n = (a << 18) | (b << 12) | (c << 6) | e;
        bytes.extend_from_slice(&[(n >> 16) as u8, (n >> 8) as u8, n as u8]);
        read += 4;
    }
    let rest: Vec<u16> = s[read..].iter().map(|&c| u16::from(c)).collect();
    let d = from_base64(&rest, false, 0, usize::MAX);
    if d.error {
        return None;
    }
    bytes.extend_from_slice(&d.bytes);
    Some(bytes)
}

/// FromBase64 ( string, alphabet, lastChunkHandling [ , maxLength ] ). `last`: 0 loose,
/// 1 strict, 2 stop-before-partial.
pub(crate) fn from_base64(s: &[u16], url: bool, last: u8, max: usize) -> Decoded {
    let mut d = Decoded {
        read: 0,
        bytes: Vec::new(),
        error: false,
    };
    if max == 0 {
        return d;
    }
    let fail = |mut d: Decoded| {
        d.error = true;
        d
    };
    let len = s.len();
    let mut chunk: Vec<u32> = crate::buf::vec_with_capacity(4);
    let mut i = 0;
    // Whole groups of four plain digits first, while they fit (the common case): the
    // same bytes as the steps below, without their per-unit checks. Only in the dump
    // module, which loads dumps' buffers (+250 B gzip in the slim module, 0215), and for
    // Rust hosts, whose scripts decode with atob.
    #[cfg(any(feature = "dump", feature = "embed"))]
    d.bytes.reserve(len.min(max) / 4 * 3);
    #[cfg(any(feature = "dump", feature = "embed"))]
    while i + 4 <= len && max - d.bytes.len() >= 3 {
        let v = |k: usize| {
            let c = s[i + k];
            let c = match (url, c) {
                (true, 0x2B | 0x2F) => return None,
                (true, 0x2D) => 0x2B,
                (true, 0x5F) => 0x2F,
                _ => c,
            };
            b64_value(c)
        };
        let (Some(a), Some(b), Some(c), Some(e)) = (v(0), v(1), v(2), v(3)) else {
            break;
        };
        let n = (a << 18) | (b << 12) | (c << 6) | e;
        d.bytes
            .extend_from_slice(&[(n >> 16) as u8, (n >> 8) as u8, n as u8]);
        i += 4;
        d.read = i;
    }
    #[cfg(any(feature = "dump", feature = "embed"))]
    if d.bytes.len() == max {
        return d;
    }
    loop {
        while i < len && is_ascii_ws(s[i]) {
            i += 1;
        }
        if i == len {
            if !chunk.is_empty() {
                if last == 2 {
                    return d;
                }
                if last == 1 || chunk.len() == 1 {
                    return fail(d);
                }
                decode_chunk(&chunk, false, &mut d.bytes);
            }
            d.read = len;
            return d;
        }
        let mut c = s[i];
        i += 1;
        if c == b'=' as u16 {
            if chunk.len() < 2 {
                return fail(d);
            }
            while i < len && is_ascii_ws(s[i]) {
                i += 1;
            }
            if chunk.len() == 2 {
                if i == len {
                    if last == 2 {
                        return d;
                    }
                    return fail(d);
                }
                if s[i] == b'=' as u16 {
                    i += 1;
                    while i < len && is_ascii_ws(s[i]) {
                        i += 1;
                    }
                }
            }
            if i < len {
                return fail(d);
            }
            if !decode_chunk(&chunk, last == 1, &mut d.bytes) {
                return fail(d);
            }
            d.read = len;
            return d;
        }
        if url {
            if c == b'+' as u16 || c == b'/' as u16 {
                return fail(d);
            }
            if c == b'-' as u16 {
                c = b'+' as u16;
            } else if c == b'_' as u16 {
                c = b'/' as u16;
            }
        }
        let Some(v) = b64_value(c) else {
            return fail(d);
        };
        let remaining = max - d.bytes.len();
        if (remaining == 1 && chunk.len() == 2) || (remaining == 2 && chunk.len() == 3) {
            return d;
        }
        chunk.push_out(v);
        if chunk.len() == 4 {
            decode_chunk(&chunk, false, &mut d.bytes);
            chunk.clear();
            d.read = i;
            if d.bytes.len() == max {
                return d;
            }
        }
    }
}

/// FromHex ( string [ , maxLength ] ).
pub(crate) fn from_hex(s: &[u16], max: usize) -> Decoded {
    let mut d = Decoded {
        read: 0,
        bytes: Vec::new(),
        error: false,
    };
    if !s.len().is_multiple_of(2) {
        d.error = true;
        return d;
    }
    let digit = |c: u16| (c < 128).then(|| (c as u8 as char).to_digit(16)).flatten();
    while d.read < s.len() && d.bytes.len() < max {
        let (Some(h), Some(l)) = (digit(s[d.read]), digit(s[d.read + 1])) else {
            d.error = true;
            return d;
        };
        d.read += 2;
        d.bytes.push_out((h * 16 + l) as u8);
    }
    d
}

/// The result of a decode as a record for the builtins: `{ read, bytes (an
/// ArrayBuffer), error }`, a prototype-less object.
pub(crate) fn decoded_record(heap: &mut Heap, d: Decoded) -> Result<Value, Fault> {
    let buffer = buffer_from(heap, &d.bytes)?;
    let keys = [
        heap.atom_str("read")?,
        heap.atom_str("bytes")?,
        heap.atom_str("error")?,
    ];
    let values = [
        Value::number(d.read as f64),
        Value::object(buffer),
        Value::bool(d.error),
    ];
    Ok(Value::object(
        heap.new_object_from_entries(None, &keys, &values)?,
    ))
}

/// The kind of a typed array as its constructor's realm intrinsic prototype.
pub(crate) fn kind_proto(kind: Kind) -> Intrinsic {
    match kind {
        Kind::Int8 => Intrinsic::Int8ArrayProto,
        Kind::Uint8 => Intrinsic::Uint8ArrayProto,
        Kind::Uint8Clamped => Intrinsic::Uint8ClampedArrayProto,
        Kind::Int16 => Intrinsic::Int16ArrayProto,
        Kind::Uint16 => Intrinsic::Uint16ArrayProto,
        Kind::Int32 => Intrinsic::Int32ArrayProto,
        Kind::Uint32 => Intrinsic::Uint32ArrayProto,
        Kind::Float16 => Intrinsic::Float16ArrayProto,
        Kind::Float32 => Intrinsic::Float32ArrayProto,
        Kind::Float64 => Intrinsic::Float64ArrayProto,
        Kind::BigInt64 => Intrinsic::BigInt64ArrayProto,
        Kind::BigUint64 => Intrinsic::BigUint64ArrayProto,
    }
}

// ---- DataView ----------------------------------------------------------------------------

/// The DataView `v`, TypeError "DataView object expected" otherwise.
pub(crate) fn as_data_view(heap: &Heap, v: Value) -> Result<Obj, Fault> {
    match v.as_object() {
        Some(o) if heap.object_class(o) == ObjectClass::DataView => Ok(o),
        _ => type_error("DataView object expected"),
    }
}

/// A DataView over `buffer`: slot 0 the buffer, 1 the byte offset, 2 the byte length
/// (-1: length-tracking).
pub(crate) fn data_view_create(
    heap: &mut Heap,
    proto: Obj,
    buffer: Obj,
    byte_offset: f64,
    byte_length: f64,
) -> Result<Obj, Fault> {
    let o = heap.alloc_object(ObjectClass::DataView, Some(proto), 3)?;
    heap.set_object_slot(o, 0, Value::object(buffer));
    heap.set_object_slot(o, 1, Value::number(byte_offset));
    heap.set_object_slot(o, 2, Value::number(byte_length));
    Ok(o)
}

/// GetValueFromBuffer for a DataView: the element of `kind` at byte `at` of the view's
/// buffer (absolute), `little` endian or not. The caller checked the bounds; a stale
/// index reads NaN instead of panicking.
pub(crate) fn view_get(heap: &Heap, view: Obj, at: f64, kind: Kind, little: bool) -> typed::Elem {
    let size = kind.size() as usize;
    let Some(range) = view_range(heap, view, at, size) else {
        return typed::Elem::Number(f64::NAN);
    };
    let buffer = heap
        .object_slot(view, 0)
        .as_object()
        .expect("DataView buffer");
    let mut b = [0u8; 8];
    b[..size].copy_from_slice(&heap.buffer_data(buffer)[range]);
    if !little {
        b[..size].reverse();
    }
    typed::decode_elem(kind, &b[..size])
}

/// SetValueInBuffer for a DataView (see `view_get`); out of range writes nothing.
pub(crate) fn view_set(
    heap: &mut Heap,
    view: Obj,
    at: f64,
    kind: Kind,
    e: typed::Elem,
    little: bool,
) {
    let size = kind.size() as usize;
    let Some(range) = view_range(heap, view, at, size) else {
        return;
    };
    let buffer = heap
        .object_slot(view, 0)
        .as_object()
        .expect("DataView buffer");
    let mut b = [0u8; 8];
    typed::encode_elem(kind, e, &mut b);
    if !little {
        b[..size].reverse();
    }
    heap.buffer_data_mut(buffer)[range].copy_from_slice(&b[..size]);
}

fn view_range(heap: &Heap, view: Obj, at: f64, size: usize) -> Option<core::ops::Range<usize>> {
    let buffer = heap.object_slot(view, 0).as_object()?;
    heap.buffer_bytes(buffer)?;
    if !(at >= 0.0 && at == djs_libm::trunc(at)) {
        return None;
    }
    let start = at as usize;
    (start + size <= heap.buffer_len(buffer) as usize).then_some(start..start + size)
}
