#![allow(clippy::approx_constant)] // many false positives

macro_rules! force_eval {
    ($e:expr) => {
        unsafe { ::core::ptr::read_volatile(&$e) }
    };
}

#[cfg(not(debug_assertions))]
macro_rules! i {
    ($array:expr, $index:expr) => {
        unsafe { *$array.get_unchecked($index) }
    };
    ($array:expr, $index:expr, = , $rhs:expr) => {
        unsafe {
            *$array.get_unchecked_mut($index) = $rhs;
        }
    };
    ($array:expr, $index:expr, += , $rhs:expr) => {
        unsafe {
            *$array.get_unchecked_mut($index) += $rhs;
        }
    };
    ($array:expr, $index:expr, -= , $rhs:expr) => {
        unsafe {
            *$array.get_unchecked_mut($index) -= $rhs;
        }
    };
    ($array:expr, $index:expr, &= , $rhs:expr) => {
        unsafe {
            *$array.get_unchecked_mut($index) &= $rhs;
        }
    };
    ($array:expr, $index:expr, == , $rhs:expr) => {
        unsafe { *$array.get_unchecked_mut($index) == $rhs }
    };
}

#[cfg(debug_assertions)]
macro_rules! i {
    ($array:expr, $index:expr) => {
        *$array.get($index).unwrap()
    };
    ($array:expr, $index:expr, = , $rhs:expr) => {
        *$array.get_mut($index).unwrap() = $rhs;
    };
    ($array:expr, $index:expr, -= , $rhs:expr) => {
        *$array.get_mut($index).unwrap() -= $rhs;
    };
    ($array:expr, $index:expr, += , $rhs:expr) => {
        *$array.get_mut($index).unwrap() += $rhs;
    };
    ($array:expr, $index:expr, &= , $rhs:expr) => {
        *$array.get_mut($index).unwrap() &= $rhs;
    };
    ($array:expr, $index:expr, == , $rhs:expr) => {
        *$array.get_mut($index).unwrap() == $rhs
    };
}

// Temporary macro to avoid panic codegen for division (in debug mode too). At
// the time of this writing this is only used in a few places, and once
// rust-lang/rust#72751 is fixed then this macro will no longer be necessary and
// the native `/` operator can be used and panics won't be codegen'd.
#[cfg(any(debug_assertions, not(intrinsics_enabled)))]
macro_rules! div {
    ($a:expr, $b:expr) => {
        $a / $b
    };
}

#[cfg(all(not(debug_assertions), intrinsics_enabled))]
macro_rules! div {
    ($a:expr, $b:expr) => {
        unsafe { core::intrinsics::unchecked_div($a, $b) }
    };
}

#[macro_use]
pub(crate) mod support;

mod generic;

// Private modules
mod arch;
mod expo2;
mod k_cos;
mod k_expo2;
mod k_sin;
mod k_tan;
mod rem_pio2;
mod rem_pio2_large;

// Private re-imports
use self::expo2::expo2;
use self::k_cos::k_cos;
use self::k_expo2::k_expo2;
use self::k_sin::k_sin;
use self::k_tan::k_tan;
use self::rem_pio2::rem_pio2;
use self::rem_pio2_large::rem_pio2_large;
#[allow(unused_imports)]
use self::support::{CastFrom, CastInto, DInt, Float, HInt, Int, IntTy, MinInt};

// Public modules
mod acos;
mod acosh;
mod asin;
mod asinh;
mod atan;
mod atan2;
mod atanh;
mod cbrt;
mod ceil;
mod cos;
mod cosh;
mod exp;
mod expm1;
mod fabs;
mod floor;
mod fma;
mod fmod;
mod hypot;
mod ldexp;
mod log;
mod log10;
mod log1p;
mod log2;
mod pow;
mod scalbn;
mod sin;
mod sinh;
mod sqrt;
mod tan;
mod tanh;
mod trunc;

// Use separated imports instead of {}-grouped imports for easier merging.
pub use self::acos::acos;
pub use self::acosh::acosh;
pub use self::asin::asin;
pub use self::asinh::asinh;
pub use self::atan::atan;
pub use self::atan2::atan2;
pub use self::atanh::atanh;
pub use self::cbrt::cbrt;
pub use self::ceil::ceil;
pub use self::cos::cos;
pub use self::cosh::cosh;
pub use self::exp::exp;
pub use self::expm1::expm1;
pub use self::fabs::fabs;
pub use self::floor::floor;
pub use self::fma::fma;
pub use self::fmod::fmod;
pub use self::hypot::hypot;
pub use self::ldexp::ldexp;
pub use self::log::log;
pub use self::log1p::log1p;
pub use self::log2::log2;
pub use self::log10::log10;
pub use self::pow::pow;
pub use self::scalbn::scalbn;
pub use self::sin::sin;
pub use self::sinh::sinh;
pub use self::sqrt::sqrt;
pub use self::tan::tan;
pub use self::tanh::tanh;
pub use self::trunc::trunc;

#[inline]
fn get_high_word(x: f64) -> u32 {
    (x.to_bits() >> 32) as u32
}

#[inline]
fn get_low_word(x: f64) -> u32 {
    x.to_bits() as u32
}

#[inline]
fn with_set_low_word(f: f64, lo: u32) -> f64 {
    let mut tmp = f.to_bits();
    tmp &= 0xffffffff_00000000;
    tmp |= lo as u64;
    f64::from_bits(tmp)
}

#[inline]
fn combine_words(hi: u32, lo: u32) -> f64 {
    f64::from_bits(((hi as u64) << 32) | lo as u64)
}
