//! Type conversions and operator semantics on primitives. Objects that would need
//! ToPrimitive (valueOf/toString, which run user code) fail with a "not supported yet"
//! TypeError until the privileged slow paths exist (P1-2).

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use core::cmp::Ordering;

use super::realm::ErrorKind;
use crate::bigint::BigInt;
use crate::heap::{Atom, Heap, HeapError, StrView, atoms};
use crate::num;
use crate::value::{Kind, Str, Value};

/// A failed operation: a JS exception to throw. Boxed so that `Result<(), Fault>` is
/// returned in a register and `?` moves one pointer; the inline 16-byte enum cost ~10 KB
/// of wasm across the interpreter's `?` sites (D1574). Faults are the exceptional path:
/// one allocation per fault.
#[derive(Debug)]
pub(crate) struct Fault(pub(crate) Box<FaultKind>);

#[derive(Debug)]
pub(crate) enum FaultKind {
    /// Throw this value.
    Throw(Value),
    /// Throw a new error of this kind.
    Error(ErrorKind, String),
    /// Out of memory, string too long, invalid array length.
    Heap(HeapError),
}

impl Fault {
    /// The arena reached its cap (a JS out-of-memory error, not the host's, D1690).
    pub(crate) fn is_out_of_memory(&self) -> bool {
        matches!(
            *self.0,
            FaultKind::Heap(crate::heap::HeapError::OutOfMemory)
        )
    }

    #[cold]
    #[inline(never)]
    pub(crate) fn throw(v: Value) -> Fault {
        Fault(Box::new(FaultKind::Throw(v)))
    }

    #[cold]
    #[inline(never)]
    pub(crate) fn error(kind: ErrorKind, msg: String) -> Fault {
        Fault(Box::new(FaultKind::Error(kind, msg)))
    }

    #[cold]
    #[inline(never)]
    pub(crate) fn heap(e: HeapError) -> Fault {
        Fault(Box::new(FaultKind::Heap(e)))
    }
}

impl From<HeapError> for Fault {
    fn from(e: HeapError) -> Fault {
        Fault::heap(e)
    }
}

pub(crate) fn type_error<T>(msg: &str) -> Result<T, Fault> {
    Err(type_error_fault(msg))
}

/// Out of line: `String::from` inlined at each of the ~90 `type_error` sites was a
/// copy loop and an allocation call per site (0107).
#[cold]
#[inline(never)]
fn type_error_fault(msg: &str) -> Fault {
    Fault::error(ErrorKind::TypeError, String::from(msg))
}

fn to_primitive_unsupported<T>() -> Result<T, Fault> {
    type_error("not supported yet: conversion of an object to a primitive")
}

const MAX_SAFE_INTEGER: u64 = (1 << 53) - 1;

pub(crate) fn rem(x: f64, y: f64) -> f64 {
    let (i, j) = (x as i64, y as i64);
    if i as f64 == x && j as f64 == y && j != 0 && i.unsigned_abs() <= MAX_SAFE_INTEGER {
        let r = i % j;
        return if r == 0 && x.is_sign_negative() {
            -0.0
        } else {
            r as f64
        };
    }
    djs_libm::fmod(x, y)
}

/// A Number in the int32 form when it is an integer in range (and not -0).
#[inline]
pub(crate) fn number(n: f64) -> Value {
    let i = n as i32;
    if i as f64 == n && !(i == 0 && n.is_sign_negative()) {
        Value::int(i)
    } else {
        Value::number(n)
    }
}

pub(crate) fn to_boolean(heap: &Heap, v: Value) -> bool {
    match v.kind() {
        Kind::Undefined | Kind::Null | Kind::Hole | Kind::Uninitialized => false,
        Kind::Bool => v.as_bool().unwrap(),
        Kind::Number => {
            let n = v.as_number().unwrap();
            !(n == 0.0 || n.is_nan())
        }
        Kind::String => heap.str_len(v.as_string().unwrap()) > 0,
        Kind::BigInt => !heap.bigint_is_zero(v),
        _ => true,
    }
}

/// The code units of a string as a vector (for the number parser on two-byte strings).
fn units(heap: &mut Heap, s: Str) -> Result<Vec<u16>, Fault> {
    let v = heap.str_view_flat(s)?;
    Ok(v.units().collect())
}

pub(crate) fn string_to_number(heap: &mut Heap, s: Str) -> Result<f64, Fault> {
    let flat = heap.contiguous(s)?;
    if let Some(StrView::Latin1(b)) = heap.str_view(flat) {
        return Ok(num::string_to_number(b));
    }
    let b = num::narrow_units(heap.str_view_flat(flat)?.units());
    Ok(num::string_to_number(&b[..]))
}

pub(crate) fn to_number(heap: &mut Heap, v: Value) -> Result<f64, Fault> {
    if let Some(n) = v.as_number() {
        return Ok(n);
    }
    match v.kind() {
        Kind::Undefined => Ok(f64::NAN),
        Kind::Null => Ok(0.0),
        Kind::Bool => Ok(if v.as_bool().unwrap() { 1.0 } else { 0.0 }),
        Kind::String => string_to_number(heap, v.as_string().unwrap()),
        Kind::Symbol => type_error("cannot convert symbol to number"),
        Kind::BigInt => type_error("cannot convert bigint to number"),
        Kind::Object => to_primitive_unsupported(),
        _ => type_error("cannot convert value to number"),
    }
}

/// ToString for a Number.
pub(crate) fn number_to_string(heap: &mut Heap, v: Value) -> Result<Str, Fault> {
    if let Some(i) = v.as_int()
        && (0..=9).contains(&i)
    {
        // Single digits are frequent (array indexes, counters); still allocate, but skip
        // the float formatter.
        return Ok(heap.alloc_latin1(&[b'0' + i as u8])?);
    }
    let n = v.as_number().unwrap();
    Ok(heap.alloc_latin1(num::to_string(n).as_bytes())?)
}

pub(crate) fn to_string(heap: &mut Heap, v: Value) -> Result<Str, Fault> {
    let atom = match v.kind() {
        Kind::String => return Ok(v.as_string().unwrap()),
        Kind::Number => return number_to_string(heap, v),
        Kind::Undefined => atoms::UNDEFINED,
        Kind::Null => atoms::NULL,
        Kind::Bool => {
            if v.as_bool().unwrap() {
                atoms::TRUE
            } else {
                atoms::FALSE
            }
        }
        Kind::Symbol => return type_error("cannot convert symbol to string"),
        Kind::BigInt => return bigint_to_string(heap, v, 10),
        Kind::Object => return to_primitive_unsupported(),
        _ => return type_error("cannot convert value to string"),
    };
    Ok(heap.atom_string(atom)?)
}

/// The digits of a BigInt in `radix`.
pub(crate) fn bigint_to_string(heap: &mut Heap, v: Value, radix: u32) -> Result<Str, Fault> {
    let s = heap.bigint(v).to_string_radix(radix);
    Ok(heap.alloc_latin1(s.as_bytes())?)
}

/// StringToBigInt: `None` if the string is not a StringIntegerLiteral.
pub(crate) fn string_to_bigint(heap: &mut Heap, s: Str) -> Result<Option<BigInt>, Fault> {
    let u = units(heap, s)?;
    Ok(BigInt::parse_string(&u))
}

/// ToPropertyKey for primitives.
pub(crate) fn to_property_key(heap: &mut Heap, v: Value) -> Result<Atom, Fault> {
    if let Some(i) = v.as_int()
        && i >= 0
    {
        return Ok(Atom::int(i as u32).unwrap());
    }
    if let Some(n) = v.as_number()
        && n >= 0.0
        && n <= u32::MAX as f64 - 1.0
        && (n as u32) as f64 == n
    {
        return Ok(heap.atom_from_index(n as u32)?);
    }
    match v.kind() {
        Kind::Symbol => Ok(heap.symbol_atom(v.as_symbol().unwrap())),
        _ => {
            let s = to_string(heap, v)?;
            Ok(heap.intern(s)?)
        }
    }
}

/// ToInt32.
pub(crate) fn to_int32(n: f64) -> i32 {
    if let Some(i) = exact_i32(n) {
        return i;
    }
    if n > i64::MIN as f64 && n < i64::MAX as f64 {
        return n as i64 as i32;
    }
    if !n.is_finite() {
        return 0;
    }
    // Truncate, then reduce modulo 2^32.
    let t = if n < 0.0 {
        -djs_libm::floor(-n)
    } else {
        djs_libm::floor(n)
    };
    let m = djs_libm::fmod(t, 4294967296.0);
    let m = if m < 0.0 { m + 4294967296.0 } else { m };
    m as u32 as i32
}

#[inline]
fn exact_i32(n: f64) -> Option<i32> {
    let i = n as i32;
    (i as f64 == n).then_some(i)
}

/// The `typeof` string.
pub(crate) fn typeof_atom(heap: &Heap, v: Value) -> Atom {
    match v.kind() {
        Kind::Undefined => atoms::UNDEFINED,
        Kind::Null => atoms::OBJECT,
        Kind::Bool => atoms::BOOLEAN,
        Kind::Number => atoms::NUMBER,
        Kind::String => atoms::STRING,
        Kind::Symbol => atoms::SYMBOL,
        Kind::BigInt => atoms::BIGINT,
        Kind::Object => {
            if heap.is_callable(v.as_object().unwrap()) {
                atoms::FUNCTION
            } else {
                atoms::OBJECT
            }
        }
        _ => atoms::UNDEFINED,
    }
}

/// IsStrictlyEqual.
pub(crate) fn strict_equals(heap: &Heap, a: Value, b: Value) -> bool {
    if let (Some(x), Some(y)) = (a.as_number(), b.as_number()) {
        return x == y;
    }
    if let (Some(x), Some(y)) = (a.as_string(), b.as_string()) {
        return heap.str_eq(x, y);
    }
    if a.as_bigint().is_some() && b.as_bigint().is_some() {
        return heap.bigint_eq(a, b);
    }
    a == b
}

/// IsLooselyEqual.
pub(crate) fn loose_equals(heap: &mut Heap, a: Value, b: Value) -> Result<bool, Fault> {
    let (ka, kb) = (a.kind(), b.kind());
    if ka == kb {
        return Ok(strict_equals(heap, a, b));
    }
    Ok(match (ka, kb) {
        (Kind::Null, Kind::Undefined) | (Kind::Undefined, Kind::Null) => true,
        (Kind::Null | Kind::Undefined, _) | (_, Kind::Null | Kind::Undefined) => false,
        (Kind::Number, Kind::String) => {
            a.as_number().unwrap() == string_to_number(heap, b.as_string().unwrap())?
        }
        (Kind::String, Kind::Number) => {
            string_to_number(heap, a.as_string().unwrap())? == b.as_number().unwrap()
        }
        (Kind::BigInt, Kind::Number) => {
            heap.bigint(a).cmp_f64(b.as_number().unwrap()) == Some(Ordering::Equal)
        }
        (Kind::Number, Kind::BigInt) => {
            heap.bigint(b).cmp_f64(a.as_number().unwrap()) == Some(Ordering::Equal)
        }
        (Kind::BigInt, Kind::String) => {
            string_to_bigint(heap, b.as_string().unwrap())?.is_some_and(|y| heap.bigint(a) == y)
        }
        (Kind::String, Kind::BigInt) => {
            string_to_bigint(heap, a.as_string().unwrap())?.is_some_and(|x| heap.bigint(b) == x)
        }
        (Kind::Bool, _) => {
            let n = number(to_number(heap, a)?);
            return loose_equals(heap, n, b);
        }
        (_, Kind::Bool) => {
            let n = number(to_number(heap, b)?);
            return loose_equals(heap, a, n);
        }
        (Kind::Object, Kind::Symbol) | (Kind::Symbol, Kind::Object) => false,
        (Kind::Object, _) | (_, Kind::Object) => return to_primitive_unsupported(),
        _ => false,
    })
}

/// IsLessThan for primitives: `None` is undefined (a NaN was involved).
pub(crate) fn less_than(heap: &mut Heap, a: Value, b: Value) -> Result<Option<bool>, Fault> {
    if let (Some(x), Some(y)) = (a.as_int(), b.as_int()) {
        return Ok(Some(x < y));
    }
    if let (Some(x), Some(y)) = (a.as_string(), b.as_string()) {
        return Ok(Some(heap.str_cmp(x, y) == Ordering::Less));
    }
    if a.kind() == Kind::Object || b.kind() == Kind::Object {
        return to_primitive_unsupported();
    }
    if a.as_bigint().is_some() || b.as_bigint().is_some() {
        return bigint_less_than(heap, a, b);
    }
    let x = to_number(heap, a)?;
    let y = to_number(heap, b)?;
    if x.is_nan() || y.is_nan() {
        Ok(None)
    } else {
        Ok(Some(x < y))
    }
}

/// IsLessThan when one side is a BigInt (exact, no rounding through f64).
fn bigint_less_than(heap: &mut Heap, a: Value, b: Value) -> Result<Option<bool>, Fault> {
    // A string against a BigInt is StringToBigInt; anything else is ToNumeric.
    let side = |heap: &mut Heap, v: Value| -> Result<Option<Result<BigInt, f64>>, Fault> {
        Ok(Some(match v.kind() {
            Kind::BigInt => Ok(heap.bigint(v)),
            Kind::String => match string_to_bigint(heap, v.as_string().unwrap())? {
                Some(b) => Ok(b),
                None => return Ok(None),
            },
            _ => Err(to_number(heap, v)?),
        }))
    };
    let (Some(x), Some(y)) = (side(heap, a)?, side(heap, b)?) else {
        return Ok(None);
    };
    let ord = match (x, y) {
        (Ok(x), Ok(y)) => Some(x.cmp(&y)),
        (Ok(x), Err(y)) => x.cmp_f64(y),
        (Err(x), Ok(y)) => y.cmp_f64(x).map(Ordering::reverse),
        (Err(_), Err(_)) => unreachable!("one side is a BigInt"),
    };
    Ok(ord.map(|o| o == Ordering::Less))
}

/// Number::exponentiate. C's pow disagrees with the spec for a NaN exponent and for
/// `(±1) ** ±Infinity` (C gives 1, JS NaN), so those are handled first (D1150).
pub(crate) fn pow(base: f64, exp: f64) -> f64 {
    if exp.is_nan() {
        return f64::NAN;
    }
    if exp == 0.0 {
        return 1.0;
    }
    if exp.is_infinite() && (base == 1.0 || base == -1.0) {
        return f64::NAN;
    }
    djs_libm::pow(base, exp)
}

/// The `+` operator on primitives.
pub(crate) fn add(heap: &mut Heap, a: Value, b: Value) -> Result<Value, Fault> {
    if let (Some(x), Some(y)) = (a.as_int(), b.as_int()) {
        return Ok(match x.checked_add(y) {
            Some(z) => Value::int(z),
            None => Value::number(x as f64 + y as f64),
        });
    }
    if let (Some(x), Some(y)) = (a.as_number(), b.as_number()) {
        return Ok(number(x + y));
    }
    if a.kind() == Kind::Object || b.kind() == Kind::Object {
        return to_primitive_unsupported();
    }
    if a.as_string().is_some() || b.as_string().is_some() {
        let x = to_string(heap, a)?;
        let y = to_string(heap, b)?;
        return Ok(Value::string(heap.concat(x, y)?));
    }
    let x = to_number(heap, a)?;
    let y = to_number(heap, b)?;
    Ok(number(x + y))
}

/// A property key in an error message: a symbol is its bare description, as qjs
/// prints it (`'Symbol.iterator'`, `''` without one), not the `[desc]` of a function
/// name.
pub(crate) fn describe_key(heap: &Heap, key: Atom) -> String {
    let name = heap.atom_name(key);
    if key.is_symbol() {
        // `atom_name` gives `[desc]` for a symbol.
        let inner = name.strip_prefix('[').and_then(|n| n.strip_suffix(']'));
        return String::from(inner.unwrap_or(&name));
    }
    name
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn int32() {
        for (n, want) in [
            (0.0, 0),
            (-0.0, 0),
            (1.9, 1),
            (-1.9, -1),
            (2147483648.0, -2147483648),
            (4294967296.0, 0),
            (4294967297.5, 1),
            (-4294967297.0, -1),
            (f64::NAN, 0),
            (f64::INFINITY, 0),
            (1e21, -559939584),
        ] {
            assert_eq!(to_int32(n), want, "{n}");
        }
    }

    #[test]
    fn pow_special_cases() {
        assert!(pow(1.0, f64::NAN).is_nan());
        assert!(pow(1.0, f64::INFINITY).is_nan());
        assert!(pow(-1.0, f64::NEG_INFINITY).is_nan());
        assert!(pow(f64::NAN, 1.0).is_nan());
        assert_eq!(pow(f64::NAN, 0.0), 1.0);
        assert_eq!(pow(2.0, 10.0), 1024.0);
        assert_eq!(pow(2.0, 0.5), core::f64::consts::SQRT_2);
        assert!(pow(-8.0, 1.0 / 3.0).is_nan());
        assert_eq!(pow(0.0, -1.0), f64::INFINITY);
        assert_eq!(pow(-0.0, -1.0), f64::NEG_INFINITY);
    }

    #[test]
    fn equality() {
        let mut h = Heap::new().unwrap();
        let s1 = Value::string(h.alloc_str("1").unwrap());
        let s1b = Value::string(h.alloc_str("1").unwrap());
        assert!(strict_equals(&h, s1, s1b));
        assert!(strict_equals(&h, Value::int(1), Value::number(1.0)));
        assert!(!strict_equals(&h, Value::NAN, Value::NAN));
        assert!(strict_equals(&h, Value::number(0.0), Value::number(-0.0)));
        assert!(loose_equals(&mut h, s1, Value::int(1)).unwrap());
        assert!(loose_equals(&mut h, Value::TRUE, Value::int(1)).unwrap());
        assert!(loose_equals(&mut h, Value::TRUE, s1).unwrap());
        assert!(loose_equals(&mut h, Value::NULL, Value::UNDEFINED).unwrap());
        assert!(!loose_equals(&mut h, Value::NULL, Value::int(0)).unwrap());
        let o = Value::object(h.new_object(None).unwrap());
        assert!(loose_equals(&mut h, o, o).unwrap());
        assert!(!loose_equals(&mut h, o, Value::NULL).unwrap());
        assert!(loose_equals(&mut h, o, Value::int(1)).is_err());
    }

    #[test]
    fn conversions() {
        let mut h = Heap::new().unwrap();
        let s = Value::string(h.alloc_str("  12.5e1 ").unwrap());
        assert_eq!(to_number(&mut h, s).unwrap(), 125.0);
        let u = Value::string(h.alloc_str("€").unwrap());
        assert!(to_number(&mut h, u).unwrap().is_nan());
        assert!(to_number(&mut h, Value::UNDEFINED).unwrap().is_nan());
        let t = to_string(&mut h, Value::number(-0.0)).unwrap();
        assert!(h.str_eq_str(t, "0"));
        let t = to_string(&mut h, Value::number(1e21)).unwrap();
        assert!(h.str_eq_str(t, "1e+21"));
        let k = to_property_key(&mut h, Value::number(3.0)).unwrap();
        assert_eq!(k, Atom::int(3).unwrap());
        let k = to_property_key(&mut h, Value::number(1.5)).unwrap();
        assert_eq!(h.atom_name(k), "1.5");
        let v = add(&mut h, Value::int(i32::MAX), Value::int(1)).unwrap();
        assert_eq!(v.as_number(), Some(2147483648.0));
        let v = add(&mut h, s, Value::NULL).unwrap();
        assert_eq!(h.str_to_string(v.as_string().unwrap()), "  12.5e1 null");
    }
}
