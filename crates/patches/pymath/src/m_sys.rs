// These symbols are all defined by `libm`,
// or by `compiler-builtins` on unsupported platforms.
#[cfg_attr(unix, link(name = "m"))]
#[allow(dead_code)]
unsafe extern "C" {
    // Trigonometric functions
    pub fn acos(n: f64) -> f64;
    pub fn asin(n: f64) -> f64;
    pub fn atan(n: f64) -> f64;
    pub fn atan2(a: f64, b: f64) -> f64;
    pub fn cos(n: f64) -> f64;
    pub fn sin(n: f64) -> f64;
    pub fn tan(n: f64) -> f64;

    // Hyperbolic functions
    pub fn acosh(n: f64) -> f64;
    pub fn asinh(n: f64) -> f64;
    pub fn atanh(n: f64) -> f64;
    pub fn cosh(n: f64) -> f64;
    pub fn sinh(n: f64) -> f64;
    pub fn tanh(n: f64) -> f64;

    // Exponential and logarithmic functions
    pub fn exp(n: f64) -> f64;
    pub fn exp2(n: f64) -> f64;
    pub fn expm1(n: f64) -> f64;
    pub fn expm1f(n: f32) -> f32;
    pub fn log(n: f64) -> f64;
    pub fn log10(n: f64) -> f64;
    pub fn log1p(n: f64) -> f64;
    pub fn log1pf(n: f32) -> f32;
    pub fn log2(n: f64) -> f64;

    // Power functions
    pub fn cbrt(n: f64) -> f64;
    pub fn cbrtf(n: f32) -> f32;
    #[cfg_attr(target_env = "msvc", link_name = "_hypot")]
    pub fn hypot(x: f64, y: f64) -> f64;
    #[cfg_attr(target_env = "msvc", link_name = "_hypotf")]
    pub fn hypotf(x: f32, y: f32) -> f32;
    pub fn pow(x: f64, y: f64) -> f64;
    pub fn sqrt(n: f64) -> f64;

    // Floating-point manipulation functions
    pub fn ceil(n: f64) -> f64;
    pub fn copysign(x: f64, y: f64) -> f64;
    pub fn fabs(n: f64) -> f64;
    // pub fn fdim(a: f64, b: f64) -> f64;
    pub fn fdimf(a: f32, b: f32) -> f32;
    pub fn floor(n: f64) -> f64;
    pub fn fmod(x: f64, y: f64) -> f64;
    pub fn frexp(n: f64, exp: *mut i32) -> f64;
    pub fn ldexp(x: f64, n: i32) -> f64;
    pub fn modf(n: f64, iptr: *mut f64) -> f64;
    pub fn nextafter(x: f64, y: f64) -> f64;
    pub fn remainder(x: f64, y: f64) -> f64;
    pub fn trunc(n: f64) -> f64;

    // Special functions
    pub fn erf(n: f64) -> f64;
    pub fn erfc(n: f64) -> f64;
    pub fn erff(n: f32) -> f32;
    pub fn erfcf(n: f32) -> f32;
    // pub fn lgamma_r(n: f64, s: &mut i32) -> f64;
    #[cfg(not(target_os = "aix"))]
    pub fn lgammaf_r(n: f32, s: &mut i32) -> f32;
    // pub fn tgamma(n: f64) -> f64;
    pub fn tgammaf(n: f32) -> f32;

    // pub fn acosf128(n: f128) -> f128;
    // pub fn asinf128(n: f128) -> f128;
    // pub fn atanf128(n: f128) -> f128;
    // pub fn atan2f128(a: f128, b: f128) -> f128;
    // pub fn cbrtf128(n: f128) -> f128;
    // pub fn coshf128(n: f128) -> f128;
    // pub fn expm1f128(n: f128) -> f128;
    // pub fn hypotf128(x: f128, y: f128) -> f128;
    // pub fn log1pf128(n: f128) -> f128;
    // pub fn sinhf128(n: f128) -> f128;
    // pub fn tanf128(n: f128) -> f128;
    // pub fn tanhf128(n: f128) -> f128;
    // pub fn tgammaf128(n: f128) -> f128;
    // pub fn lgammaf128_r(n: f128, s: &mut i32) -> f128;
    // pub fn erff128(n: f128) -> f128;
    // pub fn erfcf128(n: f128) -> f128;

    // cfg_if::cfg_if! {
    // if #[cfg(not(all(target_os = "windows", target_env = "msvc", target_arch = "x86")))] {
    //     pub fn acosf(n: f32) -> f32;
    //     pub fn asinf(n: f32) -> f32;
    //     pub fn atan2f(a: f32, b: f32) -> f32;
    //     pub fn atanf(n: f32) -> f32;
    //     pub fn coshf(n: f32) -> f32;
    //     pub fn sinhf(n: f32) -> f32;
    //     pub fn tanf(n: f32) -> f32;
    //     pub fn tanhf(n: f32) -> f32;
    // }}
}
