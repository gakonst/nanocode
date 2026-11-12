//! Complex math functions matching Python's cmath module behavior.
//!
//! These implementations follow the algorithms from cmathmodule.c
//! to ensure numerical precision and correct handling of edge cases.

mod exponential;
mod misc;
mod trigonometric;

pub use exponential::{exp, log, log10, sqrt};
pub use misc::{abs, isclose, isfinite, isinf, isnan, phase, polar, rect};
pub use trigonometric::{acos, acosh, asin, asinh, atan, atanh, cos, cosh, sin, sinh, tan, tanh};

use num_complex::Complex64;

// Public constants (matching Python's cmath module)

/// The mathematical constant e = 2.718281...
pub const E: f64 = std::f64::consts::E;

/// The mathematical constant π = 3.141592...
pub const PI: f64 = std::f64::consts::PI;

/// The mathematical constant τ = 6.283185...
pub const TAU: f64 = std::f64::consts::TAU;

/// Positive infinity.
pub const INF: f64 = f64::INFINITY;

/// A floating point "not a number" (NaN) value.
pub const NAN: f64 = f64::NAN;

/// Complex number with zero real part and positive infinity imaginary part.
pub const INFJ: Complex64 = Complex64::new(0.0, f64::INFINITY);

/// Complex number with zero real part and NaN imaginary part.
pub const NANJ: Complex64 = Complex64::new(0.0, f64::NAN);

#[cfg(test)]
use crate::Result;
use crate::m;

// Shared constants

const M_LN2: f64 = core::f64::consts::LN_2;

/// Used to avoid spurious overflow in sqrt, log, inverse trig/hyperbolic functions.
const CM_LARGE_DOUBLE: f64 = f64::MAX / 4.0;
const CM_LOG_LARGE_DOUBLE: f64 = 709.0895657128241; // log(CM_LARGE_DOUBLE)

// Special value table constants
const P: f64 = core::f64::consts::PI;
const P14: f64 = 0.25 * core::f64::consts::PI;
const P12: f64 = 0.5 * core::f64::consts::PI;
const P34: f64 = 0.75 * core::f64::consts::PI;
const N: f64 = f64::NAN;
#[allow(clippy::excessive_precision)]
const U: f64 = -9.5426319407711027e33; // unlikely value, used as placeholder

/// Helper to create Complex64 in const context (for special value tables)
#[inline]
const fn c(re: f64, im: f64) -> num_complex::Complex64 {
    num_complex::Complex64::new(re, im)
}

/// Special value types for classifying doubles.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(usize)]
enum SpecialType {
    NInf = 0,  // negative infinity
    Neg = 1,   // negative finite (nonzero)
    NZero = 2, // -0.
    PZero = 3, // +0.
    Pos = 4,   // positive finite (nonzero)
    PInf = 5,  // positive infinity
    Nan = 6,   // NaN
}

/// Return special value from table if input is non-finite.
macro_rules! special_value {
    ($z:expr, $table:expr) => {
        if !$z.re.is_finite() || !$z.im.is_finite() {
            return Ok($table[special_type($z.re) as usize][special_type($z.im) as usize]);
        }
    };
}
pub(crate) use special_value;

/// Classify a double into one of seven special types.
#[inline]
fn special_type(d: f64) -> SpecialType {
    if d.is_finite() {
        if d != 0.0 {
            if m::copysign(1.0, d) == 1.0 {
                SpecialType::Pos
            } else {
                SpecialType::Neg
            }
        } else if m::copysign(1.0, d) == 1.0 {
            SpecialType::PZero
        } else {
            SpecialType::NZero
        }
    } else if d.is_nan() {
        SpecialType::Nan
    } else if m::copysign(1.0, d) == 1.0 {
        SpecialType::PInf
    } else {
        SpecialType::NInf
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Compare complex result with CPython, allowing small ULP differences for finite values.
    pub fn assert_complex_eq(py_re: f64, py_im: f64, rs: Complex64, func: &str, re: f64, im: f64) {
        let check_component = |py: f64, rs: f64, component: &str| {
            if py.is_nan() && rs.is_nan() {
                // Both NaN - OK
            } else if py.is_nan() || rs.is_nan() {
                panic!("{func}({re}, {im}).{component}: py={py} vs rs={rs} (one is NaN)",);
            } else if py.is_infinite() && rs.is_infinite() {
                // Check sign matches
                if py.is_sign_positive() != rs.is_sign_positive() {
                    panic!("{func}({re}, {im}).{component}: py={py} vs rs={rs} (sign mismatch)",);
                }
            } else if py.is_infinite() || rs.is_infinite() {
                panic!("{func}({re}, {im}).{component}: py={py} vs rs={rs} (one is infinite)",);
            } else {
                // Both finite - allow small ULP difference
                let py_bits = py.to_bits() as i64;
                let rs_bits = rs.to_bits() as i64;
                let ulp_diff = (py_bits - rs_bits).abs();
                if ulp_diff != 0 {
                    panic!(
                        "{func}({re}, {im}).{component}: py={py} (bits={:#x}) vs rs={rs} (bits={:#x}), ULP diff={ulp_diff}",
                        py.to_bits(),
                        rs.to_bits()
                    );
                }
            }
        };
        check_component(py_re, rs.re, "re");
        check_component(py_im, rs.im, "im");
    }

    pub fn test_cmath_func<F>(func_name: &str, rs_func: F, re: f64, im: f64)
    where
        F: Fn(Complex64) -> Result<Complex64>,
    {
        use pyo3::prelude::*;

        let rs_result = rs_func(Complex64::new(re, im));

        pyo3::Python::attach(|py| {
            let cmath = pyo3::types::PyModule::import(py, "cmath").unwrap();
            let py_func = cmath.getattr(func_name).unwrap();
            let py_result = py_func.call1((pyo3::types::PyComplex::from_doubles(py, re, im),));

            match py_result {
                Ok(result) => {
                    use pyo3::types::PyComplexMethods;
                    let c = result.cast::<pyo3::types::PyComplex>().unwrap();
                    let py_re = c.real();
                    let py_im = c.imag();
                    match rs_result {
                        Ok(rs) => {
                            assert_complex_eq(py_re, py_im, rs, func_name, re, im);
                        }
                        Err(e) => {
                            panic!(
                                "{func_name}({re}, {im}): py=({py_re}, {py_im}) but rs returned error {e:?}"
                            );
                        }
                    }
                }
                Err(e) => {
                    // CPython raised an exception - check we got an error too
                    if let Ok(rs) = rs_result {
                        // Some special cases may return values for domain errors in Python
                        // Check if it's a domain error
                        if e.is_instance_of::<pyo3::exceptions::PyValueError>(py) {
                            panic!(
                                "{func_name}({re}, {im}): py raised ValueError but rs=({}, {})",
                                rs.re, rs.im
                            );
                        } else if e.is_instance_of::<pyo3::exceptions::PyOverflowError>(py) {
                            panic!(
                                "{func_name}({re}, {im}): py raised OverflowError but rs=({}, {})",
                                rs.re, rs.im
                            );
                        }
                    }
                    // Both raised errors - OK
                }
            }
        });
    }
}
