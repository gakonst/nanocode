// generated source. do not edit.
#![allow(non_upper_case_globals, unused_macros, unused_imports)]
use crate::low::macros::*;

// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

// ----------------------------------------------------------------------------
// Return size of bignum in bits
// Input x[k]; output function return
//
//    extern uint64_t bignum_bitsize(uint64_t k, const uint64_t *x);
//
// In the case of a zero bignum as input the result is 0
//
// In principle this has a precondition k < 2^58, but obviously that
// is always true in practice because of address space limitations.
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
macro_rules! j {
    () => {
        "x5"
    };
}

/// Return size of bignum in bits
///
/// Input x[k]; output function return
///
/// In the case of a zero bignum as input the result is 0
///
/// In principle this has a precondition k < 2^58, but obviously that
/// is always true in practice because of address space limitations.
pub(crate) fn bignum_bitsize(x: &[u64]) -> usize {
    let ret: u64;
    // SAFETY: inline assembly. see [crate::low::inline_assembly_safety] for safety info.
    unsafe {
        core::arch::asm!(

        // If the bignum is zero-length, x0 is already the right answer of 0

        Q!("    cbz             " k!() ", " Label!("Lbignum_bitsize_end", 2, After)),

        // Use w = a[i-1] to store nonzero words in a bottom-up sweep
        // Set the initial default to be as if we had a 11...11 word directly below

        Q!("    mov             " i!() ", xzr"),
        Q!("    mov             " w!() ", #-1"),
        Q!("    mov             " j!() ", xzr"),
        Q!(Label!("Lbignum_bitsize_loop", 3) ":"),
        Q!("    ldr             " a!() ", [" x!() ", " j!() ", lsl #3]"),
        Q!("    add             " j!() ", " j!() ", #1"),
        Q!("    cmp             " a!() ", #0"),
        Q!("    csel            " i!() ", " j!() ", " i!() ", ne"),
        Q!("    csel            " w!() ", " a!() ", " w!() ", ne"),
        Q!("    cmp             " j!() ", " k!()),
        Q!("    bne             " Label!("Lbignum_bitsize_loop", 3, Before)),

        // Now w = a[i-1] is the highest nonzero word, or in the zero case the
        // default of the "extra" 11...11 = a[0-1]. We now want 64* i - clz(w).
        // Note that this code does not rely on the behavior of the clz instruction
        // for zero inputs, though the ARM manual does in fact guarantee clz(0) = 64.

        Q!("    lsl             " i!() ", " i!() ", #6"),
        Q!("    clz             " a!() ", " w!()),
        Q!("    sub             " "x0, " i!() ", " a!()),

        Q!(Label!("Lbignum_bitsize_end", 2) ":"),
        inout("x0") x.len() => ret,
        inout("x1") x.as_ptr() => _,
        // clobbers
        out("x2") _,
        out("x3") _,
        out("x4") _,
        out("x5") _,
            )
    };
    ret as usize
}
