// generated source. do not edit.
#![allow(non_upper_case_globals, unused_macros, unused_imports)]
use crate::low::macros::*;

// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

// ----------------------------------------------------------------------------
// Optionally subtract, z := x - y (if p nonzero) or z := x (if p zero)
// Inputs x[k], p, y[k]; outputs function return (carry-out) and z[k]
//
//    extern uint64_t bignum_optsub(uint64_t k, uint64_t *z, const uint64_t *x,
//                                  uint64_t p, const uint64_t *y);
//
// It is assumed that all numbers x, y and z have the same size k digits.
// Returns carry-out as per usual subtraction, always 0 if p was zero.
//
// Standard ARM ABI: X0 = k, X1 = z, X2 = x, X3 = p, X4 = y, returns X0
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
macro_rules! p {
    () => {
        "x3"
    };
}
macro_rules! m {
    () => {
        "x3"
    };
}
macro_rules! y {
    () => {
        "x4"
    };
}
macro_rules! a {
    () => {
        "x5"
    };
}
macro_rules! b {
    () => {
        "x6"
    };
}
macro_rules! i {
    () => {
        "x7"
    };
}

/// Optionally subtract, z := x - y (if p nonzero) or z := x (if p zero)
///
/// Inputs x[k], p, y[k]; outputs function return (carry-out) and z[k]
///
/// It is assumed that all numbers x, y and z have the same size k digits.
/// Returns carry-out as per usual subtraction, always 0 if p was zero.
pub(crate) fn bignum_optsub(z: &mut [u64], x: &[u64], y: &[u64], p: u64) {
    // SAFETY: inline assembly. see [crate::low::inline_assembly_safety] for safety info.
    unsafe {
        core::arch::asm!(

        // if k = 0 do nothing. This is also the right top carry in X0

        Q!("    cbz             " k!() ", " Label!("Lbignum_optsub_end", 2, After)),

        // Convert p into a strict bitmask (same register in fact)

        Q!("    cmp             " p!() ", xzr"),
        Q!("    csetm           " m!() ", ne"),

        // Set i = 0 *and* make sure initial ~CF = 0

        Q!("    subs            " i!() ", xzr, xzr"),

        // Main loop

        Q!(Label!("Lbignum_optsub_loop", 3) ":"),
        Q!("    ldr             " a!() ", [" x!() ", " i!() "]"),
        Q!("    ldr             " b!() ", [" y!() ", " i!() "]"),
        Q!("    and             " b!() ", " b!() ", " m!()),
        Q!("    sbcs            " a!() ", " a!() ", " b!()),
        Q!("    str             " a!() ", [" z!() ", " i!() "]"),
        Q!("    add             " i!() ", " i!() ", #8"),
        Q!("    sub             " k!() ", " k!() ", #1"),
        Q!("    cbnz            " k!() ", " Label!("Lbignum_optsub_loop", 3, Before)),

        // Return (non-inverted) carry flag

        Q!("    cset            " "x0, cc"),

        Q!(Label!("Lbignum_optsub_end", 2) ":"),
        inout("x0") z.len() => _,
        inout("x1") z.as_mut_ptr() => _,
        inout("x2") x.as_ptr() => _,
        inout("x3") p => _,
        inout("x4") y.as_ptr() => _,
        // clobbers
        out("x5") _,
        out("x6") _,
        out("x7") _,
            )
    };
}
