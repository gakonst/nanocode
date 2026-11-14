// generated source. do not edit.
#![allow(non_upper_case_globals, unused_macros, unused_imports)]
use crate::low::macros::*;

// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

// ----------------------------------------------------------------------------
// Negated modular inverse, z := (-1/x) mod 2^{64k}
// Input x[k]; output z[k]
//
//    extern void bignum_negmodinv(uint64_t k, uint64_t *z, const uint64_t *x);
//
// Assuming x is odd (otherwise nothing makes sense) the result satisfies
//
//       x * z + 1 == 0 (mod 2^{64 * k})
//
// but is not necessarily reduced mod x.
//
// Standard ARM ABI: X0 = k, X1 = z, X2 = x
// ----------------------------------------------------------------------------

macro_rules! k {
    () => {
        "x0"
    };
}
macro_rules! z {
    () => {
        "x1"
    };
}
macro_rules! x {
    () => {
        "x2"
    };
}

macro_rules! w {
    () => {
        "x3"
    };
}
macro_rules! a {
    () => {
        "x4"
    };
}
macro_rules! m {
    () => {
        "x5"
    };
}
macro_rules! h {
    () => {
        "x6"
    };
}
macro_rules! l {
    () => {
        "x7"
    };
}
macro_rules! e {
    () => {
        "x8"
    };
}
macro_rules! i {
    () => {
        "x9"
    };
}

/// Negated modular inverse, z := (-1/x) mod 2^{64k}
///
/// Input x[k]; output z[k]
///
/// Assuming x is odd (otherwise nothing makes sense) the result satisfies
///
/// x * z + 1 == 0 (mod 2^{64 * k})
///
/// but is not necessarily reduced mod x.
pub(crate) fn bignum_negmodinv(z: &mut [u64], x: &[u64]) {
    debug_assert!(z.len() == x.len());
    // SAFETY: inline assembly. see [crate::low::inline_assembly_safety] for safety info.
    unsafe {
        core::arch::asm!(

        // If k = 0 do nothing

        Q!("    cbz             " k!() ", " Label!("Lbignum_negmodinv_end", 2, After)),

        // Compute word-level negated modular inverse w for x[0].

        Q!("    ldr             " a!() ", [" x!() "]"),
        Q!("    lsl             " w!() ", " a!() ", #2"),
        Q!("    sub             " w!() ", " a!() ", " w!()),
        Q!("    eor             " w!() ", " w!() ", #2"),
        Q!("    mov             " h!() ", #1"),
        Q!("    madd            " h!() ", " a!() ", " w!() ", " h!()),
        Q!("    mul             " l!() ", " h!() ", " h!()),
        Q!("    madd            " w!() ", " h!() ", " w!() ", " w!()),
        Q!("    mul             " h!() ", " l!() ", " l!()),
        Q!("    madd            " w!() ", " l!() ", " w!() ", " w!()),
        Q!("    mul             " l!() ", " h!() ", " h!()),
        Q!("    madd            " w!() ", " h!() ", " w!() ", " w!()),
        Q!("    madd            " w!() ", " l!() ", " w!() ", " w!()),

        // Write that as lowest word of the output, then if k = 1 we're finished

        Q!("    str             " w!() ", [" z!() "]"),
        Q!("    cmp             " k!() ", #1"),
        Q!("    beq             " Label!("Lbignum_negmodinv_end", 2, After)),

        // Otherwise compute and write the other digits (1..k-1) of w * x + 1.
        // Note that at this point CF was set by the comparison (subtraction) "k - 1".
        // Since k >= 2 if we got here, this subtraction didn't carry; allowing
        // for the inverted carry on ARM that means that CF is guaranteed to be set.
        // This allows us to ignore the nominal "a * w + 1" from adding the low
        // part of the product, since its only contribution is to set the carry
        // flag. Thus, we only calculate the high part of a * w explicitly.

        Q!("    umulh           " h!() ", " a!() ", " w!()),
        Q!("    mov             " i!() ", #1"),
        Q!(Label!("Lbignum_negmodinv_initloop", 3) ":"),
        Q!("    ldr             " a!() ", [" x!() ", " i!() ", lsl #3]"),
        Q!("    mul             " l!() ", " a!() ", " w!()),
        Q!("    adcs            " l!() ", " l!() ", " h!()),
        Q!("    umulh           " h!() ", " a!() ", " w!()),
        Q!("    str             " l!() ", [" z!() ", " i!() ", lsl #3]"),
        Q!("    add             " i!() ", " i!() ", #1"),
        Q!("    sub             " a!() ", " k!() ", " i!()),
        Q!("    cbnz            " a!() ", " Label!("Lbignum_negmodinv_initloop", 3, Before)),

        // For simpler indexing, z := z + 8 and k := k - 1 per outer iteration
        // Then we can use the same index for x and for z and effective size k.
        //
        // But we also offset k by 1 so the "real" size is k + 1, which is why the
        // test at the end of the inner loop is i < k <=> i' = i + 1 < k + 1.
        // This lets us avoid some special cases inside the loop at the cost
        // of needing the additional "finale" tail for the final iteration
        // since we do one outer loop iteration too few.

        Q!("    subs            " k!() ", " k!() ", #2"),
        Q!("    beq             " Label!("Lbignum_negmodinv_finale", 4, After)),

        Q!(Label!("Lbignum_negmodinv_outerloop", 5) ":"),
        Q!("    add             " z!() ", " z!() ", #8"),
        Q!("    ldr             " e!() ", [" z!() "]"),
        Q!("    mul             " m!() ", " e!() ", " w!()),
        Q!("    str             " m!() ", [" z!() "]"),
        Q!("    ldr             " a!() ", [" x!() "]"),
        Q!("    umulh           " h!() ", " a!() ", " m!()),
        Q!("    subs            " "xzr, " e!() ", #1"),
        Q!("    mov             " i!() ", #1"),
        Q!(Label!("Lbignum_negmodinv_innerloop", 6) ":"),
        Q!("    ldr             " a!() ", [" x!() ", " i!() ", lsl #3]"),
        Q!("    ldr             " e!() ", [" z!() ", " i!() ", lsl #3]"),
        Q!("    mul             " l!() ", " a!() ", " m!()),
        Q!("    adcs            " e!() ", " e!() ", " h!()),
        Q!("    umulh           " h!() ", " a!() ", " m!()),
        Q!("    adc             " h!() ", " h!() ", xzr"),
        Q!("    adds            " e!() ", " e!() ", " l!()),
        Q!("    str             " e!() ", [" z!() ", " i!() ", lsl #3]"),
        Q!("    sub             " a!() ", " i!() ", " k!()),
        Q!("    add             " i!() ", " i!() ", #1"),
        Q!("    cbnz            " a!() ", " Label!("Lbignum_negmodinv_innerloop", 6, Before)),

        Q!("    subs            " k!() ", " k!() ", #1"),
        Q!("    bne             " Label!("Lbignum_negmodinv_outerloop", 5, Before)),

        Q!(Label!("Lbignum_negmodinv_finale", 4) ":"),
        Q!("    ldr             " e!() ", [" z!() ", #8]"),
        Q!("    mul             " m!() ", " e!() ", " w!()),
        Q!("    str             " m!() ", [" z!() ", #8]"),

        Q!(Label!("Lbignum_negmodinv_end", 2) ":"),
        inout("x0") z.len() => _,
        inout("x1") z.as_mut_ptr() => _,
        inout("x2") x.as_ptr() => _,
        // clobbers
        out("x3") _,
        out("x4") _,
        out("x5") _,
        out("x6") _,
        out("x7") _,
        out("x8") _,
        out("x9") _,
            )
    };
}
