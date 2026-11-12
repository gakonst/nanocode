//! Exponential, logarithmic, and power functions.

use crate::Result;

// Exponential functions

/// Return e raised to the power of x.
#[inline]
pub fn exp(x: f64) -> Result<f64> {
    super::math_1(x, crate::m::exp, true)
}

/// Return 2 raised to the power of x.
#[inline]
pub fn exp2(x: f64) -> Result<f64> {
    super::math_1(x, crate::m::exp2, true)
}

/// Return exp(x) - 1.
#[inline]
pub fn expm1(x: f64) -> Result<f64> {
    super::math_1(x, crate::m::expm1, true)
}

// Logarithmic functions

/// m_log: log implementation
#[inline]
fn m_log(x: f64) -> f64 {
    if x.is_finite() {
        if x > 0.0 {
            crate::m::log(x)
        } else if x == 0.0 {
            f64::NEG_INFINITY // log(0) = -inf
        } else {
            f64::NAN // log(-ve) = nan
        }
    } else if x.is_nan() || x > 0.0 {
        x // log(nan) = nan, log(inf) = inf
    } else {
        f64::NAN // log(-inf) = nan
    }
}

/// m_log10: log10 implementation
#[inline]
fn m_log10(x: f64) -> f64 {
    if x.is_finite() {
        if x > 0.0 {
            crate::m::log10(x)
        } else if x == 0.0 {
            f64::NEG_INFINITY // log10(0) = -inf
        } else {
            f64::NAN // log10(-ve) = nan
        }
    } else if x.is_nan() || x > 0.0 {
        x // log10(nan) = nan, log10(inf) = inf
    } else {
        f64::NAN // log10(-inf) = nan
    }
}

/// m_log2: log2 implementation
#[inline]
fn m_log2(x: f64) -> f64 {
    if !x.is_finite() {
        if x.is_nan() || x > 0.0 {
            x // log2(nan) = nan, log2(+inf) = +inf
        } else {
            f64::NAN // log2(-inf) = nan
        }
    } else if x > 0.0 {
        crate::m::log2(x)
    } else if x == 0.0 {
        f64::NEG_INFINITY // log2(0) = -inf
    } else {
        f64::NAN // log2(-ve) = nan
    }
}

/// m_log1p: CPython's m_log1p implementation
#[inline]
fn m_log1p(x: f64) -> f64 {
    // For x > -1, log1p is well-defined
    // For x == -1, result is -inf
    // For x < -1, result is nan
    if x.is_nan() {
        return x;
    }
    crate::m::log1p(x)
}

/// Return the logarithm of x to the given base.
#[inline]
pub fn log(x: f64, base: Option<f64>) -> Result<f64> {
    let num = m_log(x);

    // math_1 logic: check for domain errors
    if num.is_nan() && !x.is_nan() {
        return Err(crate::Error::EDOM);
    }
    if num.is_infinite() && x.is_finite() {
        return Err(crate::Error::EDOM);
    }

    match base {
        None => Ok(num),
        Some(b) => {
            let den = m_log(b);
            if den.is_nan() && !b.is_nan() {
                return Err(crate::Error::EDOM);
            }
            if den.is_infinite() && b.is_finite() {
                return Err(crate::Error::EDOM);
            }
            // log(x, 1) -> division by zero.
            // CPython raises ZeroDivisionError here (via PyNumber_TrueDivide),
            // but we return EDOM since our error type has no ZeroDivisionError
            // variant. The caller (e.g. RustPython) may remap this if needed.
            if den == 0.0 {
                return Err(crate::Error::EDOM);
            }
            Ok(num / den)
        }
    }
}

/// math_1 for Rust functions (m_log10, m_log2, m_log1p)
#[inline]
fn math_1_fn(x: f64, func: fn(f64) -> f64, can_overflow: bool) -> Result<f64> {
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
    Ok(r)
}

/// Return the base-10 logarithm of x.
#[inline]
pub fn log10(x: f64) -> Result<f64> {
    math_1_fn(x, m_log10, false)
}

/// Return the base-2 logarithm of x.
#[inline]
pub fn log2(x: f64) -> Result<f64> {
    math_1_fn(x, m_log2, false)
}

/// Return the natural logarithm of 1+x (base e).
#[inline]
pub fn log1p(x: f64) -> Result<f64> {
    math_1_fn(x, m_log1p, false)
}

// Power functions

/// Return the square root of x.
#[inline]
pub fn sqrt(x: f64) -> Result<f64> {
    super::math_1(x, crate::m::sqrt, false)
}

/// Return the cube root of x.
#[inline]
pub fn cbrt(x: f64) -> Result<f64> {
    super::math_1(x, crate::m::cbrt, false)
}

/// Return x raised to the power y.
#[inline]
pub fn pow(x: f64, y: f64) -> Result<f64> {
    // Deal directly with IEEE specials, to cope with problems on various
    // platforms whose semantics don't exactly match C99
    if !x.is_finite() || !y.is_finite() {
        if x.is_nan() {
            // NaN**0 = 1
            return Ok(if y == 0.0 { 1.0 } else { x });
        } else if y.is_nan() {
            // 1**NaN = 1
            return Ok(if x == 1.0 { 1.0 } else { y });
        } else if x.is_infinite() {
            let odd_y = y.is_finite() && crate::m::fmod(y.abs(), 2.0) == 1.0;
            if y > 0.0 {
                return Ok(if odd_y { x } else { x.abs() });
            } else if y == 0.0 {
                return Ok(1.0);
            } else {
                // y < 0
                return Ok(if odd_y {
                    crate::m::copysign(0.0, x)
                } else {
                    0.0
                });
            }
        } else {
            // y is infinite
            debug_assert!(y.is_infinite());
            if x.abs() == 1.0 {
                return Ok(1.0);
            } else if y > 0.0 && x.abs() > 1.0 {
                return Ok(y);
            } else if y < 0.0 && x.abs() < 1.0 {
                return Ok(-y); // result is +inf
            } else {
                return Ok(0.0);
            }
        }
    }

    // Let libm handle finite**finite
    let r = crate::m::pow(x, y);

    // A NaN result should arise only from (-ve)**(finite non-integer);
    // in this case we want to raise ValueError.
    if !r.is_finite() {
        if r.is_nan() {
            return Err(crate::Error::EDOM);
        } else if r.is_infinite() {
            // An infinite result here arises either from:
            // (A) (+/-0.)**negative (-> divide-by-zero)
            // (B) overflow of x**y with x and y finite
            if x == 0.0 {
                return Err(crate::Error::EDOM);
            } else {
                return Err(crate::Error::ERANGE);
            }
        }
    }

    Ok(r)
}

// Tests

#[cfg(test)]
mod tests {
    use super::*;

    fn test_exp(x: f64) {
        crate::test::test_math_1(x, "exp", exp);
    }
    fn test_exp2(x: f64) {
        crate::test::test_math_1(x, "exp2", exp2);
    }
    fn test_expm1(x: f64) {
        crate::test::test_math_1(x, "expm1", expm1);
    }
    fn test_log_n(x: f64) {
        crate::test::test_math_1(x, "log", |x| log(x, None));
    }
    fn test_log(x: f64, base: f64) {
        crate::test::test_math_2(x, base, "log", |x, b| log(x, Some(b)));
    }
    fn test_log10(x: f64) {
        crate::test::test_math_1(x, "log10", log10);
    }
    fn test_log2(x: f64) {
        crate::test::test_math_1(x, "log2", log2);
    }
    fn test_log1p(x: f64) {
        crate::test::test_math_1(x, "log1p", log1p);
    }
    fn test_sqrt(x: f64) {
        crate::test::test_math_1(x, "sqrt", sqrt);
    }
    fn test_cbrt(x: f64) {
        crate::test::test_math_1(x, "cbrt", cbrt);
    }
    fn test_pow(x: f64, y: f64) {
        crate::test::test_math_2(x, y, "pow", pow);
    }

    proptest::proptest! {
        #[test]
        fn proptest_exp(x: f64) {
            test_exp(x);
        }

        #[test]
        fn proptest_exp2(x: f64) {
            test_exp2(x);
        }

        #[test]
        fn proptest_sqrt(x: f64) {
            test_sqrt(x);
        }

        #[test]
        fn proptest_cbrt(x: f64) {
            test_cbrt(x);
        }

        #[test]
        fn proptest_expm1(x: f64) {
            test_expm1(x);
        }

        #[test]
        fn proptest_log_n(x: f64) {
            test_log_n(x);
        }

        #[test]
        fn proptest_log(x: f64, base: f64) {
            test_log(x, base);
        }

        #[test]
        fn proptest_log10(x: f64) {
            test_log10(x);
        }

        #[test]
        fn proptest_log2(x: f64) {
            test_log2(x);
        }

        #[test]
        fn proptest_log1p(x: f64) {
            test_log1p(x);
        }

        #[test]
        fn proptest_pow(x: f64, y: f64) {
            test_pow(x, y);
        }
    }

    #[test]
    fn edgetest_exp() {
        for &x in crate::test::EDGE_VALUES {
            test_exp(x);
        }
    }

    #[test]
    fn edgetest_exp2() {
        for &x in crate::test::EDGE_VALUES {
            test_exp2(x);
        }
    }

    #[test]
    fn edgetest_expm1() {
        for &x in crate::test::EDGE_VALUES {
            test_expm1(x);
        }
    }

    #[test]
    fn edgetest_sqrt() {
        for &x in crate::test::EDGE_VALUES {
            test_sqrt(x);
        }
    }

    #[test]
    fn edgetest_cbrt() {
        for &x in crate::test::EDGE_VALUES {
            test_cbrt(x);
        }
    }

    #[test]
    fn edgetest_log_n() {
        for &x in crate::test::EDGE_VALUES {
            test_log_n(x);
        }
    }

    #[test]
    fn edgetest_log10() {
        for &x in crate::test::EDGE_VALUES {
            test_log10(x);
        }
    }

    #[test]
    fn edgetest_log2() {
        for &x in crate::test::EDGE_VALUES {
            test_log2(x);
        }
    }

    #[test]
    fn edgetest_log1p() {
        for &x in crate::test::EDGE_VALUES {
            test_log1p(x);
        }
    }

    #[test]
    fn edgetest_pow() {
        for &x in crate::test::EDGE_VALUES {
            for &y in crate::test::EDGE_VALUES {
                test_pow(x, y);
            }
        }
    }
}
