//! Number to string conversion.
//!
//! Decimal shortest output uses Ryu. Everything else (fixed-point, precision, exponential,
//! other radixes) follows QuickJS' `js_dtoa` (`dtoa.c`, MIT, Fabrice Bellard) with exact big
//! integer arithmetic, so the digits and the corner cases match qjs.

use super::big::Big;
use super::exact::{Round, scale_round, scale_to_f64};
use super::ryu;
use super::{MAX_FIXED_LEN, MAX_RADIX_LEN, MAX_SHORT_LEN, NumStr, RangeError};

/// Largest number of digits accepted by toFixed / toExponential / toPrecision.
const MAX_DIGITS: i32 = 100;

/// `round(2^24 / log2(radix))` for radix 2..=36, 0 for powers of two (from qjs).
#[rustfmt::skip]
const MUL_LOG2_RADIX: [u32; 35] = [
    0x000000, 0xa1849d, 0x000000, 0x6e40d2, 0x6308c9, 0x5b3065, 0x000000, 0x50c24e,
    0x4d104d, 0x4a0027, 0x4768ce, 0x452e54, 0x433d00, 0x418677, 0x000000, 0x3ea16b,
    0x3d645a, 0x3c43c2, 0x3b3b9a, 0x3a4899, 0x39680b, 0x3897b3, 0x37d5af, 0x372069,
    0x367686, 0x35d6df, 0x354072, 0x34b261, 0x342bea, 0x33ac62, 0x000000, 0x32bfd9,
    0x3251dd, 0x31e8d6, 0x318465,
];

/// `1 + ceil(53 / log2(radix))`: enough digits to round-trip any double (from qjs).
#[rustfmt::skip]
const DTOA_MAX_DIGITS: [u8; 35] = [
    54, 35, 28, 24, 22, 20, 19, 18, 17, 17, 16, 16, 15, 15, 15, 14, 14, 14, 14, 14, 13, 13,
    13, 13, 13, 13, 13, 12, 12, 12, 12, 12, 12, 12, 12,
];

/// `floor(a / log2(radix))` for -2048 <= a <= 2047.
fn mul_log2_radix(a: i32, radix: u32) -> i32 {
    if radix.is_power_of_two() {
        a.div_euclid(radix.trailing_zeros() as i32)
    } else {
        ((a as i64 * MUL_LOG2_RADIX[radix as usize - 2] as i64) >> 24) as i32
    }
}

/// How the exponent notation is chosen.
#[derive(Clone, Copy)]
enum Exp {
    /// Exponential notation when the decimal point position `e` is `<= -6` or `> max`.
    Auto(i32),
    Always,
    Never,
}

/// Writes `digits` (the significant digits) with the decimal point after `e` digits (so the
/// value is `0.digits * radix^e`), the way qjs' `js_dtoa` lays them out.
fn layout<const N: usize>(out: &mut NumStr<N>, digits: &[u8], e: i32, exp: Exp) {
    let p = digits.len() as i32;
    let exponential = match exp {
        Exp::Always => true,
        Exp::Never => false,
        Exp::Auto(max) => e <= -6 || e > max,
    };
    if exponential {
        out.push(digits[0]);
        if p > 1 {
            out.push(b'.');
            out.extend(&digits[1..]);
        }
        out.push(b'e');
        let x = e - 1;
        out.push(if x < 0 { b'-' } else { b'+' });
        let mut tmp = [0u8; 20];
        out.extend(u64_digits(x.unsigned_abs() as u64, 10, &mut tmp));
    } else if e <= 0 {
        out.extend(b"0.");
        out.push_zeros((-e) as usize);
        out.extend(digits);
    } else if e < p {
        out.extend(&digits[..e as usize]);
        out.push(b'.');
        out.extend(&digits[e as usize..]);
    } else {
        out.extend(digits);
        out.push_zeros((e - p) as usize);
    }
}

/// Writes `n` in `radix` at the end of `buf` and returns the digits.
fn u64_digits(mut n: u64, radix: u32, buf: &mut [u8]) -> &[u8] {
    let mut i = buf.len();
    loop {
        i -= 1;
        let d = (n % radix as u64) as u8;
        buf[i] = if d < 10 { b'0' + d } else { b'a' + d - 10 };
        n /= radix as u64;
        if n == 0 {
            return &buf[i..];
        }
    }
}

/// Writes the decimal digits of `b` at the end of `buf` and returns them.
fn big_decimal_digits(mut b: Big, buf: &mut [u8]) -> &[u8] {
    if b.is_zero() {
        let i = buf.len() - 1;
        buf[i] = b'0';
        return &buf[i..];
    }
    let mut i = buf.len();
    while !b.is_zero() {
        let mut chunk = b.div_small(1_000_000_000);
        let last = b.is_zero();
        for _ in 0..9 {
            i -= 1;
            buf[i] = b'0' + (chunk % 10) as u8;
            chunk /= 10;
            if last && chunk == 0 {
                break;
            }
        }
    }
    &buf[i..]
}

/// `|x|` as `m * 2^(e - 53)` with `2^52 <= m < 2^53` (subnormals are normalized, so `e` may
/// be below the normal range), as qjs does. `x` is finite and non-zero.
fn decompose(x: f64) -> (u64, i32) {
    let bits = x.to_bits();
    let mut e = ((bits >> 52) & 0x7ff) as i32;
    let mut m = bits & ((1 << 52) - 1);
    if e == 0 {
        let l = m.leading_zeros() as i32 - 11;
        e -= l - 1;
        m <<= l;
    } else {
        m |= 1 << 52;
    }
    (m, e - 1022)
}

/// `ToString(x)` for a Number: `Number::toString(x, 10)` in the spec.
///
/// `-0` gives `"0"`; a caller that wants `console.log`'s `-0` checks for it itself.
pub fn to_string(x: f64) -> NumStr<MAX_SHORT_LEN> {
    let mut out = NumStr::new();
    if non_finite_or_zero(&mut out, x) {
        return out;
    }
    if x < 0.0 {
        out.push(b'-');
    }
    let a = x.abs();
    if a < 9007199254740992.0 && (a as u64) as f64 == a {
        let mut tmp = [0u8; 20];
        out.extend(u64_digits(a as u64, 10, &mut tmp));
        return out;
    }
    let mut tmp = [0u8; 20];
    let (digits, e) = shortest(a, &mut tmp);
    layout(&mut out, digits, e, Exp::Auto(21));
    out
}

/// Handles NaN, the infinities and zero (all signs dropped for zero). Returns true if it did.
fn non_finite_or_zero<const N: usize>(out: &mut NumStr<N>, x: f64) -> bool {
    if x.is_nan() {
        out.extend(b"NaN");
    } else if x.is_infinite() {
        out.extend(if x < 0.0 {
            b"-Infinity" as &[u8]
        } else {
            b"Infinity"
        });
    } else if x == 0.0 {
        out.push(b'0');
    } else {
        return false;
    }
    true
}

/// Shortest decimal digits of a finite positive `a` and the decimal point position.
fn shortest(a: f64, tmp: &mut [u8; 20]) -> (&[u8], i32) {
    if let Some((m, k)) = short_decimal(a) {
        let digits = u64_digits(m, 10, tmp);
        return (digits, digits.len() as i32 - k);
    }
    ryu_shortest(a, tmp)
}

fn ryu_shortest(a: f64, tmp: &mut [u8; 20]) -> (&[u8], i32) {
    let bits = a.to_bits();
    let ryu::Decimal {
        mut mantissa,
        mut exponent,
    } = ryu::d2d(bits & ((1 << 52) - 1), (bits >> 52) as u32);
    while mantissa % 10 == 0 {
        mantissa /= 10;
        exponent += 1;
    }
    let digits = u64_digits(mantissa, 10, tmp);
    let e = exponent + digits.len() as i32;
    (digits, e)
}

/// The shortest form of `a` (positive, finite) when it has at most 8 decimals, such as
/// prices, scores and coordinates: `(m, k)` with `a == m / 10^k`. Much cheaper than Ryu,
/// and the same result (D1262):
/// - `m as f64 / 10^k` is the correctly rounded value of the decimal `m * 10^-k` (both
///   operands are exact), so the check is exactly "the decimal round-trips";
/// - while `ulp(a) * 10^k < 1`, the round-trip interval (at most `ulp(a)` wide, also for
///   powers of two) holds at most one integer multiple of `10^-k`, so the first `k` that
///   works gives the unique shortest digits Ryu would find, and no shorter form exists;
/// - a candidate near a half-way point cannot round-trip under that bound, so rounding in
///   `a * 10^k` never hides the right candidate.
///
/// `None` means "use Ryu".
fn short_decimal(a: f64) -> Option<(u64, i32)> {
    const P10: [f64; 9] = [1e0, 1e1, 1e2, 1e3, 1e4, 1e5, 1e6, 1e7, 1e8];
    const MAX: f64 = 9007199254740992.0; // 2^53
    let ulp = f64::from_bits(a.to_bits() + 1) - a;
    // Early exit for full-precision numbers (D1782), which otherwise try all eight `k`:
    // if some `m / 10^k` (k <= 8) round-trips, then `M = m * 10^(8-k)` is within
    // `ulp * 1e8 / 2` of `a * 1e8`, and the product's rounding adds less than
    // `ulp(a * 1e8) / 2 < ulp * 1e8`. So `a * 1e8` is within 1.5 `tol8` of the integer
    // `M`, which is then its nearest integer while `tol8 < 1/4`; a distance over 2 `tol8`
    // rules every `k` out. (With `tol8` that large, or a product at 2^53, the loop
    // decides.) A wrong exit would only cost speed: Ryu gives the same digits.
    let tol8 = ulp * 1e8;
    let y8 = a * 1e8;
    if tol8 < 0.25 && y8 < MAX {
        let d = (y8 + 0.5) as u64 as f64;
        if (y8 - d).abs() > 2.0 * tol8 {
            return None;
        }
    }
    for (k, &p) in P10.iter().enumerate().skip(1) {
        let tol = ulp * p;
        if tol >= 1.0 {
            return None;
        }
        let y = a * p;
        if y >= MAX {
            return None;
        }
        let m = (y + 0.5) as u64;
        let d = m as f64;
        // Cheap filter before the division: the candidate is within ulp(a) * 10^k / 2 of
        // the exact product, and `y` within half an ulp of that.
        if y - d > 2.0 * tol || d - y > 2.0 * tol {
            continue;
        }
        if d / p == a {
            // Not a multiple of 10, or a smaller k would have matched.
            return if m.is_multiple_of(10) {
                None
            } else {
                Some((m, k as i32))
            };
        }
    }
    None
}

/// `Number.prototype.toString(radix)` for an integer `radix` (the caller applies
/// ToIntegerOrInfinity and saturates). Radix 10 is [`to_string`].
///
/// Other radixes print the shortest digit string that round-trips in that radix (qjs'
/// behavior; the spec leaves non-integers implementation-defined), never in exponential
/// notation.
pub fn to_string_radix(x: f64, radix: i32) -> Result<NumStr<MAX_RADIX_LEN>, RangeError> {
    if !(2..=36).contains(&radix) {
        return Err(RangeError::Radix);
    }
    let radix = radix as u32;
    let mut out = NumStr::new();
    if radix == 10 {
        out.extend(to_string(x).as_bytes());
        return Ok(out);
    }
    if non_finite_or_zero(&mut out, x) {
        return Ok(out);
    }
    if x < 0.0 {
        out.push(b'-');
    }
    let a = x.abs();
    let (m, e) = decompose(a);
    let mut tmp = [0u8; 64];
    if (1..=53).contains(&e) && m & ((1 << (53 - e)) - 1) == 0 {
        // An integer below 2^53: print it exactly.
        out.extend(u64_digits(m >> (53 - e), radix, &mut tmp));
        return Ok(out);
    }
    let (mant, p, e10) = shortest_radix(a, m, e, radix);
    let digits = u64_digits(mant, radix, &mut tmp);
    // `mant` has had its trailing zeros removed, so it may have fewer than `p` digits only if
    // it has leading zeros, which cannot happen (mant >= radix^(p - 1)).
    debug_assert_eq!(digits.len() as i32, p);
    layout(&mut out, digits, e10, Exp::Never);
    Ok(out)
}

/// The shortest `mant` (with `p` digits) such that `mant * radix^(e - p)` rounds to `a`, and
/// `e`: qjs' JS_DTOA_FORMAT_FREE search. `a = m * 2^(e2 - 53)` as from `decompose`.
fn shortest_radix(a: f64, m: u64, e2: i32, radix: u32) -> (u64, i32, i32) {
    let p_max = DTOA_MAX_DIGITS[radix as usize - 2] as i32;
    let e0 = 1 + mul_log2_radix(e2 - 1, radix);
    let mut found: Option<(u64, i32, i32)> = None;
    let mut p = p_max;
    loop {
        let mant_max = (radix as u64).pow(p as u32);
        // The nearest p-digit mantissa and its exponent.
        let mut e = e0;
        let mut mant;
        loop {
            let t = scale_round(Big::from_u64(m), e2 - 53, radix, p - e, Round::HalfEven);
            mant = if t.fits_u64() { t.low_u64() } else { u64::MAX };
            if mant < mant_max {
                break;
            }
            e += 1;
        }
        let (p1, e1, mant1) = (p, e, mant);
        let mut ok = false;
        let mut i = 0;
        loop {
            while mant % radix as u64 == 0 {
                mant /= radix as u64;
                p -= 1;
            }
            if found.is_none() {
                // p_max digits always round-trip.
                ok = true;
                break;
            }
            if scale_to_f64(Big::from_u64(mant), 0, radix, e - p, false) == a {
                ok = true;
                break;
            }
            // When m is a power of two the rounding interval is asymmetric: the nearest
            // p-digit number may be below the interval while the next one up is inside.
            if i != 0 || m != 1 << 52 {
                break;
            }
            p = p1;
            e = e1;
            mant = mant1 + 1;
            if mant == mant_max {
                mant /= radix as u64;
                e += 1;
            }
            i += 1;
        }
        if !ok {
            break;
        }
        found = Some((mant, p, e));
        if p == 1 {
            break;
        }
        p -= 1;
    }
    found.unwrap_or((m, 0, 0))
}

/// `Number.prototype.toFixed(digits)`. The caller passes ToIntegerOrInfinity(digits),
/// saturated to `i32`.
pub fn to_fixed(x: f64, digits: i32) -> Result<NumStr<MAX_FIXED_LEN>, RangeError> {
    if !(0..=MAX_DIGITS).contains(&digits) {
        return Err(RangeError::Digits);
    }
    let mut out = NumStr::new();
    if x.is_nan() {
        out.extend(b"NaN");
        return Ok(out);
    }
    if x.abs() >= 1e21 {
        out.extend(to_string(x).as_bytes());
        return Ok(out);
    }
    let f = digits as usize;
    let mut tmp = [0u8; 128];
    let n: &[u8] = if x == 0.0 {
        b"0"
    } else {
        if x < 0.0 {
            out.push(b'-');
        }
        let (m, e) = decompose(x.abs());
        let b = scale_round(Big::from_u64(m), e - 53, 10, digits, Round::HalfUp);
        big_decimal_digits(b, &mut tmp)
    };
    // n is round(|x| * 10^f); print it with f fractional digits.
    if n.len() > f {
        let k = n.len() - f;
        out.extend(&n[..k]);
        if f > 0 {
            out.push(b'.');
            out.extend(&n[k..]);
        }
    } else {
        out.extend(b"0.");
        out.push_zeros(f - n.len());
        out.extend(n);
    }
    Ok(out)
}

/// `Number.prototype.toExponential(digits)`. `None` is an undefined argument (as many digits
/// as needed); otherwise the caller passes ToIntegerOrInfinity(digits), saturated to `i32`.
pub fn to_exponential(x: f64, digits: Option<i32>) -> Result<NumStr<MAX_FIXED_LEN>, RangeError> {
    let mut out = NumStr::new();
    if !x.is_finite() {
        out.extend(to_string(x).as_bytes());
        return Ok(out);
    }
    match digits {
        None => {
            if x == 0.0 {
                out.extend(b"0e+0");
                return Ok(out);
            }
            if x < 0.0 {
                out.push(b'-');
            }
            let mut tmp = [0u8; 20];
            let (d, e) = shortest(x.abs(), &mut tmp);
            layout(&mut out, d, e, Exp::Always);
        }
        Some(f) => {
            if !(0..=MAX_DIGITS).contains(&f) {
                return Err(RangeError::Digits);
            }
            precision_digits(&mut out, x, f + 1, Exp::Always);
        }
    }
    Ok(out)
}

/// `Number.prototype.toPrecision(precision)`. `None` is an undefined argument (same as
/// `ToString`); otherwise the caller passes ToIntegerOrInfinity(precision), saturated to `i32`.
pub fn to_precision(x: f64, precision: Option<i32>) -> Result<NumStr<MAX_FIXED_LEN>, RangeError> {
    let mut out = NumStr::new();
    let p = match precision {
        Some(p) if x.is_finite() => p,
        _ => {
            out.extend(to_string(x).as_bytes());
            return Ok(out);
        }
    };
    if !(1..=MAX_DIGITS).contains(&p) {
        return Err(RangeError::Digits);
    }
    precision_digits(&mut out, x, p, Exp::Auto(p));
    Ok(out)
}

/// Writes finite `x` rounded (half up) to `p` significant digits.
fn precision_digits(out: &mut NumStr<MAX_FIXED_LEN>, x: f64, p: i32, exp: Exp) {
    let mut tmp = [0u8; 128];
    if x == 0.0 {
        tmp[..p as usize].fill(b'0');
        layout(out, &tmp[..p as usize], 1, exp);
        return;
    }
    if x < 0.0 {
        out.push(b'-');
    }
    let (m, e2) = decompose(x.abs());
    // Start from a lower bound of the decimal exponent and step up until the rounded
    // mantissa has p digits.
    let mut e = 1 + mul_log2_radix(e2 - 1, 10);
    loop {
        let b = scale_round(Big::from_u64(m), e2 - 53, 10, p - e, Round::HalfUp);
        let len = big_decimal_digits(b.clone(), &mut tmp).len() as i32;
        if len <= p {
            let digits = big_decimal_digits(b, &mut tmp);
            layout(out, digits, e, exp);
            return;
        }
        e += 1;
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;

    #[test]
    fn mul_log2_radix_is_exact() {
        for radix in 2..=36u32 {
            let l = (radix as f64).log2();
            for a in -2048..=2047 {
                assert_eq!(
                    mul_log2_radix(a, radix),
                    (a as f64 / l).floor() as i32,
                    "radix {radix}, a {a}"
                );
            }
        }
    }

    /// The radix search (qjs' algorithm) and Ryu are independent; in radix 10 they must agree.
    #[test]
    fn radix_search_matches_ryu_in_base_10() {
        let mut x = 0x5eed_1234_abcd_ef01u64;
        for i in 0..100_000 {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            let a = if i % 4 == 0 {
                // Powers of two have asymmetric rounding intervals.
                f64::from_bits(x & 0x7ff0_0000_0000_0000)
            } else {
                f64::from_bits(x >> 1)
            };
            if !a.is_finite() || a == 0.0 {
                continue;
            }
            let (m, e2) = decompose(a);
            let (mant, p, e) = shortest_radix(a, m, e2, 10);
            let mut tmp = [0u8; 20];
            let digits = u64_digits(mant, 10, &mut tmp);
            assert_eq!(digits.len() as i32, p);
            let mut tmp2 = [0u8; 20];
            let (ryu_digits, ryu_e) = shortest(a, &mut tmp2);
            assert_eq!((digits, e), (ryu_digits, ryu_e), "{a:e}");
        }
    }

    /// [`short_decimal`] without the early exit of D1782: the reference for it.
    fn short_decimal_loop_only(a: f64) -> Option<(u64, i32)> {
        const P10: [f64; 9] = [1e0, 1e1, 1e2, 1e3, 1e4, 1e5, 1e6, 1e7, 1e8];
        const MAX: f64 = 9007199254740992.0; // 2^53
        let ulp = f64::from_bits(a.to_bits() + 1) - a;
        for (k, &p) in P10.iter().enumerate().skip(1) {
            let tol = ulp * p;
            if tol >= 1.0 {
                return None;
            }
            let y = a * p;
            if y >= MAX {
                return None;
            }
            let m = (y + 0.5) as u64;
            let d = m as f64;
            // Cheap filter before the division: the candidate is within ulp(a) * 10^k / 2 of
            // the exact product, and `y` within half an ulp of that.
            if y - d > 2.0 * tol || d - y > 2.0 * tol {
                continue;
            }
            if d / p == a {
                // Not a multiple of 10, or a smaller k would have matched.
                return if m.is_multiple_of(10) {
                    None
                } else {
                    Some((m, k as i32))
                };
            }
        }
        None
    }

    /// D1782: the early exit never gives up on a number the loop would have answered.
    #[test]
    fn short_decimal_early_exit_loses_nothing() {
        let mut x = 0x0078_5eed_dead_beefu64;
        let mut next = || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x
        };
        let mut hits = 0;
        for i in 0..3_000_000u64 {
            let r = next();
            let a = match i % 4 {
                0 => f64::from_bits(r >> 1),
                1 => (r % 1_000_000_000_000) as f64 / [1e0, 1e2, 1e5, 1e8][(r >> 60) as usize % 4],
                2 => {
                    let b = ((r % 100_000_000) as f64 / 1e8).to_bits();
                    f64::from_bits(b.wrapping_add(r >> 62).wrapping_sub(2))
                }
                _ => (r % 10_000) as f64 * [1e-8, 1e-3, 1e3, 1e9][(r >> 60) as usize % 4],
            };
            if !a.is_finite() || a <= 0.0 {
                continue;
            }
            let want = short_decimal_loop_only(a);
            hits += want.is_some() as usize;
            assert_eq!(short_decimal(a), want, "{a:e}");
        }
        assert!(hits > 1_000_000, "{hits}");
    }

    /// The short-decimal fast path agrees with Ryu wherever it answers.
    #[test]
    fn short_decimal_matches_ryu() {
        let mut x = 0x0bad_5eed_dead_beefu64;
        let mut next = || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x
        };
        let mut hits = 0;
        let check = |a: f64, hits: &mut usize| {
            if !a.is_finite() || a <= 0.0 {
                return;
            }
            if let Some((m, k)) = short_decimal(a) {
                *hits += 1;
                let mut t1 = [0u8; 20];
                let mut t2 = [0u8; 20];
                let digits = u64_digits(m, 10, &mut t1);
                let fast = (digits, digits.len() as i32 - k);
                assert_eq!(fast, ryu_shortest(a, &mut t2), "{a:e} = {m}e-{k}");
            }
        };
        for i in 0..2_000_000u64 {
            let r = next();
            let a = match i % 5 {
                // Random doubles (rarely short).
                0 => f64::from_bits(r >> 1),
                // Short decimals of every size and scale, and their neighbours.
                1 | 2 => {
                    let m = (r >> 20) % 10u64.pow(1 + (r % 15) as u32);
                    let k = ((r >> 8) % 12) as i32;
                    let a = m as f64 / 10f64.powi(k);
                    match (r >> 4) % 3 {
                        0 => a,
                        1 => f64::from_bits(a.to_bits() + 1),
                        _ => f64::from_bits(a.to_bits().wrapping_sub(1)),
                    }
                }
                // Powers of two (asymmetric intervals) times short decimals.
                3 => 2f64.powi((r % 80) as i32 - 40) * ((r >> 16) % 1000) as f64 / 8.0,
                // Near the 2^53 / 10^k limits.
                _ => {
                    let k = 1 + (r % 8) as i32;
                    9007199254740992.0 / 10f64.powi(k) - ((r >> 8) % 1000) as f64 / 10f64.powi(k)
                }
            };
            check(a, &mut hits);
        }
        // The path is actually exercised.
        assert!(hits > 200_000, "{hits}");
    }

    #[test]
    fn digits_tables() {
        for radix in 2..=36u32 {
            let expect = 1 + (53.0 / (radix as f64).log2()).ceil() as u8;
            assert_eq!(DTOA_MAX_DIGITS[radix as usize - 2], expect);
        }
    }
}
