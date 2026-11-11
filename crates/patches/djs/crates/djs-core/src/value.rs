//! The JS value representation: 64-bit NaN-boxed values.
//!
//! # Encoding
//!
//! A [`Value`] is a `u64`. Every bit pattern whose upper 16 bits are *not* `0xFFF8` is an
//! IEEE-754 `f64`. Values whose upper 16 bits are `0xFFF8` are tagged:
//!
//! ```text
//!  63        48 47        32 31                         0
//! +------------+------------+----------------------------+
//! |   0xFFF8   |    tag     |          payload           |
//! +------------+------------+----------------------------+
//! ```
//!
//! Seen as two little-endian `u32` words (which is how the JS-side reader sees it), the
//! high word is `0xFFF8_0000 | tag` and the low word is the payload.
//!
//! `0xFFF8_xxxx_xxxx_xxxx` is a negative quiet NaN. [`Value::number`] canonicalizes every
//! NaN to `0x7FF8_0000_0000_0000`, so no number ever collides with the tagged space.
//!
//! | tag | meaning | payload |
//! | --- | --- | --- |
//! | 1 | `undefined` | 0 |
//! | 2 | `null` | 0 |
//! | 3 | boolean | 0 or 1 |
//! | 4 | int32 | the `i32` bits (optional fast path; a Number like the f64 form) |
//! | 5 | hole (empty array slot, never visible to JS) | 0 |
//! | 6 | uninitialized (TDZ marker, never visible to JS) | 0 |
//! | 8 | string | arena offset of a string cell |
//! | 9 | object | arena offset of an object cell |
//! | 10 | symbol | arena offset of a symbol cell |
//! | 11 | bigint | arena offset of a bigint cell (D1460) |
//! | 12 | host ref (reserved) | arena offset of a host-ref cell |
//! | 13 | internal (D007) | arena offset of a non-JS cell (function template, Box, realm, ...) |
//!
//! Tags `>= 8` are *references*: the payload is the `u32` arena offset of a heap cell,
//! never 0. The GC traces exactly those. Tag 0 and the other values are invalid.
//!
//! Internal references let VM-internal cells sit in `Value` slots (a closure's template
//! and Boxes in its object slots, the realm record in the roots). They never reach JS
//! code: every operation that could expose a value to JS treats them as a VM bug.
//!
//! The all-zero bit pattern is the number `+0`, which is not a reference. That is what
//! makes zero-filled arena memory safe to trace.
//!
//! Equality (`==` on [`Value`]) is *bit identity*, not any JS equality. The int32 form and
//! the f64 form of the same number are different bits, and two equal strings in different
//! cells are different bits. Use the heap's `same_value` for JS semantics.

use core::fmt;

/// Upper 16 bits of every tagged value.
pub const TAG_PREFIX: u64 = 0xFFF8 << 48;
const PREFIX_MASK: u64 = 0xFFFF << 48;

/// Value tags. See the module docs for the encoding.
pub mod tag {
    pub const UNDEFINED: u16 = 1;
    pub const NULL: u16 = 2;
    pub const BOOL: u16 = 3;
    pub const INT: u16 = 4;
    pub const HOLE: u16 = 5;
    pub const UNINITIALIZED: u16 = 6;
    /// The first reference tag. Every tag `>= FIRST_REF` carries an arena offset.
    pub const FIRST_REF: u16 = 8;
    pub const STRING: u16 = 8;
    pub const OBJECT: u16 = 9;
    pub const SYMBOL: u16 = 10;
    pub const BIGINT: u16 = 11;
    pub const HOST_REF: u16 = 12;
    /// A reference to an internal (non-JS) cell (D007).
    pub const INTERNAL: u16 = 13;
    /// One past the last valid tag.
    pub const END: u16 = 14;
}

/// The canonical NaN. Every NaN is stored with these bits.
pub const CANONICAL_NAN: u64 = 0x7FF8_0000_0000_0000;

/// A string reference: the arena offset of a string cell (flat or rope).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Str(pub(crate) u32);

/// An object reference: the arena offset of an object cell.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Obj(pub(crate) u32);

/// A symbol reference: the arena offset of a symbol cell.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Sym(pub(crate) u32);

macro_rules! impl_ref {
    ($($t:ident),*) => {$(
        impl $t {
            /// The arena offset of the cell.
            #[inline]
            pub const fn offset(self) -> u32 {
                self.0
            }
        }
    )*};
}
impl_ref!(Str, Obj, Sym);

/// A 64-bit NaN-boxed JS value. See the module docs for the encoding.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct Value(u64);

/// The kind of a value, decoded from its bits.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Undefined,
    Null,
    Bool,
    /// A Number, in either the f64 or the int32 form.
    Number,
    Hole,
    Uninitialized,
    String,
    Object,
    Symbol,
    BigInt,
    HostRef,
    /// A reference to an internal cell; never visible to JS.
    Internal,
}

impl Value {
    pub const UNDEFINED: Value = Value::tagged(tag::UNDEFINED, 0);
    pub const NULL: Value = Value::tagged(tag::NULL, 0);
    pub const FALSE: Value = Value::tagged(tag::BOOL, 0);
    pub const TRUE: Value = Value::tagged(tag::BOOL, 1);
    pub const HOLE: Value = Value::tagged(tag::HOLE, 0);
    pub const UNINITIALIZED: Value = Value::tagged(tag::UNINITIALIZED, 0);
    pub const NAN: Value = Value(CANONICAL_NAN);
    pub const ZERO: Value = Value(0);

    #[inline]
    const fn tagged(tag: u16, payload: u32) -> Value {
        Value(TAG_PREFIX | ((tag as u64) << 32) | payload as u64)
    }

    /// A Number in f64 form. NaNs are canonicalized.
    #[inline]
    pub fn number(n: f64) -> Value {
        if n.is_nan() {
            Value::NAN
        } else {
            Value(n.to_bits())
        }
    }

    /// A Number in int32 form.
    #[inline]
    pub const fn int(i: i32) -> Value {
        Value::tagged(tag::INT, i as u32)
    }

    #[inline]
    pub const fn bool(b: bool) -> Value {
        Value::tagged(tag::BOOL, b as u32)
    }

    #[inline]
    pub const fn string(s: Str) -> Value {
        Value::tagged(tag::STRING, s.0)
    }

    #[inline]
    pub const fn object(o: Obj) -> Value {
        Value::tagged(tag::OBJECT, o.0)
    }

    #[inline]
    pub const fn symbol(s: Sym) -> Value {
        Value::tagged(tag::SYMBOL, s.0)
    }

    /// A BigInt: the arena offset of a `BigInt` cell (nonzero).
    #[inline]
    pub const fn bigint(cell: u32) -> Value {
        Value::tagged(tag::BIGINT, cell)
    }

    /// A reference to the internal cell at arena offset `cell` (nonzero).
    #[inline]
    pub const fn internal(cell: u32) -> Value {
        Value::tagged(tag::INTERNAL, cell)
    }

    /// The raw bits.
    #[inline]
    pub const fn to_bits(self) -> u64 {
        self.0
    }

    /// Reinterprets raw bits as a value, accepting only valid encodings: a tagged value with
    /// a known tag (and a nonzero offset for references), or an f64 that is not a
    /// non-canonical NaN.
    pub const fn from_bits(bits: u64) -> Option<Value> {
        if bits & PREFIX_MASK == TAG_PREFIX {
            let tag = (bits >> 32) as u16;
            let payload = bits as u32;
            let ok = match tag {
                tag::UNDEFINED | tag::NULL | tag::HOLE | tag::UNINITIALIZED => payload == 0,
                tag::BOOL => payload <= 1,
                tag::INT => true,
                t => t >= tag::FIRST_REF && t < tag::END && payload != 0,
            };
            if ok { Some(Value(bits)) } else { None }
        } else {
            let f = f64::from_bits(bits);
            if f.is_nan() && bits != CANONICAL_NAN {
                None
            } else {
                Some(Value(bits))
            }
        }
    }

    /// Reinterprets raw bits without validation. Used for arena reads: garbage bits can
    /// only cause wrong results or a panic, never undefined behavior.
    #[inline]
    pub(crate) const fn from_raw(bits: u64) -> Value {
        Value(bits)
    }

    #[inline]
    const fn is_tagged(self) -> bool {
        self.0 & PREFIX_MASK == TAG_PREFIX
    }

    /// The tag, or `None` for an f64.
    #[inline]
    pub const fn tag(self) -> Option<u16> {
        if self.is_tagged() {
            Some((self.0 >> 32) as u16)
        } else {
            None
        }
    }

    #[inline]
    const fn payload(self) -> u32 {
        self.0 as u32
    }

    /// Decodes the kind. Panics on an invalid tag, which only a VM bug or a corrupted
    /// heap can produce.
    pub fn kind(self) -> Kind {
        match self.tag() {
            None => Kind::Number,
            Some(tag::UNDEFINED) => Kind::Undefined,
            Some(tag::NULL) => Kind::Null,
            Some(tag::BOOL) => Kind::Bool,
            Some(tag::INT) => Kind::Number,
            Some(tag::HOLE) => Kind::Hole,
            Some(tag::UNINITIALIZED) => Kind::Uninitialized,
            Some(tag::STRING) => Kind::String,
            Some(tag::OBJECT) => Kind::Object,
            Some(tag::SYMBOL) => Kind::Symbol,
            // The slim build has no BigInt values (D2411): its code for them goes.
            Some(tag::BIGINT) if crate::FULL => Kind::BigInt,
            Some(tag::HOST_REF) => Kind::HostRef,
            Some(tag::INTERNAL) => Kind::Internal,
            Some(t) => panic!("invalid value tag {t}"),
        }
    }

    #[inline]
    pub const fn is_undefined(self) -> bool {
        self.0 == Value::UNDEFINED.0
    }

    #[inline]
    pub const fn is_null(self) -> bool {
        self.0 == Value::NULL.0
    }

    /// `undefined` or `null`.
    #[inline]
    pub const fn is_nullish(self) -> bool {
        self.is_undefined() || self.is_null()
    }

    #[inline]
    pub const fn is_hole(self) -> bool {
        self.0 == Value::HOLE.0
    }

    #[inline]
    pub const fn is_uninitialized(self) -> bool {
        self.0 == Value::UNINITIALIZED.0
    }

    /// A Number in either form.
    #[inline]
    pub const fn is_number(self) -> bool {
        !self.is_tagged() || (self.0 >> 32) as u16 == tag::INT
    }

    /// The int32 form only.
    #[inline]
    pub const fn as_int(self) -> Option<i32> {
        match self.tag() {
            Some(tag::INT) => Some(self.payload() as i32),
            _ => None,
        }
    }

    /// The numeric value of a Number in either form.
    #[inline]
    pub fn as_number(self) -> Option<f64> {
        match self.tag() {
            None => Some(f64::from_bits(self.0)),
            Some(tag::INT) => Some(self.payload() as i32 as f64),
            _ => None,
        }
    }

    #[inline]
    pub const fn as_bool(self) -> Option<bool> {
        match self.tag() {
            Some(tag::BOOL) => Some(self.payload() != 0),
            _ => None,
        }
    }

    /// Whether this is a reference to a heap cell (string, object, symbol, bigint,
    /// host ref).
    #[inline]
    pub const fn is_ref(self) -> bool {
        match self.tag() {
            Some(t) => t >= tag::FIRST_REF,
            None => false,
        }
    }

    /// The arena offset of a reference value.
    #[inline]
    pub const fn ref_offset(self) -> Option<u32> {
        if self.is_ref() {
            Some(self.payload())
        } else {
            None
        }
    }

    #[inline]
    pub const fn as_string(self) -> Option<Str> {
        match self.tag() {
            Some(tag::STRING) => Some(Str(self.payload())),
            _ => None,
        }
    }

    #[inline]
    pub const fn as_object(self) -> Option<Obj> {
        match self.tag() {
            Some(tag::OBJECT) => Some(Obj(self.payload())),
            _ => None,
        }
    }

    /// The cell offset of an internal reference.
    #[inline]
    pub const fn as_internal(self) -> Option<u32> {
        match self.tag() {
            Some(tag::INTERNAL) => Some(self.payload()),
            _ => None,
        }
    }

    /// The cell offset of a BigInt.
    #[inline]
    pub const fn as_bigint(self) -> Option<u32> {
        match self.tag() {
            Some(tag::BIGINT) if crate::FULL => Some(self.payload()),
            _ => None,
        }
    }

    #[inline]
    pub const fn as_symbol(self) -> Option<Sym> {
        match self.tag() {
            Some(tag::SYMBOL) => Some(Sym(self.payload())),
            _ => None,
        }
    }
}

impl From<bool> for Value {
    fn from(b: bool) -> Value {
        Value::bool(b)
    }
}

impl From<f64> for Value {
    fn from(n: f64) -> Value {
        Value::number(n)
    }
}

impl From<i32> for Value {
    fn from(i: i32) -> Value {
        Value::int(i)
    }
}

impl From<Str> for Value {
    fn from(s: Str) -> Value {
        Value::string(s)
    }
}

impl From<Obj> for Value {
    fn from(o: Obj) -> Value {
        Value::object(o)
    }
}

impl From<Sym> for Value {
    fn from(s: Sym) -> Value {
        Value::symbol(s)
    }
}

impl fmt::Debug for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.tag() {
            None => write!(f, "{:?}", f64::from_bits(self.0)),
            Some(tag::UNDEFINED) => f.write_str("undefined"),
            Some(tag::NULL) => f.write_str("null"),
            Some(tag::BOOL) => write!(f, "{}", self.payload() != 0),
            Some(tag::INT) => write!(f, "{}i", self.payload() as i32),
            Some(tag::HOLE) => f.write_str("<hole>"),
            Some(tag::UNINITIALIZED) => f.write_str("<uninitialized>"),
            Some(tag::STRING) => write!(f, "string@{}", self.payload()),
            Some(tag::OBJECT) => write!(f, "object@{}", self.payload()),
            Some(tag::SYMBOL) => write!(f, "symbol@{}", self.payload()),
            Some(tag::BIGINT) => write!(f, "bigint@{}", self.payload()),
            Some(tag::HOST_REF) => write!(f, "hostref@{}", self.payload()),
            Some(tag::INTERNAL) => write!(f, "internal@{}", self.payload()),
            Some(t) => write!(f, "<invalid tag {t}: {:#018x}>", self.0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn size() {
        assert_eq!(core::mem::size_of::<Value>(), 8);
    }

    #[test]
    fn numbers_round_trip() {
        for n in [
            0.0,
            -0.0,
            1.0,
            -1.5,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::MAX,
            f64::MIN_POSITIVE,
            5e-324,
        ] {
            let v = Value::number(n);
            assert_eq!(v.kind(), Kind::Number);
            assert!(v.is_number());
            assert_eq!(v.as_number().unwrap().to_bits(), n.to_bits());
            assert!(!v.is_ref());
            assert_eq!(Value::from_bits(v.to_bits()), Some(v));
        }
    }

    #[test]
    fn nan_is_canonicalized() {
        let nans = [
            f64::NAN,
            -f64::NAN,
            f64::from_bits(0x7FF0_0000_0000_0001),
            f64::from_bits(0xFFF8_0000_0000_0001),
            f64::from_bits(0xFFF8_0009_0000_1234), // would look like an object ref
            f64::from_bits(0xFFFF_FFFF_FFFF_FFFF),
        ];
        for n in nans {
            let v = Value::number(n);
            assert_eq!(v.to_bits(), CANONICAL_NAN);
            assert_eq!(v.kind(), Kind::Number);
            assert!(v.as_number().unwrap().is_nan());
        }
        assert_eq!(Value::from_bits(0x7FF0_0000_0000_0001), None);
        assert_eq!(Value::from_bits(0xFFFF_0000_0000_0000), None);
    }

    #[test]
    fn tagged() {
        assert_eq!(Value::UNDEFINED.kind(), Kind::Undefined);
        assert_eq!(Value::NULL.kind(), Kind::Null);
        assert!(Value::UNDEFINED.is_nullish() && Value::NULL.is_nullish());
        assert_eq!(Value::TRUE.as_bool(), Some(true));
        assert_eq!(Value::FALSE.as_bool(), Some(false));
        assert_eq!(Value::bool(true), Value::TRUE);
        assert_eq!(Value::HOLE.kind(), Kind::Hole);
        assert_eq!(Value::UNINITIALIZED.kind(), Kind::Uninitialized);
        assert_eq!(Value::int(-7).as_int(), Some(-7));
        assert_eq!(Value::int(-7).as_number(), Some(-7.0));
        assert_eq!(Value::int(i32::MIN).as_number(), Some(i32::MIN as f64));
        assert!(Value::int(3).is_number());
        assert_eq!(Value::number(3.0).as_int(), None);
        assert_eq!(Value::TRUE.as_number(), None);
        // The documented word layout.
        assert_eq!(Value::UNDEFINED.to_bits(), 0xFFF8_0001_0000_0000);
        assert_eq!(Value::object(Obj(0x40)).to_bits(), 0xFFF8_0009_0000_0040);
    }

    #[test]
    fn references() {
        let s = Value::string(Str(16));
        let o = Value::object(Obj(24));
        let y = Value::symbol(Sym(32));
        assert_eq!(s.as_string(), Some(Str(16)));
        assert_eq!(o.as_object(), Some(Obj(24)));
        assert_eq!(y.as_symbol(), Some(Sym(32)));
        assert_eq!(s.as_object(), None);
        for (v, k) in [(s, Kind::String), (o, Kind::Object), (y, Kind::Symbol)] {
            assert!(v.is_ref());
            assert_eq!(v.kind(), k);
            assert!(!v.is_number());
            assert_eq!(Value::from_bits(v.to_bits()), Some(v));
        }
        assert_eq!(o.ref_offset(), Some(24));
        assert!(!Value::UNDEFINED.is_ref());
        assert!(!Value::ZERO.is_ref());
        // A reference to offset 0 (null) is not a valid encoding.
        assert_eq!(Value::from_bits(Value::object(Obj(0)).to_bits()), None);
        // Invalid tags.
        assert_eq!(Value::from_bits(TAG_PREFIX), None);
        assert_eq!(Value::from_bits(TAG_PREFIX | (7 << 32)), None);
        assert_eq!(Value::from_bits(TAG_PREFIX | (14 << 32) | 8), None);
        let i = Value::internal(40);
        assert_eq!(i.kind(), Kind::Internal);
        assert!(i.is_ref());
        assert_eq!(i.ref_offset(), Some(40));
        assert_eq!(i.as_internal(), Some(40));
        assert_eq!(i.as_object(), None);
        assert_eq!(Value::from_bits(i.to_bits()), Some(i));
        assert_eq!(Value::from_bits(Value::internal(0).to_bits()), None);
        assert_eq!(Value::from_bits(Value::TRUE.to_bits() + 1), None);
    }

    #[test]
    fn zero_bits_are_plus_zero() {
        let v = Value::from_raw(0);
        assert_eq!(v, Value::ZERO);
        assert_eq!(v.as_number().unwrap().to_bits(), 0);
        assert!(!v.is_ref());
    }
}
