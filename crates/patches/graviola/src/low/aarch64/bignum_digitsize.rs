// generated source. do not edit.
#![allow(non_upper_case_globals, unused_macros, unused_imports)]
use crate::low::macros::*;

// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

// ----------------------------------------------------------------------------
// Return size of bignum in digits (64-bit word)
// Input x[k]; output function return
//
//    extern uint64_t bignum_digitsize(uint64_t k, const uint64_t *x);
//
// In the case of a zero bignum as input the result is 0
//
// Standard ARM ABI: X0 = k, X1 = x, returns X0
// ----------------------------------------------------------------------------

macro_rules! k {
    () => {
        "x0"
    };
}
macro_rules! x {
    () => {
        "x1"
    };
}
macro_rules! i {
    () => {
        "x2"
    };
}
macro_rules! a {
    () => {
        "x3"
    };
}
macro_rules! j {
    () => {
        "x4"
    };
}

/// Return size of bignum in digits (64-bit word)
///
/// Input x[k]; output function return
///
/// In the case of a zero bignum as input the result is 0
pub(crate) fn bignum_digitsize(z: &[u64]) -> usize {
    let ret: u64;
    // SAFETY: inline assembly. see [crate::low::inline_assembly_safety] for safety info.
    unsafe {
        core::arch::asm!(

        // If the bignum is zero-length, x0 is already the right answer of 0

        Q!("    cbz             " k!() ", " Label!("Lbignum_digitsize_end", 2, After)),

        // Run over the words j = 0..i-1, and set i := j + 1 when hitting nonzero a[j]

        Q!("    mov             " i!() ", xzr"),
        Q!("    mov             " j!() ", xzr"),
        Q!(Label!("Lbignum_digitsize_loop", 3) ":"),
        Q!("    ldr             " a!() ", [" x!() ", " j!() ", lsl #3]"),
        Q!("    add             " j!() ", " j!() ", #1"),
        Q!("    cmp             " a!() ", #0"),
        Q!("    csel            " i!() ", " j!() ", " i!() ", ne"),
        Q!("    cmp             " j!() ", " k!()),
        Q!("    bne             " Label!("Lbignum_digitsize_loop", 3, Before)),

        Q!("    mov             " "x0, " i!()),
        Q!(Label!("Lbignum_digitsize_end", 2) ":"),
        inout("x0") z.len() => ret,
        inout("x1") z.as_ptr() => _,
        // clobbers
        out("x2") _,
        out("x3") _,
        out("x4") _,
            )
    };
    ret as usize
}
