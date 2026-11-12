//! Real number mathematical functions matching Python's math module.

// Submodules
mod aggregate;
#[cfg(feature = "_bigint")]
mod bigint;
mod exponential;
mod gamma;
#[cfg(feature = "_bigint")]
pub mod integer;
mod misc;
mod trigonometric;

// Re-export from submodules
pub use aggregate::{dist, fsum, prod, prod_int, sumprod, sumprod_int};
#[cfg(feature = "_bigint")]
pub use bigint::{comb_bigint, ldexp_bigint, log_bigint, log2_bigint, log10_bigint, perm_bigint};
pub use exponential::{cbrt, exp, exp2, expm1, log, log1p, log2, log10, pow, sqrt};
pub use gamma::{erf, erfc, gamma, lgamma};
pub use misc::{
    ceil, copysign, fabs, floor, fma, fmod, frexp, isclose, isfinite, isinf, isnan, ldexp, modf,
    nextafter, remainder, trunc, ulp,
};
pub use trigonometric::{
    acos, acosh, asin, asinh, atan, atan2, atanh, cos, cosh, sin, sinh, tan, tanh,
};

/// Simple libm wrapper macro for functions that don't need errno handling.
macro_rules! libm_simple {
    // 1-arg: (f64) -> f64
    (@1 $($name:ident),* $(,)?) => {
        $(
            #[inline]
            pub fn $name(x: f64) -> f64 {
                crate::m::$name(x)
            }
        )*
    };
    // 2-arg: (f64, f64) -> f64
    (@2 $($name:ident),* $(,)?) => {
        $(
            #[inline]
            pub fn $name(x: f64, y: f64) -> f64 {
                crate::m::$name(x, y)
            }
        )*
    };
}

pub(crate) use libm_simple;

/// Wrapper for 1-arg libm functions, corresponding to FUNC1/is_error in
/// mathmodule.c.
///
/// - isnan(r) && !isnan(x) -> domain error
/// - isinf(r) && isfinite(x) -> overflow (can_overflow=true) or domain error (can_overflow=false)
/// - isfinite(r) && errno -> check errno (unnecessary on most platforms)
///
/// CPython's approach: clear errno, call libm, then inspect both the result
/// and errno to classify errors. We rely primarily on output inspection
/// (NaN/Inf checks) because:
///
/// - On macOS and Windows, libm functions do not reliably set errno for
///   edge cases, so CPython's own is_error() skips the errno check there
///   too (it only uses it as a fallback on other Unixes).
/// - The NaN/Inf output checks are sufficient to detect all domain and
///   range errors on every platform we test against (verified by proptest
///   and edgetest against CPython via pyo3).
/// - The errno-only branch (finite result with errno set) is kept for
///   non-macOS/non-Windows Unixes where libm might signal an error
///   without producing a NaN/Inf result.
#[inline]
pub(crate) fn math_1(x: f64, func: fn(f64) -> f64, can_overflow: bool) -> crate::Result<f64> {
    crate::err::set_errno(0);
    let r = func(x);
    if r.is_nan() && !x.is_nan() {
        return Err(crate::Error::EDOM);
    }
    if r.is_infinite() && x.is_finite() {
        return Err(if can_overflow {
            crate::Error::ERANGE
        } else {
            crate::Error::EDOM
        });
    }
    // This branch unnecessary on most platforms
    #[cfg(not(any(windows, target_os = "macos")))]
    if r.is_finite() && crate::err::get_errno() != 0 {
        return crate::err::is_error(r);
    }
    Ok(r)
}

/// Wrapper for 2-arg libm functions, corresponding to FUNC2 in
/// mathmodule.c.
///
/// - isnan(r) && !isnan(x) && !isnan(y) -> domain error
/// - isinf(r) && isfinite(x) && isfinite(y) -> range error
///
/// Unlike math_1, this does not set/check errno at all. CPython's FUNC2
/// does clear and check errno, but the NaN/Inf output checks already
/// cover all error cases for the 2-arg functions we wrap (atan2, fmod,
/// copysign, remainder, pow). This is verified by bit-exact proptest
/// and edgetest against CPython.
#[inline]
pub(crate) fn math_2(x: f64, y: f64, func: fn(f64, f64) -> f64) -> crate::Result<f64> {
    let r = func(x, y);
    if r.is_nan() && !x.is_nan() && !y.is_nan() {
        return Err(crate::Error::EDOM);
    }
    if r.is_infinite() && x.is_finite() && y.is_finite() {
        return Err(crate::Error::ERANGE);
    }
    Ok(r)
}

/// math_1a: wrapper for 1-arg functions that set errno properly.
/// Used when the libm function is known to set errno correctly
/// (EDOM for invalid, ERANGE for overflow).
#[inline]
pub(crate) fn math_1a(x: f64, func: fn(f64) -> f64) -> crate::Result<f64> {
    crate::err::set_errno(0);
    let r = func(x);
    crate::err::is_error(r)
}

/// Return the Euclidean norm of n-dimensional coordinates.
///
/// Equivalent to sqrt(sum(x**2 for x in coords)).
/// Uses high-precision vector_norm algorithm for consistent results
/// across platforms and better handling of overflow/underflow.
#[inline]
pub fn hypot(coords: &[f64]) -> f64 {
    let n = coords.len();
    if n == 0 {
        return 0.0;
    }

    let mut max = 0.0_f64;
    let mut found_nan = false;
    let mut abs_coords = Vec::with_capacity(n);

    for &x in coords {
        let ax = x.abs();
        // Use is_nan() check separately to prevent compiler from
        // reordering NaN detection relative to max comparison
        if ax.is_nan() {
            found_nan = true;
        } else if ax > max {
            max = ax;
        }
        abs_coords.push(ax);
    }

    aggregate::vector_norm(&abs_coords, max, found_nan)
}

// Mathematical constants

/// The mathematical constant π = 3.141592...
pub const PI: f64 = std::f64::consts::PI;

/// The mathematical constant e = 2.718281...
pub const E: f64 = std::f64::consts::E;

/// The mathematical constant τ = 6.283185...
pub const TAU: f64 = std::f64::consts::TAU;

/// Positive infinity.
pub const INF: f64 = f64::INFINITY;

/// A floating point "not a number" (NaN) value.
pub const NAN: f64 = f64::NAN;

// Angle conversion functions

/// Convert angle x from radians to degrees.
#[inline]
pub fn degrees(x: f64) -> f64 {
    x * (180.0 / PI)
}

/// Convert angle x from degrees to radians.
#[inline]
pub fn radians(x: f64) -> f64 {
    x * (PI / 180.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pyo3::prelude::*;

    // Angle conversion tests
    fn test_degrees(x: f64) {
        let rs_result = Ok(degrees(x));

        pyo3::Python::attach(|py| {
            let math = PyModule::import(py, "math").unwrap();
            let py_func = math.getattr("degrees").unwrap();
            let r = py_func.call1((x,));
            let Some((py_result, rs_result)) = crate::test::unwrap(py, r, rs_result) else {
                return;
            };
            let py_result_repr = py_result.to_bits();
            let rs_result_repr = rs_result.to_bits();
            assert_eq!(
                py_result_repr, rs_result_repr,
                "x = {x}, py_result = {py_result}, rs_result = {rs_result}"
            );
        });
    }

    fn test_radians(x: f64) {
        let rs_result = Ok(radians(x));

        pyo3::Python::attach(|py| {
            let math = PyModule::import(py, "math").unwrap();
            let py_func = math.getattr("radians").unwrap();
            let r = py_func.call1((x,));
            let Some((py_result, rs_result)) = crate::test::unwrap(py, r, rs_result) else {
                return;
            };
            let py_result_repr = py_result.to_bits();
            let rs_result_repr = rs_result.to_bits();
            assert_eq!(
                py_result_repr, rs_result_repr,
                "x = {x}, py_result = {py_result}, rs_result = {rs_result}"
            );
        });
    }

    #[test]
    fn edgetest_degrees() {
        for &x in crate::test::EDGE_VALUES {
            test_degrees(x);
        }
    }

    #[test]
    fn edgetest_radians() {
        for &x in crate::test::EDGE_VALUES {
            test_radians(x);
        }
    }

    // Constants test
    #[test]
    fn test_constants() {
        assert_eq!(PI, std::f64::consts::PI);
        assert_eq!(E, std::f64::consts::E);
        assert_eq!(TAU, std::f64::consts::TAU);
        assert!(INF.is_infinite() && INF > 0.0);
        assert!(NAN.is_nan());
    }

    // hypot tests - n-dimensional
    fn test_hypot(coords: &[f64]) {
        let rs_result = hypot(coords);

        pyo3::Python::attach(|py| {
            let math = PyModule::import(py, "math").unwrap();
            let py_func = math.getattr("hypot").unwrap();
            let py_args = pyo3::types::PyTuple::new(py, coords).unwrap();
            let py_result: f64 = py_func.call1(py_args).unwrap().extract().unwrap();

            if py_result.is_nan() && rs_result.is_nan() {
                return;
            }
            assert_eq!(
                py_result.to_bits(),
                rs_result.to_bits(),
                "hypot({:?}): py={} vs rs={}",
                coords,
                py_result,
                rs_result
            );
        });
    }

    #[test]
    fn test_hypot_basic() {
        test_hypot(&[3.0, 4.0]); // 5.0
        test_hypot(&[1.0, 2.0, 2.0]); // 3.0
        test_hypot(&[]); // 0.0
        test_hypot(&[5.0]); // 5.0
    }

    #[test]
    fn edgetest_hypot() {
        for &x in crate::test::EDGE_VALUES {
            for &y in crate::test::EDGE_VALUES {
                test_hypot(&[x, y]);
            }
        }
    }

    proptest::proptest! {
        #[test]
        fn proptest_degrees(x: f64) {
            test_degrees(x);
        }

        #[test]
        fn proptest_radians(x: f64) {
            test_radians(x);
        }

        #[test]
        fn proptest_hypot(x: f64, y: f64) {
            test_hypot(&[x, y]);
        }
    }
}
