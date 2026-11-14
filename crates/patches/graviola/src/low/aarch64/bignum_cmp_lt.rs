// generated source. do not edit.
#![allow(non_upper_case_globals, unused_macros, unused_imports)]
use crate::low::macros::*;

// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

// ----------------------------------------------------------------------------
// Compare bignums, x < y
// Inputs x[m], y[n]; output function return
//
//    extern uint64_t bignum_lt(uint64_t m, const uint64_t *x, uint64_t n,
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
macro_rules! i {
    () => {
        "x4"
    };
}
macro_rules! a {
    () => {
        "x5"
    };
}
macro_rules! d {
    () => {
        "x6"
    };
}

/// Compare bignums, x < y
///
/// Inputs x[m], y[n]; output function return
pub(crate) fn bignum_cmp_lt(x: &[u64], y: &[u64]) -> u64 {
    let ret: u64;
    // SAFETY: inline assembly. see [crate::low::inline_assembly_safety] for safety info.
    unsafe {
        core::arch::asm!(

        // Zero the main index counter for both branches

        Q!("    mov             " i!() ", xzr"),

        // Speculatively form m := m - n and do case split

        Q!("    subs            " m!() ", " m!() ", " n!()),
        Q!("    bcc             " Label!("Lbignum_lt_ylonger", 2, After)),

        // The case where x is longer or of the same size (m >= n)
        // Note that CF=1 initially by the fact that we reach this point

        Q!("    cbz             " n!() ", " Label!("Lbignum_lt_xtest", 3, After)),
        Q!(Label!("Lbignum_lt_xmainloop", 4) ":"),
        Q!("    ldr             " a!() ", [" x!() ", " i!() ", lsl #3]"),
        Q!("    ldr             " d!() ", [" y!() ", " i!() ", lsl #3]"),
        Q!("    sbcs            " "xzr, " a!() ", " d!()),
        Q!("    add             " i!() ", " i!() ", #1"),
        Q!("    sub             " n!() ", " n!() ", #1"),
        Q!("    cbnz            " n!() ", " Label!("Lbignum_lt_xmainloop", 4, Before)),
        Q!(Label!("Lbignum_lt_xtest", 3) ":"),
        Q!("    cbz             " m!() ", " Label!("Lbignum_lt_xskip", 5, After)),
        Q!(Label!("Lbignum_lt_xtoploop", 6) ":"),
        Q!("    ldr             " a!() ", [" x!() ", " i!() ", lsl #3]"),
        Q!("    sbcs            " "xzr, " a!() ", xzr"),
        Q!("    add             " i!() ", " i!() ", #1"),
        Q!("    sub             " m!() ", " m!() ", #1"),
        Q!("    cbnz            " m!() ", " Label!("Lbignum_lt_xtoploop", 6, Before)),
        Q!(Label!("Lbignum_lt_xskip", 5) ":"),
        Q!("    cset            " "x0, cc"),
        // linear hoisting in -> ret after Lbignum_lt_ytoploop
        Q!("    b               " Label!("hoist_finish", 7, After)),

        // The case where y is longer (n > m)
        // The first "adds" also makes sure CF=1 initially in this branch

        Q!(Label!("Lbignum_lt_ylonger", 2) ":"),
        Q!("    adds            " m!() ", " m!() ", " n!()),
        Q!("    cbz             " m!() ", " Label!("Lbignum_lt_ytoploop", 8, After)),
        Q!("    sub             " n!() ", " n!() ", " m!()),
        Q!(Label!("Lbignum_lt_ymainloop", 9) ":"),
        Q!("    ldr             " a!() ", [" x!() ", " i!() ", lsl #3]"),
        Q!("    ldr             " d!() ", [" y!() ", " i!() ", lsl #3]"),
        Q!("    sbcs            " "xzr, " a!() ", " d!()),
        Q!("    add             " i!() ", " i!() ", #1"),
        Q!("    sub             " m!() ", " m!() ", #1"),
        Q!("    cbnz            " m!() ", " Label!("Lbignum_lt_ymainloop", 9, Before)),
        Q!(Label!("Lbignum_lt_ytoploop", 8) ":"),
        Q!("    ldr             " a!() ", [" y!() ", " i!() ", lsl #3]"),
        Q!("    sbcs            " "xzr, xzr, " a!()),
        Q!("    add             " i!() ", " i!() ", #1"),
        Q!("    sub             " n!() ", " n!() ", #1"),
        Q!("    cbnz            " n!() ", " Label!("Lbignum_lt_ytoploop", 8, Before)),

        Q!("    cset            " "x0, cc"),
        Q!(Label!("hoist_finish", 7) ":"),
        inout("x0") x.len() => ret,
        inout("x1") x.as_ptr() => _,
        inout("x2") y.len() => _,
        inout("x3") y.as_ptr() => _,
        // clobbers
        out("x4") _,
        out("x5") _,
        out("x6") _,
            )
    };
    ret
}
