use core::cmp::Ordering;

use crate::limbs::{self, Limb};
use crate::{BigInt, BigUint};

struct Format {
    mantissa_bits: u32,
    exponent_max: i64,
}

const F64_FORMAT: Format = Format {
    mantissa_bits: f64::MANTISSA_DIGITS,
    exponent_max: f64::MAX_EXP as i64 - 1,
};

const F32_FORMAT: Format = Format {
    mantissa_bits: f32::MANTISSA_DIGITS,
    exponent_max: f32::MAX_EXP as i64 - 1,
};

const F64_EXPONENT_BIAS: i64 = f64::MAX_EXP as i64 - 1;
const F64_STORED_MANTISSA_BITS: u32 = f64::MANTISSA_DIGITS - 1;
const F64_SUBNORMAL_EXPONENT: i64 = f64::MIN_EXP as i64 - f64::MANTISSA_DIGITS as i64;
const F64_EXPONENT_MASK: u64 = (1 << (u64::BITS - 1 - F64_STORED_MANTISSA_BITS)) - 1;

enum Rounded {
    Finite {
        mantissa: Limb,
        exponent: i64,
        lost_below: bool,
    },
    Overflow,
}

fn round_natural(value: &[Limb], format: &Format) -> Rounded {
    let bits = limbs::bits(value);
    let mantissa_bits = u64::from(format.mantissa_bits);
    if bits <= mantissa_bits {
        return Rounded::Finite {
            mantissa: value.first().copied().unwrap_or(0),
            exponent: 0,
            lost_below: false,
        };
    }
    let shift = bits - mantissa_bits;
    let top = limbs::shr(value, shift);
    let mut mantissa = top[0];
    let round_bit = bit(value, shift - 1);
    let sticky = limbs::trailing_zeros(value).is_some_and(|zeros| zeros < shift - 1);
    let round_up = round_bit && (sticky || mantissa & 1 == 1);
    let mut exponent = i64::try_from(shift).expect("the exponent fits");
    if round_up {
        mantissa += 1;
        if mantissa == 1 << format.mantissa_bits {
            mantissa >>= 1;
            exponent += 1;
        }
    }
    if exponent > format.exponent_max + 1 - i64::from(format.mantissa_bits) {
        return Rounded::Overflow;
    }
    Rounded::Finite {
        mantissa,
        exponent,
        lost_below: (round_bit || sticky) && !round_up,
    }
}

fn bit(value: &[Limb], index: u64) -> bool {
    let limb = (index / u64::from(Limb::BITS)) as usize;
    value
        .get(limb)
        .is_some_and(|&word| (word >> (index % u64::from(Limb::BITS))) & 1 == 1)
}

fn is_largest_finite(mantissa: Limb, exponent: i64, format: &Format) -> bool {
    mantissa == (1 << format.mantissa_bits) - 1
        && exponent == format.exponent_max + 1 - i64::from(format.mantissa_bits)
}

pub(crate) fn natural_to_f64(value: &[Limb], negative: bool) -> Option<f64> {
    let magnitude = match round_natural(value, &F64_FORMAT) {
        Rounded::Overflow => f64::INFINITY,
        Rounded::Finite {
            mantissa,
            exponent,
            lost_below,
        } => {
            if lost_below && is_largest_finite(mantissa, exponent, &F64_FORMAT) {
                return None;
            }
            mantissa as f64 * power_of_two_f64(exponent)
        }
    };
    Some(if negative { -magnitude } else { magnitude })
}

pub(crate) fn natural_to_f32(value: &[Limb], negative: bool) -> Option<f32> {
    let magnitude = match round_natural(value, &F32_FORMAT) {
        Rounded::Overflow => f32::INFINITY,
        Rounded::Finite {
            mantissa,
            exponent,
            lost_below,
        } => {
            if lost_below && is_largest_finite(mantissa, exponent, &F32_FORMAT) {
                return None;
            }
            let power = power_of_two_f64(exponent) as f32;
            mantissa as f32 * power
        }
    };
    Some(if negative { -magnitude } else { magnitude })
}

fn power_of_two_f64(exponent: i64) -> f64 {
    if exponent >= 1 - F64_EXPONENT_BIAS {
        assert!(exponent <= F64_EXPONENT_BIAS);
        let biased = u64::try_from(exponent + F64_EXPONENT_BIAS).expect("the biased exponent is positive");
        f64::from_bits(biased << F64_STORED_MANTISSA_BITS)
    } else {
        assert!(exponent >= F64_SUBNORMAL_EXPONENT);
        let position = u32::try_from(exponent - F64_SUBNORMAL_EXPONENT).expect("the position is small");
        f64::from_bits(1 << position)
    }
}

fn decompose_f64(value: f64) -> (bool, Limb, i64) {
    let bits = value.to_bits();
    let negative = bits >> (u64::BITS - 1) == 1;
    let biased = ((bits >> F64_STORED_MANTISSA_BITS) & F64_EXPONENT_MASK) as i64;
    let fraction = bits & ((1 << F64_STORED_MANTISSA_BITS) - 1);
    if biased == 0 {
        (negative, fraction, F64_SUBNORMAL_EXPONENT)
    } else {
        (
            negative,
            fraction | (1 << F64_STORED_MANTISSA_BITS),
            biased - F64_EXPONENT_BIAS - i64::from(F64_STORED_MANTISSA_BITS),
        )
    }
}

pub(crate) fn f64_to_natural(value: f64) -> Option<(bool, BigUint)> {
    if !value.is_finite() {
        return None;
    }
    let (negative, mantissa, exponent) = decompose_f64(value);
    let magnitude = BigUint::from(mantissa);
    let magnitude = if exponent >= 0 {
        magnitude.shl_bits(exponent as u64)
    } else {
        magnitude.shr_bits(exponent.unsigned_abs())
    };
    let negative = negative && !magnitude.is_zero();
    Some((negative, magnitude))
}

pub fn f64_to_ratio(value: f64) -> Option<(BigUint, BigUint)> {
    if !value.is_finite() {
        return None;
    }
    let (_, mantissa, exponent) = decompose_f64(value);
    if mantissa == 0 {
        return Some((BigUint::ZERO, BigUint::ONE));
    }
    let zeros = mantissa.trailing_zeros();
    let mantissa = mantissa >> zeros;
    let exponent = exponent + i64::from(zeros);
    if exponent >= 0 {
        Some((BigUint::from(mantissa).shl_bits(exponent as u64), BigUint::ONE))
    } else {
        Some((BigUint::from(mantissa), BigUint::ONE.shl_bits(exponent.unsigned_abs())))
    }
}

pub fn rational_to_f64(numerator: &BigInt, denominator: &BigInt) -> (f64, Ordering) {
    assert!(!denominator.magnitude().is_zero(), "division by zero");
    if numerator.magnitude().is_zero() {
        return (0.0, Ordering::Equal);
    }
    let negative = numerator.is_negative_value() != denominator.is_negative_value();
    let (magnitude, ordering) = ratio_magnitude_to_f64(numerator.magnitude(), denominator.magnitude());
    if negative {
        (-magnitude, ordering.reverse())
    } else {
        (magnitude, ordering)
    }
}

fn ratio_magnitude_to_f64(numerator: &BigUint, denominator: &BigUint) -> (f64, Ordering) {
    let estimate = numerator.bits() as i64 - denominator.bits() as i64;
    let at_least = if estimate >= 0 {
        *numerator >= denominator.shl_bits(estimate as u64)
    } else {
        numerator.shl_bits(estimate.unsigned_abs()) >= *denominator
    };
    let exponent = if at_least { estimate } else { estimate - 1 };
    if exponent > F64_EXPONENT_BIAS {
        return (f64::INFINITY, Ordering::Greater);
    }
    let unit = (exponent - i64::from(F64_STORED_MANTISSA_BITS)).max(F64_SUBNORMAL_EXPONENT);
    let (scaled_numerator, scaled_denominator) = if unit <= 0 {
        (numerator.shl_bits(unit.unsigned_abs()), denominator.clone())
    } else {
        (numerator.clone(), denominator.shl_bits(unit as u64))
    };
    let (quotient, remainder) = scaled_numerator.div_rem_value(&scaled_denominator);
    let mut mantissa = u64::try_from(&quotient).expect("the quotient fits in a mantissa");
    if remainder.is_zero() {
        return (mantissa as f64 * power_of_two_f64(unit), Ordering::Equal);
    }
    let largest_mantissa: Limb = (1 << f64::MANTISSA_DIGITS) - 1;
    if exponent == F64_EXPONENT_BIAS && mantissa == largest_mantissa {
        return (f64::MAX, Ordering::Less);
    }
    let twice_remainder = remainder.shl_bits(1);
    let round_up = match twice_remainder.cmp(&scaled_denominator) {
        Ordering::Greater => true,
        Ordering::Equal => mantissa & 1 == 1,
        Ordering::Less => false,
    };
    if round_up {
        mantissa += 1;
    }
    let ordering = if round_up { Ordering::Greater } else { Ordering::Less };
    (mantissa as f64 * power_of_two_f64(unit), ordering)
}

