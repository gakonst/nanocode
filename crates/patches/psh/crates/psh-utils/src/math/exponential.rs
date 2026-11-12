use std::f64::consts::{LOG2_10, LOG2_E};

use super::exponential_table::{EXP2_POINTS, EXP2_VALUE_AND_ERROR_BITS};
use super::power::pow;
use super::rounding::{modf, scalbn};
use super::{
    EXPONENT_BIAS, HIGH_MAGNITUDE_MASK, NON_FINITE_HIGH_WORD, SIGNIFICAND_BITS, WORD_BITS,
    biased_exponent, high_word, power_of_two, power_of_two_high_word,
};

const LN2_HIGH: f64 = 0.6931471803691238;
const LN2_LOW: f64 = 1.9082149292705877e-10;
const EXP: [f64; 5] = [
    0.16666666666666602,
    -0.0027777777777015593,
    6.613756321437934e-05,
    -1.6533902205465252e-06,
    4.1381367970572385e-08,
];
const EXP_LARGE_HIGH_WORD: u32 = 0x4086_232b;
const EXP_OVERFLOW_LIMIT: f64 = 709.782712893384;
const EXP_UNDERFLOW_LIMIT: f64 = -745.1332191019411;
const HALF_LN2_HIGH_WORD: u32 = 0x3fd6_2e42;
const THREE_HALVES_LN2_HIGH_WORD: u32 = 0x3ff0_a2b2;
const EXP_TINY_HIGH_WORD: u32 = power_of_two_high_word(-28);
const OVERFLOW_SCALE: f64 = power_of_two(1023);
const EXP2_ROUNDING_BITS: u64 = 0x4338_0000_0000_0000;
const EXP2: [u64; 5] = [
    0x3fe6_2e42_fefa_39ef,
    0x3fce_bfbd_ff82_c575,
    0x3fac_6b08_d704_a0a6,
    0x3f83_b2ab_88f7_0400,
    0x3f55_d880_0387_5c74,
];
const EXP2_LARGE_HIGH_WORD: u32 = 0x408f_f000;
const EXP2_OVERFLOW_HIGH_WORD: u32 = power_of_two_high_word(10);
const EXP2_UNDERFLOW_LIMIT: f64 = -1075.0;
const EXP2_TINY_HIGH_WORD: u32 = power_of_two_high_word(-54);
const EXP10_TABLE_OFFSET: isize = 15;
const EXP10_TABLE_EXPONENT_LIMIT: u32 = 4;
const POWERS_OF_TEN: [f64; 31] = [
    1e-15,
    1e-14,
    1e-13,
    1e-12,
    1e-11,
    1e-10,
    1e-09,
    1e-08,
    1e-07,
    1e-06,
    1e-05,
    0.0001,
    0.001,
    0.01,
    0.1,
    1.0,
    10.0,
    100.0,
    1000.0,
    10000.0,
    100000.0,
    1000000.0,
    10000000.0,
    100000000.0,
    1000000000.0,
    10000000000.0,
    100000000000.0,
    1000000000000.0,
    10000000000000.0,
    100000000000000.0,
    1000000000000000.0,
];
const EXPM1: [f64; 5] = [
    -0.03333333333333313,
    0.0015873015872548146,
    -7.93650757867488e-05,
    4.008217827329362e-06,
    -2.0109921818362437e-07,
];
const EXPM1_LARGE_HIGH_WORD: u32 = 0x4043_687a;
const EXPM1_TINY_HIGH_WORD: u32 = power_of_two_high_word(-54);
const EXPM1_EXACT_SCALE_MAX: i32 = 56;
const EXPM1_SPLIT_SCALE: i32 = 20;
const EXPM1_OVERFLOW_SCALE: i32 = 1024;
const SCALED_EXP_OFFSET_BITS: u64 = 0x4096_2066_151a_dd8b;
const SCALED_EXP_HALF_SCALE: f64 = power_of_two(1021);

fn reduce_ln2(x: f64, magnitude: u32, negative: bool) -> (f64, f64, i32) {
    let k = if magnitude >= THREE_HALVES_LN2_HIGH_WORD {
        (LOG2_E * x + if negative { -0.5 } else { 0.5 }) as i32
    } else if negative {
        -1
    } else {
        1
    };
    (x - f64::from(k) * LN2_HIGH, f64::from(k) * LN2_LOW, k)
}

pub fn exp(x: f64) -> f64 {
    let high = high_word(x);
    let negative = high >> 31 != 0;
    let magnitude = high & HIGH_MAGNITUDE_MASK;
    if magnitude >= EXP_LARGE_HIGH_WORD {
        if x.is_nan() {
            return x;
        }
        if x > EXP_OVERFLOW_LIMIT {
            return x * OVERFLOW_SCALE;
        }
        if x < EXP_UNDERFLOW_LIMIT {
            return 0.0;
        }
    }
    let (x, high_part, low_part, k) = if magnitude > HALF_LN2_HIGH_WORD {
        let (high_part, low_part, k) = reduce_ln2(x, magnitude, negative);
        (high_part - low_part, high_part, low_part, k)
    } else if magnitude > EXP_TINY_HIGH_WORD {
        (x, x, 0.0, 0)
    } else {
        return 1.0 + x;
    };
    let xx = x * x;
    let p = &EXP;
    let c = x - xx * (p[0] + xx * (p[1] + xx * (p[2] + xx * (p[3] + xx * p[4]))));
    let y = 1.0 + (x * c / (2.0 - c) - low_part + high_part);
    if k == 0 { y } else { scalbn(y, k) }
}

pub fn exp2(x: f64) -> f64 {
    let points = EXP2_POINTS as u32;
    let redux = f64::from_bits(EXP2_ROUNDING_BITS) / f64::from(points);
    let bits = x.to_bits();
    let negative = bits >> 63 != 0;
    let magnitude = (bits >> WORD_BITS) as u32 & HIGH_MAGNITUDE_MASK;
    if magnitude >= EXP2_LARGE_HIGH_WORD {
        if magnitude >= EXP2_OVERFLOW_HIGH_WORD && !negative {
            return x * OVERFLOW_SCALE;
        }
        if magnitude >= NON_FINITE_HIGH_WORD {
            return -1.0 / x;
        }
        if negative && x <= EXP2_UNDERFLOW_LIMIT {
            return 0.0;
        }
    } else if magnitude < EXP2_TINY_HIGH_WORD {
        return 1.0 + x;
    }
    let rounded = (x + redux).to_bits();
    let index = (rounded as u32).wrapping_add(points / 2);
    let scale = (index / points * points) as i32 / points as i32;
    let index = (index % points) as usize;
    let mut z = x - (f64::from_bits(rounded) - redux);
    let (value_bits, error_bits) = EXP2_VALUE_AND_ERROR_BITS[index];
    let t = f64::from_bits(value_bits);
    z -= f64::from_bits(error_bits);
    let p = EXP2.map(f64::from_bits);
    let r = t + t * z * (p[0] + z * (p[1] + z * (p[2] + z * (p[3] + z * p[4]))));
    scalbn(r, scale)
}

pub fn exp10(x: f64) -> f64 {
    let (fraction, integral) = modf(x);
    if biased_exponent(integral) < EXPONENT_BIAS as u32 + EXP10_TABLE_EXPONENT_LIMIT {
        let power = POWERS_OF_TEN[(integral as isize + EXP10_TABLE_OFFSET) as usize];
        if fraction == 0.0 {
            return power;
        }
        return exp2(LOG2_10 * fraction) * power;
    }
    pow(10.0, x)
}

fn expm1_scale(x: f64, e: f64, k: i32) -> f64 {
    let scale = f64::from_bits(((EXPONENT_BIAS + k) as u64) << SIGNIFICAND_BITS);
    if !(0..=EXPM1_EXACT_SCALE_MAX).contains(&k) {
        let y = x - e + 1.0;
        let y = if k == EXPM1_OVERFLOW_SCALE {
            y * 2.0 * OVERFLOW_SCALE
        } else {
            y * scale
        };
        return y - 1.0;
    }
    let inverse = f64::from_bits(((EXPONENT_BIAS - k) as u64) << SIGNIFICAND_BITS);
    if k < EXPM1_SPLIT_SCALE {
        (x - e + (1.0 - inverse)) * scale
    } else {
        (x - (e + inverse) + 1.0) * scale
    }
}

pub fn expm1(x: f64) -> f64 {
    let bits = x.to_bits();
    let magnitude = (bits >> WORD_BITS) as u32 & HIGH_MAGNITUDE_MASK;
    let negative = bits >> 63 != 0;
    if magnitude >= EXPM1_LARGE_HIGH_WORD {
        if x.is_nan() {
            return x;
        }
        if negative {
            return -1.0;
        }
        if x > EXP_OVERFLOW_LIMIT {
            return x * OVERFLOW_SCALE;
        }
    }
    let (x, c, k) = if magnitude > HALF_LN2_HIGH_WORD {
        let (high_part, low_part, k) = reduce_ln2(x, magnitude, negative);
        let x = high_part - low_part;
        (x, (high_part - x) - low_part, k)
    } else if magnitude < EXPM1_TINY_HIGH_WORD {
        return x;
    } else {
        (x, 0.0, 0)
    };
    let half_x = 0.5 * x;
    let half_x_squared = x * half_x;
    let hxs = half_x_squared;
    let q = &EXPM1;
    let r1 = 1.0 + hxs * (q[0] + hxs * (q[1] + hxs * (q[2] + hxs * (q[3] + hxs * q[4]))));
    let t = 3.0 - r1 * half_x;
    let e = hxs * ((r1 - t) / (6.0 - x * t));
    if k == 0 {
        return x - (x * e - hxs);
    }
    let e = x * (e - c) - c;
    let e = e - hxs;
    if k == -1 {
        return 0.5 * (x - e) - 0.5;
    }
    if k == 1 {
        if x < -0.25 {
            return -2.0 * (e - (x + 0.5));
        }
        return 1.0 + 2.0 * (x - e);
    }
    expm1_scale(x, e, k)
}

pub(super) fn exp_scaled(x: f64) -> f64 {
    exp(x - f64::from_bits(SCALED_EXP_OFFSET_BITS)) * SCALED_EXP_HALF_SCALE * SCALED_EXP_HALF_SCALE
}
