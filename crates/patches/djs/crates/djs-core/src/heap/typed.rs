//! ArrayBuffers, typed arrays and DataViews (0039, D1390).
//!
//! An ArrayBuffer (`ObjectClass::ArrayBuffer`) keeps its bytes in a [`Bytes`] cell
//! (slot 0, an internal reference; `undefined` once detached), with `LEN` the current
//! byte length and the cell's capacity at least the maximum byte length of a resizable
//! buffer (slot 1: a Number, `undefined` for a fixed-length buffer, or `true` for an
//! immutable one, which is fixed-length too: D1970), so resizing never moves the data.
//!
//! A typed array (`ObjectClass::TypedArray`) has slots: 0 the buffer object, 1 the
//! element kind ([`Kind`]), 2 the byte offset, 3 the length in elements or -1 for a
//! length-tracking view of a resizable buffer. A DataView (`ObjectClass::DataView`):
//! 0 the buffer, 1 the byte offset, 2 the byte length or -1 (length-tracking).
//!
//! The integer-indexed exotic behavior (10.4.5) lives in the heap's property functions:
//! a canonical numeric key on a typed array never reaches the property table or the
//! prototype chain. Element values are converted here only from primitives; objects
//! and symbols need ToNumber with user code, which the interpreter and the builtins do
//! first.
//!
//! BigInt64Array and BigUint64Array (0046, D1463) hold BigInts: an element is an
//! [`Elem`] (a Number, or the 64 element bits) until it becomes a value. Making a
//! BigInt value allocates, so the `&self` reads ([`Heap::ta_get`], and through it
//! `get_own_property`/`find_property`) give `Value::HOLE` as a placeholder for a BigInt
//! element; code that hands element values to JS uses the allocating
//! [`Heap::ta_get_alloc`], [`Heap::get_own_property_alloc`] or
//! [`Heap::find_property_alloc`]. Writes take an [`Elem`]: a BigInt kind only accepts a
//! BigInt value (anything else needs ToBigInt, done by the caller), a Number kind only
//! Numbers (a BigInt needs ToNumber, which throws).

use super::cells::Bytes;
use super::{Atom, Heap, HeapError, ObjectClass, Property};
use crate::bigint::BigInt;
use crate::value::{Obj, Value};

/// A typed array element without its allocation: a Number, or the 64 bits of a BigInt
/// element (two's complement; the kind says whether they are signed).
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Elem {
    Number(f64),
    Big(u64),
}

/// The element types, in the order of `Kind as i32` (the builtins use the numbers).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Int8 = 0,
    Uint8 = 1,
    Uint8Clamped = 2,
    Int16 = 3,
    Uint16 = 4,
    Int32 = 5,
    Uint32 = 6,
    Float16 = 7,
    Float32 = 8,
    Float64 = 9,
    BigInt64 = 10,
    BigUint64 = 11,
}

impl Kind {
    pub fn from_i32(k: i32) -> Option<Kind> {
        use Kind::*;
        Some(match k {
            0 => Int8,
            1 => Uint8,
            2 => Uint8Clamped,
            3 => Int16,
            4 => Uint16,
            5 => Int32,
            6 => Uint32,
            7 => Float16,
            8 => Float32,
            9 => Float64,
            10 => BigInt64,
            11 => BigUint64,
            _ => return None,
        })
    }

    /// BigInt64 or BigUint64: the elements are BigInts.
    pub fn is_bigint(self) -> bool {
        matches!(self, Kind::BigInt64 | Kind::BigUint64)
    }

    /// Bytes per element.
    pub fn size(self) -> u32 {
        use Kind::*;
        match self {
            Int8 | Uint8 | Uint8Clamped => 1,
            Int16 | Uint16 | Float16 => 2,
            Int32 | Uint32 | Float32 => 4,
            Float64 | BigInt64 | BigUint64 => 8,
        }
    }

    /// The constructor name (`Uint8Array`, ...).
    pub fn name(self) -> &'static str {
        use Kind::*;
        match self {
            Int8 => "Int8Array",
            Uint8 => "Uint8Array",
            Uint8Clamped => "Uint8ClampedArray",
            Int16 => "Int16Array",
            Uint16 => "Uint16Array",
            Int32 => "Int32Array",
            Uint32 => "Uint32Array",
            Float16 => "Float16Array",
            Float32 => "Float32Array",
            Float64 => "Float64Array",
            BigInt64 => "BigInt64Array",
            BigUint64 => "BigUint64Array",
        }
    }
}

/// ToInt32-style modular conversion of a Number to an integer of `bits` bits.
fn to_int_bits(n: f64) -> u64 {
    if !n.is_finite() {
        return 0;
    }
    let t = djs_libm::trunc(n);
    if t.abs() < 9223372036854775808.0 {
        // Two's complement modulo 2^64; every element size divides 64.
        return t as i64 as u64;
    }
    // Large magnitudes: reduce modulo 2^64 exactly (t is an integer, so is the rest).
    let m = djs_libm::fmod(t, 18446744073709551616.0);
    let m = if m < 0.0 {
        m + 18446744073709551616.0
    } else {
        m
    };
    if m >= 18446744073709551616.0 {
        0
    } else {
        m as u64
    }
}

/// ToUint8Clamp.
fn to_uint8_clamp(n: f64) -> u8 {
    if n.is_nan() || n <= 0.0 {
        return 0;
    }
    if n >= 255.0 {
        return 255;
    }
    let f = djs_libm::floor(n);
    if f + 0.5 < n {
        return (f + 1.0) as u8;
    }
    if n < f + 0.5 {
        return f as u8;
    }
    // A tie: round to even.
    if (f as u8) % 2 == 1 {
        f as u8 + 1
    } else {
        f as u8
    }
}

/// f64 → IEEE binary16 bits, rounding to nearest with ties to even (directly from the
/// f64 bits, so there is no double rounding).
pub fn f64_to_f16_bits(x: f64) -> u16 {
    let bits = x.to_bits();
    let sign = ((bits >> 63) as u16) << 15;
    let exp = ((bits >> 52) & 0x7FF) as i32;
    let mant = bits & ((1u64 << 52) - 1);
    if exp == 0x7FF {
        // NaN is the canonical quiet NaN 0x7e00, without the sign (D1750).
        return if mant != 0 { 0x7E00 } else { sign | 0x7C00 };
    }
    let round = |m: u64, shift: u32| {
        let q = m >> shift;
        let rem = m & ((1u64 << shift) - 1);
        let half = 1u64 << (shift - 1);
        if rem > half || (rem == half && q & 1 == 1) {
            q + 1
        } else {
            q
        }
    };
    let e = exp - 1023 + 15;
    if e >= 31 {
        return sign | 0x7C00;
    }
    if e <= 0 {
        // Subnormal (or zero): the value is m * 2^-24.
        if e < -10 {
            return sign;
        }
        let m = mant | (1u64 << 52);
        let r = round(m, (42 + 1 - e) as u32);
        // r may reach 0x400, the smallest normal: the same bits.
        return sign | r as u16;
    }
    let r = ((e as u64) << 10) + round(mant, 42);
    if r >= 0x7C00 {
        return sign | 0x7C00;
    }
    sign | r as u16
}

/// IEEE binary16 bits → f64.
pub fn f16_bits_to_f64(h: u16) -> f64 {
    let sign = if h & 0x8000 != 0 { -1.0 } else { 1.0 };
    let e = ((h >> 10) & 0x1F) as i32;
    let m = (h & 0x3FF) as f64;
    match e {
        0 => sign * m * djs_libm::ldexp(1.0, -24),
        31 if m == 0.0 => sign * f64::INFINITY,
        31 => f64::NAN,
        _ => sign * (1.0 + m / 1024.0) * djs_libm::ldexp(1.0, e - 15),
    }
}

/// The canonical binary32 quiet NaN (`0x7fc00000`), what qjs and V8 write.
const F32_CANONICAL_NAN: u32 = 0x7FC0_0000;

/// Number → binary32 for Float32 elements and `Math.fround` (D1750). NaN is written as
/// the canonical quiet NaN explicitly: the f64 → f32 demotion `as f32` compiles to
/// `f32.demote_f64` on wasm, whose NaN result bits the wasm spec leaves
/// nondeterministic, and the bits are observable through a Uint8Array over the buffer.
pub fn f64_to_f32_bits(n: f64) -> u32 {
    if n.is_nan() {
        F32_CANONICAL_NAN
    } else {
        (n as f32).to_bits()
    }
}

/// binary32 → Number. NaN comes out as `f64::NAN` (`f64.promote_f32` has the same
/// nondeterministic NaN bits as demotion).
pub fn f32_bits_to_f64(b: u32) -> f64 {
    let f = f32::from_bits(b);
    if f.is_nan() { f64::NAN } else { f as f64 }
}

/// Encodes Number `n` as an element of `kind` (little endian), into `out[..size]`.
pub fn encode(kind: Kind, n: f64, out: &mut [u8; 8]) {
    use Kind::*;
    match kind {
        Int8 | Uint8 => out[0] = to_int_bits(n) as u8,
        Uint8Clamped => out[0] = to_uint8_clamp(n),
        Int16 | Uint16 => out[..2].copy_from_slice(&(to_int_bits(n) as u16).to_le_bytes()),
        Int32 | Uint32 => out[..4].copy_from_slice(&(to_int_bits(n) as u32).to_le_bytes()),
        Float16 => out[..2].copy_from_slice(&f64_to_f16_bits(n).to_le_bytes()),
        Float32 => out[..4].copy_from_slice(&f64_to_f32_bits(n).to_le_bytes()),
        Float64 => out[..8].copy_from_slice(&n.to_le_bytes()),
        BigInt64 | BigUint64 => out[..8].copy_from_slice(&to_int_bits(n).to_le_bytes()),
    }
}

/// Decodes an element of `kind` from little-endian bytes.
pub fn decode(kind: Kind, b: &[u8]) -> f64 {
    use Kind::*;
    match kind {
        Int8 => b[0] as i8 as f64,
        Uint8 | Uint8Clamped => b[0] as f64,
        Int16 => i16::from_le_bytes([b[0], b[1]]) as f64,
        Uint16 => u16::from_le_bytes([b[0], b[1]]) as f64,
        Int32 => i32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f64,
        Uint32 => u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f64,
        Float16 => f16_bits_to_f64(u16::from_le_bytes([b[0], b[1]])),
        Float32 => f32_bits_to_f64(u32::from_le_bytes([b[0], b[1], b[2], b[3]])),
        Float64 => f64::from_le_bytes(b[..8].try_into().unwrap()),
        BigInt64 => i64::from_le_bytes(b[..8].try_into().unwrap()) as f64,
        BigUint64 => u64::from_le_bytes(b[..8].try_into().unwrap()) as f64,
    }
}

/// Encodes an element of `kind` (little endian) into `out[..size]`.
pub fn encode_elem(kind: Kind, e: Elem, out: &mut [u8; 8]) {
    match e {
        Elem::Big(bits) if kind.is_bigint() => out.copy_from_slice(&bits.to_le_bytes()),
        Elem::Big(bits) => encode(kind, bits as i64 as f64, out),
        Elem::Number(n) => encode(kind, n, out),
    }
}

/// Decodes an element of `kind` from little-endian bytes.
pub fn decode_elem(kind: Kind, b: &[u8]) -> Elem {
    if kind.is_bigint() {
        Elem::Big(u64::from_le_bytes(b[..8].try_into().unwrap()))
    } else {
        Elem::Number(decode(kind, b))
    }
}

/// The BigInt of a BigInt element's bits.
pub fn elem_bigint(kind: Kind, bits: u64) -> BigInt {
    if kind == Kind::BigInt64 {
        BigInt::from_i64(bits as i64)
    } else {
        BigInt::from_u64(bits)
    }
}

/// A Number as a Value in its canonical form (int where it fits).
fn number(n: f64) -> Value {
    let i = n as i32;
    if i as f64 == n && !(i == 0 && n.is_sign_negative()) {
        Value::int(i)
    } else {
        Value::number(n)
    }
}

impl Heap {
    /// A new ArrayBuffer of `data` (prototype `buffer_proto`) and a Uint8Array over all
    /// of it (prototype `proto`): what wire bytes decode to.
    pub fn new_uint8_array(
        &mut self,
        data: &[u8],
        buffer_proto: Option<Obj>,
        proto: Option<Obj>,
    ) -> Result<Obj, super::HeapError> {
        let len = u32::try_from(data.len()).map_err(|_| super::HeapError::OutOfMemory)?;
        let size = Bytes::DATA
            .cell_size(len)
            .ok_or(super::HeapError::OutOfMemory)?;
        let bytes = self.alloc_cell(super::CellKind::Bytes, size)?;
        self.arena_mut().set(bytes, Bytes::LEN, len);
        self.arena_mut()
            .slice_mut(Bytes::DATA.at(bytes, 0), len)
            .copy_from_slice(data);
        let buffer = self.alloc_object(ObjectClass::ArrayBuffer, buffer_proto, 2)?;
        self.set_object_slot(buffer, 0, Value::internal(bytes));
        self.set_object_slot(buffer, 1, Value::UNDEFINED);
        let o = self.alloc_object(ObjectClass::TypedArray, proto, 4)?;
        self.set_object_slot(o, 0, Value::object(buffer));
        self.set_object_slot(o, 1, Value::int(Kind::Uint8 as i32));
        self.set_object_slot(o, 2, Value::int(0));
        self.set_object_slot(o, 3, Value::number(len as f64));
        Ok(o)
    }

    /// DetachArrayBuffer (also `$262.detachArrayBuffer`): the data goes away and every
    /// view sees length 0. False if `b` is not an ArrayBuffer.
    pub fn detach_buffer(&mut self, b: Obj) -> bool {
        // An immutable buffer cannot be detached (the caller's TypeError).
        if self.object_class(b) != ObjectClass::ArrayBuffer || self.buffer_immutable(b) {
            return false;
        }
        self.set_object_slot(b, 0, Value::UNDEFINED);
        true
    }

    /// IsTypedArrayFixedLength: neither length-tracking nor a view of a resizable
    /// buffer (whose length may change under it).
    pub fn ta_fixed_length(&self, o: Obj) -> bool {
        let buffer = self.ta_buffer(o);
        self.object_slot(o, 3).as_number().is_some_and(|n| n >= 0.0)
            && !self.buffer_resizable(buffer)
    }

    /// Whether an ArrayBuffer is resizable (its slot 1 holds the maximum byte length).
    pub fn buffer_resizable(&self, b: Obj) -> bool {
        self.object_slot(b, 1).is_number()
    }

    /// IsImmutableBuffer (the immutable ArrayBuffer proposal, D1970).
    pub fn buffer_immutable(&self, b: Obj) -> bool {
        self.object_slot(b, 1) == Value::TRUE
    }

    /// Makes a fixed-length ArrayBuffer immutable (it was just created for that).
    pub fn set_buffer_immutable(&mut self, b: Obj) {
        debug_assert!(!self.buffer_resizable(b));
        self.set_object_slot(b, 1, Value::TRUE);
    }

    /// Whether a typed array views an immutable buffer: its elements are read-only.
    pub fn ta_immutable(&self, o: Obj) -> bool {
        self.buffer_immutable(self.ta_buffer(o))
    }

    pub fn is_typed_array(&self, o: Obj) -> bool {
        self.object_class(o) == ObjectClass::TypedArray
    }

    /// The Bytes cell of an ArrayBuffer, `None` once detached.
    pub fn buffer_bytes(&self, buffer: Obj) -> Option<u32> {
        self.object_slot(buffer, 0).as_internal()
    }

    /// The byte length of an ArrayBuffer (0 when detached).
    pub fn buffer_len(&self, buffer: Obj) -> u32 {
        self.buffer_bytes(buffer)
            .map_or(0, |b| self.arena().get(b, Bytes::LEN))
    }

    /// The bytes of a (not detached) ArrayBuffer.
    pub fn buffer_data(&self, buffer: Obj) -> &[u8] {
        match self.buffer_bytes(buffer) {
            Some(b) => {
                let len = self.arena().get(b, Bytes::LEN);
                self.arena().slice(Bytes::DATA.at(b, 0), len)
            }
            None => &[],
        }
    }

    pub fn buffer_data_mut(&mut self, buffer: Obj) -> &mut [u8] {
        match self.buffer_bytes(buffer) {
            Some(b) => {
                let len = self.arena().get(b, Bytes::LEN);
                self.arena_mut().slice_mut(Bytes::DATA.at(b, 0), len)
            }
            None => &mut [],
        }
    }

    /// The kind of a typed array.
    pub fn ta_kind(&self, o: Obj) -> Kind {
        Kind::from_i32(self.object_slot(o, 1).as_int().unwrap_or(1)).unwrap_or(Kind::Uint8)
    }

    pub fn ta_buffer(&self, o: Obj) -> Obj {
        self.object_slot(o, 0)
            .as_object()
            .expect("typed array buffer")
    }

    pub fn ta_byte_offset(&self, o: Obj) -> u32 {
        self.object_slot(o, 2).as_number().unwrap_or(0.0) as u32
    }

    /// TypedArrayLength with IsTypedArrayOutOfBounds: `None` if the buffer is detached
    /// or the view is out of bounds (a shrunk resizable buffer).
    pub fn ta_length(&self, o: Obj) -> Option<u32> {
        let buffer = self.ta_buffer(o);
        self.buffer_bytes(buffer)?;
        let size = self.ta_kind(o).size();
        let offset = self.ta_byte_offset(o);
        let buf_len = self.buffer_len(buffer);
        if offset > buf_len {
            return None;
        }
        match self.object_slot(o, 3).as_number() {
            Some(n) if n >= 0.0 => {
                let n = n as u32;
                if offset as u64 + n as u64 * size as u64 > buf_len as u64 {
                    None
                } else {
                    Some(n)
                }
            }
            _ => Some((buf_len - offset) / size),
        }
    }

    /// Element `i` of a typed array, `None` if it is not a valid index.
    pub fn ta_elem(&self, o: Obj, i: u32) -> Option<Elem> {
        if i >= self.ta_length(o)? {
            return None;
        }
        let kind = self.ta_kind(o);
        let at = (self.ta_byte_offset(o) + i * kind.size()) as usize;
        let data = self.buffer_data(self.ta_buffer(o));
        Some(decode_elem(kind, &data[at..at + kind.size() as usize]))
    }

    /// Element `i` of a typed array, `None` if it is not a valid index. A BigInt element
    /// is `Value::HOLE` (it needs an allocation: see [`Heap::ta_get_alloc`]).
    pub fn ta_get(&self, o: Obj, i: u32) -> Option<Value> {
        Some(match self.ta_elem(o, i)? {
            Elem::Number(n) => number(n),
            Elem::Big(_) => Value::HOLE,
        })
    }

    /// Element `i` of a typed array as a value (a BigInt element is allocated).
    pub fn ta_get_alloc(&mut self, o: Obj, i: u32) -> Result<Option<Value>, HeapError> {
        let kind = self.ta_kind(o);
        match self.ta_elem(o, i) {
            None => Ok(None),
            Some(e) => self.elem_value(kind, e).map(Some),
        }
    }

    /// The value of an element of `kind`.
    pub fn elem_value(&mut self, kind: Kind, e: Elem) -> Result<Value, HeapError> {
        match e {
            Elem::Number(n) => Ok(number(n)),
            Elem::Big(bits) => self.alloc_bigint(&elem_bigint(kind, bits)),
        }
    }

    /// The element a primitive value converts to for `kind` without user code or an
    /// error: a Number kind takes ToNumber of a primitive that is not a symbol or a
    /// BigInt, a BigInt kind only a BigInt. `None` means the caller converts first.
    pub(crate) fn element_of(&self, kind: Kind, v: Value) -> Option<Elem> {
        if kind.is_bigint() {
            v.as_bigint()?;
            Some(Elem::Big(self.bigint(v).to_u64_wrapping()))
        } else {
            self.primitive_number(v).map(Elem::Number)
        }
    }

    /// Replaces the placeholder of a BigInt typed array element in a property found on
    /// `holder` (see the module docs).
    fn materialize(
        &mut self,
        holder: Obj,
        key: Atom,
        p: Option<Property>,
    ) -> Result<Option<Property>, HeapError> {
        if let Some(Property::Data { value, flags }) = p
            && value.is_hole()
            && self.is_typed_array(holder)
            && let Some(i) = self
                .canonical_numeric_index(key)
                .and_then(|n| self.ta_index(holder, n))
            && let Some(v) = self.ta_get_alloc(holder, i)?
        {
            return Ok(Some(Property::Data { value: v, flags }));
        }
        Ok(p)
    }

    /// The value of a property found on `holder` whose value is the placeholder of a
    /// BigInt element (see the module docs); `value` itself otherwise.
    pub fn materialize_value(
        &mut self,
        holder: Obj,
        key: Atom,
        value: Value,
    ) -> Result<Value, HeapError> {
        if !value.is_hole() {
            return Ok(value);
        }
        let p = Some(Property::data(value, super::PropFlags::NONE));
        Ok(match self.materialize(holder, key, p)? {
            Some(Property::Data { value, .. }) if !value.is_hole() => value,
            _ => Value::UNDEFINED,
        })
    }

    /// `[[GetOwnProperty]]` with BigInt typed array elements allocated.
    pub fn get_own_property_alloc(
        &mut self,
        o: Obj,
        key: Atom,
    ) -> Result<Option<Property>, HeapError> {
        let p = self.get_own_property(o, key);
        self.materialize(o, key, p)
    }

    /// `find_property` with BigInt typed array elements allocated.
    pub fn find_property_alloc(
        &mut self,
        o: Obj,
        key: Atom,
    ) -> Result<Option<(Obj, Property)>, HeapError> {
        let Some((holder, p)) = self.find_property(o, key) else {
            return Ok(None);
        };
        Ok(self.materialize(holder, key, Some(p))?.map(|p| (holder, p)))
    }

    /// Writes element `i`; false (nothing written) if it is not a valid index.
    pub fn ta_set(&mut self, o: Obj, i: u32, e: Elem) -> bool {
        // An immutable buffer is never written (the builtins throw before).
        if self.ta_immutable(o) {
            return false;
        }
        let Some(len) = self.ta_length(o) else {
            return false;
        };
        if i >= len {
            return false;
        }
        let kind = self.ta_kind(o);
        let size = kind.size() as usize;
        let at = (self.ta_byte_offset(o) as usize) + i as usize * size;
        let mut buf = [0u8; 8];
        encode_elem(kind, e, &mut buf);
        let buffer = self.ta_buffer(o);
        self.buffer_data_mut(buffer)[at..at + size].copy_from_slice(&buf[..size]);
        true
    }

    /// CanonicalNumericIndexString of a property key: the Number for an integer atom or
    /// a string that is the canonical form of a Number (including "-0"), else `None`.
    pub fn canonical_numeric_index(&self, key: Atom) -> Option<f64> {
        if let Some(i) = key.as_int() {
            return Some(i as f64);
        }
        // Only digits, '-', 'I'(nfinity) and 'N'(aN) can start a canonical number: most
        // keys fail on their first unit, before their text is built (an IC fill asks for
        // every key it adds to an object).
        let first = match self.str_view(self.atom_str_value(key)?) {
            Some(super::StrView::Latin1(b)) => *b.first()?,
            // A first unit above 0xFF is no ASCII character either.
            Some(super::StrView::Utf16(b)) => {
                u8::try_from(u16::from_le_bytes([*b.first()?, b[1]])).ok()?
            }
            None => *self.atom_name(key).as_bytes().first()?,
        };
        if !(first.is_ascii_digit() || first == b'-' || first == b'I' || first == b'N') {
            return None;
        }
        let s = self.atom_name(key);
        if s == "-0" {
            return Some(-0.0);
        }
        let n = crate::num::string_to_number(s.as_bytes());
        let canon = crate::num::to_string(n);
        (canon.as_bytes() == s.as_bytes()).then_some(n)
    }

    /// The valid integer index a numeric key names, if any (IsValidIntegerIndex).
    fn ta_index(&self, o: Obj, n: f64) -> Option<u32> {
        if n != djs_libm::trunc(n) || (n == 0.0 && n.is_sign_negative()) || n < 0.0 {
            return None;
        }
        let len = self.ta_length(o)?;
        (n < len as f64).then_some(n as u32)
    }

    /// ToNumber of a primitive that cannot run user code or throw (not a symbol, a BigInt
    /// or an object).
    pub(crate) fn primitive_number(&self, v: Value) -> Option<f64> {
        if let Some(n) = v.as_number() {
            return Some(n);
        }
        if v.is_undefined() {
            return Some(f64::NAN);
        }
        if v.is_null() {
            return Some(0.0);
        }
        if let Some(b) = v.as_bool() {
            return Some(if b { 1.0 } else { 0.0 });
        }
        let s = v.as_string()?;
        let b = crate::num::narrow_units(self.str_units(s));
        Some(crate::num::string_to_number(&b[..]))
    }

    /// Whether Number `n` is a valid integer index of `o`.
    pub(crate) fn ta_get_valid(&self, o: Obj, n: f64) -> bool {
        self.ta_index(o, n).is_some()
    }

    /// The typed-array part of [[GetOwnProperty]] / [[Get]] / [[HasProperty]]: for a
    /// numeric key, `Some(element)` (`Some(None)` when it is not a valid index); `None`
    /// for other keys (ordinary lookup).
    pub(crate) fn ta_numeric_get(&self, o: Obj, key: Atom) -> Option<Option<Value>> {
        let n = self.canonical_numeric_index(key)?;
        Some(self.ta_index(o, n).and_then(|i| self.ta_get(o, i)))
    }

    /// The typed-array part of [[Set]] (receiver `o`) and of [[DefineOwnProperty]] with
    /// a value: `None` for a non-numeric key; else whether it wrote. A value that needs
    /// user code to convert (object, symbol) is `Some(false)` with nothing written; the
    /// caller converts first.
    pub(crate) fn ta_numeric_set(&mut self, o: Obj, key: Atom, v: Value) -> Option<bool> {
        let n = self.canonical_numeric_index(key)?;
        let Some(x) = self.element_of(self.ta_kind(o), v) else {
            return Some(false);
        };
        Some(match self.ta_index(o, n) {
            Some(i) => self.ta_set(o, i, x),
            None => false,
        })
    }

    /// Whether [[Set]] of `key` on `o` stops at a typed array on its prototype chain that
    /// has no such element (an invalid numeric index): it then succeeds without writing
    /// (10.4.5.5). Call it after the lookup found nothing.
    pub fn typed_array_set_stops(&self, o: Obj, key: Atom) -> bool {
        if self.canonical_numeric_index(key).is_none() {
            return false;
        }
        let mut cur = Some(o);
        while let Some(c) = cur {
            if self.object_class(c) == ObjectClass::TypedArray {
                return true;
            }
            if self.has_own_property(c, key) {
                return false;
            }
            cur = self.get_prototype_of(c);
        }
        false
    }

    /// The integer keys of a typed array (0 .. length), for [[OwnPropertyKeys]].
    pub(crate) fn ta_keys(&self, o: Obj) -> alloc::vec::Vec<Atom> {
        let len = self.ta_length(o).unwrap_or(0);
        (0..len).filter_map(Atom::int).collect()
    }
}
