//! Special functions: gamma, lgamma, erf, erfc.

use crate::{Error, mul_add};
use std::f64::consts::PI;

/// Error function.
#[inline]
pub fn erf(x: f64) -> crate::Result<f64> {
    super::math_1a(x, crate::m::erf)
}

/// Complementary error function.
#[inline]
pub fn erfc(x: f64) -> crate::Result<f64> {
    super::math_1a(x, crate::m::erfc)
}

const LOG_PI: f64 = 1.144729885849400174143427351353058711647;

const LANCZOS_N: usize = 13;
#[allow(clippy::excessive_precision)]
const LANCZOS_G: f64 = 6.024680040776729583740234375;
#[allow(clippy::excessive_precision)]
const LANCZOS_G_MINUS_HALF: f64 = 5.524680040776729583740234375;
#[allow(clippy::excessive_precision)]
const LANCZOS_NUM_COEFFS: [f64; LANCZOS_N] = [
    23531376880.410759688572007674451636754734846804940,
    42919803642.649098768957899047001988850926355848959,
    35711959237.355668049440185451547166705960488635843,
    17921034426.037209699919755754458931112671403265390,
    6039542586.3520280050642916443072979210699388420708,
    1439720407.3117216736632230727949123939715485786772,
    248874557.86205415651146038641322942321632125127801,
    31426415.585400194380614231628318205362874684987640,
    2876370.6289353724412254090516208496135991145378768,
    186056.26539522349504029498971604569928220784236328,
    8071.6720023658162106380029022722506138218516325024,
    210.82427775157934587250973392071336271166969580291,
    2.5066282746310002701649081771338373386264310793408,
];
const LANCZOS_DEN_COEFFS: [f64; LANCZOS_N] = [
    0.0,
    39916800.0,
    120543840.0,
    150917976.0,
    105258076.0,
    45995730.0,
    13339535.0,
    2637558.0,
    357423.0,
    32670.0,
    1925.0,
    66.0,
    1.0,
];

fn lanczos_sum(x: f64) -> f64 {
    let mut num = 0.0;
    let mut den = 0.0;
    // evaluate the rational function lanczos_sum(x).  For large
    // x, the obvious algorithm risks overflow, so we instead
    // rescale the denominator and numerator of the rational
    // function by x**(1-LANCZOS_N) and treat this as a
    // rational function in 1/x.  This also reduces the error for
    // larger x values.  The choice of cutoff point (5.0 below) is
    // somewhat arbitrary; in tests, smaller cutoff values than
    // this resulted in lower accuracy.
    if x < 5.0 {
        for i in (0..LANCZOS_N).rev() {
            num = mul_add(num, x, LANCZOS_NUM_COEFFS[i]);
            den = mul_add(den, x, LANCZOS_DEN_COEFFS[i]);
        }
    } else {
        for i in 0..LANCZOS_N {
            num = num / x + LANCZOS_NUM_COEFFS[i];
            den = den / x + LANCZOS_DEN_COEFFS[i];
        }
    }
    num / den
}

fn m_sinpi(x: f64) -> f64 {
    // this function should only ever be called for finite arguments
    debug_assert!(x.is_finite());
    let y = x.abs() % 2.0;
    let n = (2.0 * y).round() as i32;
    let r = match n {
        0 => (PI * y).sin(),
        1 => (PI * (y - 0.5)).cos(),
        2 => {
            // N.B. -sin(pi*(y-1.0)) is *not* equivalent: it would give
            // -0.0 instead of 0.0 when y == 1.0.
            (PI * (1.0 - y)).sin()
        }
        3 => -(PI * (y - 1.5)).cos(),
        4 => (PI * (y - 2.0)).sin(),
        _ => unreachable!(),
    };
    (1.0f64).copysign(x) * r
}

const NGAMMA_INTEGRAL: usize = 23;
const GAMMA_INTEGRAL: [f64; NGAMMA_INTEGRAL] = [
    1.0,
    1.0,
    2.0,
    6.0,
    24.0,
    120.0,
    720.0,
    5040.0,
    40320.0,
    362880.0,
    3628800.0,
    39916800.0,
    479001600.0,
    6227020800.0,
    87178291200.0,
    1307674368000.0,
    20922789888000.0,
    355687428096000.0,
    6402373705728000.0,
    121645100408832000.0,
    2432902008176640000.0,
    51090942171709440000.0,
    1124000727777607680000.0,
];

// tgamma
pub fn gamma(x: f64) -> crate::Result<f64> {
    // special cases
    if !x.is_finite() {
        if x.is_nan() || x > 0.0 {
            // tgamma(nan) = nan, tgamma(inf) = inf
            return Ok(x);
        } else {
            // tgamma(-inf) = nan, invalid
            return Err((f64::NAN, Error::EDOM).1);
        }
    }
    if x == 0.0 {
        // tgamma(+-0.0) = +-inf, divide-by-zero
        let v = if x.is_sign_positive() {
            f64::INFINITY
        } else {
            f64::NEG_INFINITY
        };
        return Err((v, Error::EDOM).1);
    }
    // integer arguments
    if x == x.floor() {
        if x < 0.0 {
            // tgamma(n) = nan, invalid for
            return Err((f64::NAN, Error::EDOM).1);
        }
        if x < NGAMMA_INTEGRAL as f64 {
            return Ok(GAMMA_INTEGRAL[x as usize - 1]);
        }
    }
    let absx = x.abs();
    // tiny arguments:  tgamma(x) ~ 1/x for x near 0
    if absx < 1e-20 {
        let r = 1.0 / x;
        if r.is_infinite() {
            return Err((f64::INFINITY, Error::ERANGE).1);
        } else {
            return Ok(r);
        }
    }
    // large arguments: assuming IEEE 754 doubles, tgamma(x) overflows for
    // x > 200, and underflows to +-0.0 for x < -200, not a negative
    // integer.
    if absx > 200.0 {
        if x < 0.0 {
            return Ok(0.0 / m_sinpi(x));
        } else {
            return Err((f64::INFINITY, Error::ERANGE).1);
        }
    }

    let y = absx + LANCZOS_G_MINUS_HALF;
    let z = if absx > LANCZOS_G_MINUS_HALF {
        // note: the correction can be foiled by an optimizing
        // compiler that (incorrectly) thinks that an expression like
        // a + b - a - b can be optimized to 0.0.  This shouldn't
        // happen in a standards-conforming compiler.
        let q = y - absx;
        q - LANCZOS_G_MINUS_HALF
    } else {
        let q = y - LANCZOS_G_MINUS_HALF;
        q - absx
    };
    let z = z * LANCZOS_G / y;
    let r = if x < 0.0 {
        let mut r = -PI / m_sinpi(absx) / absx * y.exp() / lanczos_sum(absx);
        r -= z * r;
        if absx < 140.0 {
            r /= y.powf(absx - 0.5);
        } else {
            let sqrtpow = y.powf(absx / 2.0 - 0.25);
            r /= sqrtpow;
            r /= sqrtpow;
        }
        r
    } else {
        let mut r = lanczos_sum(absx) / y.exp();
        r += z * r;
        if absx < 140.0 {
            r *= y.powf(absx - 0.5);
        } else {
            let sqrtpow = y.powf(absx / 2.0 - 0.25);
            r *= sqrtpow;
            r *= sqrtpow;
        }
        r
    };
    if r.is_infinite() {
        Err((f64::INFINITY, Error::ERANGE).1)
    } else {
        Ok(r)
    }
}

// natural log of the absolute value of the Gamma function.
// For large arguments, Lanczos' formula works extremely well here.
pub fn lgamma(x: f64) -> crate::Result<f64> {
    // special cases
    if !x.is_finite() {
        if x.is_nan() {
            return Ok(x); // lgamma(nan) = nan
        } else {
            return Ok(f64::INFINITY); // lgamma(+-inf) = +inf
        }
    }

    // integer arguments
    if x == x.floor() && x <= 2.0 {
        if x <= 0.0 {
            // lgamma(n) = inf, divide-by-zero for integers n <= 0
            return Err(Error::EDOM);
        } else {
            // lgamma(1) = lgamma(2) = 0.0
            return Ok(0.0);
        }
    }

    let absx = x.abs();
    // tiny arguments: lgamma(x) ~ -log(fabs(x)) for small x
    if absx < 1e-20 {
        return Ok(-absx.ln());
    }

    // Lanczos' formula.  We could save a fraction of a ulp in accuracy by
    // having a second set of numerator coefficients for lanczos_sum that
    // absorbed the exp(-lanczos_g) term, and throwing out the lanczos_g
    // subtraction below; it's probably not worth it.
    let mut r = lanczos_sum(absx).ln() - LANCZOS_G;
    let t = absx - 0.5;
    r = mul_add(t, (absx + LANCZOS_G - 0.5).ln() - 1.0, r);

    if x < 0.0 {
        // Use reflection formula to get value for negative x
        r = LOG_PI - m_sinpi(absx).abs().ln() - absx.ln() - r;
    }
    if r.is_infinite() {
        return Err(Error::ERANGE);
    }
    Ok(r)
}

crate::pyo3_proptest!(gamma(Result<_>), test_gamma, proptest_gamma, fulltest_gamma);
crate::pyo3_proptest!(
    lgamma(Result<_>),
    test_lgamma,
    proptest_lgamma,
    fulltest_lgamma
);
crate::pyo3_proptest!(erf(Result<_>), test_erf, proptest_erf, fulltest_erf);
crate::pyo3_proptest!(erfc(Result<_>), test_erfc, proptest_erfc, fulltest_erfc);
