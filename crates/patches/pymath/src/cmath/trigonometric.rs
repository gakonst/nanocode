//! Complex trigonometric and hyperbolic functions.

use super::{
    CM_LARGE_DOUBLE, CM_LOG_LARGE_DOUBLE, INF, M_LN2, N, P, P12, P14, P34, U, c, special_type,
    special_value, sqrt,
};
use crate::{Error, Result, m, mul_add};
use num_complex::Complex64;

// Local constants
const CM_SQRT_LARGE_DOUBLE: f64 = 6.703903964971298e+153; // sqrt(CM_LARGE_DOUBLE)
const CM_SQRT_DBL_MIN: f64 = 1.4916681462400413e-154; // sqrt(f64::MIN_POSITIVE)

// Special value tables

#[rustfmt::skip]
static ACOS_SPECIAL_VALUES: [[Complex64; 7]; 7] = [
    [c(P34, INF), c(P, INF),   c(P, INF),   c(P, -INF),  c(P, -INF),  c(P34, -INF), c(N, INF)],
    [c(P12, INF), c(U, U),     c(U, U),     c(U, U),     c(U, U),     c(P12, -INF), c(N, N)],
    [c(P12, INF), c(U, U),     c(P12, 0.0), c(P12, -0.0),c(U, U),     c(P12, -INF), c(P12, N)],
    [c(P12, INF), c(U, U),     c(P12, 0.0), c(P12, -0.0),c(U, U),     c(P12, -INF), c(P12, N)],
    [c(P12, INF), c(U, U),     c(U, U),     c(U, U),     c(U, U),     c(P12, -INF), c(N, N)],
    [c(P14, INF), c(0.0, INF), c(0.0, INF), c(0.0, -INF),c(0.0, -INF),c(P14, -INF), c(N, INF)],
    [c(N, INF),   c(N, N),     c(N, N),     c(N, N),     c(N, N),     c(N, -INF),   c(N, N)],
];

#[rustfmt::skip]
static ACOSH_SPECIAL_VALUES: [[Complex64; 7]; 7] = [
    [c(INF, -P34), c(INF, -P),   c(INF, -P),   c(INF, P),   c(INF, P),   c(INF, P34),  c(INF, N)],
    [c(INF, -P12), c(U, U),      c(U, U),      c(U, U),     c(U, U),     c(INF, P12),  c(N, N)],
    [c(INF, -P12), c(U, U),      c(0.0, -P12), c(0.0, P12), c(U, U),     c(INF, P12),  c(N, P12)],
    [c(INF, -P12), c(U, U),      c(0.0, -P12), c(0.0, P12), c(U, U),     c(INF, P12),  c(N, P12)],
    [c(INF, -P12), c(U, U),      c(U, U),      c(U, U),     c(U, U),     c(INF, P12),  c(N, N)],
    [c(INF, -P14), c(INF, -0.0), c(INF, -0.0), c(INF, 0.0), c(INF, 0.0), c(INF, P14),  c(INF, N)],
    [c(INF, N),    c(N, N),      c(N, N),      c(N, N),     c(N, N),     c(INF, N),    c(N, N)],
];

#[rustfmt::skip]
static ASINH_SPECIAL_VALUES: [[Complex64; 7]; 7] = [
    [c(-INF, -P14), c(-INF, -0.0), c(-INF, -0.0), c(-INF, 0.0), c(-INF, 0.0), c(-INF, P14), c(-INF, N)],
    [c(-INF, -P12), c(U, U),       c(U, U),       c(U, U),      c(U, U),      c(-INF, P12), c(N, N)],
    [c(-INF, -P12), c(U, U),       c(-0.0, -0.0), c(-0.0, 0.0), c(U, U),      c(-INF, P12), c(N, N)],
    [c(INF, -P12),  c(U, U),       c(0.0, -0.0),  c(0.0, 0.0),  c(U, U),      c(INF, P12),  c(N, N)],
    [c(INF, -P12),  c(U, U),       c(U, U),       c(U, U),      c(U, U),      c(INF, P12),  c(N, N)],
    [c(INF, -P14),  c(INF, -0.0),  c(INF, -0.0),  c(INF, 0.0),  c(INF, 0.0),  c(INF, P14),  c(INF, N)],
    [c(INF, N),     c(N, N),       c(N, -0.0),    c(N, 0.0),    c(N, N),      c(INF, N),    c(N, N)],
];

#[rustfmt::skip]
static ATANH_SPECIAL_VALUES: [[Complex64; 7]; 7] = [
    [c(-0.0, -P12), c(-0.0, -P12), c(-0.0, -P12), c(-0.0, P12), c(-0.0, P12), c(-0.0, P12), c(-0.0, N)],
    [c(-0.0, -P12), c(U, U),       c(U, U),       c(U, U),      c(U, U),      c(-0.0, P12), c(N, N)],
    [c(-0.0, -P12), c(U, U),       c(-0.0, -0.0), c(-0.0, 0.0), c(U, U),      c(-0.0, P12), c(-0.0, N)],
    [c(0.0, -P12),  c(U, U),       c(0.0, -0.0),  c(0.0, 0.0),  c(U, U),      c(0.0, P12),  c(0.0, N)],
    [c(0.0, -P12),  c(U, U),       c(U, U),       c(U, U),      c(U, U),      c(0.0, P12),  c(N, N)],
    [c(0.0, -P12),  c(0.0, -P12),  c(0.0, -P12),  c(0.0, P12),  c(0.0, P12),  c(0.0, P12),  c(0.0, N)],
    [c(0.0, -P12),  c(N, N),       c(N, N),       c(N, N),      c(N, N),      c(0.0, P12),  c(N, N)],
];

#[rustfmt::skip]
static COSH_SPECIAL_VALUES: [[Complex64; 7]; 7] = [
    [c(INF, N),  c(U, U), c(INF, 0.0),  c(INF, -0.0), c(U, U), c(INF, N),  c(INF, N)],
    [c(N, N),    c(U, U), c(U, U),      c(U, U),      c(U, U), c(N, N),    c(N, N)],
    [c(N, 0.0),  c(U, U), c(1.0, 0.0),  c(1.0, -0.0), c(U, U), c(N, 0.0),  c(N, 0.0)],
    [c(N, 0.0),  c(U, U), c(1.0, -0.0), c(1.0, 0.0),  c(U, U), c(N, 0.0),  c(N, 0.0)],
    [c(N, N),    c(U, U), c(U, U),      c(U, U),      c(U, U), c(N, N),    c(N, N)],
    [c(INF, N),  c(U, U), c(INF, -0.0), c(INF, 0.0),  c(U, U), c(INF, N),  c(INF, N)],
    [c(N, N),    c(N, N), c(N, 0.0),    c(N, 0.0),    c(N, N), c(N, N),    c(N, N)],
];

#[rustfmt::skip]
static SINH_SPECIAL_VALUES: [[Complex64; 7]; 7] = [
    [c(INF, N),  c(U, U), c(-INF, -0.0), c(-INF, 0.0), c(U, U), c(INF, N),  c(INF, N)],
    [c(N, N),    c(U, U), c(U, U),       c(U, U),      c(U, U), c(N, N),    c(N, N)],
    [c(0.0, N),  c(U, U), c(-0.0, -0.0), c(-0.0, 0.0), c(U, U), c(0.0, N),  c(0.0, N)],
    [c(0.0, N),  c(U, U), c(0.0, -0.0),  c(0.0, 0.0),  c(U, U), c(0.0, N),  c(0.0, N)],
    [c(N, N),    c(U, U), c(U, U),       c(U, U),      c(U, U), c(N, N),    c(N, N)],
    [c(INF, N),  c(U, U), c(INF, -0.0),  c(INF, 0.0),  c(U, U), c(INF, N),  c(INF, N)],
    [c(N, N),    c(N, N), c(N, -0.0),    c(N, 0.0),    c(N, N), c(N, N),    c(N, N)],
];

#[rustfmt::skip]
static TANH_SPECIAL_VALUES: [[Complex64; 7]; 7] = [
    [c(-1.0, 0.0), c(U, U), c(-1.0, -0.0), c(-1.0, 0.0), c(U, U), c(-1.0, 0.0), c(-1.0, 0.0)],
    [c(N, N),      c(U, U), c(U, U),       c(U, U),      c(U, U), c(N, N),      c(N, N)],
    [c(-0.0, N),   c(U, U), c(-0.0, -0.0), c(-0.0, 0.0), c(U, U), c(-0.0, N),   c(-0.0, N)],
    [c(0.0, N),    c(U, U), c(0.0, -0.0),  c(0.0, 0.0),  c(U, U), c(0.0, N),    c(0.0, N)],
    [c(N, N),      c(U, U), c(U, U),       c(U, U),      c(U, U), c(N, N),      c(N, N)],
    [c(1.0, 0.0),  c(U, U), c(1.0, -0.0),  c(1.0, 0.0),  c(U, U), c(1.0, 0.0),  c(1.0, 0.0)],
    [c(N, N),      c(N, N), c(N, -0.0),    c(N, 0.0),    c(N, N), c(N, N),      c(N, N)],
];

/// Complex hyperbolic cosine.
#[inline]
pub fn cosh(z: Complex64) -> Result<Complex64> {
    // Special treatment for cosh(+/-inf + iy) if y is finite and nonzero
    if !z.re.is_finite() || !z.im.is_finite() {
        let r = if z.re.is_infinite() && z.im.is_finite() && z.im != 0.0 {
            if z.re > 0.0 {
                Complex64::new(
                    m::copysign(INF, m::cos(z.im)),
                    m::copysign(INF, m::sin(z.im)),
                )
            } else {
                Complex64::new(
                    m::copysign(INF, m::cos(z.im)),
                    -m::copysign(INF, m::sin(z.im)),
                )
            }
        } else {
            COSH_SPECIAL_VALUES[special_type(z.re) as usize][special_type(z.im) as usize]
        };
        // need to set errno = EDOM if y is +/- infinity and x is not a NaN
        if z.im.is_infinite() && !z.re.is_nan() {
            return Err(Error::EDOM);
        }
        return Ok(r);
    }

    let (r_re, r_im);
    let (sin_im, cos_im) = m::sincos(z.im);
    if m::fabs(z.re) > CM_LOG_LARGE_DOUBLE {
        // deal correctly with cases where cosh(z.real) overflows but cosh(z) does not
        let x_minus_one = z.re - m::copysign(1.0, z.re);
        r_re = cos_im * m::cosh(x_minus_one) * core::f64::consts::E;
        r_im = sin_im * m::sinh(x_minus_one) * core::f64::consts::E;
    } else {
        r_re = cos_im * m::cosh(z.re);
        r_im = sin_im * m::sinh(z.re);
    }

    // detect overflow
    if r_re.is_infinite() || r_im.is_infinite() {
        return Err(Error::ERANGE);
    }
    Ok(Complex64::new(r_re, r_im))
}

/// Complex hyperbolic sine.
#[inline]
pub fn sinh(z: Complex64) -> Result<Complex64> {
    // Special treatment for sinh(+/-inf + iy) if y is finite and nonzero
    if !z.re.is_finite() || !z.im.is_finite() {
        let r = if z.re.is_infinite() && z.im.is_finite() && z.im != 0.0 {
            if z.re > 0.0 {
                Complex64::new(
                    m::copysign(INF, m::cos(z.im)),
                    m::copysign(INF, m::sin(z.im)),
                )
            } else {
                Complex64::new(
                    -m::copysign(INF, m::cos(z.im)),
                    m::copysign(INF, m::sin(z.im)),
                )
            }
        } else {
            SINH_SPECIAL_VALUES[special_type(z.re) as usize][special_type(z.im) as usize]
        };
        // need to set errno = EDOM if y is +/- infinity and x is not a NaN
        if z.im.is_infinite() && !z.re.is_nan() {
            return Err(Error::EDOM);
        }
        return Ok(r);
    }

    let (r_re, r_im);
    let (sin_im, cos_im) = m::sincos(z.im);
    if m::fabs(z.re) > CM_LOG_LARGE_DOUBLE {
        let x_minus_one = z.re - m::copysign(1.0, z.re);
        r_re = cos_im * m::sinh(x_minus_one) * core::f64::consts::E;
        r_im = sin_im * m::cosh(x_minus_one) * core::f64::consts::E;
    } else {
        r_re = cos_im * m::sinh(z.re);
        r_im = sin_im * m::cosh(z.re);
    }

    // detect overflow
    if r_re.is_infinite() || r_im.is_infinite() {
        return Err(Error::ERANGE);
    }
    Ok(Complex64::new(r_re, r_im))
}

/// Complex hyperbolic tangent.
#[inline]
pub fn tanh(z: Complex64) -> Result<Complex64> {
    // Special treatment for tanh(+/-inf + iy) if y is finite and nonzero
    if !z.re.is_finite() || !z.im.is_finite() {
        let r = if z.re.is_infinite() && z.im.is_finite() && z.im != 0.0 {
            if z.re > 0.0 {
                Complex64::new(1.0, m::copysign(0.0, 2.0 * m::sin(z.im) * m::cos(z.im)))
            } else {
                Complex64::new(-1.0, m::copysign(0.0, 2.0 * m::sin(z.im) * m::cos(z.im)))
            }
        } else {
            TANH_SPECIAL_VALUES[special_type(z.re) as usize][special_type(z.im) as usize]
        };
        // need to set errno = EDOM if z.imag is +/-infinity and z.real is finite
        if z.im.is_infinite() && z.re.is_finite() {
            return Err(Error::EDOM);
        }
        return Ok(r);
    }

    // danger of overflow in 2.*z.im!
    if m::fabs(z.re) > CM_LOG_LARGE_DOUBLE {
        let r = Complex64::new(
            m::copysign(1.0, z.re),
            4.0 * m::sin(z.im) * m::cos(z.im) * m::exp(-2.0 * m::fabs(z.re)),
        );
        return Ok(r);
    }

    let tx = m::tanh(z.re);
    let ty = m::tan(z.im);
    let cx = 1.0 / m::cosh(z.re);
    let txty = tx * ty;
    let denom = mul_add(txty, txty, 1.0);
    let r = Complex64::new(tx * mul_add(ty, ty, 1.0) / denom, ((ty / denom) * cx) * cx);
    Ok(r)
}

/// Complex cosine.
/// cos(z) = cosh(iz)
#[inline]
pub fn cos(z: Complex64) -> Result<Complex64> {
    let r = Complex64::new(-z.im, z.re);
    cosh(r)
}

/// Complex sine.
/// sin(z) = -i * sinh(iz)
#[inline]
pub fn sin(z: Complex64) -> Result<Complex64> {
    let s = Complex64::new(-z.im, z.re);
    let s = sinh(s)?;
    Ok(Complex64::new(s.im, -s.re))
}

/// Complex tangent.
/// tan(z) = -i * tanh(iz)
#[inline]
pub fn tan(z: Complex64) -> Result<Complex64> {
    let s = Complex64::new(-z.im, z.re);
    let s = tanh(s)?;
    Ok(Complex64::new(s.im, -s.re))
}

/// Complex inverse hyperbolic sine.
#[inline]
pub fn asinh(z: Complex64) -> Result<Complex64> {
    special_value!(z, ASINH_SPECIAL_VALUES);

    if m::fabs(z.re) > CM_LARGE_DOUBLE || m::fabs(z.im) > CM_LARGE_DOUBLE {
        // Avoid overflow for large arguments
        let r_re = if z.im >= 0.0 {
            m::copysign(m::log(m::hypot(z.re / 2.0, z.im / 2.0)) + M_LN2 * 2.0, z.re)
        } else {
            -m::copysign(
                m::log(m::hypot(z.re / 2.0, z.im / 2.0)) + M_LN2 * 2.0,
                -z.re,
            )
        };
        let r_im = m::atan2(z.im, m::fabs(z.re));
        return Ok(Complex64::new(r_re, r_im));
    }

    let s1 = sqrt(Complex64::new(1.0 + z.im, -z.re))?;
    let s2 = sqrt(Complex64::new(1.0 - z.im, z.re))?;
    let r_re = m::asinh(mul_add(s1.re, s2.im, -(s2.re * s1.im)));
    let r_im = m::atan2(z.im, mul_add(s1.re, s2.re, -(s1.im * s2.im)));
    Ok(Complex64::new(r_re, r_im))
}

/// Complex inverse hyperbolic cosine.
#[inline]
pub fn acosh(z: Complex64) -> Result<Complex64> {
    special_value!(z, ACOSH_SPECIAL_VALUES);

    if m::fabs(z.re) > CM_LARGE_DOUBLE || m::fabs(z.im) > CM_LARGE_DOUBLE {
        // Avoid overflow for large arguments
        let r_re = m::log(m::hypot(z.re / 2.0, z.im / 2.0)) + M_LN2 * 2.0;
        let r_im = m::atan2(z.im, z.re);
        return Ok(Complex64::new(r_re, r_im));
    }

    let s1 = sqrt(Complex64::new(z.re - 1.0, z.im))?;
    let s2 = sqrt(Complex64::new(z.re + 1.0, z.im))?;
    let r_re = m::asinh(mul_add(s1.re, s2.re, s1.im * s2.im));
    let r_im = 2.0 * m::atan2(s1.im, s2.re);
    Ok(Complex64::new(r_re, r_im))
}

/// Complex inverse hyperbolic tangent.
#[inline]
pub fn atanh(z: Complex64) -> Result<Complex64> {
    special_value!(z, ATANH_SPECIAL_VALUES);

    // Reduce to case where z.real >= 0., using atanh(z) = -atanh(-z)
    if z.re < 0.0 {
        let r = atanh(Complex64::new(-z.re, -z.im))?;
        return Ok(Complex64::new(-r.re, -r.im));
    }

    let ay = m::fabs(z.im);
    if z.re > CM_SQRT_LARGE_DOUBLE || ay > CM_SQRT_LARGE_DOUBLE {
        // if abs(z) is large then we use the approximation
        // atanh(z) ~ 1/z +/- i*pi/2 (+/- depending on sign of z.imag)
        let h = m::hypot(z.re / 2.0, z.im / 2.0);
        let r_re = z.re / 4.0 / h / h;
        let r_im = m::copysign(P12, z.im);
        return Ok(Complex64::new(r_re, r_im));
    } else if z.re == 1.0 && ay < CM_SQRT_DBL_MIN {
        // C99 standard says: atanh(1+/-0.) should be inf +/- 0i
        if ay == 0.0 {
            return Err(Error::EDOM);
        } else {
            let r_re = -m::log(m::sqrt(ay) / m::sqrt(m::hypot(ay, 2.0)));
            let r_im = m::copysign(m::atan2(2.0, -ay) / 2.0, z.im);
            return Ok(Complex64::new(r_re, r_im));
        }
    }

    let one_minus_re = 1.0 - z.re;
    let r_re = m::log1p(4.0 * z.re / mul_add(one_minus_re, one_minus_re, ay * ay)) / 4.0;
    let r_im = -m::atan2(-2.0 * z.im, mul_add(one_minus_re, 1.0 + z.re, -(ay * ay))) / 2.0;
    Ok(Complex64::new(r_re, r_im))
}

/// Complex arc cosine.
#[inline]
pub fn acos(z: Complex64) -> Result<Complex64> {
    special_value!(z, ACOS_SPECIAL_VALUES);

    if m::fabs(z.re) > CM_LARGE_DOUBLE || m::fabs(z.im) > CM_LARGE_DOUBLE {
        // Avoid overflow for large arguments
        let r_re = m::atan2(m::fabs(z.im), z.re);
        let r_im = if z.re < 0.0 {
            -m::copysign(m::log(m::hypot(z.re / 2.0, z.im / 2.0)) + M_LN2 * 2.0, z.im)
        } else {
            m::copysign(
                m::log(m::hypot(z.re / 2.0, z.im / 2.0)) + M_LN2 * 2.0,
                -z.im,
            )
        };
        return Ok(Complex64::new(r_re, r_im));
    }

    let s1 = sqrt(Complex64::new(1.0 - z.re, -z.im))?;
    let s2 = sqrt(Complex64::new(1.0 + z.re, z.im))?;
    let r_re = 2.0 * m::atan2(s1.re, s2.re);
    let r_im = m::asinh(mul_add(s2.re, s1.im, -(s2.im * s1.re)));
    Ok(Complex64::new(r_re, r_im))
}

/// Complex arc sine.
/// asin(z) = -i * asinh(iz)
#[inline]
pub fn asin(z: Complex64) -> Result<Complex64> {
    let s = asinh(Complex64::new(-z.im, z.re))?;
    Ok(Complex64::new(s.im, -s.re))
}

/// Complex arc tangent.
/// atan(z) = -i * atanh(iz)
#[inline]
pub fn atan(z: Complex64) -> Result<Complex64> {
    let s = atanh(Complex64::new(-z.im, z.re))?;
    Ok(Complex64::new(s.im, -s.re))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_cmath_func<F>(func_name: &str, rs_func: F, re: f64, im: f64)
    where
        F: Fn(Complex64) -> Result<Complex64>,
    {
        crate::cmath::tests::test_cmath_func(func_name, rs_func, re, im);
    }

    fn test_sin(re: f64, im: f64) {
        test_cmath_func("sin", sin, re, im);
    }
    fn test_cos(re: f64, im: f64) {
        test_cmath_func("cos", cos, re, im);
    }
    fn test_tan(re: f64, im: f64) {
        test_cmath_func("tan", tan, re, im);
    }
    fn test_sinh(re: f64, im: f64) {
        test_cmath_func("sinh", sinh, re, im);
    }
    fn test_cosh(re: f64, im: f64) {
        test_cmath_func("cosh", cosh, re, im);
    }
    fn test_tanh(re: f64, im: f64) {
        test_cmath_func("tanh", tanh, re, im);
    }
    fn test_asin(re: f64, im: f64) {
        test_cmath_func("asin", asin, re, im);
    }
    fn test_acos(re: f64, im: f64) {
        test_cmath_func("acos", acos, re, im);
    }
    fn test_atan(re: f64, im: f64) {
        test_cmath_func("atan", atan, re, im);
    }
    fn test_asinh(re: f64, im: f64) {
        test_cmath_func("asinh", asinh, re, im);
    }
    fn test_acosh(re: f64, im: f64) {
        test_cmath_func("acosh", acosh, re, im);
    }
    fn test_atanh(re: f64, im: f64) {
        test_cmath_func("atanh", atanh, re, im);
    }

    use crate::test::EDGE_VALUES;

    #[test]
    fn edgetest_sin() {
        for &re in EDGE_VALUES {
            for &im in EDGE_VALUES {
                test_sin(re, im);
            }
        }
    }

    #[test]
    fn edgetest_cos() {
        for &re in EDGE_VALUES {
            for &im in EDGE_VALUES {
                test_cos(re, im);
            }
        }
    }

    #[test]
    fn edgetest_tan() {
        for &re in EDGE_VALUES {
            for &im in EDGE_VALUES {
                test_tan(re, im);
            }
        }
    }

    #[test]
    fn edgetest_sinh() {
        for &re in EDGE_VALUES {
            for &im in EDGE_VALUES {
                test_sinh(re, im);
            }
        }
    }

    #[test]
    fn edgetest_cosh() {
        for &re in EDGE_VALUES {
            for &im in EDGE_VALUES {
                test_cosh(re, im);
            }
        }
    }

    #[test]
    fn edgetest_tanh() {
        for &re in EDGE_VALUES {
            for &im in EDGE_VALUES {
                test_tanh(re, im);
            }
        }
    }

    #[test]
    fn edgetest_asin() {
        for &re in EDGE_VALUES {
            for &im in EDGE_VALUES {
                test_asin(re, im);
            }
        }
    }

    #[test]
    fn edgetest_acos() {
        for &re in EDGE_VALUES {
            for &im in EDGE_VALUES {
                test_acos(re, im);
            }
        }
    }

    #[test]
    fn edgetest_atan() {
        for &re in EDGE_VALUES {
            for &im in EDGE_VALUES {
                test_atan(re, im);
            }
        }
    }

    #[test]
    fn edgetest_asinh() {
        for &re in EDGE_VALUES {
            for &im in EDGE_VALUES {
                test_asinh(re, im);
            }
        }
    }

    #[test]
    fn edgetest_acosh() {
        for &re in EDGE_VALUES {
            for &im in EDGE_VALUES {
                test_acosh(re, im);
            }
        }
    }

    #[test]
    fn edgetest_atanh() {
        for &re in EDGE_VALUES {
            for &im in EDGE_VALUES {
                test_atanh(re, im);
            }
        }
    }

    proptest::proptest! {
        #[test]
        fn proptest_sin(re: f64, im: f64) {
            test_sin(re, im);
        }

        #[test]
        fn proptest_cos(re: f64, im: f64) {
            test_cos(re, im);
        }

        #[test]
        fn proptest_tan(re: f64, im: f64) {
            test_tan(re, im);
        }

        #[test]
        fn proptest_sinh(re: f64, im: f64) {
            test_sinh(re, im);
        }

        #[test]
        fn proptest_cosh(re: f64, im: f64) {
            test_cosh(re, im);
        }

        #[test]
        fn proptest_tanh(re: f64, im: f64) {
            test_tanh(re, im);
        }

        #[test]
        fn proptest_asin(re: f64, im: f64) {
            test_asin(re, im);
        }

        #[test]
        fn proptest_acos(re: f64, im: f64) {
            test_acos(re, im);
        }

        #[test]
        fn proptest_atan(re: f64, im: f64) {
            test_atan(re, im);
        }

        #[test]
        fn proptest_asinh(re: f64, im: f64) {
            test_asinh(re, im);
        }

        #[test]
        fn proptest_acosh(re: f64, im: f64) {
            test_acosh(re, im);
        }

        #[test]
        fn proptest_atanh(re: f64, im: f64) {
            test_atanh(re, im);
        }
    }
}
