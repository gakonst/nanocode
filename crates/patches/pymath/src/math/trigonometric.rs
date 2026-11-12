//! Trigonometric and hyperbolic functions.

use crate::Result;

// Trigonometric functions

/// Return the sine of x (in radians).
#[inline]
pub fn sin(x: f64) -> Result<f64> {
    super::math_1(x, crate::m::sin, false)
}

/// Return the cosine of x (in radians).
#[inline]
pub fn cos(x: f64) -> Result<f64> {
    super::math_1(x, crate::m::cos, false)
}

/// Return the tangent of x (in radians).
#[inline]
pub fn tan(x: f64) -> Result<f64> {
    super::math_1(x, crate::m::tan, false)
}

/// Return the arc sine of x, in radians.
/// Result is in the range [-pi/2, pi/2].
#[inline]
pub fn asin(x: f64) -> Result<f64> {
    super::math_1(x, crate::m::asin, false)
}

/// Return the arc cosine of x, in radians.
/// Result is in the range [0, pi].
#[inline]
pub fn acos(x: f64) -> Result<f64> {
    super::math_1(x, crate::m::acos, false)
}

/// Return the arc tangent of x, in radians.
/// Result is in the range [-pi/2, pi/2].
#[inline]
pub fn atan(x: f64) -> Result<f64> {
    super::math_1(x, crate::m::atan, false)
}

/// Return the arc tangent of y/x, in radians.
/// Result is in the range [-pi, pi].
#[inline]
pub fn atan2(y: f64, x: f64) -> Result<f64> {
    super::math_2(y, x, crate::m::atan2)
}

// Hyperbolic functions

/// Hyperbolic sine.
#[inline]
pub fn sinh(x: f64) -> Result<f64> {
    super::math_1(x, crate::m::sinh, true)
}

/// Hyperbolic cosine.
#[inline]
pub fn cosh(x: f64) -> Result<f64> {
    super::math_1(x, crate::m::cosh, true)
}

/// Hyperbolic tangent.
#[inline]
pub fn tanh(x: f64) -> Result<f64> {
    super::math_1(x, crate::m::tanh, false)
}

/// Inverse hyperbolic sine.
#[inline]
pub fn asinh(x: f64) -> Result<f64> {
    super::math_1(x, crate::m::asinh, false)
}

/// Inverse hyperbolic cosine.
#[inline]
pub fn acosh(x: f64) -> Result<f64> {
    super::math_1(x, crate::m::acosh, false)
}

/// Inverse hyperbolic tangent.
#[inline]
pub fn atanh(x: f64) -> Result<f64> {
    super::math_1(x, crate::m::atanh, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_sin(x: f64) {
        crate::test::test_math_1(x, "sin", sin);
    }
    fn test_cos(x: f64) {
        crate::test::test_math_1(x, "cos", cos);
    }
    fn test_tan(x: f64) {
        crate::test::test_math_1(x, "tan", tan);
    }
    fn test_asin(x: f64) {
        crate::test::test_math_1(x, "asin", asin);
    }
    fn test_acos(x: f64) {
        crate::test::test_math_1(x, "acos", acos);
    }
    fn test_atan(x: f64) {
        crate::test::test_math_1(x, "atan", atan);
    }
    fn test_atan2(y: f64, x: f64) {
        crate::test::test_math_2(y, x, "atan2", atan2);
    }
    fn test_sinh(x: f64) {
        crate::test::test_math_1(x, "sinh", sinh);
    }
    fn test_cosh(x: f64) {
        crate::test::test_math_1(x, "cosh", cosh);
    }
    fn test_tanh(x: f64) {
        crate::test::test_math_1(x, "tanh", tanh);
    }
    fn test_asinh(x: f64) {
        crate::test::test_math_1(x, "asinh", asinh);
    }
    fn test_acosh(x: f64) {
        crate::test::test_math_1(x, "acosh", acosh);
    }
    fn test_atanh(x: f64) {
        crate::test::test_math_1(x, "atanh", atanh);
    }

    // Trigonometric edge tests
    #[test]
    fn edgetest_sin() {
        for &x in crate::test::EDGE_VALUES {
            test_sin(x);
        }
    }

    #[test]
    fn edgetest_cos() {
        for &x in crate::test::EDGE_VALUES {
            test_cos(x);
        }
    }

    #[test]
    fn edgetest_tan() {
        for &x in crate::test::EDGE_VALUES {
            test_tan(x);
        }
    }

    #[test]
    fn edgetest_asin() {
        for &x in crate::test::EDGE_VALUES {
            test_asin(x);
        }
    }

    #[test]
    fn edgetest_acos() {
        for &x in crate::test::EDGE_VALUES {
            test_acos(x);
        }
    }

    #[test]
    fn edgetest_atan() {
        for &x in crate::test::EDGE_VALUES {
            test_atan(x);
        }
    }

    #[test]
    fn edgetest_atan2() {
        for &y in crate::test::EDGE_VALUES {
            for &x in crate::test::EDGE_VALUES {
                test_atan2(y, x);
            }
        }
    }

    // Hyperbolic edge tests
    #[test]
    fn edgetest_tanh() {
        for &x in crate::test::EDGE_VALUES {
            test_tanh(x);
        }
    }

    #[test]
    fn edgetest_asinh() {
        for &x in crate::test::EDGE_VALUES {
            test_asinh(x);
        }
    }

    #[test]
    fn edgetest_sinh() {
        for &x in crate::test::EDGE_VALUES {
            test_sinh(x);
        }
    }

    #[test]
    fn edgetest_cosh() {
        for &x in crate::test::EDGE_VALUES {
            test_cosh(x);
        }
    }

    #[test]
    fn edgetest_acosh() {
        for &x in crate::test::EDGE_VALUES {
            test_acosh(x);
        }
    }

    #[test]
    fn edgetest_atanh() {
        for &x in crate::test::EDGE_VALUES {
            test_atanh(x);
        }
    }

    proptest::proptest! {
        #[test]
        fn proptest_sin(x: f64) {
            test_sin(x);
        }

        #[test]
        fn proptest_cos(x: f64) {
            test_cos(x);
        }

        #[test]
        fn proptest_tan(x: f64) {
            test_tan(x);
        }

        #[test]
        fn proptest_atan2(y: f64, x: f64) {
            test_atan2(y, x);
        }

        // Trigonometric proptests
        #[test]
        fn proptest_asin(x: f64) {
            test_asin(x);
        }

        #[test]
        fn proptest_acos(x: f64) {
            test_acos(x);
        }

        #[test]
        fn proptest_atan(x: f64) {
            test_atan(x);
        }

        // Hyperbolic proptests
        #[test]
        fn proptest_tanh(x: f64) {
            test_tanh(x);
        }

        #[test]
        fn proptest_asinh(x: f64) {
            test_asinh(x);
        }

        #[test]
        fn proptest_sinh(x: f64) {
            test_sinh(x);
        }

        #[test]
        fn proptest_cosh(x: f64) {
            test_cosh(x);
        }

        #[test]
        fn proptest_acosh(x: f64) {
            test_acosh(x);
        }

        #[test]
        fn proptest_atanh(x: f64) {
            test_atanh(x);
        }
    }
}
