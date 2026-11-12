//! Complex exponential and logarithmic functions.

use super::{
    CM_LARGE_DOUBLE, CM_LOG_LARGE_DOUBLE, INF, M_LN2, N, P, P12, P14, P34, U, c, special_type,
    special_value,
};
use crate::{Error, Result, m, mul_add};
use num_complex::Complex64;

// Local constants
const M_LN10: f64 = core::f64::consts::LN_10;

/// Scale factors for subnormal handling in sqrt.
const CM_SCALE_UP: i32 = 2 * (f64::MANTISSA_DIGITS as i32 / 2) + 1; // 54 for IEEE 754
const CM_SCALE_DOWN: i32 = -(CM_SCALE_UP + 1) / 2; // -27

// Special value tables

#[rustfmt::skip]
static EXP_SPECIAL_VALUES: [[Complex64; 7]; 7] = [
    [c(0.0, 0.0), c(U, U), c(0.0, -0.0), c(0.0, 0.0),  c(U, U), c(0.0, 0.0), c(0.0, 0.0)],
    [c(N, N),     c(U, U), c(U, U),      c(U, U),      c(U, U), c(N, N),     c(N, N)],
    [c(N, N),     c(U, U), c(1.0, -0.0), c(1.0, 0.0),  c(U, U), c(N, N),     c(N, N)],
    [c(N, N),     c(U, U), c(1.0, -0.0), c(1.0, 0.0),  c(U, U), c(N, N),     c(N, N)],
    [c(N, N),     c(U, U), c(U, U),      c(U, U),      c(U, U), c(N, N),     c(N, N)],
    [c(INF, N),   c(U, U), c(INF, -0.0), c(INF, 0.0),  c(U, U), c(INF, N),   c(INF, N)],
    [c(N, N),     c(N, N), c(N, -0.0),   c(N, 0.0),    c(N, N), c(N, N),     c(N, N)],
];

#[rustfmt::skip]
static LOG_SPECIAL_VALUES: [[Complex64; 7]; 7] = [
    [c(INF, -P34), c(INF, -P),    c(INF, -P),    c(INF, P),    c(INF, P),   c(INF, P34), c(INF, N)],
    [c(INF, -P12), c(U, U),       c(U, U),       c(U, U),      c(U, U),     c(INF, P12), c(N, N)],
    [c(INF, -P12), c(U, U),       c(-INF, -P),   c(-INF, P),   c(U, U),     c(INF, P12), c(N, N)],
    [c(INF, -P12), c(U, U),       c(-INF, -0.0), c(-INF, 0.0), c(U, U),     c(INF, P12), c(N, N)],
    [c(INF, -P12), c(U, U),       c(U, U),       c(U, U),      c(U, U),     c(INF, P12), c(N, N)],
    [c(INF, -P14), c(INF, -0.0),  c(INF, -0.0),  c(INF, 0.0),  c(INF, 0.0), c(INF, P14), c(INF, N)],
    [c(INF, N),    c(N, N),       c(N, N),       c(N, N),      c(N, N),     c(INF, N),   c(N, N)],
];

#[rustfmt::skip]
static SQRT_SPECIAL_VALUES: [[Complex64; 7]; 7] = [
    [c(INF, -INF), c(0.0, -INF), c(0.0, -INF), c(0.0, INF), c(0.0, INF), c(INF, INF), c(N, INF)],
    [c(INF, -INF), c(U, U),      c(U, U),      c(U, U),     c(U, U),     c(INF, INF), c(N, N)],
    [c(INF, -INF), c(U, U),      c(0.0, -0.0), c(0.0, 0.0), c(U, U),     c(INF, INF), c(N, N)],
    [c(INF, -INF), c(U, U),      c(0.0, -0.0), c(0.0, 0.0), c(U, U),     c(INF, INF), c(N, N)],
    [c(INF, -INF), c(U, U),      c(U, U),      c(U, U),     c(U, U),     c(INF, INF), c(N, N)],
    [c(INF, -INF), c(INF, -0.0), c(INF, -0.0), c(INF, 0.0), c(INF, 0.0), c(INF, INF), c(INF, N)],
    [c(INF, -INF), c(N, N),      c(N, N),      c(N, N),     c(N, N),     c(INF, INF), c(N, N)],
];

/// Complex square root.
///
/// Uses symmetries to reduce to the case when x = z.real and y = z.imag
/// are nonnegative, with careful handling of overflow and subnormals.
#[inline]
pub fn sqrt(z: Complex64) -> Result<Complex64> {
    special_value!(z, SQRT_SPECIAL_VALUES);

    if z.re == 0.0 && z.im == 0.0 {
        return Ok(Complex64::new(0.0, z.im));
    }

    let ax = m::fabs(z.re);
    let ay = m::fabs(z.im);

    let s = if ax < f64::MIN_POSITIVE && ay < f64::MIN_POSITIVE {
        // Handle subnormal case
        let ax_scaled = m::ldexp(ax, CM_SCALE_UP);
        m::ldexp(
            m::sqrt(ax_scaled + m::hypot(ax_scaled, m::ldexp(ay, CM_SCALE_UP))),
            CM_SCALE_DOWN,
        )
    } else {
        let ax8 = ax / 8.0;
        2.0 * m::sqrt(ax8 + m::hypot(ax8, ay / 8.0))
    };

    let d = ay / (2.0 * s);

    if z.re >= 0.0 {
        Ok(Complex64::new(s, m::copysign(d, z.im)))
    } else {
        Ok(Complex64::new(d, m::copysign(s, z.im)))
    }
}

/// Complex exponential.
#[inline]
pub fn exp(z: Complex64) -> Result<Complex64> {
    // Handle special values
    if !z.re.is_finite() || !z.im.is_finite() {
        let r = if z.re.is_infinite() && z.im.is_finite() && z.im != 0.0 {
            if z.re > 0.0 {
                Complex64::new(
                    m::copysign(INF, m::cos(z.im)),
                    m::copysign(INF, m::sin(z.im)),
                )
            } else {
                Complex64::new(
                    m::copysign(0.0, m::cos(z.im)),
                    m::copysign(0.0, m::sin(z.im)),
                )
            }
        } else {
            EXP_SPECIAL_VALUES[special_type(z.re) as usize][special_type(z.im) as usize]
        };
        // need to set errno = EDOM if y is +/- infinity and x is not a NaN and not -infinity
        if z.im.is_infinite() && (z.re.is_finite() || (z.re.is_infinite() && z.re > 0.0)) {
            return Err(Error::EDOM);
        }
        return Ok(r);
    }

    let (sin_im, cos_im) = m::sincos(z.im);
    let (r_re, r_im);
    if z.re > CM_LOG_LARGE_DOUBLE {
        let l = m::exp(z.re - 1.0);
        r_re = l * cos_im * core::f64::consts::E;
        r_im = l * sin_im * core::f64::consts::E;
    } else {
        let l = m::exp(z.re);
        r_re = l * cos_im;
        r_im = l * sin_im;
    }

    // detect overflow
    if r_re.is_infinite() || r_im.is_infinite() {
        return Err(Error::ERANGE);
    }
    Ok(Complex64::new(r_re, r_im))
}

/// Complex natural logarithm (ln, base e).
/// TODO: consider to expose API
#[inline]
pub(crate) fn ln(z: Complex64) -> Result<Complex64> {
    special_value!(z, LOG_SPECIAL_VALUES);

    let ax = m::fabs(z.re);
    let ay = m::fabs(z.im);

    let r_re = if ax > CM_LARGE_DOUBLE || ay > CM_LARGE_DOUBLE {
        m::log(m::hypot(ax / 2.0, ay / 2.0)) + M_LN2
    } else if ax < f64::MIN_POSITIVE && ay < f64::MIN_POSITIVE {
        if ax > 0.0 || ay > 0.0 {
            // catch cases where hypot(ax, ay) is subnormal
            m::log(m::hypot(
                m::ldexp(ax, f64::MANTISSA_DIGITS as i32),
                m::ldexp(ay, f64::MANTISSA_DIGITS as i32),
            )) - f64::MANTISSA_DIGITS as f64 * M_LN2
        } else {
            // log(+/-0. +/- 0i) - return -inf like CPython does
            // Note: CPython sets errno=EDOM but still returns a value.
            // When used with a base, the second c_log call clears errno.
            f64::NEG_INFINITY
        }
    } else {
        let h = m::hypot(ax, ay);
        if (0.71..=1.73).contains(&h) {
            let am = if ax > ay { ax } else { ay }; // max(ax, ay)
            let an = if ax > ay { ay } else { ax }; // min(ax, ay)
            let log1p_arg = mul_add(am - 1.0, am + 1.0, an * an);
            m::log1p(log1p_arg) / 2.0
        } else {
            m::log(h)
        }
    };

    let r_im = m::atan2(z.im, z.re);
    Ok(Complex64::new(r_re, r_im))
}

/// Complex logarithm with optional base.
///
/// If base is None, returns the natural logarithm.
/// If base is Some(b), returns log(z) / log(b).
#[inline]
pub fn log(z: Complex64, base: Option<Complex64>) -> Result<Complex64> {
    let (log_z, mut err) = c_log(z);
    match base {
        None => err.map_or(Ok(log_z), Err),
        Some(b) => {
            // Like cmath_log_impl, the second c_log call overwrites
            // any pending error from the first one.
            let (log_b, base_err) = c_log(b);
            err = base_err;
            let (q, quot_err) = c_quot(log_z, log_b);
            if let Some(e) = quot_err {
                err = Some(e);
            }
            err.map_or(Ok(q), Err)
        }
    }
}

/// c_log behavior: always returns a value, but reports EDOM for zero.
#[inline]
fn c_log(z: Complex64) -> (Complex64, Option<Error>) {
    let r = ln(z).expect("ln handles special values without failing");
    if z.re == 0.0 && z.im == 0.0 {
        (r, Some(Error::EDOM))
    } else {
        (r, None)
    }
}

/// Complex division following _Py_c_quot algorithm.
/// This preserves the sign of zero correctly and recovers infinities
/// from NaN results per C11 Annex G.5.2.
#[inline]
fn c_quot(a: Complex64, b: Complex64) -> (Complex64, Option<Error>) {
    let abs_breal = m::fabs(b.re);
    let abs_bimag = m::fabs(b.im);
    let mut err = None;

    let mut r = if abs_breal >= abs_bimag {
        if abs_breal == 0.0 {
            err = Some(Error::EDOM);
            Complex64::new(0.0, 0.0)
        } else {
            let ratio = b.im / b.re;
            let denom = b.re + b.im * ratio;
            Complex64::new((a.re + a.im * ratio) / denom, (a.im - a.re * ratio) / denom)
        }
    } else if abs_bimag >= abs_breal {
        let ratio = b.re / b.im;
        let denom = b.re * ratio + b.im;
        Complex64::new((a.re * ratio + a.im) / denom, (a.im * ratio - a.re) / denom)
    } else {
        // At least one of b.re or b.im is NaN
        Complex64::new(f64::NAN, f64::NAN)
    };

    // Recover infinities and zeros that computed as nan+nanj.
    // See C11 Annex G.5.2, routine _Cdivd().
    if r.re.is_nan() && r.im.is_nan() {
        if (a.re.is_infinite() || a.im.is_infinite()) && b.re.is_finite() && b.im.is_finite() {
            let x = m::copysign(if a.re.is_infinite() { 1.0 } else { 0.0 }, a.re);
            let y = m::copysign(if a.im.is_infinite() { 1.0 } else { 0.0 }, a.im);
            r.re = f64::INFINITY * (x * b.re + y * b.im);
            r.im = f64::INFINITY * (y * b.re - x * b.im);
        } else if (abs_breal.is_infinite() || abs_bimag.is_infinite())
            && a.re.is_finite()
            && a.im.is_finite()
        {
            let x = m::copysign(if b.re.is_infinite() { 1.0 } else { 0.0 }, b.re);
            let y = m::copysign(if b.im.is_infinite() { 1.0 } else { 0.0 }, b.im);
            r.re = 0.0 * (a.re * x + a.im * y);
            r.im = 0.0 * (a.im * x - a.re * y);
        }
    }

    (r, err)
}

/// Complex base-10 logarithm.
#[inline]
pub fn log10(z: Complex64) -> Result<Complex64> {
    // Like log(z) without base, log10(0) raises EDOM
    if z.re == 0.0 && z.im == 0.0 {
        return Err(Error::EDOM);
    }
    let r = ln(z)?;
    Ok(Complex64::new(r.re / M_LN10, r.im / M_LN10))
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

    fn test_sqrt(re: f64, im: f64) {
        test_cmath_func("sqrt", sqrt, re, im);
    }
    fn test_exp(re: f64, im: f64) {
        test_cmath_func("exp", exp, re, im);
    }
    fn test_log_n(re: f64, im: f64) {
        test_cmath_func("log", |z| log(z, None), re, im);
    }
    fn test_log10(re: f64, im: f64) {
        test_cmath_func("log10", log10, re, im);
    }

    /// Test log with base - compares with Python's cmath.log(z, base)
    fn test_log(z_re: f64, z_im: f64, base_re: f64, base_im: f64) {
        use pyo3::prelude::*;

        let z = Complex64::new(z_re, z_im);
        let base = Complex64::new(base_re, base_im);
        let rs_result = log(z, Some(base));

        pyo3::Python::attach(|py| {
            let cmath = pyo3::types::PyModule::import(py, "cmath").unwrap();
            let py_z = pyo3::types::PyComplex::from_doubles(py, z_re, z_im);
            let py_base = pyo3::types::PyComplex::from_doubles(py, base_re, base_im);
            let py_result = cmath.getattr("log").unwrap().call1((py_z, py_base));

            match py_result {
                Ok(result) => {
                    use pyo3::types::PyComplexMethods;
                    let c = result.cast::<pyo3::types::PyComplex>().unwrap();
                    let py_re = c.real();
                    let py_im = c.imag();
                    match rs_result {
                        Ok(rs) => {
                            crate::cmath::tests::assert_complex_eq(
                                py_re, py_im, rs, "log", z_re, z_im,
                            );
                        }
                        Err(e) => {
                            panic!(
                                "log({z_re}+{z_im}j, {base_re}+{base_im}j): py=({py_re}, {py_im}) but rs returned error {e:?}"
                            );
                        }
                    }
                }
                Err(_) => {
                    assert!(
                        rs_result.is_err(),
                        "log({z_re}+{z_im}j, {base_re}+{base_im}j): py raised error but rs={rs_result:?}"
                    );
                }
            }
        });
    }

    fn test_log_error(z: Complex64, base: Complex64) {
        use pyo3::prelude::*;

        let rs_result = log(z, Some(base));

        Python::attach(|py| {
            let cmath = pyo3::types::PyModule::import(py, "cmath").unwrap();
            let py_z = pyo3::types::PyComplex::from_doubles(py, z.re, z.im);
            let py_base = pyo3::types::PyComplex::from_doubles(py, base.re, base.im);
            let py_result = cmath.getattr("log").unwrap().call1((py_z, py_base));

            match py_result {
                Ok(result) => {
                    use pyo3::types::PyComplexMethods;
                    let c = result.cast::<pyo3::types::PyComplex>().unwrap();
                    panic!(
                        "log({}+{}j, {}+{}j): expected ValueError, got ({}, {})",
                        z.re,
                        z.im,
                        base.re,
                        base.im,
                        c.real(),
                        c.imag()
                    );
                }
                Err(err) => {
                    assert!(
                        err.is_instance_of::<pyo3::exceptions::PyValueError>(py),
                        "log({}+{}j, {}+{}j): expected ValueError, got {err:?}",
                        z.re,
                        z.im,
                        base.re,
                        base.im,
                    );
                    assert!(
                        matches!(rs_result, Err(crate::Error::EDOM)),
                        "log({}+{}j, {}+{}j): expected Err(EDOM), got {rs_result:?}",
                        z.re,
                        z.im,
                        base.re,
                        base.im,
                    );
                }
            }
        });
    }

    use crate::test::EDGE_VALUES;

    #[test]
    fn edgetest_sqrt() {
        for &re in EDGE_VALUES {
            for &im in EDGE_VALUES {
                test_sqrt(re, im);
            }
        }
    }

    #[test]
    fn edgetest_exp() {
        for &re in EDGE_VALUES {
            for &im in EDGE_VALUES {
                test_exp(re, im);
            }
        }
    }

    #[test]
    fn edgetest_log_n() {
        for &re in EDGE_VALUES {
            for &im in EDGE_VALUES {
                test_log_n(re, im);
            }
        }
    }

    #[test]
    fn edgetest_log10() {
        for &re in EDGE_VALUES {
            for &im in EDGE_VALUES {
                test_log10(re, im);
            }
        }
    }

    #[test]
    fn edgetest_log() {
        // Test log with various bases - sign preservation edge cases
        let bases = [0.5, 2.0, 10.0];
        let values = [0.01, 0.1, 0.5, 1.0, 2.0, 10.0, 100.0];
        for &base in &bases {
            for &v in &values {
                test_log(v, 0.0, base, 0.0);
            }
        }
        // Additional edge cases with imaginary parts
        for &z_re in EDGE_VALUES {
            for &z_im in EDGE_VALUES {
                test_log(z_re, z_im, 2.0, 0.0);
                test_log(z_re, z_im, 0.5, 0.0);
            }
        }
    }

    #[test]
    fn regression_c_quot_zero_denominator_sets_edom() {
        let (q, err) = c_quot(Complex64::new(2.0, -3.0), Complex64::new(0.0, 0.0));
        assert_eq!(err, Some(crate::Error::EDOM));
        assert_eq!(q.re.to_bits(), 0.0f64.to_bits());
        assert_eq!(q.im.to_bits(), 0.0f64.to_bits());
    }

    #[test]
    fn regression_log_zero_quotient_denominator_raises_edom() {
        let cases = [
            (Complex64::new(2.0, 0.0), Complex64::new(1.0, 0.0)),
            (Complex64::new(1.0, 0.0), Complex64::new(1.0, 0.0)),
            (Complex64::new(2.0, 0.0), Complex64::new(0.0, 0.0)),
            (Complex64::new(0.0, 0.0), Complex64::new(1.0, 0.0)),
            (Complex64::new(0.0, 0.0), Complex64::new(0.0, 0.0)),
        ];

        for (z, base) in cases {
            test_log_error(z, base);
        }
    }

    proptest::proptest! {
        #[test]
        fn proptest_sqrt(re: f64, im: f64) {
            test_sqrt(re, im);
        }

        #[test]
        fn proptest_exp(re: f64, im: f64) {
            test_exp(re, im);
        }

        #[test]
        fn proptest_log(re: f64, im: f64) {
            test_log_n(re, im);
        }

        #[test]
        fn proptest_log10(re: f64, im: f64) {
            test_log10(re, im);
        }
    }
}
