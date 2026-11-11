//! Number <-> string conversion, matching QuickJS (`qjs`) output.
//!
//! Formatting:
//! - [`to_string`] is `Number::toString(x)` / `ToString(x)` (shortest round-trip digits, Ryu).
//! - [`to_string_radix`] is `Number.prototype.toString(radix)`.
//! - [`to_fixed`], [`to_exponential`] and [`to_precision`] are the `Number.prototype` methods,
//!   with exact decimal digits.
//!
//! Parsing:
//! - [`string_to_number`] is `StringToNumber` (what `Number(str)` and `+str` do).
//! - [`parse_float`] and [`parse_int`] are the global functions.
//! - [`literal_value`] and [`literal_digits_value`] give the value of a numeric literal the
//!   lexer has already validated.
//!
//! The formatters return a small stack buffer ([`NumStr`]) and never allocate. The parsers take
//! Latin-1 (`&[u8]`) or UTF-16 (`&[u16]`) code units, the two string representations of the
//! heap, and never allocate either.
//!
//! Callers do the JS-level argument conversion (ToIntegerOrInfinity, ToInt32, which may run
//! user code) and pass plain integers. Range errors come back as [`RangeError`]; the caller
//! throws the JS `RangeError` with [`RangeError::message`].

mod big;
mod exact;
mod format;
mod parse;
mod pow5;
mod ryu;

#[cfg(test)]
mod corpus_tests;
#[cfg(test)]
mod tests;

pub use format::{to_exponential, to_fixed, to_precision, to_string, to_string_radix};
pub use parse::{
    literal_digits_value, literal_value, narrow_units, parse_float, parse_int, string_to_number,
};

/// Longest output of [`to_string`]: `-0.0000012345678901234567`.
pub const MAX_SHORT_LEN: usize = 25;
/// Longest output of [`to_fixed`], [`to_exponential`] and [`to_precision`]: `toFixed(100)` of
/// a negative number just below 1e21 (sign, 21 integer digits, dot, 100 digits).
pub const MAX_FIXED_LEN: usize = 123;
/// Longest output of [`to_string_radix`]: `(-5e-324).toString(2)`, which is `-0.` followed by
/// 1073 zeros and a 1.
pub const MAX_RADIX_LEN: usize = 1077;

/// A formatted number: ASCII bytes in a fixed-capacity stack buffer.
#[derive(Clone)]
pub struct NumStr<const N: usize> {
    len: usize,
    buf: [u8; N],
}

impl<const N: usize> NumStr<N> {
    fn new() -> Self {
        NumStr {
            len: 0,
            buf: [0; N],
        }
    }

    fn push(&mut self, b: u8) {
        self.buf[self.len] = b;
        self.len += 1;
    }

    fn extend(&mut self, bytes: &[u8]) {
        self.buf[self.len..self.len + bytes.len()].copy_from_slice(bytes);
        self.len += bytes.len();
    }

    fn push_zeros(&mut self, n: usize) {
        self.buf[self.len..self.len + n].fill(b'0');
        self.len += n;
    }

    /// The output as bytes (always ASCII, so also valid Latin-1).
    pub fn as_bytes(&self) -> &[u8] {
        &self.buf[..self.len]
    }

    pub fn as_str(&self) -> &str {
        // Only ASCII is ever written.
        core::str::from_utf8(self.as_bytes()).unwrap_or("")
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl<const N: usize> PartialEq for NumStr<N> {
    fn eq(&self, other: &Self) -> bool {
        self.as_bytes() == other.as_bytes()
    }
}

impl<const N: usize> Eq for NumStr<N> {}

impl<const N: usize> core::ops::Deref for NumStr<N> {
    type Target = str;
    fn deref(&self) -> &str {
        self.as_str()
    }
}

impl<const N: usize> core::fmt::Debug for NumStr<N> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Debug::fmt(self.as_str(), f)
    }
}

/// A RangeError condition of a `Number.prototype` method.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RangeError {
    /// `toFixed` digits outside 0..=100, `toExponential` digits outside 0..=100 or
    /// `toPrecision` precision outside 1..=100.
    Digits,
    /// `toString` radix outside 2..=36.
    Radix,
}

impl RangeError {
    /// The QuickJS message text.
    pub fn message(self) -> &'static str {
        match self {
            RangeError::Digits => "invalid number of digits",
            RangeError::Radix => "radix must be between 2 and 36",
        }
    }
}

/// A string code unit: `u8` for Latin-1 strings, `u16` for UTF-16 strings.
pub trait CodeUnit: Copy + Into<u32> + sealed::Sealed {}
impl CodeUnit for u8 {}
impl CodeUnit for u16 {}

mod sealed {
    pub trait Sealed {}
    impl Sealed for u8 {}
    impl Sealed for u16 {}
}
