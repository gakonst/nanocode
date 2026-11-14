// generated source. do not edit.
#![allow(non_upper_case_globals, unused_macros, unused_imports)]
use crate::low::macros::*;

// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

// ----------------------------------------------------------------------------
// Test bignums for equality, x = y
// Inputs x[m], y[n]; output function return
//
//    extern uint64_t bignum_eq(uint64_t m, const uint64_t *x, uint64_t n,
//                              const uint64_t *y);
//
// Standard ARM ABI: X0 = m, X1 = x, X2 = n, X3 = y, returns X0
// ----------------------------------------------------------------------------

macro_rules! m {
    () => {
        "x0"
    };
}
macro_rules! x {
    () => {
        "x1"
    };
}
macro_rules! n {
    () => {
        "x2"
    };
}
macro_rules! y {
    () => {
        "x3"
    };
}
macro_rules! a {
    () => {
        "x4"
    };
}
macro_rules! c {
    () => {
        "x5"
    };
}
//  We can re-use n for this, not needed when d appears
macro_rules! d {
    () => {
        "x2"
    };
}

/// Test bignums for equality, x = y
///
/// Inputs x[m], y[n]; output function return
pub(crate) fn bignum_eq(x: &[u64], y: &[u64]) -> bool {
    let ret: u64;
    // SAFETY: inline assembly. see [crate::low::inline_assembly_safety] for safety info.
    unsafe {
        core::arch::asm!(

        // Initialize the accumulated OR of differences to zero

        Q!("    mov             " c!() ", xzr"),

        // If m >= n jump into the m > n loop at the final equality test
        // This will drop through for m = n

        Q!("    cmp             " m!() ", " n!()),
        Q!("    bcs             " Label!("Lbignum_eq_mtest", 2, After)),

        // Toploop for the case n > m

        Q!(Label!("Lbignum_eq_nloop", 3) ":"),
        Q!("    sub             " n!() ", " n!() ", #1"),
        Q!("    ldr             " a!() ", [" y!() ", " n!() ", lsl #3]"),
        Q!("    orr             " c!() ", " c!() ", " a!()),
        Q!("    cmp             " m!() ", " n!()),
        Q!("    bne             " Label!("Lbignum_eq_nloop", 3, Before)),
        Q!("    b               " Label!("Lbignum_eq_mmain", 4, After)),

        // Toploop for the case m > n (or n = m which enters at "mtest")

        Q!(Label!("Lbignum_eq_mloop", 5) ":"),
        Q!("    sub             " m!() ", " m!() ", #1"),
        Q!("    ldr             " a!() ", [" x!() ", " m!() ", lsl #3]"),
        Q!("    orr             " c!() ", " c!() ", " a!()),
        Q!("    cmp             " m!() ", " n!()),
        Q!(Label!("Lbignum_eq_mtest", 2) ":"),
        Q!("    bne             " Label!("Lbignum_eq_mloop", 5, Before)),

        // Combined main loop for the min(m,n) lower words

        Q!(Label!("Lbignum_eq_mmain", 4) ":"),
        Q!("    cbz             " m!() ", " Label!("Lbignum_eq_end", 6, After)),

        Q!(Label!("Lbignum_eq_loop", 7) ":"),
        Q!("    sub             " m!() ", " m!() ", #1"),
        Q!("    ldr             " a!() ", [" x!() ", " m!() ", lsl #3]"),
        Q!("    ldr             " d!() ", [" y!() ", " m!() ", lsl #3]"),
        Q!("    eor             " a!() ", " a!() ", " d!()),
        Q!("    orr             " c!() ", " c!() ", " a!()),
        Q!("    cbnz            " m!() ", " Label!("Lbignum_eq_loop", 7, Before)),

        Q!(Label!("Lbignum_eq_end", 6) ":"),
        Q!("    cmp             " c!() ", xzr"),
        Q!("    cset            " "x0, eq"),
        inout("x0") x.len() => ret,
        inout("x1") x.as_ptr() => _,
        inout("x2") y.len() => _,
        inout("x3") y.as_ptr() => _,
        // clobbers
        out("x4") _,
        out("x5") _,
            )
    };
    ret > 0
}
