// Public modules
#[cfg(feature = "complex")]
pub mod cmath;
pub mod math;

// Internal modules
mod err;
// Native libm via FFI (unix/windows)
#[cfg(any(unix, windows))]
pub(crate) mod m;
// Pure Rust libm for WASM and other targets
#[cfg(not(any(unix, windows)))]
#[path = "m_rust.rs"]
pub(crate) mod m;
#[cfg(any(unix, windows))]
mod m_sys;
#[cfg(test)]
mod test;

// Re-export error types at root level
pub use err::{Error, Result};

/// Fused multiply-add operation.
/// When `mul_add` feature is enabled, uses hardware FMA instruction.
/// Otherwise, uses separate multiply and add operations.
#[inline(always)]
pub(crate) fn mul_add(a: f64, b: f64, c: f64) -> f64 {
    if cfg!(feature = "mul_add") {
        a.mul_add(b, c)
    } else {
        a * b + c
    }
}

macro_rules! pyo3_proptest {
    ($fn_name:ident(Result<_>), $test_name:ident, $proptest_name:ident, $edgetest_name:ident) => {
        #[cfg(test)]
        fn $test_name(x: f64) {
            use pyo3::prelude::*;

            let rs_result = $fn_name(x);

            pyo3::Python::attach(|py| {
                let math = PyModule::import(py, "math").unwrap();
                let py_func = math
                    .getattr(stringify!($fn_name))
                    .unwrap();
                let r = py_func.call1((x,));
                let Some((py_result, rs_result)) = crate::test::unwrap(py, r, rs_result) else {
                    return;
                };
                let py_result_repr = py_result.to_bits();
                let rs_result_repr = rs_result.to_bits();
                assert_eq!(py_result_repr, rs_result_repr, "x = {x}, py_result = {py_result}, rs_result = {rs_result}");
            });
        }

        crate::pyo3_proptest!(@proptest, $test_name, $proptest_name);
        crate::pyo3_proptest!(@edgetest, $test_name, $edgetest_name);
    };
    ($fn_name:ident(_), $test_name:ident, $proptest_name:ident, $edgetest_name:ident) => {
        #[cfg(test)]
        fn $test_name(x: f64) {
            use pyo3::prelude::*;

            let rs_result = Ok($fn_name(x));

            pyo3::Python::attach(|py| {
                let math = PyModule::import(py, "math").unwrap();
                let py_func = math
                    .getattr(stringify!($fn_name))
                    .unwrap();
                let r = py_func.call1((x,));
                let Some((py_result, rs_result)) = crate::test::unwrap(py, r, rs_result) else {
                    return;
                };
                let py_result_repr = py_result.to_bits();
                let rs_result_repr = rs_result.to_bits();
                assert_eq!(py_result_repr, rs_result_repr, "x = {x}, py_result = {py_result}, rs_result = {rs_result}");
            });
        }
        crate::pyo3_proptest!(@proptest, $test_name, $proptest_name);
    };
    (@proptest, $test_name:ident, $proptest_name:ident) => {
        #[cfg(test)]
        proptest::proptest! {
            #[test]
            fn $proptest_name(x: f64) {
                $test_name(x);
            }
        }
    };
    (@edgetest, $test_name:ident, $edgetest_name:ident) => {
        #[test]
        fn $edgetest_name() {
            $test_name(f64::MIN);
            $test_name(-f64::MIN);
            $test_name(f64::NAN);
            $test_name(-f64::NAN);
            $test_name(f64::INFINITY);
            $test_name(-f64::NEG_INFINITY);
            $test_name(0.0);
            $test_name(-0.0);
        }
    };
}

use pyo3_proptest;
