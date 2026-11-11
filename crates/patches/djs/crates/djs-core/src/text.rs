//! Building strings without core::fmt (D2339): `cat!` concatenates pieces (strings,
//! characters, integers, zero-padded or hexadecimal integers) into a `String`.
//!
//! The engine's messages and its few formatted outputs (Date strings, stack traces, error
//! summaries) are built with it instead of `format!`, so that the shipped module carries no
//! formatting machinery (`core::fmt::write`, `Formatter`, the `Display` impls of every
//! argument type). The output is byte for byte what the `format!` it replaces produced.
use crate::PushOut;

use alloc::string::String;

/// Something that can be appended to a message.
pub(crate) trait Piece {
    fn put(&self, out: &mut String);
    /// How many bytes `put` appends, when known cheaply (the dumper's `cat!` sizes its
    /// string with it).
    #[cfg(feature = "dump")]
    fn size_hint(&self) -> usize {
        0
    }
}

impl Piece for str {
    fn put(&self, out: &mut String) {
        out.push_str(self);
    }
    #[cfg(feature = "dump")]
    fn size_hint(&self) -> usize {
        self.len()
    }
}

impl Piece for String {
    fn put(&self, out: &mut String) {
        out.push_str(self);
    }
    #[cfg(feature = "dump")]
    fn size_hint(&self) -> usize {
        self.len()
    }
}

impl Piece for char {
    fn put(&self, out: &mut String) {
        out.push_out(*self);
    }
}

impl<T: Piece + ?Sized> Piece for &T {
    fn put(&self, out: &mut String) {
        (**self).put(out);
    }
    #[cfg(feature = "dump")]
    fn size_hint(&self) -> usize {
        (**self).size_hint()
    }
}

impl<T: Piece + ?Sized> Piece for &mut T {
    #[cfg(feature = "dump")]
    fn size_hint(&self) -> usize {
        (**self).size_hint()
    }
    fn put(&self, out: &mut String) {
        (**self).put(out);
    }
}

macro_rules! unsigned {
    ($($t:ty),*) => {$(
        impl Piece for $t {
            fn put(&self, out: &mut String) {
                decimal(*self as u64, 0, out);
            }
        }
    )*};
}
unsigned!(u8, u16, u32, u64, usize);

macro_rules! signed {
    ($($t:ty),*) => {$(
        impl Piece for $t {
            fn put(&self, out: &mut String) {
                signed(*self as i64, 0, out);
            }
        }
    )*};
}
signed!(i8, i16, i32, i64, isize);

/// The decimal digits of `v`, zero-padded to `width`.
fn decimal(mut v: u64, width: usize, out: &mut String) {
    // Leading zeros and digits in one buffer, appended at once (Date strings are built
    // from many small padded numbers).
    let mut buf = [b'0'; 24];
    let mut i = buf.len();
    loop {
        i -= 1;
        buf[i] = b'0' + (v % 10) as u8;
        v /= 10;
        if v == 0 {
            break;
        }
    }
    let start = buf.len() - width.clamp(buf.len() - i, buf.len());
    out.push_str(core::str::from_utf8(&buf[start..]).unwrap_or_default());
}

/// `v` as `format!("{v:0width$}")` writes it: the sign counts towards the width.
fn signed(v: i64, width: usize, out: &mut String) {
    if v < 0 {
        out.push_out('-');
        decimal(v.unsigned_abs(), width.saturating_sub(1), out);
    } else {
        decimal(v as u64, width, out);
    }
}

/// An integer zero-padded to a width: `pad(v, 2)` is `format!("{v:02}")`.
pub(crate) struct Pad(i64, usize);

pub(crate) fn pad(v: impl Into<i64>, width: usize) -> Pad {
    Pad(v.into(), width)
}

impl Piece for Pad {
    fn put(&self, out: &mut String) {
        signed(self.0, self.1, out);
    }
}

/// Lowercase hexadecimal, zero-padded to a width: `hex(v, 4)` is `format!("{v:04x}")`.
pub(crate) struct Hex(u64, usize);

pub(crate) fn hex(v: impl Into<u64>, width: usize) -> Hex {
    Hex(v.into(), width)
}

impl Piece for Hex {
    fn put(&self, out: &mut String) {
        let mut buf = [0u8; 16];
        let mut i = buf.len();
        let mut v = self.0;
        loop {
            i -= 1;
            buf[i] = b"0123456789abcdef"[(v & 15) as usize];
            v >>= 4;
            if v == 0 {
                break;
            }
        }
        for _ in buf.len() - i..self.1 {
            out.push_out('0');
        }
        for &b in &buf[i..] {
            out.push_out(b as char);
        }
    }
}

/// Appends each piece to `out`.
macro_rules! cat_to {
    ($out:expr, $($piece:expr),+ $(,)?) => {{
        let out: &mut alloc::string::String = $out;
        $( $crate::text::Piece::put(&$piece, out); )+
    }};
}

/// A new `String` of the pieces.
macro_rules! cat {
    ($($piece:expr),+ $(,)?) => {{
        let mut out = alloc::string::String::new();
        $( $crate::text::Piece::put(&$piece, &mut out); )+
        out
    }};
}

pub(crate) use {cat, cat_to};

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::format;

    #[test]
    fn same_as_format() {
        for v in [0i64, 7, -7, 42, -42, 999, -1000, 12345, i64::MIN, i64::MAX] {
            for w in [0, 2, 3, 4, 6] {
                assert_eq!(cat!(pad(v, w)), format!("{v:0w$}"), "{v} {w}");
            }
            assert_eq!(cat!(v), format!("{v}"));
        }
        for v in [0u64, 9, 10, 255, 0xFFFF, 0x10FFFF, u64::MAX] {
            for w in [0, 2, 4, 8] {
                assert_eq!(cat!(hex(v, w)), format!("{v:0w$x}"));
            }
            assert_eq!(cat!(v), format!("{v}"));
        }
        assert_eq!(cat!("a", 'b', String::from("c"), 1u8, -2i32), "abc1-2");
        let mut s = String::from("x");
        cat_to!(&mut s, "y", 3u32);
        assert_eq!(s, "xy3");
    }
}
