/* SPDX-License-Identifier: MIT
 * origin: musl src/math/trunc.c */

use crate::support::{Float, FpResult, Int, IntTy, MinInt, Status};

#[inline]
pub fn trunc<F: Float>(x: F) -> F {
    trunc_status(x).val
}

#[inline]
pub fn trunc_status<F: Float>(x: F) -> FpResult<F> {
    let mut xi: F::Int = x.to_bits();
    let e: i32 = x.exp_unbiased();

    // C1: The represented value has no fractional part, so no truncation is needed
    if e >= F::SIG_BITS as i32 {
        return FpResult::ok(x);
    }

    let mask = if e < 0 {
        // C2: If the exponent is negative, the result will be zero so we mask out everything
        // except the sign.
        F::SIGN_MASK
    } else {
        // C3: Otherwise, we mask out the last `e` bits of the significand.
        !(F::SIG_MASK >> e.unsigned())
    };

    // C4: If the to-be-masked-out portion is already zero, we have an exact result
    if (xi & !mask) == IntTy::<F>::ZERO {
        return FpResult::ok(x);
    }

    // C5: Otherwise the result is inexact and we will truncate. Raise `FE_INEXACT`, mask the
    // result, and return.

    let status = if xi & F::SIG_MASK == F::Int::ZERO {
        Status::OK
    } else {
        Status::INEXACT
    };
    xi &= mask;
    FpResult::new(F::from_bits(xi), status)
}
