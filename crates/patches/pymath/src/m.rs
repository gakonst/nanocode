//! Safe wrappers for system libm functions.

use crate::m_sys;

// Trigonometric functions

#[inline(always)]
pub fn acos(n: f64) -> f64 {
    unsafe { m_sys::acos(n) }
}

#[inline(always)]
pub fn asin(n: f64) -> f64 {
    unsafe { m_sys::asin(n) }
}

#[inline(always)]
pub fn atan(n: f64) -> f64 {
    unsafe { m_sys::atan(n) }
}

#[inline(always)]
pub fn atan2(y: f64, x: f64) -> f64 {
    unsafe { m_sys::atan2(y, x) }
}

#[inline(always)]
pub fn cos(n: f64) -> f64 {
    unsafe { m_sys::cos(n) }
}

#[inline(always)]
pub fn sin(n: f64) -> f64 {
    unsafe { m_sys::sin(n) }
}

#[inline(always)]
pub fn tan(n: f64) -> f64 {
    unsafe { m_sys::tan(n) }
}

// Hyperbolic functions

#[inline(always)]
pub fn acosh(n: f64) -> f64 {
    unsafe { m_sys::acosh(n) }
}

#[inline(always)]
pub fn asinh(n: f64) -> f64 {
    unsafe { m_sys::asinh(n) }
}

#[inline(always)]
pub fn atanh(n: f64) -> f64 {
    unsafe { m_sys::atanh(n) }
}

#[inline(always)]
pub fn cosh(n: f64) -> f64 {
    unsafe { m_sys::cosh(n) }
}

#[inline(always)]
pub fn sinh(n: f64) -> f64 {
    unsafe { m_sys::sinh(n) }
}

#[inline(always)]
pub fn tanh(n: f64) -> f64 {
    unsafe { m_sys::tanh(n) }
}

// Exponential and logarithmic functions

#[inline(always)]
pub fn exp(n: f64) -> f64 {
    unsafe { m_sys::exp(n) }
}

#[inline(always)]
pub fn exp2(n: f64) -> f64 {
    unsafe { m_sys::exp2(n) }
}

#[inline(always)]
pub fn expm1(n: f64) -> f64 {
    unsafe { m_sys::expm1(n) }
}

#[inline(always)]
pub fn log(n: f64) -> f64 {
    unsafe { m_sys::log(n) }
}

#[inline(always)]
pub fn log10(n: f64) -> f64 {
    unsafe { m_sys::log10(n) }
}

#[inline(always)]
pub fn log1p(n: f64) -> f64 {
    unsafe { m_sys::log1p(n) }
}

#[inline(always)]
pub fn log2(n: f64) -> f64 {
    unsafe { m_sys::log2(n) }
}

// Power functions

#[inline(always)]
pub fn cbrt(n: f64) -> f64 {
    unsafe { m_sys::cbrt(n) }
}

#[inline(always)]
pub fn hypot(x: f64, y: f64) -> f64 {
    unsafe { m_sys::hypot(x, y) }
}

#[inline(always)]
pub fn pow(x: f64, y: f64) -> f64 {
    unsafe { m_sys::pow(x, y) }
}

#[inline(always)]
pub fn sqrt(n: f64) -> f64 {
    unsafe { m_sys::sqrt(n) }
}

// Floating-point manipulation functions

#[inline(always)]
pub fn ceil(n: f64) -> f64 {
    unsafe { m_sys::ceil(n) }
}

#[inline(always)]
pub fn copysign(x: f64, y: f64) -> f64 {
    unsafe { m_sys::copysign(x, y) }
}

#[inline(always)]
pub fn fabs(n: f64) -> f64 {
    unsafe { m_sys::fabs(n) }
}

// #[inline(always)]
// pub fn fdim(a: f64, b: f64) -> f64 {
//     unsafe { m_sys::fdim(a, b) }
// }

#[inline(always)]
pub fn floor(n: f64) -> f64 {
    unsafe { m_sys::floor(n) }
}

#[inline(always)]
pub fn fmod(x: f64, y: f64) -> f64 {
    unsafe { m_sys::fmod(x, y) }
}

#[inline(always)]
pub fn frexp(n: f64, exp: &mut i32) -> f64 {
    unsafe { m_sys::frexp(n, exp) }
}

#[inline(always)]
pub fn ldexp(x: f64, n: i32) -> f64 {
    unsafe { m_sys::ldexp(x, n) }
}

#[inline(always)]
pub fn modf(n: f64, iptr: &mut f64) -> f64 {
    unsafe { m_sys::modf(n, iptr) }
}

#[inline(always)]
pub fn nextafter(x: f64, y: f64) -> f64 {
    unsafe { m_sys::nextafter(x, y) }
}

#[inline(always)]
pub fn remainder(x: f64, y: f64) -> f64 {
    unsafe { m_sys::remainder(x, y) }
}

#[inline(always)]
pub fn trunc(n: f64) -> f64 {
    unsafe { m_sys::trunc(n) }
}

// Special functions

#[inline(always)]
pub fn erf(n: f64) -> f64 {
    unsafe { m_sys::erf(n) }
}

#[inline(always)]
pub fn erfc(n: f64) -> f64 {
    unsafe { m_sys::erfc(n) }
}

// #[inline(always)]
// pub fn lgamma_r(n: f64, s: &mut i32) -> f64 {
//     unsafe { m_sys::lgamma_r(n, s) }
// }

// #[inline(always)]
// pub fn tgamma(n: f64) -> f64 {
//     unsafe { m_sys::tgamma(n) }
// }

// Platform-specific sincos

/// Result type for sincos function (matches Apple's __double2)
#[cfg(all(feature = "complex", target_os = "macos"))]
#[repr(C)]
struct SinCosResult {
    sin: f64,
    cos: f64,
}

#[cfg(all(feature = "complex", target_os = "macos"))]
unsafe extern "C" {
    #[link_name = "__sincos_stret"]
    fn sincos_stret(x: f64) -> SinCosResult;
}

/// Compute sin and cos together using Apple's optimized sincos.
/// This matches Python's cmath behavior on macOS.
#[cfg(all(feature = "complex", target_os = "macos"))]
#[inline(always)]
pub fn sincos(x: f64) -> (f64, f64) {
    let sc = unsafe { sincos_stret(x) };
    (sc.sin, sc.cos)
}

/// Fallback for non-macOS: call sin and cos separately
#[cfg(all(feature = "complex", not(target_os = "macos")))]
#[inline(always)]
pub fn sincos(x: f64) -> (f64, f64) {
    (sin(x), cos(x))
}
