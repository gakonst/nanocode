//! Math leaf intrinsics that are not libm functions (0082): `Math.f16round` and the
//! exact summation of `Math.sumPrecise`.

/// Rounds to the nearest IEEE binary16 value (ties to even), directly from f64 (going
/// through f32 would round twice).
pub(crate) fn f16_round(x: f64) -> f64 {
    if x.is_nan() || x.is_infinite() || x == 0.0 {
        return x;
    }
    let a = x.abs();
    // 65520 is halfway between the largest finite binary16 (65504) and 2^16; the tie
    // goes to the even neighbour, 2^16, which overflows.
    if a >= 65520.0 {
        return f64::INFINITY.copysign(x);
    }
    let e = ((a.to_bits() >> 52) & 0x7ff) as i32 - 1023;
    // The spacing of binary16 values around a: 2^(e-10) for normals, 2^-24 below 2^-14.
    let quantum = pow2(if e < -14 { -24 } else { e - 10 });
    // a / quantum < 2^12: exact, and its integer part fits easily.
    let v = a / quantum;
    let t = v as u64;
    let frac = v - t as f64;
    let n = if frac > 0.5 || (frac == 0.5 && t & 1 == 1) {
        t + 1
    } else {
        t
    };
    (n as f64 * quantum).copysign(x)
}

/// 2^e for -1022 <= e <= 1023.
fn pow2(e: i32) -> f64 {
    f64::from_bits(((e + 1023) as u64) << 52)
}

/// Limbs of 32 bits of the fixed-point accumulator: the finite doubles are multiples of
/// 2^-1074 below 2^1024, so 2098 bits plus carry room for 2^53 terms.
const LIMBS: usize = 72;

/// The sum of `values` rounded once (Math.sumPrecise, proposal-math-sum): -0 when there
/// are no values or only -0s, NaN for a NaN or both infinities.
pub(crate) fn sum_precise(values: impl Iterator<Item = f64>) -> f64 {
    let mut acc = [0i64; LIMBS];
    let (mut pos_inf, mut neg_inf, mut nan, mut all_neg_zero) = (false, false, false, true);
    let mut pending = 0u32;
    for x in values {
        if x.is_nan() {
            nan = true;
            continue;
        }
        if x.is_infinite() {
            if x > 0.0 {
                pos_inf = true;
            } else {
                neg_inf = true;
            }
            continue;
        }
        if !(x == 0.0 && x.is_sign_negative()) {
            all_neg_zero = false;
        }
        if x == 0.0 {
            continue;
        }
        let bits = x.to_bits();
        let biased = ((bits >> 52) & 0x7ff) as u32;
        let mut mant = bits & ((1 << 52) - 1);
        if biased != 0 {
            mant |= 1 << 52;
        }
        // x = mant * 2^(shift - 1074).
        let shift = biased.max(1) - 1;
        let (limb, off) = ((shift / 32) as usize, shift % 32);
        let wide = (mant as u128) << off;
        let neg = x < 0.0;
        for k in 0..3 {
            let part = ((wide >> (32 * k)) & 0xffff_ffff) as i64;
            if part != 0 {
                acc[limb + k] += if neg { -part } else { part };
            }
        }
        pending += 1;
        if pending == 1 << 28 {
            normalize(&mut acc);
            pending = 0;
        }
    }
    if nan || (pos_inf && neg_inf) {
        return f64::NAN;
    }
    if pos_inf {
        return f64::INFINITY;
    }
    if neg_inf {
        return f64::NEG_INFINITY;
    }
    if all_neg_zero {
        return -0.0;
    }
    normalize(&mut acc);
    let negative = acc[LIMBS - 1] < 0;
    if negative {
        // Two's complement: the magnitude.
        let mut carry = 1i64;
        for l in acc.iter_mut() {
            let v = (!*l & 0xffff_ffff) + carry;
            *l = v & 0xffff_ffff;
            carry = v >> 32;
        }
    }
    let m = magnitude_to_f64(&acc);
    if negative { -m } else { m }
}

/// Propagates the carries: every limb ends in 0..2^32 but the top one, which keeps the
/// sign.
fn normalize(acc: &mut [i64; LIMBS]) {
    for i in 0..LIMBS - 1 {
        let carry = acc[i] >> 32;
        acc[i] &= 0xffff_ffff;
        acc[i + 1] += carry;
    }
}

/// The unsigned fixed-point number in units of 2^-1074, rounded to the nearest double
/// (ties to even).
fn magnitude_to_f64(acc: &[i64; LIMBS]) -> f64 {
    let bit = |p: usize| (acc[p / 32] >> (p % 32)) & 1 == 1;
    let Some(top) = (0..LIMBS).rev().find(|&i| acc[i] != 0) else {
        return 0.0;
    };
    let p = top * 32 + (63 - (acc[top] as u64).leading_zeros() as usize);
    if p <= 52 {
        // Fewer than 54 significant bits: exact, a subnormal or a small normal.
        let mut n = 0u64;
        for i in (0..=p).rev() {
            n = (n << 1) | bit(i) as u64;
        }
        return n as f64 * pow2(-537) * pow2(-537);
    }
    let mut mant = 0u64;
    for i in (p - 52..=p).rev() {
        mant = (mant << 1) | bit(i) as u64;
    }
    let round = bit(p - 53);
    let sticky = (0..p - 53).any(bit);
    let mut p = p;
    if round && (sticky || mant & 1 == 1) {
        mant += 1;
        if mant == 1 << 53 {
            mant >>= 1;
            p += 1;
        }
    }
    // value = mant * 2^(p - 52 - 1074); the biased exponent of mant's leading bit.
    let biased = p as u64 - 51;
    if biased >= 2047 {
        return f64::INFINITY;
    }
    f64::from_bits((biased << 52) | (mant & ((1 << 52) - 1)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn f16() {
        let cases = [
            (1.0, 1.0),
            (65504.0, 65504.0),
            (65519.99, 65504.0),
            (65520.0, f64::INFINITY),
            (-65520.0, f64::NEG_INFINITY),
            (5.960464477539063e-8, 5.960464477539063e-8), // 2^-24
            (2.9802322387695312e-8, 0.0),                 // 2^-25: tie to even (0)
            (2.980232238769532e-8, 5.960464477539063e-8),
            (1.00048828125, 1.0),           // 1 + 2^-11: tie, to even
            (1.00146484375, 1.001953125),   // 1 + 3*2^-11: tie, to even
            (1.000732421875, 1.0009765625), // 1 + 3*2^-12: up
            (0.1, 0.0999755859375),
            (-0.0, -0.0),
        ];
        for (x, want) in cases {
            let got = f16_round(x);
            assert!(
                got == want && got.is_sign_negative() == want.is_sign_negative(),
                "{x}: {got} != {want}"
            );
        }
    }

    #[test]
    fn sums() {
        let s = |v: &[f64]| sum_precise(v.iter().copied());
        assert!(s(&[]) == 0.0 && s(&[]).is_sign_negative());
        assert!(s(&[-0.0, -0.0]).is_sign_negative());
        assert!(!s(&[-0.0, 0.0]).is_sign_negative());
        assert_eq!(s(&[1e20, 0.1, -1e20]), 0.1);
        assert_eq!(s(&[0.1, 0.2, 0.3]), 0.6);
        assert_eq!(s(&[1.0, 2.0]), 3.0);
        assert_eq!(s(&[-1.0, -2.0]), -3.0);
        assert_eq!(s(&[1.0, -1.0]), 0.0);
        assert!(!s(&[1.0, -1.0]).is_sign_negative());
        // Intermediate overflow.
        assert_eq!(s(&[f64::MAX, f64::MAX, -f64::MAX]), f64::MAX);
        assert_eq!(s(&[f64::MAX, f64::MAX]), f64::INFINITY);
        assert_eq!(s(&[-f64::MAX, -f64::MAX]), f64::NEG_INFINITY);
        assert_eq!(s(&[5e-324, 5e-324]), 1e-323);
        assert_eq!(s(&[f64::MIN_POSITIVE, -5e-324]), f64::MIN_POSITIVE - 5e-324);
        assert!(s(&[f64::INFINITY, f64::NEG_INFINITY]).is_nan());
        assert_eq!(s(&[f64::INFINITY, 1.0]), f64::INFINITY);
        // Ties to even at the last bit: 2^53 + 1 is a tie between 2^53 and 2^53 + 2.
        assert_eq!(s(&[9007199254740992.0, 1.0]), 9007199254740992.0);
        assert_eq!(s(&[9007199254740994.0, 1.0]), 9007199254740996.0);
        assert_eq!(s(&[9007199254740992.0, 1.0, 1e-300]), 9007199254740994.0);
    }
}
