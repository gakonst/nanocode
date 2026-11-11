//! Exact scaling and rounding of `x * 2^e2 * radix^k` with big integers.
//!
//! Used where a fast algorithm does not apply: fixed and precision formatting, formatting in
//! radixes other than 10, and the slow path of decimal parsing. All results are exact
//! (correctly rounded); division by `radix^k` is done limb-sized step by step, which yields
//! the exact floor plus an inexact flag.

use super::big::Big;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Round {
    /// Round half to even (IEEE default, used when parsing and for shortest digits).
    HalfEven,
    /// Round half away from zero (Number.prototype.toFixed / toExponential / toPrecision).
    HalfUp,
}

/// Splits `radix` into `2^s * r` with `r` odd.
fn split_radix(radix: u32) -> (u32, i32) {
    let s = radix.trailing_zeros();
    (radix >> s, s as i32)
}

/// An upper bound for the bit length of `r^n` (`r` odd, `r >= 3`).
///
/// With `r^c < 2^64`, `log2(r) < 64 / c`, so `bits(r^n) <= floor(n * log2(r)) + 1 <= n * 64 / c
/// + 1`. The overestimate is at most 7% (r = 5), which the big integer capacity allows for.
fn pow_bits_upper(r: u32, n: u32) -> u32 {
    let mut c = 1u32;
    let mut p = r as u64;
    while let Some(next) = p.checked_mul(r as u64) {
        p = next;
        c += 1;
    }
    (n * 64).div_ceil(c) + 1
}

/// `round(x * 2^e2 * radix^k)` as an integer.
pub(crate) fn scale_round(mut x: Big, e2: i32, radix: u32, k: i32, mode: Round) -> Big {
    let (r, s) = split_radix(radix);
    // Compute floor(2 * value) and whether it was inexact; the low bit is then the half bit.
    let t = e2 + s * k + 1;
    let mut sticky = false;
    if k >= 0 {
        x.mul_pow(r, k as u32);
        if t >= 0 {
            x.shl(t as u32);
        } else {
            sticky = x.shr_sticky((-t) as u32);
        }
    } else {
        if t >= 0 {
            x.shl(t as u32);
        } else {
            sticky = x.shr_sticky((-t) as u32);
        }
        sticky |= x.div_pow(r, (-k) as u32);
    }
    let half = x.bit(0);
    x.shr_sticky(1);
    let up = match mode {
        Round::HalfUp => half,
        Round::HalfEven => half && (sticky || x.bit(0)),
    };
    if up {
        x.add_small(1);
    }
    x
}

/// `x * 2^e2 * radix^k`, correctly rounded (half to even) to an `f64`. If `sticky` is set, the
/// true value is infinitesimally larger than that (used for truncated decimal input).
///
/// Callers bound the magnitude (see `big::LIMBS`).
pub(crate) fn scale_to_f64(mut x: Big, e2: i32, radix: u32, k: i32, mut sticky: bool) -> f64 {
    if x.is_zero() {
        return 0.0;
    }
    let (r, s) = split_radix(radix);
    let mut bin = e2 + s * k;
    if k >= 0 {
        x.mul_pow(r, k as u32);
    } else if r != 1 {
        // Pre-shift so the quotient keeps at least 56 significant bits.
        let n = (-k) as u32;
        let g = (56 + pow_bits_upper(r, n)).saturating_sub(x.bit_len());
        x.shl(g);
        bin -= g as i32;
        sticky |= x.div_pow(r, n);
    }
    round_to_f64(x, bin, sticky)
}

/// Rounds `(x + sticky * epsilon) * 2^bin` to the nearest double, ties to even.
pub(crate) fn round_to_f64(mut x: Big, mut bin: i32, mut sticky: bool) -> f64 {
    let bits = x.bit_len() as i32;
    if bits == 0 {
        return 0.0;
    }
    let lead = bits - 1 + bin;
    if lead > 1023 {
        return f64::INFINITY;
    }
    if lead < -1075 {
        return 0.0;
    }
    // Precision available at this magnitude: 53 bits for normal numbers, fewer below 2^-1022.
    let prec = if lead >= -1022 {
        53
    } else {
        53 - (-1022 - lead)
    };
    let mut drop = bits - prec;
    if drop < 1 {
        // Make room for a rounding bit so the code below is uniform.
        x.shl((1 - drop) as u32);
        bin -= 1 - drop;
        drop = 1;
    }
    let half = x.bit((drop - 1) as u32);
    sticky |= x.any_below((drop - 1) as u32);
    x.shr_sticky(drop as u32);
    let mut mant = x.low_u64();
    if half && (sticky || mant & 1 == 1) {
        mant += 1;
    }
    compose(mant, bin + drop)
}

/// The double `mant * 2^e`, where the caller guarantees it is representable (or overflows):
/// `mant <= 2^53`, and `e == -1074` whenever `mant < 2^52`, except for exact small values
/// which are normalized here.
fn compose(mut mant: u64, mut e: i32) -> f64 {
    if mant == 0 {
        return 0.0;
    }
    if mant == 1 << 53 {
        mant >>= 1;
        e += 1;
    }
    while mant < 1 << 52 && e > -1074 {
        mant <<= 1;
        e -= 1;
    }
    if mant >= 1 << 52 {
        let biased = e + 52 + 1023;
        if biased >= 2047 {
            return f64::INFINITY;
        }
        f64::from_bits((biased as u64) << 52 | (mant & ((1 << 52) - 1)))
    } else {
        debug_assert_eq!(e, -1074);
        f64::from_bits(mant)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounding_modes() {
        // 2.5 = 5 * 2^-1
        let r = |m: u64, e2: i32, mode| scale_round(Big::from_u64(m), e2, 10, 0, mode).low_u64();
        assert_eq!(r(5, -1, Round::HalfUp), 3);
        assert_eq!(r(5, -1, Round::HalfEven), 2);
        assert_eq!(r(7, -1, Round::HalfEven), 4);
        assert_eq!(r(9, -2, Round::HalfEven), 2); // 2.25
        assert_eq!(r(11, -2, Round::HalfUp), 3); // 2.75
        // 0.125 * 10^2 = 12.5
        let b = scale_round(Big::from_u64(1), -3, 10, 2, Round::HalfUp);
        assert_eq!(b.low_u64(), 13);
        let b = scale_round(Big::from_u64(1), -3, 10, 2, Round::HalfEven);
        assert_eq!(b.low_u64(), 12);
        // 1234 / 10^2 = 12.34
        let b = scale_round(Big::from_u64(1234), 0, 10, -2, Round::HalfUp);
        assert_eq!(b.low_u64(), 12);
        let b = scale_round(Big::from_u64(1250), 0, 10, -2, Round::HalfEven);
        assert_eq!(b.low_u64(), 12);
        let b = scale_round(Big::from_u64(1251), 0, 10, -2, Round::HalfEven);
        assert_eq!(b.low_u64(), 13);
        // radix 6 = 2 * 3: 7 * 6^-1 = 1.1666 -> 1
        let b = scale_round(Big::from_u64(7), 0, 6, -1, Round::HalfUp);
        assert_eq!(b.low_u64(), 1);
    }

    #[test]
    fn to_f64_basics() {
        let f = |m: u64, e2: i32, radix: u32, k: i32| {
            scale_to_f64(Big::from_u64(m), e2, radix, k, false)
        };
        assert_eq!(f(1, 0, 10, 0), 1.0);
        assert_eq!(f(1, 0, 10, -1), 0.1);
        assert_eq!(f(3, 0, 10, -1), 0.3);
        assert_eq!(f(17976931348623157, 0, 10, 292), f64::MAX);
        assert_eq!(f(17976931348623159, 0, 10, 292), f64::INFINITY);
        assert_eq!(f(5, 0, 10, -324), 5e-324);
        assert_eq!(f(2, 0, 10, -324), 0.0);
        assert_eq!(f(3, 0, 10, -324), 5e-324);
        assert_eq!(f(22250738585072014, 0, 10, -324), 2.2250738585072014e-308);
        assert_eq!(f(1, -1074, 2, 0), 5e-324);
        assert_eq!(f(1, -1075, 2, 0), 0.0); // exactly half of the smallest subnormal: ties to 0
        assert_eq!(f(3, -1076, 2, 0), 5e-324); // 0.75 ulp
        assert_eq!(scale_to_f64(Big::from_u64(1), -1075, 2, 0, true), 5e-324);
        assert_eq!(f(1, 1024, 2, 0), f64::INFINITY);
        assert_eq!(f((1 << 53) - 1, 971, 2, 0), f64::MAX);
        assert_eq!(f(1, 0, 3, -2), 1.0 / 9.0);
        assert_eq!(f(1, 0, 36, 2), 1296.0);
        // 2^53 + 1 is a tie: rounds to even (2^53)
        assert_eq!(f((1 << 53) + 1, 0, 10, 0), 9007199254740992.0);
        assert_eq!(f((1 << 53) + 3, 0, 10, 0), 9007199254740996.0);
        assert_eq!(
            scale_to_f64(Big::from_u64((1 << 53) + 1), 0, 10, 0, true),
            9007199254740994.0
        );
    }

    #[test]
    fn pow_bits_bound() {
        for r in (3..=35).step_by(2) {
            for n in [1u32, 2, 10, 100, 600] {
                let mut b = Big::from_u64(1);
                b.mul_pow(r, n);
                assert!(b.bit_len() <= pow_bits_upper(r, n), "{r}^{n}");
            }
        }
    }
}
