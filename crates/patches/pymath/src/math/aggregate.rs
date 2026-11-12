//! Aggregate functions for sequences.

/// Double-length number represented as hi + lo
#[derive(Clone, Copy)]
struct DoubleLength {
    hi: f64,
    lo: f64,
}

/// Algorithm 1.1. Compensated summation of two floating-point numbers.
/// Requires: |a| >= |b|
#[inline]
fn dl_fast_sum(a: f64, b: f64) -> DoubleLength {
    debug_assert!(a.abs() >= b.abs());
    let x = a + b;
    let y = (a - x) + b;
    DoubleLength { hi: x, lo: y }
}

/// Algorithm 3.1 Error-free transformation of the sum
#[inline]
fn dl_sum(a: f64, b: f64) -> DoubleLength {
    let x = a + b;
    let z = x - a;
    let y = (a - (x - z)) + (b - z);
    DoubleLength { hi: x, lo: y }
}

/// Algorithm 3.5. Error-free transformation of a product using FMA
#[inline]
fn dl_mul(x: f64, y: f64) -> DoubleLength {
    let z = x * y;
    let zz = x.mul_add(y, -z);
    DoubleLength { hi: z, lo: zz }
}

/// Triple-length number for extra precision
#[derive(Clone, Copy)]
struct TripleLength {
    hi: f64,
    lo: f64,
    tiny: f64,
}

const TL_ZERO: TripleLength = TripleLength {
    hi: 0.0,
    lo: 0.0,
    tiny: 0.0,
};

/// Algorithm 5.10 with SumKVert for K=3
#[inline]
fn tl_fma(x: f64, y: f64, total: TripleLength) -> TripleLength {
    let pr = dl_mul(x, y);
    let sm = dl_sum(total.hi, pr.hi);
    let r1 = dl_sum(total.lo, pr.lo);
    let r2 = dl_sum(r1.hi, sm.lo);
    TripleLength {
        hi: sm.hi,
        lo: r2.hi,
        tiny: total.tiny + r1.lo + r2.lo,
    }
}

#[inline]
fn tl_to_d(total: TripleLength) -> f64 {
    let last = dl_sum(total.lo, total.hi);
    total.tiny + last.lo + last.hi
}

// FSUM - Shewchuk's algorithm

const NUM_PARTIALS: usize = 32;

/// Return an accurate floating-point sum of values in the iterable.
///
/// Uses Shewchuk's algorithm for full precision summation.
/// Assumes IEEE-754 floating-point arithmetic.
///
/// Returns ERANGE for intermediate overflow, EDOM for -inf + inf.
pub fn fsum(iter: impl IntoIterator<Item = f64>) -> crate::Result<f64> {
    let mut p: Vec<f64> = Vec::with_capacity(NUM_PARTIALS);
    let mut special_sum = 0.0;
    let mut inf_sum = 0.0;

    for x in iter {
        let xsave = x;
        let mut x = x;
        let mut i = 0;

        for j in 0..p.len() {
            let mut y = p[j];
            if x.abs() < y.abs() {
                std::mem::swap(&mut x, &mut y);
            }
            let hi = x + y;
            let yr = hi - x;
            let lo = y - yr;
            if lo != 0.0 {
                p[i] = lo;
                i += 1;
            }
            x = hi;
        }

        p.truncate(i);
        if x != 0.0 {
            if !x.is_finite() {
                // a nonfinite x could arise either as a result of
                // intermediate overflow, or as a result of a nan or inf
                // in the summands
                if xsave.is_finite() {
                    // intermediate overflow
                    return Err(crate::Error::ERANGE);
                }
                if xsave.is_infinite() {
                    inf_sum += xsave;
                }
                special_sum += xsave;
                // reset partials
                p.clear();
            } else {
                p.push(x);
            }
        }
    }

    if special_sum != 0.0 {
        if inf_sum.is_nan() {
            // -inf + inf
            return Err(crate::Error::EDOM);
        }
        return Ok(special_sum);
    }

    let n = p.len();
    let mut hi = 0.0;
    let mut lo = 0.0;

    if n > 0 {
        let mut idx = n - 1;
        hi = p[idx];

        // sum_exact(ps, hi) from the top, stop when the sum becomes inexact
        while idx > 0 {
            idx -= 1;
            let x = hi;
            let y = p[idx];
            hi = x + y;
            let yr = hi - x;
            lo = y - yr;
            if lo != 0.0 {
                break;
            }
        }

        // Make half-even rounding work across multiple partials.
        if idx > 0 && ((lo < 0.0 && p[idx - 1] < 0.0) || (lo > 0.0 && p[idx - 1] > 0.0)) {
            let y = lo * 2.0;
            let x = hi + y;
            let yr = x - hi;
            if y == yr {
                hi = x;
            }
        }
    }

    Ok(hi)
}

// VECTOR_NORM - for dist and hypot

/// Compute the Euclidean norm of a vector with high precision.
pub fn vector_norm(vec: &[f64], max: f64, found_nan: bool) -> f64 {
    let n = vec.len();

    if max.is_infinite() {
        return max;
    }
    if found_nan {
        return f64::NAN;
    }
    if max == 0.0 || n <= 1 {
        return max;
    }

    let mut max_e: i32 = 0;
    crate::m::frexp(max, &mut max_e);

    if max_e < -1023 {
        // When max_e < -1023, ldexp(1.0, -max_e) would overflow.
        // TODO: This can be in-place ops, but we allocate a copy since we take &[f64].
        // This is acceptable because subnormal inputs are extremely rare in practice.
        let vec_copy: Vec<f64> = vec.iter().map(|&x| x / f64::MIN_POSITIVE).collect();
        return f64::MIN_POSITIVE * vector_norm(&vec_copy, max / f64::MIN_POSITIVE, found_nan);
    }

    let scale = crate::m::ldexp(1.0, -max_e);
    debug_assert!(max * scale >= 0.5);
    debug_assert!(max * scale < 1.0);

    let mut csum = 1.0;
    let mut frac1 = 0.0;
    let mut frac2 = 0.0;

    for &v in vec {
        debug_assert!(v.is_finite() && v.abs() <= max);
        let x = v * scale; // lossless scaling
        debug_assert!(x.abs() < 1.0);
        let pr = dl_mul(x, x); // lossless squaring
        debug_assert!(pr.hi <= 1.0);
        let sm = dl_fast_sum(csum, pr.hi); // lossless addition
        csum = sm.hi;
        frac1 += pr.lo; // lossy addition
        frac2 += sm.lo; // lossy addition
    }

    let mut h = (csum - 1.0 + (frac1 + frac2)).sqrt();
    let pr = dl_mul(-h, h);
    let sm = dl_fast_sum(csum, pr.hi);
    csum = sm.hi;
    frac1 += pr.lo;
    frac2 += sm.lo;
    let x = csum - 1.0 + (frac1 + frac2);
    h += x / (2.0 * h); // differential correction

    h / scale
}

/// Return the Euclidean distance between two points.
///
/// The points are given as sequences of coordinates.
/// Uses high-precision vector_norm algorithm.
///
/// Panics if `p` and `q` have different lengths. CPython raises ValueError
/// for mismatched dimensions, but in this Rust API the caller is expected
/// to guarantee equal-length slices. A length mismatch is a programming
/// error, not a runtime condition.
pub fn dist(p: &[f64], q: &[f64]) -> f64 {
    assert_eq!(
        p.len(),
        q.len(),
        "both points must have the same number of dimensions"
    );

    let n = p.len();
    if n == 0 {
        return 0.0;
    }

    let mut max = 0.0;
    let mut found_nan = false;
    let mut diffs: Vec<f64> = Vec::with_capacity(n);

    for i in 0..n {
        let x = (p[i] - q[i]).abs();
        diffs.push(x);
        found_nan |= x.is_nan();
        if x > max {
            max = x;
        }
    }

    vector_norm(&diffs, max, found_nan)
}

/// Return the sum of products of values from two sequences (float version).
///
/// Uses TripleLength arithmetic for the fast path, then falls back to
/// ordinary floating-point multiply/add starting at the first unsupported
/// pair, matching Python's staged `math.sumprod` behavior for float inputs.
///
/// CPython's math_sumprod_impl is a 3-stage state machine that handles
/// int/float/generic Python objects. This function only covers the float
/// path (`&[f64]`). The int accumulation and generic PyNumber fallback
/// stages are Python type-system concerns and should be handled by the
/// caller (e.g. RustPython) before delegating here.
///
/// Returns EDOM if the inputs are not the same length.
pub fn sumprod(p: &[f64], q: &[f64]) -> crate::Result<f64> {
    if p.len() != q.len() {
        return Err(crate::Error::EDOM);
    }

    let mut total = 0.0;
    let mut flt_total = TL_ZERO;
    let mut flt_path_enabled = true;
    let mut i = 0;

    while i < p.len() {
        let pi = p[i];
        let qi = q[i];

        if flt_path_enabled {
            let new_flt_total = tl_fma(pi, qi, flt_total);
            if new_flt_total.hi.is_finite() {
                flt_total = new_flt_total;
                i += 1;
                continue;
            }

            flt_path_enabled = false;
            total += tl_to_d(flt_total);
        }

        total += pi * qi;
        i += 1;
    }

    Ok(if flt_path_enabled {
        tl_to_d(flt_total)
    } else {
        total
    })
}

/// Return the sum of products of values from two sequences (integer version).
///
/// Uses checked arithmetic to detect overflow.
/// Returns None if overflow occurs during computation.
/// Equivalent to sum(p[i] * q[i] for i in range(len(p))).
pub fn sumprod_int(p: &[i64], q: &[i64]) -> Option<i64> {
    assert_eq!(p.len(), q.len(), "Inputs are not the same length");

    let mut total: i64 = 0;

    for (&pi, &qi) in p.iter().zip(q.iter()) {
        let prod = pi.checked_mul(qi)?;
        total = total.checked_add(prod)?;
    }

    Some(total)
}

/// Return the product of all elements in the iterable (float version).
///
/// If start is None, uses 1.0 as the start value.
pub fn prod(iter: impl IntoIterator<Item = f64>, start: Option<f64>) -> f64 {
    let mut result = start.unwrap_or(1.0);
    for x in iter {
        result *= x;
    }
    result
}

/// Return the product of all elements using i64 arithmetic.
///
/// Returns None if overflow occurs during multiplication.
/// Uses checked arithmetic to detect overflow.
pub fn prod_int(iter: impl IntoIterator<Item = i64>, start: Option<i64>) -> Option<i64> {
    let mut result = start.unwrap_or(1);
    for x in iter {
        result = result.checked_mul(x)?;
    }
    Some(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pyo3::prelude::*;

    fn test_fsum_impl(values: &[f64]) {
        let rs_result = fsum(values.iter().copied());

        pyo3::Python::attach(|py| {
            let math = pyo3::types::PyModule::import(py, "math").unwrap();
            let py_func = math.getattr("fsum").unwrap();
            let py_list = pyo3::types::PyList::new(py, values).unwrap();
            let r = py_func.call1((py_list,));

            match r {
                Ok(py_val) => {
                    let py_result: f64 = py_val.extract().unwrap();
                    let rs_val = rs_result.unwrap_or_else(|e| {
                        panic!(
                            "fsum({:?}): py={} but rs returned error {:?}",
                            values, py_result, e
                        )
                    });
                    if py_result.is_nan() && rs_val.is_nan() {
                        return;
                    }
                    assert_eq!(
                        py_result.to_bits(),
                        rs_val.to_bits(),
                        "fsum({:?}): py={} vs rs={}",
                        values,
                        py_result,
                        rs_val
                    );
                }
                Err(e) => {
                    let rs_err = rs_result.as_ref().err();
                    if e.is_instance_of::<pyo3::exceptions::PyValueError>(py) {
                        assert_eq!(
                            rs_err,
                            Some(&crate::Error::EDOM),
                            "fsum({:?}): py raised ValueError but rs={:?}",
                            values,
                            rs_err
                        );
                    } else if e.is_instance_of::<pyo3::exceptions::PyOverflowError>(py) {
                        assert_eq!(
                            rs_err,
                            Some(&crate::Error::ERANGE),
                            "fsum({:?}): py raised OverflowError but rs={:?}",
                            values,
                            rs_err
                        );
                    } else {
                        panic!("fsum({:?}): py raised unexpected error {}", values, e);
                    }
                }
            }
        });
    }

    #[test]
    fn test_fsum() {
        test_fsum_impl(&[1.0, 2.0, 3.0]);
        test_fsum_impl(&[]);
        test_fsum_impl(&[0.1, 0.2, 0.3]);
        test_fsum_impl(&[1e100, 1.0, -1e100, 1e-100, 1e50, -1e50]);
        test_fsum_impl(&[f64::INFINITY, 1.0]);
        test_fsum_impl(&[f64::NEG_INFINITY, 1.0]);
        test_fsum_impl(&[f64::INFINITY, f64::NEG_INFINITY]); // -inf + inf -> ValueError (EDOM)
        test_fsum_impl(&[f64::NAN, 1.0]);
        // Intermediate overflow cases
        test_fsum_impl(&[1e308, 1e308]); // intermediate overflow -> OverflowError (ERANGE)
        test_fsum_impl(&[1e308, 1e308, -1e308]); // intermediate overflow
    }

    fn test_dist_impl(p: &[f64], q: &[f64]) {
        let rs = dist(p, q);
        crate::test::with_py_math(|py, math| {
            let py_p = pyo3::types::PyList::new(py, p).unwrap();
            let py_q = pyo3::types::PyList::new(py, q).unwrap();
            let py: f64 = math
                .getattr("dist")
                .unwrap()
                .call1((py_p, py_q))
                .unwrap()
                .extract()
                .unwrap();
            crate::test::assert_f64_eq(py, rs, format_args!("dist({p:?}, {q:?})"));
        });
    }

    #[test]
    fn test_dist() {
        test_dist_impl(&[0.0, 0.0], &[3.0, 4.0]); // 3-4-5 triangle
        test_dist_impl(&[1.0, 2.0], &[1.0, 2.0]); // same point
        test_dist_impl(&[0.0], &[5.0]); // 1D
        test_dist_impl(&[0.0, 0.0, 0.0], &[1.0, 1.0, 1.0]); // 3D
    }

    fn test_sumprod_impl(p: &[f64], q: &[f64]) {
        let rs = sumprod(p, q);
        crate::test::with_py_math(|py, math| {
            let py_p = pyo3::types::PyList::new(py, p).unwrap();
            let py_q = pyo3::types::PyList::new(py, q).unwrap();
            let py_result = math.getattr("sumprod").unwrap().call1((py_p, py_q));
            match py_result {
                Ok(py_val) => {
                    let py: f64 = py_val.extract().unwrap();
                    let rs = rs.unwrap_or_else(|e| {
                        panic!("sumprod({p:?}, {q:?}): py={py} but rs returned error {e:?}")
                    });
                    crate::test::assert_f64_eq(py, rs, format_args!("sumprod({p:?}, {q:?})"));
                }
                Err(e) => {
                    if e.is_instance_of::<pyo3::exceptions::PyValueError>(py) {
                        assert_eq!(
                            rs.as_ref().err(),
                            Some(&crate::Error::EDOM),
                            "sumprod({p:?}, {q:?}): py raised ValueError but rs={rs:?}"
                        );
                    } else {
                        panic!("sumprod({p:?}, {q:?}): py raised unexpected error {e}");
                    }
                }
            }
        });
    }

    #[test]
    fn test_sumprod() {
        test_sumprod_impl(&[1.0, 2.0, 3.0], &[4.0, 5.0, 6.0]);
        test_sumprod_impl(&[], &[]);
        test_sumprod_impl(&[1.0], &[2.0]);
        test_sumprod_impl(&[1e100, 1e100], &[1e100, -1e100]);
        test_sumprod_impl(&[1.0, 1e308, -1e308], &[1.0, 2.0, 2.0]);
        test_sumprod_impl(&[1e-16, 1e308, -1e308], &[1.0, 2.0, 2.0]);
        test_sumprod_impl(&[1.0], &[]);
    }

    fn test_prod_impl(values: &[f64], start: Option<f64>) {
        let rs = prod(values.iter().copied(), start);
        crate::test::with_py_math(|py, math| {
            let py_list = pyo3::types::PyList::new(py, values).unwrap();
            let py_func = math.getattr("prod").unwrap();
            let py: f64 = match start {
                Some(s) => {
                    let kwargs = pyo3::types::PyDict::new(py);
                    kwargs.set_item("start", s).unwrap();
                    py_func.call((py_list,), Some(&kwargs))
                }
                None => py_func.call1((py_list,)),
            }
            .unwrap()
            .extract()
            .unwrap();
            crate::test::assert_f64_eq(py, rs, format_args!("prod({values:?}, {start:?})"));
        });
    }

    #[test]
    fn test_prod() {
        test_prod_impl(&[1.0, 2.0, 3.0, 4.0], None);
        test_prod_impl(&[2.0, 3.0], None);
        test_prod_impl(&[], None);
        test_prod_impl(&[1.0, 2.0, 3.0], Some(2.0));
        test_prod_impl(&[], Some(5.0));
    }

    proptest::proptest! {
        #[test]
        fn proptest_fsum(v1: f64, v2: f64, v3: f64, v4: f64) {
            test_fsum_impl(&[v1, v2, v3, v4]);
        }

        #[test]
        fn proptest_dist(p1: f64, p2: f64, q1: f64, q2: f64) {
            test_dist_impl(&[p1, p2], &[q1, q2]);
        }

        #[test]
        fn proptest_sumprod(p1: f64, p2: f64, q1: f64, q2: f64) {
            test_sumprod_impl(&[p1, p2], &[q1, q2]);
        }

        #[test]
        fn proptest_prod(v1: f64, v2: f64, v3: f64) {
            test_prod_impl(&[v1, v2, v3], None);
        }
    }
}
