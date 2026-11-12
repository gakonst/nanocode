//! Floating-point manipulation and validation functions.

use crate::{Error, Result, m};

super::libm_simple!(@1 ceil, floor, trunc);

/// Return the next floating-point value after x towards y.
///
/// If steps is provided, move that many steps towards y using O(1) bit
/// manipulation on the IEEE 754 representation. Steps that overshoot y
/// are clamped so the result never passes y.
///
/// CPython's math_nextafter_impl accepts a Python integer for steps,
/// rejects negative values, and saturates overflows to UINT64_MAX. This
/// Rust API takes `Option<u64>`, so negative rejection and big-int
/// saturation are structurally unnecessary. The caller (e.g. RustPython)
/// should handle Python int conversion and negative checks before calling.
///
/// See math_nextafter_impl in mathmodule.c.
#[inline]
pub fn nextafter(x: f64, y: f64, steps: Option<u64>) -> f64 {
    let usteps = match steps {
        None => return crate::m::nextafter(x, y),
        Some(n) => n,
    };

    if usteps == 0 || x.is_nan() {
        return x;
    }
    if y.is_nan() {
        return y;
    }

    let mut ux = x.to_bits();
    let uy = y.to_bits();
    if ux == uy {
        return x;
    }

    const SIGN_BIT: u64 = 1u64 << 63;
    let ax = ux & !SIGN_BIT;
    let ay = uy & !SIGN_BIT;

    if (ux ^ uy) & SIGN_BIT != 0 {
        // opposite signs — may need to cross zero
        // ax + ay can never overflow because bit 63 is cleared in both
        if ax + ay <= usteps {
            y
        } else if ax < usteps {
            // cross zero: remaining steps land on y's side
            f64::from_bits((uy & SIGN_BIT) | (usteps - ax))
        } else {
            ux -= usteps;
            f64::from_bits(ux)
        }
    } else if ax > ay {
        // same sign, moving toward zero
        if ax - ay >= usteps {
            ux -= usteps;
            f64::from_bits(ux)
        } else {
            y
        }
    } else {
        // same sign, moving away from zero
        if ay - ax >= usteps {
            ux += usteps;
            f64::from_bits(ux)
        } else {
            y
        }
    }
}

/// Return the absolute value of x.
#[inline]
pub fn fabs(x: f64) -> Result<f64> {
    super::math_1(x, crate::m::fabs, false)
}

/// Return a float with the magnitude of x but the sign of y.
#[inline]
pub fn copysign(x: f64, y: f64) -> crate::Result<f64> {
    super::math_2(x, y, crate::m::copysign)
}

// Validation functions

/// Return True if x is neither an infinity nor a NaN, False otherwise.
#[inline]
pub fn isfinite(x: f64) -> bool {
    x.is_finite()
}

/// Return True if x is a positive or negative infinity, False otherwise.
#[inline]
pub fn isinf(x: f64) -> bool {
    x.is_infinite()
}

/// Return True if x is a NaN, False otherwise.
#[inline]
pub fn isnan(x: f64) -> bool {
    x.is_nan()
}

/// Return True if a and b are close to each other.
///
/// Whether or not two values are considered close is determined according to
/// given absolute and relative tolerances:
/// `abs(a-b) <= max(rel_tol * max(abs(a), abs(b)), abs_tol)`
///
/// Default tolerances: rel_tol = 1e-09, abs_tol = 0.0
/// Returns Err(EDOM) if rel_tol or abs_tol is negative.
#[inline]
pub fn isclose(a: f64, b: f64, rel_tol: Option<f64>, abs_tol: Option<f64>) -> Result<bool> {
    let rel_tol = rel_tol.unwrap_or(1e-09);
    let abs_tol = abs_tol.unwrap_or(0.0);
    // Tolerances must be non-negative
    if rel_tol < 0.0 || abs_tol < 0.0 {
        return Err(Error::EDOM);
    }
    if a == b {
        return Ok(true);
    }
    if a.is_nan() || b.is_nan() {
        return Ok(false);
    }
    if a.is_infinite() || b.is_infinite() {
        return Ok(false);
    }
    let diff = (a - b).abs();
    Ok(diff <= abs_tol.max(rel_tol * a.abs().max(b.abs())))
}

/// Fused multiply-add operation: (x * y) + z.
///
/// Returns EDOM for invalid operation (NaN result from non-NaN inputs).
/// Returns ERANGE for overflow (infinite result from finite inputs).
#[inline]
pub fn fma(x: f64, y: f64, z: f64) -> Result<f64> {
    let r = x.mul_add(y, z);

    // Fast path: if we got a finite result, we're done.
    if r.is_finite() {
        return Ok(r);
    }

    // Non-finite result. Raise an exception if appropriate, else return r.
    if r.is_nan() {
        if !x.is_nan() && !y.is_nan() && !z.is_nan() {
            // NaN result from non-NaN inputs.
            return Err(Error::EDOM);
        }
    } else if x.is_finite() && y.is_finite() && z.is_finite() {
        // Infinite result from finite inputs.
        return Err(Error::ERANGE);
    }

    Ok(r)
}

/// Return the mantissa and exponent of x as (m, e).
///
/// m is a float and e is an integer such that x == m * 2**e exactly.
#[inline]
pub fn frexp(x: f64) -> (f64, i32) {
    // Handle special cases directly, to sidestep platform differences
    if x.is_nan() || x.is_infinite() || x == 0.0 {
        return (x, 0);
    }
    let mut exp: i32 = 0;
    let mantissa = m::frexp(x, &mut exp);
    (mantissa, exp)
}

/// Return x * (2**i).
///
/// Returns ERANGE if the result overflows.
#[inline]
pub fn ldexp(x: f64, i: i32) -> Result<f64> {
    // NaNs, zeros and infinities are returned unchanged
    if x == 0.0 || !x.is_finite() {
        return Ok(x);
    }
    let r = m::ldexp(x, i);
    if r.is_infinite() {
        return Err(Error::ERANGE);
    }
    Ok(r)
}

/// Return the fractional and integer parts of x.
///
/// Returns (fractional_part, integer_part).
#[inline]
pub fn modf(x: f64) -> (f64, f64) {
    // Some platforms don't do the right thing for NaNs and infinities,
    // so we take care of special cases directly.
    if x.is_infinite() {
        return (m::copysign(0.0, x), x);
    }
    if x.is_nan() {
        return (x, x);
    }
    let mut int_part: f64 = 0.0;
    let frac_part = m::modf(x, &mut int_part);
    (frac_part, int_part)
}

/// Return the remainder of x / y.
///
/// Returns EDOM if y is zero or x is infinite.
#[inline]
pub fn fmod(x: f64, y: f64) -> Result<f64> {
    // fmod(x, +/-Inf) returns x for finite x.
    if y.is_infinite() && x.is_finite() {
        return Ok(x);
    }
    let r = m::fmod(x, y);
    if r.is_nan() && !x.is_nan() && !y.is_nan() {
        return Err(Error::EDOM);
    }
    Ok(r)
}

/// Return the IEEE 754-style remainder of x with respect to y.
///
/// CPython implements this from scratch using fmod (m_remainder in
/// mathmodule.c) rather than calling the C library's remainder().
/// We delegate to libm's remainder() which is correct on all platforms
/// where it conforms to IEEE 754. If you find a platform where the
/// results differ from CPython, please file a bug.
#[inline]
pub fn remainder(x: f64, y: f64) -> Result<f64> {
    super::math_2(x, y, crate::m::remainder)
}

/// Return the value of the least significant bit of x.
#[inline]
pub fn ulp(x: f64) -> f64 {
    if x.is_nan() {
        return x;
    }
    let x = x.abs();
    if x.is_infinite() {
        return x;
    }
    let x2 = crate::m::nextafter(x, f64::INFINITY);
    if x2.is_infinite() {
        // Special case: x is the largest positive representable float
        let x2 = crate::m::nextafter(x, f64::NEG_INFINITY);
        return x - x2;
    }
    x2 - x
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Edge integer values for testing functions like ldexp
    const EDGE_INTS: &[i32] = &[0, 1, -1, 100, -100, 1024, -1024, i32::MAX, i32::MIN];

    fn test_ldexp(x: f64, i: i32) {
        use pyo3::prelude::*;

        let rs_result = ldexp(x, i);

        pyo3::Python::attach(|py| {
            let math = pyo3::types::PyModule::import(py, "math").unwrap();
            let py_func = math.getattr("ldexp").unwrap();
            let r = py_func.call1((x, i));

            match (r, &rs_result) {
                (Ok(py_val), Ok(rs_val)) => {
                    let py_f: f64 = py_val.extract().unwrap();
                    assert_eq!(
                        py_f.to_bits(),
                        rs_val.to_bits(),
                        "ldexp({x}, {i}): py={py_f} vs rs={rs_val}"
                    );
                }
                (Err(_), Err(_)) => {}
                (Ok(py_val), Err(e)) => {
                    let py_f: f64 = py_val.extract().unwrap();
                    panic!("ldexp({x}, {i}): py={py_f} but rs returned error {e:?}");
                }
                (Err(e), Ok(rs_val)) => {
                    panic!("ldexp({x}, {i}): py raised {e} but rs={rs_val}");
                }
            }
        });
    }

    fn test_frexp(x: f64) {
        use pyo3::prelude::*;

        let rs_result = frexp(x);

        pyo3::Python::attach(|py| {
            let math = pyo3::types::PyModule::import(py, "math").unwrap();
            let py_func = math.getattr("frexp").unwrap();
            let r = py_func.call1((x,));

            if let Ok(py_val) = r {
                let (py_m, py_e): (f64, i32) = py_val.extract().unwrap();
                assert_eq!(
                    py_m.to_bits(),
                    rs_result.0.to_bits(),
                    "frexp({x}) mantissa: py={py_m} vs rs={}",
                    rs_result.0
                );
                assert_eq!(
                    py_e, rs_result.1,
                    "frexp({x}) exponent: py={py_e} vs rs={}",
                    rs_result.1
                );
            }
        });
    }

    fn test_modf(x: f64) {
        use pyo3::prelude::*;

        let rs_result = modf(x);

        pyo3::Python::attach(|py| {
            let math = pyo3::types::PyModule::import(py, "math").unwrap();
            let py_func = math.getattr("modf").unwrap();
            let r = py_func.call1((x,));

            if let Ok(py_val) = r {
                let (py_frac, py_int): (f64, f64) = py_val.extract().unwrap();
                assert_eq!(
                    py_frac.to_bits(),
                    rs_result.0.to_bits(),
                    "modf({x}) frac: py={py_frac} vs rs={}",
                    rs_result.0
                );
                assert_eq!(
                    py_int.to_bits(),
                    rs_result.1.to_bits(),
                    "modf({x}) int: py={py_int} vs rs={}",
                    rs_result.1
                );
            }
        });
    }

    fn test_fmod(x: f64, y: f64) {
        crate::test::test_math_2(x, y, "fmod", fmod);
    }
    fn test_remainder(x: f64, y: f64) {
        crate::test::test_math_2(x, y, "remainder", remainder);
    }
    fn test_copysign(x: f64, y: f64) {
        crate::test::test_math_2(x, y, "copysign", copysign);
    }
    fn test_ulp(x: f64) {
        crate::test::test_math_1(x, "ulp", |x| Ok(ulp(x)));
    }

    #[test]
    fn edgetest_frexp() {
        for &x in crate::test::EDGE_VALUES {
            test_frexp(x);
        }
    }

    #[test]
    fn edgetest_ldexp() {
        for &x in crate::test::EDGE_VALUES {
            for &i in EDGE_INTS {
                test_ldexp(x, i);
            }
        }
    }

    #[test]
    fn edgetest_modf() {
        for &x in crate::test::EDGE_VALUES {
            test_modf(x);
        }
    }

    #[test]
    fn edgetest_fmod() {
        for &x in crate::test::EDGE_VALUES {
            for &y in crate::test::EDGE_VALUES {
                test_fmod(x, y);
            }
        }
    }

    #[test]
    fn edgetest_remainder() {
        for &x in crate::test::EDGE_VALUES {
            for &y in crate::test::EDGE_VALUES {
                test_remainder(x, y);
            }
        }
    }

    #[test]
    fn regression_remainder_halfway_even_cases() {
        // These cases exercise the half-way branch in CPython's custom
        // m_remainder implementation, where ties are resolved toward the
        // even multiple of y.
        let cases = [
            ((6.0, 4.0), 0xc000_0000_0000_0000_u64),  // -2.0
            ((3.0, 2.0), 0xbff0_0000_0000_0000_u64),  // -1.0
            ((5.0, 2.0), 0x3ff0_0000_0000_0000_u64),  // 1.0
            ((-6.0, 4.0), 0x4000_0000_0000_0000_u64), // 2.0
            ((-5.0, 2.0), 0xbff0_0000_0000_0000_u64), // -1.0
            ((1.5, 1.0), 0xbfe0_0000_0000_0000_u64),  // -0.5
            ((2.5, 1.0), 0x3fe0_0000_0000_0000_u64),  // 0.5
            ((3.5, 1.0), 0xbfe0_0000_0000_0000_u64),  // -0.5
            ((4.5, 1.0), 0x3fe0_0000_0000_0000_u64),  // 0.5
            ((5.5, 1.0), 0xbfe0_0000_0000_0000_u64),  // -0.5
        ];

        for &((x, y), expected_bits) in &cases {
            let r = remainder(x, y).unwrap();
            assert_eq!(
                r.to_bits(),
                expected_bits,
                "remainder({x}, {y}) = {r} ({:#x}), expected {:#x}",
                r.to_bits(),
                expected_bits
            );
            test_remainder(x, y);
        }
    }

    #[test]
    fn regression_remainder_boundary_and_signed_zero_cases() {
        // These cases pin the sign of zero and the behavior immediately
        // around the half-way boundary.
        let just_below_half = f64::from_bits(0x3fdf_ffff_ffff_ffff);
        let just_above_half = f64::from_bits(0x3fe0_0000_0000_0001);
        let cases = [
            ((4.0, 2.0), 0x0000_0000_0000_0000_u64),  // +0.0
            ((-4.0, 2.0), 0x8000_0000_0000_0000_u64), // -0.0
            ((4.0, -2.0), 0x0000_0000_0000_0000_u64), // +0.0
            ((just_below_half, 1.0), 0x3fdf_ffff_ffff_ffff_u64),
            ((just_above_half, 1.0), 0xbfdf_ffff_ffff_fffe_u64),
        ];

        for &((x, y), expected_bits) in &cases {
            let r = remainder(x, y).unwrap();
            assert_eq!(
                r.to_bits(),
                expected_bits,
                "remainder({x}, {y}) = {r} ({:#x}), expected {:#x}",
                r.to_bits(),
                expected_bits
            );
            test_remainder(x, y);
        }
    }

    #[test]
    fn edgetest_copysign() {
        for &x in crate::test::EDGE_VALUES {
            for &y in crate::test::EDGE_VALUES {
                test_copysign(x, y);
            }
        }
    }

    #[test]
    fn edgetest_ulp() {
        for &x in crate::test::EDGE_VALUES {
            test_ulp(x);
        }
    }

    proptest::proptest! {
        #[test]
        fn proptest_frexp(x: f64) {
            test_frexp(x);
        }

        #[test]
        fn proptest_ldexp(x: f64, i: i32) {
            test_ldexp(x, i);
        }

        #[test]
        fn proptest_modf(x: f64) {
            test_modf(x);
        }

        #[test]
        fn proptest_fmod(x: f64, y: f64) {
            test_fmod(x, y);
        }

        #[test]
        fn proptest_remainder(x: f64, y: f64) {
            test_remainder(x, y);
        }

        #[test]
        fn proptest_copysign(x: f64, y: f64) {
            test_copysign(x, y);
        }

        #[test]
        fn proptest_ulp(x: f64) {
            test_ulp(x);
        }
    }

    #[test]
    fn test_validation_functions() {
        // isfinite
        assert!(isfinite(0.0));
        assert!(isfinite(1.0));
        assert!(isfinite(-1.0));
        assert!(!isfinite(f64::INFINITY));
        assert!(!isfinite(f64::NEG_INFINITY));
        assert!(!isfinite(f64::NAN));

        // isinf
        assert!(!isinf(0.0));
        assert!(!isinf(1.0));
        assert!(!isinf(f64::NAN));
        assert!(isinf(f64::INFINITY));
        assert!(isinf(f64::NEG_INFINITY));

        // isnan
        assert!(!isnan(0.0));
        assert!(!isnan(1.0));
        assert!(!isnan(f64::INFINITY));
        assert!(!isnan(f64::NEG_INFINITY));
        assert!(isnan(f64::NAN));
    }

    fn test_isclose_impl(a: f64, b: f64, rel_tol: f64, abs_tol: f64) {
        use pyo3::prelude::*;

        let rs_result = isclose(a, b, Some(rel_tol), Some(abs_tol));

        pyo3::Python::attach(|py| {
            let math = pyo3::types::PyModule::import(py, "math").unwrap();
            let py_func = math.getattr("isclose").unwrap();
            let kwargs = pyo3::types::PyDict::new(py);
            kwargs.set_item("rel_tol", rel_tol).unwrap();
            kwargs.set_item("abs_tol", abs_tol).unwrap();
            let py_result = py_func.call((a, b), Some(&kwargs));

            match py_result {
                Ok(result) => {
                    let py_bool: bool = result.extract().unwrap();
                    let rs_bool = rs_result.unwrap();
                    assert_eq!(
                        py_bool, rs_bool,
                        "a = {a}, b = {b}, rel_tol = {rel_tol}, abs_tol = {abs_tol}"
                    );
                }
                Err(e) => {
                    if e.is_instance_of::<pyo3::exceptions::PyValueError>(py) {
                        assert_eq!(rs_result.err(), Some(Error::EDOM));
                    } else {
                        panic!("isclose({a}, {b}): py raised unexpected error {e}");
                    }
                }
            }
        });
    }

    #[test]
    fn test_isclose() {
        // Equal values
        test_isclose_impl(1.0, 1.0, 1e-9, 0.0);
        test_isclose_impl(0.0, 0.0, 1e-9, 0.0);
        test_isclose_impl(-1.0, -1.0, 1e-9, 0.0);

        // Close values
        test_isclose_impl(1.0, 1.0 + 1e-10, 1e-9, 0.0);
        test_isclose_impl(1.0, 1.0 + 1e-8, 1e-9, 0.0);

        // Not close values
        test_isclose_impl(1.0, 2.0, 1e-9, 0.0);
        test_isclose_impl(1.0, 1.1, 1e-9, 0.0);

        // With abs_tol
        test_isclose_impl(0.0, 1e-10, 1e-9, 1e-9);
        test_isclose_impl(0.0, 1e-8, 1e-9, 1e-9);

        // Infinities
        test_isclose_impl(f64::INFINITY, f64::INFINITY, 1e-9, 0.0);
        test_isclose_impl(f64::NEG_INFINITY, f64::NEG_INFINITY, 1e-9, 0.0);
        test_isclose_impl(f64::INFINITY, f64::NEG_INFINITY, 1e-9, 0.0);
        test_isclose_impl(f64::INFINITY, 1.0, 1e-9, 0.0);

        // NaN
        test_isclose_impl(f64::NAN, f64::NAN, 1e-9, 0.0);
        test_isclose_impl(f64::NAN, 1.0, 1e-9, 0.0);

        // Zero comparison
        test_isclose_impl(0.0, 1e-10, 1e-9, 0.0);
    }

    proptest::proptest! {
        #[test]
        fn proptest_isclose(a: f64, b: f64) {
            // Use default tolerances
            test_isclose_impl(a, b, 1e-9, 0.0);
        }
    }

    fn test_fma_impl(x: f64, y: f64, z: f64) {
        use pyo3::prelude::*;

        let rs_result = fma(x, y, z);

        pyo3::Python::attach(|py| {
            let math = pyo3::types::PyModule::import(py, "math").unwrap();
            let py_func = math.getattr("fma").unwrap();
            let py_result = py_func.call1((x, y, z));

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
                                "fma({x}, {y}, {z}): py={py_val} vs rs={rs_val}"
                            );
                        }
                        Err(e) => {
                            panic!("fma({x}, {y}, {z}): py={py_val} but rs returned error {e:?}");
                        }
                    }
                }
                Err(e) => {
                    if e.is_instance_of::<pyo3::exceptions::PyValueError>(py) {
                        assert_eq!(
                            rs_result.as_ref().err(),
                            Some(&Error::EDOM),
                            "fma({x}, {y}, {z}): py raised ValueError but rs={:?}",
                            rs_result
                        );
                    } else if e.is_instance_of::<pyo3::exceptions::PyOverflowError>(py) {
                        assert_eq!(
                            rs_result.as_ref().err(),
                            Some(&Error::ERANGE),
                            "fma({x}, {y}, {z}): py raised OverflowError but rs={:?}",
                            rs_result
                        );
                    } else {
                        panic!("fma({x}, {y}, {z}): py raised unexpected error {e}");
                    }
                }
            }
        });
    }

    #[test]
    fn test_fma() {
        // Basic tests
        test_fma_impl(2.0, 3.0, 4.0); // 2*3+4 = 10
        test_fma_impl(0.0, 0.0, 0.0);
        test_fma_impl(1.0, 1.0, 1.0);
        test_fma_impl(-1.0, 2.0, 3.0);
        // Edge cases
        test_fma_impl(f64::INFINITY, 1.0, 0.0);
        test_fma_impl(0.0, f64::INFINITY, 0.0); // 0 * inf -> NaN -> ValueError
        test_fma_impl(f64::NAN, 1.0, 0.0);
        test_fma_impl(1.0, f64::NAN, 0.0);
        test_fma_impl(1.0, 1.0, f64::NAN);
        // Overflow cases
        test_fma_impl(1e308, 10.0, 0.0); // overflow -> OverflowError
        test_fma_impl(1e308, 1.0, 1e308);
    }

    #[test]
    fn edgetest_fma() {
        for &x in crate::test::EDGE_VALUES {
            for &y in crate::test::EDGE_VALUES {
                test_fma_impl(x, y, 0.0);
                test_fma_impl(x, y, 1.0);
            }
        }
    }

    proptest::proptest! {
        #[test]
        fn proptest_fma(x: f64, y: f64, z: f64) {
            test_fma_impl(x, y, z);
        }
    }

    fn test_nextafter(x: f64, y: f64) {
        use pyo3::prelude::*;

        let rs = nextafter(x, y, None);
        pyo3::Python::attach(|py| {
            let math = pyo3::types::PyModule::import(py, "math").unwrap();
            let py_f: f64 = math
                .getattr("nextafter")
                .unwrap()
                .call1((x, y))
                .unwrap()
                .extract()
                .unwrap();
            if py_f.is_nan() && rs.is_nan() {
                return;
            }
            assert_eq!(
                py_f.to_bits(),
                rs.to_bits(),
                "nextafter({x}, {y}): py={py_f} vs rs={rs}"
            );
        });
    }

    fn test_nextafter_steps(x: f64, y: f64, steps: u64) {
        use pyo3::prelude::*;

        let rs = nextafter(x, y, Some(steps));
        pyo3::Python::attach(|py| {
            let math = pyo3::types::PyModule::import(py, "math").unwrap();
            let kwargs = pyo3::types::PyDict::new(py);
            kwargs.set_item("steps", steps).unwrap();
            let py_f: f64 = math
                .getattr("nextafter")
                .unwrap()
                .call((x, y), Some(&kwargs))
                .unwrap()
                .extract()
                .unwrap();
            if py_f.is_nan() && rs.is_nan() {
                return;
            }
            assert_eq!(
                py_f.to_bits(),
                rs.to_bits(),
                "nextafter({x}, {y}, steps={steps}): py={py_f} vs rs={rs}"
            );
        });
    }

    #[test]
    fn edgetest_nextafter() {
        for &x in crate::test::EDGE_VALUES {
            for &y in crate::test::EDGE_VALUES {
                test_nextafter(x, y);
            }
        }
    }

    #[test]
    fn edgetest_nextafter_steps() {
        let x_vals = [
            0.0,
            -0.0,
            1.0,
            -1.0,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NAN,
        ];
        let y_vals = [
            0.0,
            -0.0,
            1.0,
            -1.0,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NAN,
        ];
        let steps = [0, 1, 2, 10, 100, 1000, u64::MAX];

        for &x in &x_vals {
            for &y in &y_vals {
                for &s in &steps {
                    test_nextafter_steps(x, y, s);
                }
            }
        }
    }

    #[test]
    fn test_nextafter_steps_large() {
        // Large steps should saturate to target
        test_nextafter_steps(0.0, 1.0, u64::MAX);
        test_nextafter_steps(0.0, f64::INFINITY, u64::MAX);
        test_nextafter_steps(1.0, -1.0, u64::MAX);
        test_nextafter_steps(-1.0, 1.0, u64::MAX);

        // Steps exactly reaching a value
        // From 0.0 toward inf, 10 steps = 10 * 5e-324
        test_nextafter_steps(0.0, f64::INFINITY, 10);
        test_nextafter_steps(0.0, f64::NEG_INFINITY, 10);

        // Crossing zero
        test_nextafter_steps(5e-324, -5e-324, 1);
        test_nextafter_steps(5e-324, -5e-324, 2);
        test_nextafter_steps(5e-324, -5e-324, 3);
        test_nextafter_steps(-5e-324, 5e-324, 1);
        test_nextafter_steps(-5e-324, 5e-324, 2);
        test_nextafter_steps(-5e-324, 5e-324, 3);

        // Extreme steps that would hang with O(n) loop
        let extreme_steps: &[u64] = &[
            10u64.pow(9),
            10u64.pow(15),
            10u64.pow(18),
            u64::MAX / 2,
            u64::MAX - 1,
            u64::MAX,
        ];
        for &s in extreme_steps {
            test_nextafter_steps(0.0, 1.0, s);
            test_nextafter_steps(0.0, f64::INFINITY, s);
            test_nextafter_steps(1.0, 0.0, s);
            test_nextafter_steps(-1.0, 1.0, s);
            test_nextafter_steps(f64::MIN_POSITIVE, f64::MAX, s);
            test_nextafter_steps(f64::MAX, f64::MIN_POSITIVE, s);
        }
    }

    proptest::proptest! {
        #[test]
        fn proptest_nextafter(x: f64, y: f64) {
            test_nextafter(x, y);
        }

        #[test]
        fn proptest_nextafter_steps(x: f64, y: f64, steps: u64) {
            test_nextafter_steps(x, y, steps);
        }
    }
}
