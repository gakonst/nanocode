// generated source. do not edit.
#![allow(non_upper_case_globals, unused_macros, unused_imports)]
use crate::low::macros::*;

// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

// ----------------------------------------------------------------------------
// Multiplex/select z := x (if p nonzero) or z := y (if p zero)
// Inputs p, x[k], y[k]; output z[k]
//
//    extern void bignum_mux(uint64_t p, uint64_t k, uint64_t *z, const uint64_t *x,
//                           const uint64_t *y);
//
// It is assumed that all numbers x, y and z have the same size k digits.
//
// Standard ARM ABI: X0 = p, X1 = k, X2 = z, X3 = x, X4 = y
// ----------------------------------------------------------------------------

macro_rules! b {
    () => {
        "x0"
    };
}
macro_rules! k {
    () => {
        "x1"
    };
}
macro_rules! z {
    () => {
        "x2"
    };
}
macro_rules! x {
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

/// Multiplex/select z := x (if p nonzero) or z := y (if p zero)
///
/// Inputs p, x[k], y[k]; output z[k]
///
/// It is assumed that all numbers x, y and z have the same size k digits.
pub(crate) fn bignum_mux(p: u64, z: &mut [u64], x_if_p: &[u64], y_if_not_p: &[u64]) {
    debug_assert!(z.len() == x_if_p.len());
    debug_assert!(z.len() == y_if_not_p.len());
    // SAFETY: inline assembly. see [crate::low::inline_assembly_safety] for safety info.
    unsafe {
        core::arch::asm!(

        Q!("    cbz             " k!() ", " Label!("Lbignum_mux_end", 2, After)),
        Q!("    cmp             " b!() ", #0"),

        // We've set cc's from b once and for all and can now re-use "b" as a temporary

        Q!(Label!("Lbignum_mux_loop", 3) ":"),
        Q!("    sub             " k!() ", " k!() ", #1"),
        Q!("    ldr             " a!() ", [" x!() ", " k!() ", lsl #3]"),
        Q!("    ldr             " b!() ", [" y!() ", " k!() ", lsl #3]"),
        Q!("    csel            " a!() ", " a!() ", " b!() ", ne"),
        Q!("    str             " a!() ", [" z!() ", " k!() ", lsl #3]"),
        Q!("    cbnz            " k!() ", " Label!("Lbignum_mux_loop", 3, Before)),

        Q!(Label!("Lbignum_mux_end", 2) ":"),
        inout("x0") p => _,
        inout("x1") z.len() => _,
        inout("x2") z.as_mut_ptr() => _,
        inout("x3") x_if_p.as_ptr() => _,
        inout("x4") y_if_not_p.as_ptr() => _,
        // clobbers
        out("x5") _,
            )
    };
}
