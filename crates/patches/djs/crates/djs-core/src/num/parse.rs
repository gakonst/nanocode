//! String to number conversion.
//!
//! Decimal input is correctly rounded: Clinger's fast path for short inputs, then the
//! Eisel-Lemire algorithm (ported from Rust's `core::num::dec2flt`, MIT/Apache-2.0), then an
//! exact big integer slow path. Power-of-two radixes are converted exactly. Other radixes
//! (only reachable through `parseInt`) follow qjs: about 128 bits worth of leading digits are
//! kept and the rest truncated, then the result is rounded exactly.

use super::CodeUnit;
use super::big::Big;
use super::exact::{round_to_f64, scale_to_f64};
use super::pow5::lemire_pow5;

/// Significant decimal digits kept by the slow path. Any halfway point between two doubles has
/// at most 768 significant digits, so truncating after more than that (and remembering
/// whether the rest was non-zero) never changes the rounding.
const MAX_SIG_DIGITS: u32 = 800;

/// Exact powers of ten for Clinger's fast path.
const POW10_F64: [f64; 23] = [
    1e0, 1e1, 1e2, 1e3, 1e4, 1e5, 1e6, 1e7, 1e8, 1e9, 1e10, 1e11, 1e12, 1e13, 1e14, 1e15, 1e16,
    1e17, 1e18, 1e19, 1e20, 1e21, 1e22,
];

/// StrWhiteSpaceChar: WhiteSpace (TAB, VT, FF, ZWNBSP, any Zs) and LineTerminator.
fn is_space(c: u32) -> bool {
    matches!(
        c,
        0x09..=0x0D
            | 0x20
            | 0xA0
            | 0x1680
            | 0x2000..=0x200A
            | 0x2028
            | 0x2029
            | 0x202F
            | 0x205F
            | 0x3000
            | 0xFEFF
    )
}

/// The code unit at `i`, or 0 past the end (0 is neither a digit nor a sign).
fn at<T: CodeUnit>(s: &[T], i: usize) -> u32 {
    s.get(i).map_or(0, |&c| c.into())
}

/// The digit value of `c` in radix 36, or 36 if it is not a digit.
fn digit(c: u32) -> u32 {
    match c {
        0x30..=0x39 => c - 0x30,
        0x41..=0x5A => c - 0x41 + 10,
        0x61..=0x7A => c - 0x61 + 10,
        _ => 36,
    }
}

fn trim_start<T: CodeUnit>(s: &[T]) -> &[T] {
    let n = s.iter().take_while(|&&c| is_space(c.into())).count();
    &s[n..]
}

fn trim<T: CodeUnit>(s: &[T]) -> &[T] {
    let s = trim_start(s);
    let n = s.iter().rev().take_while(|&&c| is_space(c.into())).count();
    &s[..s.len() - n]
}

/// Parses an optional sign at the start of `s`: (negative, index after it).
fn sign<T: CodeUnit>(s: &[T]) -> (bool, usize) {
    match at(s, 0) {
        0x2B => (false, 1),
        0x2D => (true, 1),
        _ => (false, 0),
    }
}

fn starts_with_infinity<T: CodeUnit>(s: &[T], i: usize) -> bool {
    let word = b"Infinity";
    s.len() >= i + word.len()
        && word
            .iter()
            .enumerate()
            .all(|(k, &b)| at(s, i + k) == b as u32)
}

fn apply_sign(neg: bool, v: f64) -> f64 {
    if neg { -v } else { v }
}

/// A scanned decimal literal.
struct Decimal {
    /// Index after the literal.
    end: usize,
    /// The digits before and after the decimal point, as index ranges (they may contain `_`
    /// separators when those are allowed).
    int: (usize, usize),
    frac: (usize, usize),
    /// The explicit exponent, saturated far outside the double range.
    exp: i64,
    /// The first 19 significant digits, and the value is about `w * 10^q`.
    w: u64,
    q: i64,
    /// Non-zero digits were dropped from `w`: the value is in (w, w + 1) * 10^q.
    truncated: bool,
}

/// Scans `DecimalDigits [. DecimalDigits?] [ExponentPart]` or `. DecimalDigits
/// [ExponentPart]` starting at `start`, taking the longest valid prefix. `sep` skips `_`
/// inside digit runs (numeric literals, already validated by the lexer); `int_only` stops
/// before a dot or exponent (parseInt). Returns `None` if there are no digits.
fn scan_decimal<T: CodeUnit>(s: &[T], start: usize, sep: bool, int_only: bool) -> Option<Decimal> {
    // Accumulate every digit with wrapping arithmetic; that is exact when there are at most
    // 19 of them, and the rare longer inputs are re-scanned below.
    let mut w = 0u64;
    let mut i = start;
    while i < s.len() {
        let c: u32 = s[i].into();
        let d = c.wrapping_sub(0x30);
        if d < 10 {
            w = w.wrapping_mul(10).wrapping_add(d as u64);
        } else if !(sep && c == 0x5F) {
            break;
        }
        i += 1;
    }
    let int = (start, i);
    let mut frac = (i, i);
    let mut n_frac = 0i64;
    if !int_only && at(s, i) == 0x2E && (i > start || at(s, i + 1).wrapping_sub(0x30) < 10) {
        i += 1;
        let frac_start = i;
        while i < s.len() {
            let c: u32 = s[i].into();
            let d = c.wrapping_sub(0x30);
            if d < 10 {
                w = w.wrapping_mul(10).wrapping_add(d as u64);
                n_frac += 1;
            } else if !(sep && c == 0x5F) {
                break;
            }
            i += 1;
        }
        frac = (frac_start, i);
    }
    // Digits, counting separators too (which can only make this an overestimate).
    let n_digits = (int.1 - int.0) + (frac.1 - frac.0);
    if n_digits == 0 {
        return None;
    }
    let mut exp = 0i64;
    if !int_only && at(s, i) | 0x20 == 0x65 {
        let (neg, mut j) = match at(s, i + 1) {
            0x2B => (false, i + 2),
            0x2D => (true, i + 2),
            _ => (false, i + 1),
        };
        if at(s, j).wrapping_sub(0x30) < 10 {
            let mut e = 0i64;
            while j < s.len() {
                let c: u32 = s[j].into();
                let d = c.wrapping_sub(0x30);
                if d < 10 {
                    // Saturate far outside the double range.
                    if e < 1_000_000_000_000 {
                        e = e * 10 + d as i64;
                    }
                } else if !(sep && c == 0x5F) {
                    break;
                }
                j += 1;
            }
            exp = if neg { -e } else { e };
            i = j;
        }
    }
    let mut d = Decimal {
        end: i,
        int,
        frac,
        exp,
        w,
        q: exp - n_frac,
        truncated: false,
    };
    if n_digits > 19 {
        first_19_digits(&mut decimal_digits(s, &d), &mut d);
    }
    Some(d)
}

/// The digits of a scanned decimal as (value, after the point), without separators.
///
/// The rare paths that re-read digits take this as a `dyn Iterator`, so that they are not
/// instantiated once per code unit type.
fn decimal_digits<'a, T: CodeUnit>(
    s: &'a [T],
    d: &Decimal,
) -> impl Iterator<Item = (u32, bool)> + 'a {
    let (int, frac) = (d.int, d.frac);
    (int.0..int.1)
        .map(move |i| (at(s, i).wrapping_sub(0x30), false))
        .chain((frac.0..frac.1).map(move |i| (at(s, i).wrapping_sub(0x30), true)))
        .filter(|&(c, _)| c < 10)
}

/// Recomputes `w` from the first 19 significant digits, with `q` and `truncated` to match.
fn first_19_digits(digits: &mut dyn Iterator<Item = (u32, bool)>, d: &mut Decimal) {
    let mut w = 0u64;
    let mut n_sig = 0u32;
    let mut truncated = false;
    let mut shift = 0i64;
    for (c, in_frac) in digits {
        if n_sig < 19 {
            w = w * 10 + c as u64;
            n_sig += (w != 0) as u32;
            shift -= in_frac as i64;
        } else {
            truncated |= c != 0;
            shift += !in_frac as i64;
        }
    }
    d.w = w;
    d.q = d.exp + shift;
    d.truncated = truncated;
}

/// The value of a scanned decimal (without sign).
fn decimal_value<T: CodeUnit>(s: &[T], d: &Decimal) -> f64 {
    if d.w == 0 {
        // No significant digits: zero, whatever the exponent.
        return 0.0;
    }
    if !d.truncated && d.w <= 1 << 53 {
        // Clinger's fast path: w and 10^|q| are exact doubles, so one IEEE operation rounds
        // correctly.
        if (-22..=22).contains(&d.q) {
            let v = d.w as f64;
            return if d.q < 0 {
                v / POW10_F64[(-d.q) as usize]
            } else {
                v * POW10_F64[d.q as usize]
            };
        }
        // "Disguised" fast path: move some of the exponent into the (still exact) mantissa.
        if (23..=22 + 15).contains(&d.q)
            && let Some(w) = d.w.checked_mul(10u64.pow((d.q - 22) as u32))
            && w <= 1 << 53
        {
            return w as f64 * 1e22;
        }
    }
    if let Some(v) = lemire(d.w, d.q)
        && (!d.truncated || lemire(d.w + 1, d.q) == Some(v))
    {
        return v;
    }
    slow_decimal(&mut decimal_digits(s, d), d.exp)
}

/// Eisel-Lemire: `w * 10^q` correctly rounded, or `None` when the 128-bit approximation
/// cannot decide (then the slow path does). Ported from `core::num::dec2flt::lemire`.
fn lemire(w: u64, q: i64) -> Option<f64> {
    if w == 0 || q < -342 {
        return Some(0.0);
    }
    if q > 308 {
        return Some(f64::INFINITY);
    }
    let lz = w.leading_zeros();
    let w = w << lz;
    let (lo, hi) = product_approx(q as i32, w);
    if lo == u64::MAX && !(-27..=55).contains(&q) {
        // The approximation may be off by one in a way that matters for rounding. Within
        // [-27, 55] the product is exact or the error cannot cross a halfway point.
        return None;
    }
    let upperbit = (hi >> 63) as i32;
    let shift = upperbit + 64 - 52 - 3;
    let mut mantissa = hi >> shift;
    let mut power2 = power(q as i32) + upperbit - lz as i32 + 1022 + 1;
    if power2 <= 0 {
        if -power2 + 1 >= 64 {
            return Some(0.0);
        }
        // Subnormal.
        mantissa >>= -power2 + 1;
        mantissa += mantissa & 1;
        mantissa >>= 1;
        let p = (mantissa >= 1 << 52) as u64;
        return Some(f64::from_bits(mantissa | p << 52));
    }
    // A product with an exact low half that falls exactly between two doubles must round to
    // even rather than up. That needs 5^q to fit in 64 bits (or be exact in 128 bits).
    if lo <= 1 && (-4..=23).contains(&q) && mantissa & 3 == 1 && (mantissa << shift) == hi {
        mantissa &= !1;
    }
    mantissa += mantissa & 1;
    mantissa >>= 1;
    if mantissa >= 2 << 52 {
        mantissa = 1 << 52;
        power2 += 1;
    }
    mantissa &= !(1 << 52);
    if power2 >= 0x7FF {
        return Some(f64::INFINITY);
    }
    Some(f64::from_bits(mantissa | (power2 as u64) << 52))
}

/// `floor(q * log2(10)) + 63`, exact for the whole exponent range.
fn power(q: i32) -> i32 {
    (q.wrapping_mul(152_170 + 65536) >> 16) + 63
}

/// The high 128 bits of `w * 5^q`, approximately (the second multiplication is only done when
/// the first one cannot decide the 55 bits that matter).
fn product_approx(q: i32, w: u64) -> (u64, u64) {
    const MASK: u64 = u64::MAX >> 55;
    let p = lemire_pow5(q);
    let (hi5, lo5) = ((p >> 64) as u64, p as u64);
    let first = w as u128 * hi5 as u128;
    let (mut first_lo, mut first_hi) = (first as u64, (first >> 64) as u64);
    if first_hi & MASK == MASK {
        let second_hi = ((w as u128 * lo5 as u128) >> 64) as u64;
        first_lo = first_lo.wrapping_add(second_hi);
        if second_hi > first_lo {
            first_hi += 1;
        }
    }
    (first_lo, first_hi)
}

/// Exact conversion with big integers: the decimal with these digits and explicit exponent.
fn slow_decimal(digits: &mut dyn Iterator<Item = (u32, bool)>, exp: i64) -> f64 {
    let mut acc = DigitAcc::new(10);
    let mut sticky = false;
    let mut e10 = exp;
    for (c, in_frac) in digits {
        if acc.n == 0 && c == 0 {
            // A leading zero; after the point it still moves the point.
            e10 -= in_frac as i64;
        } else if acc.n < MAX_SIG_DIGITS {
            acc.push(c);
            e10 -= in_frac as i64;
        } else {
            sticky |= c != 0;
            e10 += !in_frac as i64;
        }
    }
    let n = acc.n as i64;
    if n == 0 {
        return 0.0;
    }
    let big = acc.finish();
    // The value is in [10^(n - 1 + e10), 10^(n + e10)).
    if n + e10 <= -324 {
        // Below 10^-324, less than half the smallest subnormal.
        return 0.0;
    }
    if n - 1 + e10 >= 309 {
        return f64::INFINITY;
    }
    scale_to_f64(big, 0, 10, e10 as i32, sticky)
}

/// Accumulates digits into a big integer, a limb-sized chunk at a time.
struct DigitAcc {
    big: Big,
    radix: u32,
    chunk: u32,
    chunk_mul: u32,
    chunk_max: u32,
    /// Digits pushed so far.
    n: u32,
}

impl DigitAcc {
    fn new(radix: u32) -> DigitAcc {
        let (max, _) = super::big::limb_power(radix);
        DigitAcc {
            big: Big::zero(),
            radix,
            chunk: 0,
            chunk_mul: 1,
            chunk_max: max,
            n: 0,
        }
    }

    fn push(&mut self, d: u32) {
        self.chunk = self.chunk * self.radix + d;
        self.chunk_mul *= self.radix;
        self.n += 1;
        if self.chunk_mul == self.chunk_max {
            self.big.mul_add_small(self.chunk_mul, self.chunk);
            self.chunk = 0;
            self.chunk_mul = 1;
        }
    }

    fn finish(mut self) -> Big {
        if self.chunk_mul > 1 {
            self.big.mul_add_small(self.chunk_mul, self.chunk);
        }
        self.big
    }
}

/// The value of the radix-`radix` digits in `s[start..end]` (skipping `_`), `radix != 10`.
fn radix_int_value<T: CodeUnit>(s: &[T], start: usize, end: usize, radix: u32) -> f64 {
    let mut digits = (start..end)
        .map(|i| digit(at(s, i)))
        .filter(|&c| c < radix)
        .skip_while(|&c| c == 0);
    radix_digits_value(&mut digits, radix)
}

/// The value of radix-`radix` digits without leading zeros.
fn radix_digits_value(digits: &mut dyn Iterator<Item = u32>, radix: u32) -> f64 {
    if radix.is_power_of_two() {
        // Exact: keep at least 59 significant bits, the rest only matters as a sticky bit.
        let bits = radix.trailing_zeros();
        let mut acc = 0u64;
        let mut exp = 0i32;
        let mut sticky = false;
        for c in digits {
            if acc < 1 << 58 {
                acc = acc << bits | c as u64;
            } else {
                sticky |= c != 0;
                // Saturate far above the double range.
                exp = exp.saturating_add(bits as i32).min(1 << 20);
            }
        }
        return round_to_f64(Big::from_u64(acc), exp, sticky);
    }
    // Like qjs: keep the leading digits that fit in 128 bits, drop (not round) the others.
    let max_digits = {
        let mut n = 0;
        let mut p = 1u128;
        while let Some(next) = p.checked_mul(radix as u128) {
            p = next;
            n += 1;
        }
        n
    };
    let mut acc = DigitAcc::new(radix);
    let mut extra = 0u32;
    for c in digits {
        if acc.n < max_digits {
            acc.push(c);
        } else {
            extra = extra.saturating_add(1);
        }
    }
    // With r^(digits - 1) >= 2^1025 the value overflows; testing with floor(log2(r)) keeps
    // the exact computation below within the big integer capacity.
    let total = acc.n.saturating_add(extra);
    if total > 0 && (total - 1).saturating_mul(31 - radix.leading_zeros()) >= 1025 {
        return f64::INFINITY;
    }
    scale_to_f64(acc.finish(), 0, radix, extra as i32, false)
}

/// End of the run of radix-`radix` digits (and `_` if `sep`) starting at `start`.
fn scan_radix_digits<T: CodeUnit>(s: &[T], start: usize, radix: u32, sep: bool) -> usize {
    let mut i = start;
    loop {
        let c = at(s, i);
        if !(digit(c) < radix || (sep && c == 0x5F)) {
            return i;
        }
        i += 1;
    }
}

/// `0x`, `0o` or `0b` (either case) at the start of `s`: the radix.
fn radix_prefix<T: CodeUnit>(s: &[T], bin_oct: bool) -> Option<u32> {
    if at(s, 0) != 0x30 {
        return None;
    }
    match at(s, 1) | 0x20 {
        0x78 => Some(16),
        0x6F if bin_oct => Some(8),
        0x62 if bin_oct => Some(2),
        _ => None,
    }
}

/// Two-byte input for the parsers below as bytes (D2021): every JS whitespace and line
/// terminator becomes a space, any other unit above 0x7F a byte that no number syntax
/// accepts (0x80, which is not whitespace either). Numbers are ASCII, so the parse result
/// is the same, and the parsers need only their one-byte instance in the module.
pub fn narrow_units(units: impl Iterator<Item = u16>) -> alloc::vec::Vec<u8> {
    units
        .map(|u| match u {
            0..=0x7F => u as u8,
            _ if is_space(u as u32) => b' ',
            _ => 0x80,
        })
        .collect()
}

/// `StringToNumber(str)`: the conversion `Number(str)` and unary `+` do.
///
/// Leading and trailing whitespace and line terminators are ignored, the empty string is 0,
/// `Infinity` may be signed, `0x`/`0o`/`0b` integers may not, and anything else invalid is NaN.
pub fn string_to_number<T: CodeUnit>(s: &[T]) -> f64 {
    let s = trim(s);
    if s.is_empty() {
        return 0.0;
    }
    if let Some(radix) = radix_prefix(s, true) {
        let end = scan_radix_digits(s, 2, radix, false);
        if end == 2 || end != s.len() {
            return f64::NAN;
        }
        return radix_int_value(s, 2, end, radix);
    }
    let (neg, i) = sign(s);
    if starts_with_infinity(s, i) {
        return if s.len() == i + 8 {
            apply_sign(neg, f64::INFINITY)
        } else {
            f64::NAN
        };
    }
    match scan_decimal(s, i, false, false) {
        Some(d) if d.end == s.len() => apply_sign(neg, decimal_value(s, &d)),
        _ => f64::NAN,
    }
}

/// The global `parseFloat(string)`, after ToString: the longest prefix (after leading
/// whitespace) that is a decimal literal or a signed `Infinity`.
pub fn parse_float<T: CodeUnit>(s: &[T]) -> f64 {
    let s = trim_start(s);
    let (neg, i) = sign(s);
    if starts_with_infinity(s, i) {
        return apply_sign(neg, f64::INFINITY);
    }
    match scan_decimal(s, i, false, false) {
        Some(d) => apply_sign(neg, decimal_value(s, &d)),
        None => f64::NAN,
    }
}

/// The global `parseInt(string, radix)`, after ToString; `radix` is ToInt32(radix).
pub fn parse_int<T: CodeUnit>(s: &[T], radix: i32) -> f64 {
    let s = trim_start(s);
    let (neg, mut i) = sign(s);
    let mut r = radix;
    let mut strip_prefix = true;
    if r != 0 {
        if !(2..=36).contains(&r) {
            return f64::NAN;
        }
        if r != 16 {
            strip_prefix = false;
        }
    } else {
        r = 10;
    }
    if strip_prefix && radix_prefix(&s[i..], false).is_some() {
        i += 2;
        r = 16;
    }
    let r = r as u32;
    let end = scan_radix_digits(s, i, r, false);
    if end == i {
        return f64::NAN;
    }
    let v = if r == 10 {
        match scan_decimal(&s[..end], i, false, true) {
            Some(d) => decimal_value(s, &d),
            None => return f64::NAN,
        }
    } else {
        radix_int_value(s, i, end, r)
    };
    apply_sign(neg, v)
}

/// The value of a numeric literal: decimal (`1_000.5e-3`, `.5`, `5.`), `0x`, `0o` or `0b`,
/// with `_` separators. The lexer has validated the syntax; this only computes the value
/// (correctly rounded). Legacy octal (`017`) and `08` are not valid in strict code and are
/// read as decimal. Returns NaN for input that is not a numeric literal.
pub fn literal_value(s: &[u8]) -> f64 {
    match radix_prefix(s, true) {
        Some(radix) => literal_digits_value(radix, &s[2..]),
        None => literal_digits_value(10, s),
    }
}

/// [`literal_value`] for a literal already split by the lexer (`lexer::NumberLiteral`):
/// `radix` is 10, 16, 8 or 2 and `digits` excludes the `0x`/`0o`/`0b` prefix. Decimal digits
/// include the fraction and exponent.
pub fn literal_digits_value(radix: u32, digits: &[u8]) -> f64 {
    if radix == 10 {
        return match scan_decimal(digits, 0, true, false) {
            Some(d) if d.end == digits.len() => decimal_value(digits, &d),
            _ => f64::NAN,
        };
    }
    let end = scan_radix_digits(digits, 0, radix, true);
    if end == 0 || end != digits.len() || !matches!(radix, 2 | 8 | 16) {
        return f64::NAN;
    }
    radix_int_value(digits, 0, end, radix)
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use std::string::{String, ToString};

    /// Exercises the rarely taken paths directly: the slow path for every input, and
    /// Eisel-Lemire whenever it claims an answer.
    #[test]
    fn slow_path_and_lemire_match_std() {
        let mut x = 0x0bad_cafe_dead_beefu64;
        let mut next = move || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x
        };
        let mut text = String::new();
        let mut lemire_none = 0;
        for i in 0..200_000 {
            text.clear();
            let n_int = next() % 30;
            let n_frac = next() % 30;
            for _ in 0..n_int {
                text.push((b'0' + (next() % 10) as u8) as char);
            }
            text.push('.');
            for _ in 0..n_frac {
                text.push((b'0' + (next() % 10) as u8) as char);
            }
            if n_int + n_frac == 0 {
                text.push('3');
            }
            let e = (next() % 760) as i64 - 380;
            text.push('e');
            text.push_str(&e.to_string());
            if i % 3 == 0 {
                // Doubles printed with 17 to 25 digits sit close to halfway points.
                let v = f64::from_bits(next() >> 1);
                if !v.is_finite() {
                    continue;
                }
                text = std::format!("{:.*e}", 16 + (next() % 9) as usize, v);
            }
            let expect: f64 = text.parse().unwrap();
            let d = scan_decimal(text.as_bytes(), 0, false, false).unwrap();
            assert_eq!(d.end, text.len());
            let slow = slow_decimal(&mut decimal_digits(text.as_bytes(), &d), d.exp);
            assert_eq!(slow.to_bits(), expect.to_bits(), "slow path: {text}");
            match lemire(d.w, d.q) {
                Some(v) if !d.truncated => {
                    assert_eq!(v.to_bits(), expect.to_bits(), "lemire: {text}")
                }
                Some(_) => {}
                None => lemire_none += 1,
            }
        }
        // The fallback is genuinely rare.
        assert!(lemire_none < 100, "{lemire_none}");
    }
}
