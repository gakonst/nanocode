#![allow(
    clippy::assign_op_pattern,
    clippy::eq_op,
    clippy::excessive_precision,
    clippy::float_cmp,
    clippy::int_plus_one,
    clippy::just_underscores_and_digits,
    clippy::many_single_char_names,
    clippy::mixed_case_hex_literals,
    clippy::needless_late_init,
    clippy::needless_return,
    clippy::unreadable_literal,
    clippy::zero_divided_by_zero
)]

mod acos;
mod acosh;
mod asin;
mod asinh;
mod atan;
mod atan2;
mod atanh;
mod cbrt;
mod ceil;
mod copysign;
mod cos;
mod cosh;
mod erf;
mod exp;
mod exp2;
mod expm1;
mod expo2;
mod fabs;
mod floor;
mod fmod;
mod frexp;
mod hypot;
mod k_cos;
mod k_expo2;
mod k_sin;
mod k_tan;
mod ldexp;
mod log;
mod log10;
mod log1p;
mod log2;
mod modf;
mod nextafter;
mod pow;
mod rem_pio2;
mod rem_pio2_large;
mod remainder;
mod remquo;
mod scalbn;
mod sin;
mod sinh;
mod sqrt;
mod tan;
mod tanh;
mod trunc;

pub use acos::acos;
pub use acosh::acosh;
pub use asin::asin;
pub use asinh::asinh;
pub use atan::atan;
pub use atan2::atan2;
pub use atanh::atanh;
pub use cbrt::cbrt;
pub use ceil::ceil;
pub use copysign::copysign;
pub use cos::cos;
pub use cosh::cosh;
pub use erf::{erf, erfc};
pub use exp::exp;
pub use exp2::exp2;
pub use expm1::expm1;
pub use fabs::fabs;
pub use floor::floor;
pub use fmod::fmod;
pub use frexp::frexp;
pub use hypot::hypot;
pub use ldexp::ldexp;
pub use log::log;
pub use log1p::log1p;
pub use log2::log2;
pub use log10::log10;
pub use modf::modf;
pub use nextafter::nextafter;
pub use pow::pow;
pub use remainder::remainder;
pub use sin::sin;
pub use sinh::sinh;
pub use sqrt::sqrt;
pub use tan::tan;
pub use tanh::tanh;
pub use trunc::trunc;

use expo2::expo2;
use k_cos::k_cos;
use k_expo2::k_expo2;
use k_sin::k_sin;
use k_tan::k_tan;
use rem_pio2::rem_pio2;
use rem_pio2_large::rem_pio2_large;
use remquo::remquo;
use scalbn::scalbn;

const SIGNIFICAND_BITS: u32 = f64::MANTISSA_DIGITS - 1;
const EXPONENT_BITS: u32 = u64::BITS - 1 - SIGNIFICAND_BITS;
const EXPONENT_SATURATED: u32 = (1 << EXPONENT_BITS) - 1;
const EXPONENT_BIAS: u32 = EXPONENT_SATURATED >> 1;
const EXPONENT_MAX: i32 = f64::MAX_EXP - 1;
const EXPONENT_MIN: i32 = f64::MIN_EXP - 1;
const SIGN_MASK: u64 = 1 << (u64::BITS - 1);
const IMPLICIT_BIT: u64 = 1 << SIGNIFICAND_BITS;
const SIGNIFICAND_MASK: u64 = IMPLICIT_BIT - 1;
const EXPONENT_MASK: u64 = !(SIGN_MASK | SIGNIFICAND_MASK);

const fn exponent_unbiased(x: f64) -> i32 {
    ((x.to_bits() >> SIGNIFICAND_BITS) as u32 & EXPONENT_SATURATED) as i32 - EXPONENT_BIAS as i32
}

const fn from_exponent(exponent: u32) -> f64 {
    f64::from_bits(((exponent & EXPONENT_SATURATED) as u64) << SIGNIFICAND_BITS)
}

#[inline]
fn combine_words(hi: u32, lo: u32) -> f64 {
    f64::from_bits(((hi as u64) << 32) | lo as u64)
}

#[inline]
fn get_high_word(x: f64) -> u32 {
    (x.to_bits() >> 32) as u32
}

#[inline]
fn get_low_word(x: f64) -> u32 {
    x.to_bits() as u32
}

#[inline]
fn with_set_high_word(f: f64, hi: u32) -> f64 {
    let mut tmp = f.to_bits();
    tmp &= 0x00000000_ffffffff;
    tmp |= (hi as u64) << 32;
    f64::from_bits(tmp)
}

#[inline]
fn with_set_low_word(f: f64, lo: u32) -> f64 {
    let mut tmp = f.to_bits();
    tmp &= 0xffffffff_00000000;
    tmp |= lo as u64;
    f64::from_bits(tmp)
}
