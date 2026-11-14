// generated source. do not edit.
#![allow(non_upper_case_globals, unused_macros, unused_imports)]
use crate::low::macros::*;

// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

// ----------------------------------------------------------------------------
// Multiply z := x * y
// Inputs x[m], y[n]; output z[k]
//
//    extern void bignum_mul(uint64_t k, uint64_t *z, uint64_t m, const uint64_t *x,
//                           uint64_t n, const uint64_t *y);
//
// Does the "z := x * y" operation where x is m digits, y is n, result z is k.
// Truncates the result in general unless k >= m + n
//
// Standard ARM ABI: X0 = k, X1 = z, X2 = m, X3 = x, X4 = n, X5 = y
// ----------------------------------------------------------------------------

macro_rules! p {
    () => {
        "x0"
    };
}
macro_rules! z {
    () => {
        "x1"
    };
}
macro_rules! m {
    () => {
        "x2"
    };
}
macro_rules! x {
    () => {
        "x3"
    };
}
macro_rules! n {
    () => {
        "x4"
    };
}
macro_rules! y {
    () => {
        "x5"
    };
}
macro_rules! l {
    () => {
        "x6"
    };
}
macro_rules! h {
    () => {
        "x7"
    };
}
macro_rules! c {
    () => {
        "x8"
    };
}
macro_rules! k {
    () => {
        "x9"
    };
}
macro_rules! i {
    () => {
        "x10"
    };
}
macro_rules! a {
    () => {
        "x11"
    };
}
macro_rules! b {
    () => {
        "x12"
    };
}
macro_rules! d {
    () => {
        "x13"
    };
}
macro_rules! xx {
    () => {
        "x14"
    };
}
macro_rules! yy {
    () => {
        "x15"
    };
}

/// Multiply z := x * y
///
/// Inputs x[m], y[n]; output z[k]
///
/// Does the "z := x * y" operation where x is m digits, y is n, result z is k.
/// Truncates the result in general unless k >= m + n
pub(crate) fn bignum_mul(z: &mut [u64], x: &[u64], y: &[u64]) {
    debug_assert!(z.len() >= x.len() + y.len());
    // SAFETY: inline assembly. see [crate::low::inline_assembly_safety] for safety info.
    unsafe {
        core::arch::asm!(

        // If p = 0 the result is trivial and nothing needs doing

        Q!("    cbz             " p!() ", " Label!("Lbignum_mul_end", 2, After)),

        // initialize (h,l) = 0, saving c = 0 for inside the loop

        Q!("    mov             " l!() ", xzr"),
        Q!("    mov             " h!() ", xzr"),

        // Iterate outer loop from k = 0 ... k = p - 1 producing result digits

        Q!("    mov             " k!() ", xzr"),
        Q!(Label!("Lbignum_mul_outerloop", 3) ":"),

        // Zero the carry for this stage

        Q!("    mov             " c!() ", xzr"),

        // First let a = MAX 0 (k + 1 - n) and b = MIN (k + 1) m
        // We want to accumulate all x[i] * y[k - i] for a <= i < b

        Q!("    add             " a!() ", " k!() ", #1"),
        Q!("    cmp             " a!() ", " m!()),
        Q!("    csel            " b!() ", " a!() ", " m!() ", cc"),
        Q!("    subs            " a!() ", " a!() ", " n!()),
        Q!("    csel            " a!() ", " a!() ", xzr, cs"),

        // Set loop count i = b - a, and skip everything if it's <= 0

        Q!("    subs            " i!() ", " b!() ", " a!()),
        Q!("    bls             " Label!("Lbignum_mul_innerend", 4, After)),

        // Use temporary pointers xx = x + 8 * a and yy = y + 8 * (k - b)
        // Increment xx per iteration but just use loop counter with yy
        // So we start with [xx] = x[a] and [yy] = y[(k - b) + (b - a)] = y[k - a]

        Q!("    lsl             " xx!() ", " a!() ", #3"),
        Q!("    add             " xx!() ", " xx!() ", " x!()),

        Q!("    sub             " yy!() ", " k!() ", " b!()),
        Q!("    lsl             " yy!() ", " yy!() ", #3"),
        Q!("    add             " yy!() ", " yy!() ", " y!()),

        // And index using the loop counter i = b - a, ..., i = 1

        Q!(Label!("Lbignum_mul_innerloop", 5) ":"),
        Q!("    ldr             " a!() ", [" xx!() "], #8"),
        Q!("    ldr             " b!() ", [" yy!() ", " i!() ", lsl #3]"),
        Q!("    mul             " d!() ", " a!() ", " b!()),
        Q!("    umulh           " a!() ", " a!() ", " b!()),
        Q!("    adds            " l!() ", " l!() ", " d!()),
        Q!("    adcs            " h!() ", " h!() ", " a!()),
        Q!("    adc             " c!() ", " c!() ", xzr"),
        Q!("    subs            " i!() ", " i!() ", #1"),
        Q!("    bne             " Label!("Lbignum_mul_innerloop", 5, Before)),

        Q!(Label!("Lbignum_mul_innerend", 4) ":"),
        Q!("    str             " l!() ", [" z!() ", " k!() ", lsl #3]"),
        Q!("    mov             " l!() ", " h!()),
        Q!("    mov             " h!() ", " c!()),

        Q!("    add             " k!() ", " k!() ", #1"),
        Q!("    cmp             " k!() ", " p!()),
        Q!("    bcc             " Label!("Lbignum_mul_outerloop", 3, Before)),

        Q!(Label!("Lbignum_mul_end", 2) ":"),
        inout("x0") z.len() => _,
        inout("x1") z.as_mut_ptr() => _,
        inout("x2") x.len() => _,
        inout("x3") x.as_ptr() => _,
        inout("x4") y.len() => _,
        inout("x5") y.as_ptr() => _,
        // clobbers
        out("x10") _,
        out("x11") _,
        out("x12") _,
        out("x13") _,
        out("x14") _,
        out("x15") _,
        out("x6") _,
        out("x7") _,
        out("x8") _,
        out("x9") _,
            )
    };
}
