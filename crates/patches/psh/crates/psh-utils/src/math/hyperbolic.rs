use std::f64::consts::LN_2;

use super::exponential::{exp, exp_scaled, expm1};
use super::logarithm::{log, log1p};
use super::{
    EXPONENT_BIAS, MAGNITUDE_MASK, ONE_HIGH_WORD, SIGN_MASK, WORD_BITS, biased_exponent,
    power_of_two_high_word,
};

const LOG_MAX_HIGH_WORD: u32 = 0x4086_2e42;
const LN2_HIGH_WORD: u32 = 0x3fe6_2e42;
const HYPERBOLIC_TINY_HIGH_WORD: u32 = power_of_two_high_word(-26);
const TANH_LOG3_HALF_HIGH_WORD: u32 = 0x3fe1_93ea;
const TANH_SATURATED_HIGH_WORD: u32 = 0x4034_0000;
const TANH_LOG5_3_HALF_HIGH_WORD: u32 = 0x3fd0_58ae;
const SMALLEST_NORMAL_HIGH_WORD: u32 = super::SMALLEST_NORMAL_HIGH_WORD;
const INVERSE_LARGE_EXPONENT: u32 = 26;
const ARTANH_TINY_EXPONENT: u32 = 32;

pub fn sinh(x: f64) -> f64 {
    let bits = x.to_bits();
    let h = if bits & SIGN_MASK != 0 { -0.5 } else { 0.5 };
    let magnitude_bits = bits & MAGNITUDE_MASK;
    let absolute = f64::from_bits(magnitude_bits);
    let w = (magnitude_bits >> WORD_BITS) as u32;
    if w < LOG_MAX_HIGH_WORD {
        let t = expm1(absolute);
        if w < ONE_HIGH_WORD {
            if w < HYPERBOLIC_TINY_HIGH_WORD {
                return x;
            }
            return h * (2.0 * t - t * t / (t + 1.0));
        }
        return h * (t + t / (t + 1.0));
    }
    2.0 * h * exp_scaled(absolute)
}

pub fn cosh(x: f64) -> f64 {
    let magnitude_bits = x.to_bits() & MAGNITUDE_MASK;
    let x = f64::from_bits(magnitude_bits);
    let w = (magnitude_bits >> WORD_BITS) as u32;
    if w < LN2_HIGH_WORD {
        if w < HYPERBOLIC_TINY_HIGH_WORD {
            return 1.0;
        }
        let t = expm1(x);
        return 1.0 + t * t / (2.0 * (1.0 + t));
    }
    if w < LOG_MAX_HIGH_WORD {
        let t = exp(x);
        return 0.5 * (t + 1.0 / t);
    }
    exp_scaled(x)
}

pub fn tanh(x: f64) -> f64 {
    let bits = x.to_bits();
    let negative = bits & SIGN_MASK != 0;
    let magnitude_bits = bits & MAGNITUDE_MASK;
    let x = f64::from_bits(magnitude_bits);
    let w = (magnitude_bits >> WORD_BITS) as u32;
    let t = if w > TANH_LOG3_HALF_HIGH_WORD {
        if w > TANH_SATURATED_HIGH_WORD {
            1.0 - 0.0 / x
        } else {
            let t = expm1(2.0 * x);
            1.0 - 2.0 / (t + 2.0)
        }
    } else if w > TANH_LOG5_3_HALF_HIGH_WORD {
        let t = expm1(2.0 * x);
        t / (t + 2.0)
    } else if w >= SMALLEST_NORMAL_HIGH_WORD {
        let t = expm1(-2.0 * x);
        -t / (t + 2.0)
    } else {
        x
    };
    if negative { -t } else { t }
}

pub fn asinh(x: f64) -> f64 {
    let bits = x.to_bits();
    let exponent = biased_exponent(x);
    let negative = bits & SIGN_MASK != 0;
    let x = f64::from_bits(bits & MAGNITUDE_MASK);
    let bias = EXPONENT_BIAS as u32;
    let result = if exponent >= bias + INVERSE_LARGE_EXPONENT {
        log(x) + LN_2
    } else if exponent > bias {
        log(2.0 * x + 1.0 / ((x * x + 1.0).sqrt() + x))
    } else if exponent >= bias - INVERSE_LARGE_EXPONENT {
        log1p(x + x * x / ((x * x + 1.0).sqrt() + 1.0))
    } else {
        x
    };
    if negative { -result } else { result }
}

pub fn acosh(x: f64) -> f64 {
    let exponent = biased_exponent(x);
    let bias = EXPONENT_BIAS as u32;
    if exponent <= bias {
        return log1p(x - 1.0 + ((x - 1.0) * (x - 1.0) + 2.0 * (x - 1.0)).sqrt());
    }
    if exponent < bias + INVERSE_LARGE_EXPONENT {
        return log(2.0 * x - 1.0 / (x + (x * x - 1.0).sqrt()));
    }
    log(x) + LN_2
}

pub fn atanh(x: f64) -> f64 {
    let bits = x.to_bits();
    let exponent = biased_exponent(x);
    let negative = bits & SIGN_MASK != 0;
    let y = f64::from_bits(bits & MAGNITUDE_MASK);
    let bias = EXPONENT_BIAS as u32;
    let result = if exponent < bias - 1 {
        if exponent < bias - ARTANH_TINY_EXPONENT {
            y
        } else {
            0.5 * log1p(2.0 * y + 2.0 * y * y / (1.0 - y))
        }
    } else {
        0.5 * log1p(2.0 * (y / (1.0 - y)))
    };
    if negative { -result } else { result }
}
