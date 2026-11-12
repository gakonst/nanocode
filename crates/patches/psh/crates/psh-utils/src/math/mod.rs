mod bessel;
mod error_function;
mod exponential;
mod exponential_table;
mod gamma;
mod hyperbolic;
mod inverse_trigonometric;
mod logarithm;
mod power;
mod reduction;
mod rounding;
mod trigonometric;

pub use bessel::{j0, j1, jn, y0, y1, yn};
pub use error_function::{erf, erfc};
pub use exponential::{exp, exp2, exp10, expm1};
pub use gamma::{lgamma, lgamma_r, tgamma};
pub use hyperbolic::{acosh, asinh, atanh, cosh, sinh, tanh};
pub use inverse_trigonometric::{acos, asin, atan, atan2};
pub use logarithm::{log, log1p, log2, log10};
pub use power::{cbrt, hypot, pow};
pub use rounding::{
    fdim, fmax, fmin, fmod, frexp, ilogb, ldexp, modf, nextafter, remainder, rint, round, scalbn,
};
pub use trigonometric::{cos, sin, tan};

fn difference(minuend: f64, subtrahend: f64) -> f64 {
    minuend - subtrahend
}

fn quotient(dividend: f64, divisor: f64) -> f64 {
    dividend / divisor
}

const WORD_BITS: u32 = 32;
const SIGNIFICAND_BITS: u32 = 52;
const HIGH_SIGNIFICAND_BITS: u32 = SIGNIFICAND_BITS - WORD_BITS;
const EXPONENT_BIAS: i32 = 1023;
const EXPONENT_FIELD_MAX: u32 = 0x7ff;
const SIGN_MASK: u64 = 1 << 63;
const MAGNITUDE_MASK: u64 = !SIGN_MASK;
const SIGNIFICAND_MASK: u64 = (1 << SIGNIFICAND_BITS) - 1;
const HIGH_MAGNITUDE_MASK: u32 = (MAGNITUDE_MASK >> WORD_BITS) as u32;
const HIGH_SIGNIFICAND_MASK: u32 = (SIGNIFICAND_MASK >> WORD_BITS) as u32;
const HIGH_EXPONENT_MASK: u32 = EXPONENT_FIELD_MAX << HIGH_SIGNIFICAND_BITS;
const NON_FINITE_HIGH_WORD: u32 = HIGH_EXPONENT_MASK;
const ONE_HIGH_WORD: u32 = power_of_two_high_word(0);
const SMALLEST_NORMAL_HIGH_WORD: u32 = 1 << HIGH_SIGNIFICAND_BITS;

fn high_word(x: f64) -> u32 {
    (x.to_bits() >> WORD_BITS) as u32
}

fn low_word(x: f64) -> u32 {
    x.to_bits() as u32
}

fn from_words(high: u32, low: u32) -> f64 {
    f64::from_bits(u64::from(high) << WORD_BITS | u64::from(low))
}

fn with_high_word(x: f64, high: u32) -> f64 {
    from_words(high, low_word(x))
}

fn with_low_word(x: f64, low: u32) -> f64 {
    from_words(high_word(x), low)
}

fn biased_exponent(x: f64) -> u32 {
    (x.to_bits() >> SIGNIFICAND_BITS) as u32 & EXPONENT_FIELD_MAX
}

const fn power_of_two_high_word(exponent: i32) -> u32 {
    assert!(exponent > -EXPONENT_BIAS);
    assert!(exponent <= EXPONENT_BIAS);
    ((EXPONENT_BIAS + exponent) as u32) << HIGH_SIGNIFICAND_BITS
}

const fn power_of_two(exponent: i32) -> f64 {
    f64::from_bits((power_of_two_high_word(exponent) as u64) << WORD_BITS)
}
