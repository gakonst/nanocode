//! BigInt helper functions for RustPython.
//!
//! These functions handle cases where Python integers exceed i64 range.
//! They are not part of Python's math.integer module but are needed
//! for RustPython's internal implementation.

use super::integer::perm_comb_small;
#[cfg(feature = "malachite-bigint")]
pub(crate) use malachite_bigint::{BigInt, BigUint};
#[cfg(feature = "num-bigint")]
pub(crate) use num_bigint::{BigInt, BigUint};
use num_traits::{One, Signed, ToPrimitive};

// Perm/Comb for BigInt n

/// Compute perm(n, k) where n is a BigInt and k fits in u64.
/// Uses divide-and-conquer: P(n, k) = P(n, j) * P(n-j, k-j)
///
/// See: perm_comb in CPython mathmodule.c
pub fn perm_bigint(n: &BigInt, k: u64) -> BigUint {
    if k == 0 {
        return BigUint::one();
    }
    if k == 1 {
        return n.magnitude().clone();
    }

    // P(n, k) = P(n, j) * P(n-j, k-j)
    let j = k / 2;
    let a = perm_bigint(n, j);
    let n_minus_j = n - BigInt::from(j);
    let b = perm_bigint(&n_minus_j, k - j);
    a * b
}

/// Compute comb(n, k) where n is a BigInt and k fits in u64.
/// Uses divide-and-conquer: C(n, k) = C(n, j) * C(n-j, k-j) / C(k, j)
///
/// See: perm_comb in CPython mathmodule.c
pub fn comb_bigint(n: &BigInt, k: u64) -> BigUint {
    if k == 0 {
        return BigUint::one();
    }
    if k == 1 {
        return n.magnitude().clone();
    }

    // C(n, k) = C(n, j) * C(n-j, k-j) / C(k, j)
    let j = k / 2;
    let a = comb_bigint(n, j);
    let n_minus_j = n - BigInt::from(j);
    let b = comb_bigint(&n_minus_j, k - j);
    let numerator = a * b;
    // C(k, j) using small version since k fits in u64
    let divisor = perm_comb_small(k, j, true);
    numerator / divisor
}

// Logarithm functions for BigInt

/// Compute frexp-like decomposition for BigInt.
/// Returns (mantissa, exponent) where:
/// - mantissa is in [0.5, 1.0) for positive n
/// - n ~= mantissa * 2^exponent
///
/// `_PyLong_Frexp` extracts digits one-by-one into a fixed-size
/// accumulator and applies a `half_even_correction` lookup table for
/// rounding.  We instead extract the top 55 bits via a single right
/// shift and use a sticky-bit to mark whether any discarded bits were
/// non-zero, then delegate to `BigInt::to_f64()` which performs
/// IEEE 754 round-half-to-even.  The two approaches are equivalent
/// because the sticky bit preserves the same rounding information
/// that the digit-by-digit extraction would.
fn frexp_bigint(n: &BigInt) -> (f64, i64) {
    let bits = n.bits();
    if bits == 0 {
        return (0.0, 0);
    }

    let bits = bits as i64;

    // For small integers that fit in f64 mantissa (53 bits)
    if bits <= 53 {
        let x = n.to_f64().unwrap();
        // frexp returns (m, e) where x = m * 2^e and 0.5 <= |m| < 1
        let mut e: i32 = 0;
        let m = crate::m::frexp(x, &mut e);
        return (m, e as i64);
    }

    // For large integers, extract top DBL_MANT_DIG + 2 = 55 bits for rounding
    let shift = bits - 55;
    let mut mantissa_int = n >> shift as u64;

    // Sticky bit: if any shifted-out bits were non-zero, set the LSB.
    // This ensures correct IEEE round-half-to-even when converting to f64.
    //
    // `_PyLong_Frexp` checks the remainder from `v_rshift` first, then
    // iterates shifted-out digits top-down.  We use `trailing_zeros()`
    // which scans digits bottom-up instead.  The worst-case traversal
    // order differs (e.g. exact powers of two), but for typical inputs
    // both terminate in O(1).  If you observe a performance regression
    // from this, please file a bug report.
    let tz = n.magnitude().trailing_zeros().unwrap(); // n != 0 here
    if tz < shift as u64 {
        mantissa_int |= BigInt::from(1);
    }

    let mut x = mantissa_int.to_f64().unwrap();

    // x is now approximately n / 2^shift, with ~55 bits of precision
    // Scale to [0.5, 1.0) range
    // x is in [2^54, 2^55), so divide by 2^55 to get [0.5, 1.0)
    x /= (1u64 << 55) as f64;

    // Adjust if rounding pushed us to 1.0
    if x == 1.0 {
        x = 0.5;
        return (x, bits + 1);
    }

    (x, bits)
}

/// Return the natural logarithm of a BigInt.
///
/// Returns Err(EDOM) if n is not positive.
pub fn log_bigint(n: &BigInt, base: Option<f64>) -> crate::Result<f64> {
    if !n.is_positive() {
        return Err(crate::Error::EDOM);
    }

    // Try direct conversion first
    if let Some(x) = n.to_f64()
        && x.is_finite()
    {
        return super::log(x, base);
    }

    // Use frexp decomposition for large values
    // n ~= x * 2^e, so log(n) = log(x) + log(2) * e
    let (x, e) = frexp_bigint(n);
    let log_n = crate::mul_add(crate::m::log(2.0), e as f64, crate::m::log(x));

    match base {
        None => Ok(log_n),
        Some(b) => {
            if b <= 0.0 || b == 1.0 {
                return Err(crate::Error::EDOM);
            }
            Ok(log_n / crate::m::log(b))
        }
    }
}

/// Return the base-2 logarithm of a BigInt.
///
/// Returns Err(EDOM) if n is not positive.
pub fn log2_bigint(n: &BigInt) -> crate::Result<f64> {
    if !n.is_positive() {
        return Err(crate::Error::EDOM);
    }

    // Try direct conversion first
    if let Some(x) = n.to_f64()
        && x.is_finite()
    {
        return super::log2(x);
    }

    // Use frexp decomposition for large values
    // n ~= x * 2^e, so log2(n) = log2(x) + e
    let (x, e) = frexp_bigint(n);
    Ok(crate::mul_add(
        crate::m::log2(2.0),
        e as f64,
        crate::m::log2(x),
    ))
}

/// Return the base-10 logarithm of a BigInt.
///
/// Returns Err(EDOM) if n is not positive.
pub fn log10_bigint(n: &BigInt) -> crate::Result<f64> {
    if !n.is_positive() {
        return Err(crate::Error::EDOM);
    }

    // Try direct conversion first
    if let Some(x) = n.to_f64()
        && x.is_finite()
    {
        return super::log10(x);
    }

    // Use frexp decomposition for large values
    // n ~= x * 2^e, so log10(n) = log10(x) + log10(2) * e
    let (x, e) = frexp_bigint(n);
    Ok(crate::mul_add(
        crate::m::log10(2.0),
        e as f64,
        crate::m::log10(x),
    ))
}

/// Compute ldexp(x, exp) where exp is a BigInt.
///
/// Returns x * 2^exp, handling BigInt exponent overflow:
/// - If x is 0, inf, or nan, returns x unchanged
/// - If exp overflows i32 positively, returns ERANGE (overflow)
/// - If exp overflows i32 negatively, returns signed zero (underflow)
pub fn ldexp_bigint(x: f64, exp: &BigInt) -> crate::Result<f64> {
    // Special values are returned unchanged regardless of exponent
    if x == 0.0 || !x.is_finite() {
        return Ok(x);
    }

    // Fast path: try i64 first (like CPython's PyLong_AsLongAndOverflow)
    let exp_clamped: i64 = match exp.try_into() {
        Ok(e) => e,
        Err(_) => {
            // Exponent overflows i64, clamp to i64::MIN/MAX
            if exp.is_negative() {
                i64::MIN
            } else {
                i64::MAX
            }
        }
    };

    // Check against i32 bounds
    if exp_clamped > i32::MAX as i64 {
        // overflow
        Err(crate::Error::ERANGE)
    } else if exp_clamped < i32::MIN as i64 {
        // underflow to signed zero
        Ok(if x.is_sign_negative() { -0.0 } else { 0.0 })
    } else {
        super::ldexp(x, exp_clamped as i32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pyo3::prelude::*;

    fn test_log_bigint_impl(n: &BigInt, base: Option<f64>) {
        let rs = log_bigint(n, base);
        crate::test::with_py_math(|py, math| {
            // Convert BigInt to Python int via string using builtins.int()
            let n_str = n.to_string();
            let builtins = pyo3::types::PyModule::import(py, "builtins").unwrap();
            let py_n = builtins
                .getattr("int")
                .unwrap()
                .call1((n_str.as_str(),))
                .unwrap();
            let py_result = match base {
                Some(b) => math.getattr("log").unwrap().call1((py_n, b)),
                None => math.getattr("log").unwrap().call1((py_n,)),
            };
            match py_result {
                Ok(py_val) => {
                    let py_f: f64 = py_val.extract().unwrap();
                    let rs_f = rs.unwrap();
                    if py_f.is_nan() && rs_f.is_nan() {
                        return;
                    }
                    // Handle exact equality (e.g., log(1) = 0)
                    if py_f == rs_f {
                        return;
                    }
                    // Allow small relative error for large numbers
                    let rel_err = ((py_f - rs_f) / py_f).abs();
                    assert!(
                        rel_err < 1e-10,
                        "log_bigint({n}, {base:?}): py={py_f} vs rs={rs_f}, rel_err={rel_err}"
                    );
                }
                Err(_) => {
                    assert!(
                        rs.is_err(),
                        "log_bigint({n}, {base:?}): py raised error but rs={rs:?}"
                    );
                }
            }
        });
    }

    fn test_log2_bigint_impl(n: &BigInt) {
        let rs = log2_bigint(n);
        crate::test::with_py_math(|py, math| {
            let n_str = n.to_string();
            let builtins = pyo3::types::PyModule::import(py, "builtins").unwrap();
            let py_n = builtins
                .getattr("int")
                .unwrap()
                .call1((n_str.as_str(),))
                .unwrap();
            let py_result = math.getattr("log2").unwrap().call1((py_n,));
            match py_result {
                Ok(py_val) => {
                    let py_f: f64 = py_val.extract().unwrap();
                    let rs_f = rs.unwrap();
                    if py_f.is_nan() && rs_f.is_nan() {
                        return;
                    }
                    if py_f == rs_f {
                        return;
                    }
                    let rel_err = ((py_f - rs_f) / py_f).abs();
                    assert!(
                        rel_err < 1e-10,
                        "log2_bigint({n}): py={py_f} vs rs={rs_f}, rel_err={rel_err}"
                    );
                }
                Err(_) => {
                    assert!(
                        rs.is_err(),
                        "log2_bigint({n}): py raised error but rs={rs:?}"
                    );
                }
            }
        });
    }

    fn test_log10_bigint_impl(n: &BigInt) {
        let rs = log10_bigint(n);
        crate::test::with_py_math(|py, math| {
            let n_str = n.to_string();
            let builtins = pyo3::types::PyModule::import(py, "builtins").unwrap();
            let py_n = builtins
                .getattr("int")
                .unwrap()
                .call1((n_str.as_str(),))
                .unwrap();
            let py_result = math.getattr("log10").unwrap().call1((py_n,));
            match py_result {
                Ok(py_val) => {
                    let py_f: f64 = py_val.extract().unwrap();
                    let rs_f = rs.unwrap();
                    if py_f.is_nan() && rs_f.is_nan() {
                        return;
                    }
                    if py_f == rs_f {
                        return;
                    }
                    let rel_err = ((py_f - rs_f) / py_f).abs();
                    assert!(
                        rel_err < 1e-10,
                        "log10_bigint({n}): py={py_f} vs rs={rs_f}, rel_err={rel_err}"
                    );
                }
                Err(_) => {
                    assert!(
                        rs.is_err(),
                        "log10_bigint({n}): py raised error but rs={rs:?}"
                    );
                }
            }
        });
    }

    fn assert_exact_log_bigint_bits(n: &BigInt, func_name: &str, rs: crate::Result<f64>) {
        crate::test::with_py_math(|py, math| {
            let n_str = n.to_string();
            let builtins = pyo3::types::PyModule::import(py, "builtins").unwrap();
            let py_n = builtins
                .getattr("int")
                .unwrap()
                .call1((n_str.as_str(),))
                .unwrap();
            let py_f: f64 = math
                .getattr(func_name)
                .unwrap()
                .call1((py_n,))
                .unwrap()
                .extract()
                .unwrap();
            let rs_f = rs.unwrap();
            assert_eq!(
                py_f.to_bits(),
                rs_f.to_bits(),
                "{func_name}({n}): py={py_f} ({:#x}) vs rs={rs_f} ({:#x})",
                py_f.to_bits(),
                rs_f.to_bits()
            );
        });
    }

    #[test]
    fn edgetest_log_bigint() {
        // Small values
        for i in -10i64..=100 {
            let n = BigInt::from(i);
            test_log_bigint_impl(&n, None);
            test_log_bigint_impl(&n, Some(2.0));
            test_log_bigint_impl(&n, Some(10.0));
        }
        // Powers of 2 (important for log2)
        for exp in 0..100u32 {
            let n = BigInt::from(1u64) << exp;
            test_log_bigint_impl(&n, None);
            test_log_bigint_impl(&n, Some(2.0));
        }
        // Large values
        let ten = BigInt::from(10);
        for exp in [100, 200, 500, 1000] {
            let n = ten.pow(exp);
            test_log_bigint_impl(&n, None);
            test_log_bigint_impl(&n, Some(2.0));
            test_log_bigint_impl(&n, Some(10.0));
        }
    }

    #[test]
    fn edgetest_log2_bigint() {
        for i in -10i64..=100 {
            test_log2_bigint_impl(&BigInt::from(i));
        }
        for exp in 0..100u32 {
            test_log2_bigint_impl(&(BigInt::from(1u64) << exp));
        }
        let ten = BigInt::from(10);
        for exp in [100, 200, 500, 1000] {
            test_log2_bigint_impl(&ten.pow(exp));
        }
    }

    #[test]
    fn edgetest_log10_bigint() {
        for i in -10i64..=100 {
            test_log10_bigint_impl(&BigInt::from(i));
        }
        let ten = BigInt::from(10);
        for exp in [10, 50, 100, 200, 500, 1000] {
            test_log10_bigint_impl(&ten.pow(exp));
        }
    }

    proptest::proptest! {
        #[test]
        fn proptest_log_bigint(n in 1i64..1_000_000i64) {
            test_log_bigint_impl(&BigInt::from(n), None);
        }

        #[test]
        fn proptest_log_bigint_base2(n in 1i64..1_000_000i64) {
            test_log_bigint_impl(&BigInt::from(n), Some(2.0));
        }

        #[test]
        fn proptest_log_bigint_base10(n in 1i64..1_000_000i64) {
            test_log_bigint_impl(&BigInt::from(n), Some(10.0));
        }

        #[test]
        fn proptest_log2_bigint(n in 1i64..1_000_000i64) {
            test_log2_bigint_impl(&BigInt::from(n));
        }

        #[test]
        fn proptest_log10_bigint(n in 1i64..1_000_000i64) {
            test_log10_bigint_impl(&BigInt::from(n));
        }
    }

    #[test]
    fn regression_log_bigint_pow10_309() {
        let n = BigInt::from(10).pow(309);
        assert_exact_log_bigint_bits(&n, "log", log_bigint(&n, None));
    }

    #[test]
    fn regression_log2_bigint_pow10_309() {
        let n = BigInt::from(10).pow(309);
        assert_exact_log_bigint_bits(&n, "log2", log2_bigint(&n));
    }

    #[test]
    fn regression_log10_bigint_pow10_309() {
        let n = BigInt::from(10).pow(309);
        assert_exact_log_bigint_bits(&n, "log10", log10_bigint(&n));
    }

    #[test]
    fn regression_log_bigint_sticky_bit_rounding() {
        // If frexp_bigint drops the sticky bit, this rounds 1 ULP low.
        let n = (BigInt::from(0x40000000000c02u64) << 970u32) + BigInt::from(1u8);
        assert_exact_log_bigint_bits(&n, "log", log_bigint(&n, None));
    }

    #[test]
    fn regression_log2_bigint_sticky_bit_rounding() {
        // If frexp_bigint drops the sticky bit, this rounds 1 ULP low.
        let n = (BigInt::from(0x400000000010a2u64) << 970u32) + BigInt::from(1u8);
        assert_exact_log_bigint_bits(&n, "log2", log2_bigint(&n));
    }

    #[test]
    fn regression_log10_bigint_sticky_bit_rounding() {
        // If frexp_bigint drops the sticky bit, this rounds 1 ULP low.
        let n = (BigInt::from(0x4000000000049au64) << 970u32) + BigInt::from(1u8);
        assert_exact_log_bigint_bits(&n, "log10", log10_bigint(&n));
    }

    // ldexp_bigint tests

    fn test_ldexp_bigint_impl(x: f64, exp: &BigInt) {
        let rs = ldexp_bigint(x, exp);
        crate::test::with_py_math(|py, math| {
            let exp_str = exp.to_string();
            let builtins = pyo3::types::PyModule::import(py, "builtins").unwrap();
            let py_exp = builtins
                .getattr("int")
                .unwrap()
                .call1((exp_str.as_str(),))
                .unwrap();
            let py_result = math.getattr("ldexp").unwrap().call1((x, py_exp));
            match py_result {
                Ok(py_val) => {
                    let py_f: f64 = py_val.extract().unwrap();
                    let rs_f = rs.unwrap();
                    // Handle NaN
                    if py_f.is_nan() && rs_f.is_nan() {
                        return;
                    }
                    // Handle exact equality (including signed zeros and infinities)
                    if py_f == rs_f && py_f.is_sign_positive() == rs_f.is_sign_positive() {
                        return;
                    }
                    // Handle infinities
                    if py_f.is_infinite() && rs_f.is_infinite() {
                        assert_eq!(
                            py_f.is_sign_positive(),
                            rs_f.is_sign_positive(),
                            "ldexp_bigint({x}, {exp}): sign mismatch py={py_f} vs rs={rs_f}"
                        );
                        return;
                    }
                    panic!("ldexp_bigint({x}, {exp}): py={py_f} vs rs={rs_f}");
                }
                Err(_) => {
                    assert!(
                        rs.is_err(),
                        "ldexp_bigint({x}, {exp}): py raised error but rs={rs:?}"
                    );
                }
            }
        });
    }

    #[test]
    fn edgetest_ldexp_bigint() {
        let x_vals = [
            0.0,
            -0.0,
            1.0,
            -1.0,
            0.5,
            -0.5,
            2.0,
            -2.0,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NAN,
        ];
        let exp_vals: Vec<BigInt> = vec![
            BigInt::from(0),
            BigInt::from(1),
            BigInt::from(-1),
            BigInt::from(10),
            BigInt::from(-10),
            BigInt::from(100),
            BigInt::from(-100),
            BigInt::from(1000),
            BigInt::from(-1000),
            BigInt::from(i32::MAX),
            BigInt::from(i32::MIN),
            // Overflow cases
            BigInt::from(i32::MAX as i64 + 1),
            BigInt::from(i32::MIN as i64 - 1),
            BigInt::from(10).pow(20),
            -BigInt::from(10).pow(20),
        ];

        for &x in &x_vals {
            for exp in &exp_vals {
                test_ldexp_bigint_impl(x, exp);
            }
        }
    }
}
