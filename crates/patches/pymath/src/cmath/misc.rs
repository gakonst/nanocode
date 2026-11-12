//! Complex polar coordinate and utility functions.

use super::{INF, N, U, c, special_type};
use crate::{Error, Result, m};
use num_complex::Complex64;

#[rustfmt::skip]
static RECT_SPECIAL_VALUES: [[Complex64; 7]; 7] = [
    [c(INF, N),   c(U, U), c(-INF, 0.0), c(-INF, -0.0), c(U, U), c(INF, N),   c(INF, N)],
    [c(N, N),     c(U, U), c(U, U),      c(U, U),       c(U, U), c(N, N),     c(N, N)],
    [c(0.0, 0.0), c(U, U), c(-0.0, 0.0), c(-0.0, -0.0), c(U, U), c(0.0, 0.0), c(0.0, 0.0)],
    [c(0.0, 0.0), c(U, U), c(0.0, -0.0), c(0.0, 0.0),   c(U, U), c(0.0, 0.0), c(0.0, 0.0)],
    [c(N, N),     c(U, U), c(U, U),      c(U, U),       c(U, U), c(N, N),     c(N, N)],
    [c(INF, N),   c(U, U), c(INF, -0.0), c(INF, 0.0),   c(U, U), c(INF, N),   c(INF, N)],
    [c(N, N),     c(N, N), c(N, 0.0),    c(N, 0.0),     c(N, N), c(N, N),     c(N, N)],
];

/// Return the phase angle (argument) of z.
#[inline]
pub fn phase(z: Complex64) -> Result<f64> {
    crate::err::set_errno(0);
    let phi = m::atan2(z.im, z.re);
    match crate::err::get_errno() {
        0 => Ok(phi),
        e if e == Error::EDOM as i32 => Err(Error::EDOM),
        e if e == Error::ERANGE as i32 => Err(Error::ERANGE),
        _ => Err(Error::EDOM), // Unknown errno treated as domain error (like PyErr_SetFromErrno)
    }
}

#[inline]
fn c_abs_raw(z: Complex64) -> f64 {
    if !z.re.is_finite() || !z.im.is_finite() {
        // C99 rules: if either part is infinite, return infinity,
        // even if the other part is NaN.
        if z.re.is_infinite() {
            return m::fabs(z.re);
        }
        if z.im.is_infinite() {
            return m::fabs(z.im);
        }
        return f64::NAN;
    }
    m::hypot(z.re, z.im)
}

#[inline]
fn c_abs_checked(z: Complex64) -> Result<f64> {
    if !z.re.is_finite() || !z.im.is_finite() {
        return Ok(c_abs_raw(z));
    }
    crate::err::set_errno(0);
    let r = m::hypot(z.re, z.im);
    if r.is_infinite() {
        Err(Error::ERANGE)
    } else {
        Ok(r)
    }
}

/// Convert z to polar coordinates (r, phi).
#[inline]
pub fn polar(z: Complex64) -> Result<(f64, f64)> {
    let phi = m::atan2(z.im, z.re);
    let r = c_abs_checked(z)?;
    Ok((r, phi))
}

/// Convert polar coordinates (r, phi) to rectangular form.
#[inline]
pub fn rect(r: f64, phi: f64) -> Result<Complex64> {
    // Handle special values
    if !r.is_finite() || !phi.is_finite() {
        // if r is +/-infinity and phi is finite but nonzero then
        // result is (+-INF +-INF i), but we need to compute cos(phi)
        // and sin(phi) to figure out the signs.
        let z = if r.is_infinite() && phi.is_finite() && phi != 0.0 {
            if r > 0.0 {
                Complex64::new(m::copysign(INF, m::cos(phi)), m::copysign(INF, m::sin(phi)))
            } else {
                Complex64::new(
                    -m::copysign(INF, m::cos(phi)),
                    -m::copysign(INF, m::sin(phi)),
                )
            }
        } else {
            RECT_SPECIAL_VALUES[special_type(r) as usize][special_type(phi) as usize]
        };
        // need to set errno = EDOM if r is a nonzero number and phi is infinite
        if r != 0.0 && !r.is_nan() && phi.is_infinite() {
            return Err(Error::EDOM);
        }
        return Ok(z);
    } else if phi == 0.0 {
        // Workaround for buggy results with phi=-0.0 on OS X 10.8.
        return Ok(Complex64::new(r, r * phi));
    }

    let (sin_phi, cos_phi) = m::sincos(phi);
    Ok(Complex64::new(r * cos_phi, r * sin_phi))
}

/// Return True if both real and imaginary parts are finite.
#[inline]
pub fn isfinite(z: Complex64) -> bool {
    z.re.is_finite() && z.im.is_finite()
}

/// Return True if either real or imaginary part is NaN.
#[inline]
pub fn isnan(z: Complex64) -> bool {
    z.re.is_nan() || z.im.is_nan()
}

/// Return True if either real or imaginary part is infinite.
#[inline]
pub fn isinf(z: Complex64) -> bool {
    z.re.is_infinite() || z.im.is_infinite()
}

/// Complex absolute value (magnitude).
#[inline]
pub fn abs(z: Complex64) -> f64 {
    c_abs_raw(z)
}

/// Determine whether two complex numbers are close in value.
///
/// Default tolerances: rel_tol = 1e-09, abs_tol = 0.0
/// Returns Err(EDOM) if rel_tol or abs_tol is negative.
#[inline]
pub fn isclose(
    a: Complex64,
    b: Complex64,
    rel_tol: Option<f64>,
    abs_tol: Option<f64>,
) -> Result<bool> {
    let rel_tol = rel_tol.unwrap_or(1e-09);
    let abs_tol = abs_tol.unwrap_or(0.0);

    // Tolerances must be non-negative
    if rel_tol < 0.0 || abs_tol < 0.0 {
        return Err(Error::EDOM);
    }

    // short circuit exact equality
    if a.re == b.re && a.im == b.im {
        return Ok(true);
    }

    // This catches the case of two infinities of opposite sign, or
    // one infinity and one finite number.
    if a.re.is_infinite() || a.im.is_infinite() || b.re.is_infinite() || b.im.is_infinite() {
        return Ok(false);
    }

    // now do the regular computation
    let diff = abs(Complex64::new(a.re - b.re, a.im - b.im));
    Ok((diff <= rel_tol * abs(b)) || (diff <= rel_tol * abs(a)) || (diff <= abs_tol))
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::test::EDGE_VALUES;

    fn test_phase_impl(re: f64, im: f64) {
        use pyo3::prelude::*;

        let rs_result = phase(Complex64::new(re, im));

        pyo3::Python::attach(|py| {
            let cmath = pyo3::types::PyModule::import(py, "cmath").unwrap();
            let py_func = cmath.getattr("phase").unwrap();
            let py_result = py_func.call1((pyo3::types::PyComplex::from_doubles(py, re, im),));

            match py_result {
                Ok(result) => {
                    let py_val: f64 = result.extract().unwrap();
                    match rs_result {
                        Ok(rs_val) => {
                            if py_val.is_nan() && rs_val.is_nan() {
                                return;
                            }
                            assert_eq!(
                                py_val.to_bits(),
                                rs_val.to_bits(),
                                "phase({re}, {im}): py={py_val} vs rs={rs_val}"
                            );
                        }
                        Err(e) => {
                            panic!("phase({re}, {im}): py={py_val} but rs returned error {e:?}");
                        }
                    }
                }
                Err(e) => {
                    // Python raised an exception - check we got an error too
                    if let Ok(rs_val) = rs_result {
                        if e.is_instance_of::<pyo3::exceptions::PyValueError>(py) {
                            panic!("phase({re}, {im}): py raised ValueError but rs={rs_val}");
                        } else if e.is_instance_of::<pyo3::exceptions::PyOverflowError>(py) {
                            panic!("phase({re}, {im}): py raised OverflowError but rs={rs_val}");
                        }
                    }
                    // Both raised errors - OK
                }
            }
        });
    }

    #[test]
    fn edgetest_phase() {
        for &re in EDGE_VALUES {
            for &im in EDGE_VALUES {
                test_phase_impl(re, im);
            }
        }
    }

    fn test_polar_impl(re: f64, im: f64) {
        use pyo3::prelude::*;

        let rs_result = polar(Complex64::new(re, im));

        pyo3::Python::attach(|py| {
            let cmath = pyo3::types::PyModule::import(py, "cmath").unwrap();
            let py_func = cmath.getattr("polar").unwrap();
            let py_result = py_func.call1((pyo3::types::PyComplex::from_doubles(py, re, im),));

            match py_result {
                Ok(result) => {
                    let (py_r, py_phi): (f64, f64) = result.extract().unwrap();
                    match rs_result {
                        Ok((rs_r, rs_phi)) => {
                            // Check r
                            if !py_r.is_nan() || !rs_r.is_nan() {
                                if py_r.is_nan() || rs_r.is_nan() {
                                    panic!("polar({re}, {im}).r: py={py_r} vs rs={rs_r}");
                                }
                                assert_eq!(
                                    py_r.to_bits(),
                                    rs_r.to_bits(),
                                    "polar({re}, {im}).r: py={py_r} vs rs={rs_r}"
                                );
                            }
                            // Check phi
                            if !py_phi.is_nan() || !rs_phi.is_nan() {
                                if py_phi.is_nan() || rs_phi.is_nan() {
                                    panic!("polar({re}, {im}).phi: py={py_phi} vs rs={rs_phi}");
                                }
                                assert_eq!(
                                    py_phi.to_bits(),
                                    rs_phi.to_bits(),
                                    "polar({re}, {im}).phi: py={py_phi} vs rs={rs_phi}"
                                );
                            }
                        }
                        Err(_) => {
                            panic!(
                                "polar({re}, {im}): py=({py_r}, {py_phi}) but rs returned error"
                            );
                        }
                    }
                }
                Err(_) => {
                    // CPython raised error - check we did too
                    assert!(
                        rs_result.is_err(),
                        "polar({re}, {im}): py raised error but rs succeeded"
                    );
                }
            }
        });
    }

    #[test]
    fn edgetest_polar() {
        for &re in EDGE_VALUES {
            for &im in EDGE_VALUES {
                test_polar_impl(re, im);
            }
        }
    }

    fn test_rect_impl(r: f64, phi: f64) {
        use pyo3::prelude::*;

        let rs_result = rect(r, phi);

        pyo3::Python::attach(|py| {
            let cmath = pyo3::types::PyModule::import(py, "cmath").unwrap();
            let py_func = cmath.getattr("rect").unwrap();
            let py_result = py_func.call1((r, phi));

            match py_result {
                Ok(result) => {
                    use pyo3::types::PyComplexMethods;
                    let c = result.cast::<pyo3::types::PyComplex>().unwrap();
                    let py_re = c.real();
                    let py_im = c.imag();
                    match rs_result {
                        Ok(rs) => {
                            crate::cmath::tests::assert_complex_eq(
                                py_re, py_im, rs, "rect", r, phi,
                            );
                        }
                        Err(_) => {
                            panic!("rect({r}, {phi}): py=({py_re}, {py_im}) but rs returned error");
                        }
                    }
                }
                Err(_) => {
                    // CPython raised error
                    assert!(
                        rs_result.is_err(),
                        "rect({r}, {phi}): py raised error but rs succeeded"
                    );
                }
            }
        });
    }

    #[test]
    fn edgetest_rect() {
        for &r in EDGE_VALUES {
            for &phi in EDGE_VALUES {
                test_rect_impl(r, phi);
            }
        }
    }

    #[test]
    fn test_isfinite() {
        assert!(isfinite(Complex64::new(1.0, 2.0)));
        assert!(!isfinite(Complex64::new(f64::INFINITY, 0.0)));
        assert!(!isfinite(Complex64::new(0.0, f64::INFINITY)));
        assert!(!isfinite(Complex64::new(f64::NAN, 0.0)));
    }

    #[test]
    fn test_isnan() {
        assert!(!isnan(Complex64::new(1.0, 2.0)));
        assert!(!isnan(Complex64::new(f64::INFINITY, 0.0)));
        assert!(isnan(Complex64::new(f64::NAN, 0.0)));
        assert!(isnan(Complex64::new(0.0, f64::NAN)));
    }

    #[test]
    fn test_isinf() {
        assert!(!isinf(Complex64::new(1.0, 2.0)));
        assert!(isinf(Complex64::new(f64::INFINITY, 0.0)));
        assert!(isinf(Complex64::new(0.0, f64::INFINITY)));
        assert!(!isinf(Complex64::new(f64::NAN, 0.0)));
    }

    #[test]
    fn test_isclose_basic() {
        // Equal values
        assert_eq!(
            isclose(
                Complex64::new(1.0, 2.0),
                Complex64::new(1.0, 2.0),
                Some(1e-9),
                Some(0.0)
            ),
            Ok(true)
        );
        // Close values
        assert_eq!(
            isclose(
                Complex64::new(1.0, 2.0),
                Complex64::new(1.0 + 1e-10, 2.0),
                Some(1e-9),
                Some(0.0)
            ),
            Ok(true)
        );
        // Not close
        assert_eq!(
            isclose(
                Complex64::new(1.0, 2.0),
                Complex64::new(2.0, 2.0),
                Some(1e-9),
                Some(0.0)
            ),
            Ok(false)
        );
        // Infinities
        assert_eq!(
            isclose(
                Complex64::new(f64::INFINITY, 0.0),
                Complex64::new(f64::INFINITY, 0.0),
                Some(1e-9),
                Some(0.0)
            ),
            Ok(true)
        );
        assert_eq!(
            isclose(
                Complex64::new(f64::INFINITY, 0.0),
                Complex64::new(f64::NEG_INFINITY, 0.0),
                Some(1e-9),
                Some(0.0)
            ),
            Ok(false)
        );
        // Negative tolerance
        assert!(
            isclose(
                Complex64::new(1.0, 2.0),
                Complex64::new(1.0, 2.0),
                Some(-1.0),
                Some(0.0)
            )
            .is_err()
        );
    }

    proptest::proptest! {
        #[test]
        fn proptest_phase(re: f64, im: f64) {
            test_phase_impl(re, im);
        }

        #[test]
        fn proptest_polar(re: f64, im: f64) {
            test_polar_impl(re, im);
        }

        #[test]
        fn proptest_rect(r: f64, phi: f64) {
            test_rect_impl(r, phi);
        }
    }
}
