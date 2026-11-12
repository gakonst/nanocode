// Allow excessive precision in edge values - these are intentional test cases
#![allow(clippy::excessive_precision)]

use crate::Error;
use pyo3::{Python, prelude::*};

/// Edge values for testing floating-point functions.
/// Includes: zeros, infinities, various NaNs, subnormals, and values at different scales.
pub(crate) const EDGE_VALUES: &[f64] = &[
    // Zeros
    0.0,
    -0.0,
    // Infinities
    f64::INFINITY,
    f64::NEG_INFINITY,
    // Standard NaNs
    f64::NAN,
    -f64::NAN,
    // Additional NaN with different payload (quiet NaN with payload 1)
    f64::from_bits(0x7FF8_0000_0000_0001_u64),
    // Signaling NaN (sNaN) - may trigger FP exceptions on some platforms
    f64::from_bits(0x7FF0_0000_0000_0001_u64),
    // Subnormal (denormalized) values
    f64::MIN_POSITIVE * 0.5,
    -f64::MIN_POSITIVE * 0.5,
    5e-324,
    -5e-324,
    // Boundary values
    f64::MIN_POSITIVE,
    f64::MIN_POSITIVE * 2.0,
    f64::MAX,
    f64::MIN,
    // Near-infinity large values
    f64::MAX * 0.5,
    -f64::MAX * 0.5,
    1e308,
    -1e308,
    // Overflow/underflow thresholds for exp
    710.0,
    709.782712893384,
    -745.0,
    -745.1332191019411,
    // Small scale
    1e-10,
    -1e-10,
    1e-300,
    -1e-300,
    1e-308,
    -1e-308,
    // Normal scale
    1.0,
    -1.0,
    0.5,
    -0.5,
    2.0,
    -2.0,
    1.5,
    -1.5,
    3.0, // for cbrt
    -3.0,
    // Values near 1.0 (log, expm1, log1p, acosh boundary)
    1.0 - 1e-15,
    1.0 + 1e-15,
    f64::EPSILON,
    1.0 - f64::EPSILON,
    1.0 + f64::EPSILON,
    // asin/acos domain boundaries [-1, 1]
    1.0000000000000002, // just outside domain (1 + eps)
    -1.0000000000000002,
    // atanh domain boundaries (-1, 1)
    0.9999999999999999, // just inside domain
    -0.9999999999999999,
    // log1p domain boundary (> -1)
    -0.9999999999999999, // just above -1
    -1.0 + 1e-15,        // very close to -1
    -1.0000000000000002, // just below -1
    // gamma/lgamma poles (negative integers)
    -1.0,
    -2.0,
    -3.0,
    -0.5, // gamma(-0.5) = -2*sqrt(pi)
    // Mathematical constants
    std::f64::consts::E,
    std::f64::consts::LN_2,
    std::f64::consts::LOG10_E,
    // Trigonometric special values
    std::f64::consts::PI,
    -std::f64::consts::PI,
    std::f64::consts::FRAC_PI_2,
    -std::f64::consts::FRAC_PI_2,
    std::f64::consts::FRAC_PI_4,
    -std::f64::consts::FRAC_PI_4,
    std::f64::consts::TAU,
    1.5 * std::f64::consts::PI, // 3π/2
    // Large values for trig (precision loss)
    1e15,
    -1e15,
    // Near-integer values (ceil, floor, trunc, round)
    0.49999999999999994,
    0.50000000000000006,
    -0.49999999999999994,
    -0.50000000000000006,
];

pub(crate) fn unwrap<'py>(
    py: Python<'py>,
    py_v: PyResult<Bound<'py, PyAny>>,
    v: Result<f64, crate::Error>,
) -> Option<(f64, f64)> {
    match py_v {
        Ok(py_v) => {
            let py_v: f64 = py_v.extract().expect("failed to extract");
            Some((py_v, v.unwrap()))
        }
        Err(e) => {
            if e.is_instance_of::<pyo3::exceptions::PyValueError>(py) {
                assert_eq!(v.err(), Some(Error::EDOM));
            } else if e.is_instance_of::<pyo3::exceptions::PyOverflowError>(py) {
                assert_eq!(v.err(), Some(Error::ERANGE));
            } else {
                panic!();
            }
            None
        }
    }
}

/// Test a 1-argument function that returns Result<f64>
pub(crate) fn test_math_1(x: f64, func_name: &str, rs_func: impl Fn(f64) -> crate::Result<f64>) {
    let rs_result = rs_func(x);

    pyo3::Python::attach(|py| {
        let math = pyo3::types::PyModule::import(py, "math").unwrap();
        let py_func = math.getattr(func_name).unwrap();
        let r = py_func.call1((x,));
        let Some((py_result, rs_result)) = unwrap(py, r, rs_result) else {
            return;
        };
        if py_result.is_nan() && rs_result.is_nan() {
            return;
        }
        assert_eq!(
            py_result.to_bits(),
            rs_result.to_bits(),
            "{func_name}({x}): py={py_result} vs rs={rs_result}"
        );
    });
}

/// Run a test with Python math module
pub(crate) fn with_py_math<F, R>(f: F) -> R
where
    F: FnOnce(Python, &pyo3::Bound<pyo3::types::PyModule>) -> R,
{
    pyo3::Python::attach(|py| {
        let math = pyo3::types::PyModule::import(py, "math").unwrap();
        f(py, &math)
    })
}

/// Assert two f64 values are equal (handles NaN)
pub(crate) fn assert_f64_eq(py: f64, rs: f64, context: impl std::fmt::Display) {
    if py.is_nan() && rs.is_nan() {
        return;
    }
    assert_eq!(
        py.to_bits(),
        rs.to_bits(),
        "{}: py={} vs rs={}",
        context,
        py,
        rs
    );
}

/// Test a 2-argument function that returns Result<f64>
pub(crate) fn test_math_2(
    x: f64,
    y: f64,
    func_name: &str,
    rs_func: impl Fn(f64, f64) -> crate::Result<f64>,
) {
    let rs_result = rs_func(x, y);

    pyo3::Python::attach(|py| {
        let math = pyo3::types::PyModule::import(py, "math").unwrap();
        let py_func = math.getattr(func_name).unwrap();
        let r = py_func.call1((x, y));

        match r {
            Ok(py_val) => {
                let py_f: f64 = py_val.extract().unwrap();
                let rs_val = rs_result.unwrap_or_else(|e| {
                    panic!("{func_name}({x}, {y}): py={py_f} but rs returned error {e:?}")
                });
                if py_f.is_nan() && rs_val.is_nan() {
                    return;
                }
                assert_eq!(
                    py_f.to_bits(),
                    rs_val.to_bits(),
                    "{func_name}({x}, {y}): py={py_f} vs rs={rs_val}"
                );
            }
            Err(e) => {
                // Check error type matches
                let rs_err = rs_result.as_ref().err();
                if e.is_instance_of::<pyo3::exceptions::PyValueError>(py) {
                    assert_eq!(
                        rs_err,
                        Some(&Error::EDOM),
                        "{func_name}({x}, {y}): py raised ValueError but rs={rs_err:?}"
                    );
                } else if e.is_instance_of::<pyo3::exceptions::PyOverflowError>(py) {
                    assert_eq!(
                        rs_err,
                        Some(&Error::ERANGE),
                        "{func_name}({x}, {y}): py raised OverflowError but rs={rs_err:?}"
                    );
                } else {
                    panic!("{func_name}({x}, {y}): py raised unexpected error {e}");
                }
            }
        }
    });
}
