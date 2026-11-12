//! Pure Rust implementations of libm functions using the libm crate.
//! Used on targets without native libm (e.g., WASM).

// Trigonometric functions

#[inline(always)]
pub fn acos(n: f64) -> f64 {
    libm::acos(n)
}

#[inline(always)]
pub fn asin(n: f64) -> f64 {
    libm::asin(n)
}

#[inline(always)]
pub fn atan(n: f64) -> f64 {
    libm::atan(n)
}

#[inline(always)]
pub fn atan2(y: f64, x: f64) -> f64 {
    libm::atan2(y, x)
}

#[inline(always)]
pub fn cos(n: f64) -> f64 {
    libm::cos(n)
}

#[inline(always)]
pub fn sin(n: f64) -> f64 {
    libm::sin(n)
}

#[inline(always)]
pub fn tan(n: f64) -> f64 {
    libm::tan(n)
}

// Hyperbolic functions

#[inline(always)]
pub fn acosh(n: f64) -> f64 {
    libm::acosh(n)
}

#[inline(always)]
pub fn asinh(n: f64) -> f64 {
    libm::asinh(n)
}

#[inline(always)]
pub fn atanh(n: f64) -> f64 {
    libm::atanh(n)
}

#[inline(always)]
pub fn cosh(n: f64) -> f64 {
    libm::cosh(n)
}

#[inline(always)]
pub fn sinh(n: f64) -> f64 {
    libm::sinh(n)
}

#[inline(always)]
pub fn tanh(n: f64) -> f64 {
    libm::tanh(n)
}

// Exponential and logarithmic functions

#[inline(always)]
pub fn exp(n: f64) -> f64 {
    libm::exp(n)
}

#[inline(always)]
pub fn exp2(n: f64) -> f64 {
    libm::exp2(n)
}

#[inline(always)]
pub fn expm1(n: f64) -> f64 {
    libm::expm1(n)
}

#[inline(always)]
pub fn log(n: f64) -> f64 {
    libm::log(n)
}

#[inline(always)]
pub fn log10(n: f64) -> f64 {
    libm::log10(n)
}

#[inline(always)]
pub fn log1p(n: f64) -> f64 {
    libm::log1p(n)
}

#[inline(always)]
pub fn log2(n: f64) -> f64 {
    libm::log2(n)
}

// Power functions

#[inline(always)]
pub fn cbrt(n: f64) -> f64 {
    libm::cbrt(n)
}

#[inline(always)]
pub fn hypot(x: f64, y: f64) -> f64 {
    libm::hypot(x, y)
}

#[inline(always)]
pub fn pow(x: f64, y: f64) -> f64 {
    libm::pow(x, y)
}

#[inline(always)]
pub fn sqrt(n: f64) -> f64 {
    libm::sqrt(n)
}

// Floating-point manipulation functions

#[inline(always)]
pub fn ceil(n: f64) -> f64 {
    libm::ceil(n)
}

#[inline(always)]
pub fn copysign(x: f64, y: f64) -> f64 {
    libm::copysign(x, y)
}

#[inline(always)]
pub fn fabs(n: f64) -> f64 {
    libm::fabs(n)
}

#[inline(always)]
pub fn floor(n: f64) -> f64 {
    libm::floor(n)
}

#[inline(always)]
pub fn fmod(x: f64, y: f64) -> f64 {
    libm::fmod(x, y)
}

#[inline(always)]
pub fn frexp(n: f64, exp: &mut i32) -> f64 {
    let (mantissa, exponent) = libm::frexp(n);
    *exp = exponent;
    mantissa
}

#[inline(always)]
pub fn ldexp(x: f64, n: i32) -> f64 {
    libm::ldexp(x, n)
}

#[inline(always)]
pub fn modf(n: f64, iptr: &mut f64) -> f64 {
    let (frac, int) = libm::modf(n);
    *iptr = int;
    frac
}

#[inline(always)]
pub fn nextafter(x: f64, y: f64) -> f64 {
    libm::nextafter(x, y)
}

#[inline(always)]
pub fn remainder(x: f64, y: f64) -> f64 {
    libm::remainder(x, y)
}

#[inline(always)]
pub fn trunc(n: f64) -> f64 {
    libm::trunc(n)
}

// Special functions

#[inline(always)]
pub fn erf(n: f64) -> f64 {
    libm::erf(n)
}

#[inline(always)]
pub fn erfc(n: f64) -> f64 {
    libm::erfc(n)
}

// Platform-specific sincos (fallback: call sin and cos separately)

#[cfg(feature = "complex")]
#[inline(always)]
pub fn sincos(x: f64) -> (f64, f64) {
    (libm::sin(x), libm::cos(x))
}
