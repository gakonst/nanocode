use super::{
    EXPONENT_BIAS, EXPONENT_FIELD_MAX, MAGNITUDE_MASK, SIGN_MASK, SIGNIFICAND_BITS,
    SIGNIFICAND_MASK, biased_exponent, power_of_two,
};

const NORMAL_EXPONENT_MAX: i32 = EXPONENT_BIAS;
const NORMAL_EXPONENT_MIN: i32 = 1 - EXPONENT_BIAS;
const PRECISION_BITS: i32 = SIGNIFICAND_BITS as i32 + 1;
const SUBNORMAL_SCALE_EXPONENT: i32 = 64;
const ROUND_HALF_BELOW: f64 = 0.5 - 0.25 * f64::EPSILON;
const ILOGB_OF_ZERO_OR_NAN: i32 = i32::MIN;
const ILOGB_OF_INFINITY: i32 = i32::MAX;
const REMAINDER_ZERO_EXPONENT: i32 = -60;

fn unbiased_exponent(x: f64) -> i32 {
    biased_exponent(x) as i32 - EXPONENT_BIAS
}

pub fn trunc(x: f64) -> f64 {
    let exponent = unbiased_exponent(x);
    if exponent >= SIGNIFICAND_BITS as i32 {
        return x;
    }
    let keep = if exponent < 0 {
        SIGN_MASK
    } else {
        !(SIGNIFICAND_MASK >> exponent)
    };
    f64::from_bits(x.to_bits() & keep)
}

pub fn round(x: f64) -> f64 {
    trunc(x + ROUND_HALF_BELOW.copysign(x))
}

pub fn rint(x: f64) -> f64 {
    let to_integer = 1.0 / f64::EPSILON;
    if biased_exponent(x) as i32 >= EXPONENT_BIAS + SIGNIFICAND_BITS as i32 {
        return x;
    }
    let positive = x.is_sign_positive();
    let rounded = if positive {
        x + to_integer - to_integer
    } else {
        x - to_integer + to_integer
    };
    match (rounded == 0.0, positive) {
        (true, true) => 0.0,
        (true, false) => -0.0,
        (false, _) => rounded,
    }
}

pub fn scalbn(x: f64, n: i32) -> f64 {
    let mut x = x;
    let mut n = n;
    let up = power_of_two(NORMAL_EXPONENT_MAX);
    let down_exponent = NORMAL_EXPONENT_MIN + PRECISION_BITS;
    let down = power_of_two(down_exponent);
    if n > NORMAL_EXPONENT_MAX {
        x *= up;
        n -= NORMAL_EXPONENT_MAX;
        if n > NORMAL_EXPONENT_MAX {
            x *= up;
            n -= NORMAL_EXPONENT_MAX;
            n = n.min(NORMAL_EXPONENT_MAX);
        }
    } else if n < NORMAL_EXPONENT_MIN {
        x *= down;
        n -= down_exponent;
        if n < NORMAL_EXPONENT_MIN {
            x *= down;
            n -= down_exponent;
            n = n.max(NORMAL_EXPONENT_MIN);
        }
    }
    x * power_of_two(n)
}

pub fn ldexp(x: f64, n: i32) -> f64 {
    scalbn(x, n)
}

pub fn frexp(x: f64) -> (f64, i32) {
    let biased = biased_exponent(x);
    if biased == 0 {
        if x == 0.0 {
            return (x, 0);
        }
        let (fraction, exponent) = frexp(x * power_of_two(SUBNORMAL_SCALE_EXPONENT));
        return (fraction, exponent - SUBNORMAL_SCALE_EXPONENT);
    }
    if biased == EXPONENT_FIELD_MAX {
        return (x, 0);
    }
    let half_exponent = power_of_two(-1).to_bits();
    let fraction = x.to_bits() & (SIGN_MASK | SIGNIFICAND_MASK) | half_exponent;
    (
        f64::from_bits(fraction),
        biased as i32 - (EXPONENT_BIAS - 1),
    )
}

pub fn modf(x: f64) -> (f64, f64) {
    let bits = x.to_bits();
    let exponent = unbiased_exponent(x);
    let signed_zero = f64::from_bits(bits & SIGN_MASK);
    if exponent >= SIGNIFICAND_BITS as i32 {
        if x.is_nan() {
            return (x, x);
        }
        return (signed_zero, x);
    }
    if exponent < 0 {
        return (x, signed_zero);
    }
    let fraction_mask = SIGNIFICAND_MASK >> exponent;
    if bits & fraction_mask == 0 {
        return (signed_zero, x);
    }
    let integral = f64::from_bits(bits & !fraction_mask);
    (x - integral, integral)
}

pub fn ilogb(x: f64) -> i32 {
    let biased = biased_exponent(x);
    if biased == 0 {
        let significand = x.to_bits() << (u64::BITS - SIGNIFICAND_BITS);
        if significand == 0 {
            return ILOGB_OF_ZERO_OR_NAN;
        }
        return -EXPONENT_BIAS - significand.leading_zeros() as i32;
    }
    if biased == EXPONENT_FIELD_MAX {
        if x.is_nan() {
            return ILOGB_OF_ZERO_OR_NAN;
        }
        return ILOGB_OF_INFINITY;
    }
    biased as i32 - EXPONENT_BIAS
}

pub fn nextafter(x: f64, y: f64) -> f64 {
    if x.is_nan() {
        return x + x;
    }
    if y.is_nan() {
        return y + y;
    }
    let x_bits = x.to_bits();
    let y_bits = y.to_bits();
    if x_bits == y_bits {
        return y;
    }
    let x_magnitude = x_bits & MAGNITUDE_MASK;
    let y_magnitude = y_bits & MAGNITUDE_MASK;
    let next = if x_magnitude == 0 {
        if y_magnitude == 0 {
            return y;
        }
        y_bits & SIGN_MASK | 1
    } else if x_magnitude > y_magnitude || (x_bits ^ y_bits) & SIGN_MASK != 0 {
        x_bits - 1
    } else {
        x_bits + 1
    };
    f64::from_bits(next)
}

pub fn fmod(x: f64, y: f64) -> f64 {
    let exponent_mask = u64::from(EXPONENT_FIELD_MAX) << SIGNIFICAND_BITS;
    let x_magnitude = x.to_bits() & MAGNITUDE_MASK;
    let y_magnitude = y.to_bits() & MAGNITUDE_MASK;
    let x_non_finite = x_magnitude & exponent_mask == exponent_mask;
    let y_nan_or_zero = y_magnitude.wrapping_sub(1) & exponent_mask == exponent_mask;
    if x_non_finite || y_nan_or_zero {
        return super::quotient(x * y, x * y);
    }
    x % y
}

pub fn fmin(x: f64, y: f64) -> f64 {
    let chosen = if y.is_nan() || x < y { x } else { y };
    chosen * 1.0
}

pub fn fmax(x: f64, y: f64) -> f64 {
    let chosen = if x.is_nan() || x < y { y } else { x };
    chosen * 1.0
}

pub fn fdim(x: f64, y: f64) -> f64 {
    if x <= y { 0.0 } else { x - y }
}

struct Normalized {
    significand: u64,
    exponent: i32,
}

fn normalize(bits: u64, biased: i32) -> Normalized {
    if biased != 0 {
        return Normalized {
            significand: bits & SIGNIFICAND_MASK | 1 << SIGNIFICAND_BITS,
            exponent: biased,
        };
    }
    let shifted = bits << (u64::BITS - SIGNIFICAND_BITS);
    let exponent = -(shifted.leading_zeros() as i32);
    Normalized {
        significand: bits << (1 - exponent),
        exponent,
    }
}

pub fn remainder(x: f64, y: f64) -> f64 {
    let x_bits = x.to_bits();
    let y_bits = y.to_bits();
    let x_negative = x_bits & SIGN_MASK != 0;
    let y_negative = y_bits & SIGN_MASK != 0;
    if y_bits << 1 == 0 || y.is_nan() || biased_exponent(x) == EXPONENT_FIELD_MAX {
        return super::quotient(x * y, x * y);
    }
    if x_bits << 1 == 0 {
        return x;
    }
    let Normalized {
        significand: mut x_significand,
        exponent: mut x_exponent,
    } = normalize(x_bits, biased_exponent(x) as i32);
    let Normalized {
        significand: y_significand,
        exponent: y_exponent,
    } = normalize(y_bits, biased_exponent(y) as i32);
    let mut quotient_odd = false;
    if x_exponent + 1 != y_exponent {
        if x_exponent < y_exponent {
            return x;
        }
        while x_exponent > y_exponent {
            let difference = x_significand.wrapping_sub(y_significand);
            if difference >> 63 == 0 {
                x_significand = difference;
            }
            x_significand <<= 1;
            x_exponent -= 1;
        }
        let difference = x_significand.wrapping_sub(y_significand);
        quotient_odd = difference >> 63 == 0;
        if quotient_odd {
            x_significand = difference;
        }
        if x_significand == 0 {
            x_exponent = REMAINDER_ZERO_EXPONENT;
        } else {
            let shift = x_significand.leading_zeros() - (u64::BITS - 1 - SIGNIFICAND_BITS);
            x_significand <<= shift;
            x_exponent -= shift as i32;
        }
    }
    let x_bits = if x_exponent > 0 {
        (x_significand - (1 << SIGNIFICAND_BITS)) | (x_exponent as u64) << SIGNIFICAND_BITS
    } else {
        x_significand >> (1 - x_exponent)
    };
    let mut result = f64::from_bits(x_bits);
    let y_magnitude = if y_negative { -y } else { y };
    let twice = 2.0 * result;
    let closer_above = x_exponent + 1 == y_exponent
        && (twice > y_magnitude || (twice == y_magnitude && quotient_odd));
    if x_exponent == y_exponent || closer_above {
        result -= y_magnitude;
    }
    if x_negative { -result } else { result }
}
