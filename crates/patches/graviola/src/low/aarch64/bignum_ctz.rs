// generated source. do not edit.
#![allow(non_upper_case_globals, unused_macros, unused_imports)]
use crate::low::macros::*;

// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

// ----------------------------------------------------------------------------
// Count trailing zero bits
// Input x[k]; output function return
//
//    extern uint64_t bignum_ctz(uint64_t k, const uint64_t *x);
//
//
// In the case of a zero bignum as input the result is 64 * k
//
// In principle this has a precondition k < 2^58, but obviously that
// is always true in practice because of address space limitations
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

/// Count trailing zero bits
///
/// Input x[k]; output function return
///
///
/// In the case of a zero bignum as input the result is 64 * k
///
/// In principle this has a precondition k < 2^58, but obviously that
/// is always true in practice because of address space limitations
pub(crate) fn bignum_ctz(x: &[u64]) -> usize {
    let ret: u64;
    // SAFETY: inline assembly. see [crate::low::inline_assembly_safety] for safety info.
    unsafe {
        core::arch::asm!(

        // If the bignum is zero-length, x0 is already the right answer of 0

        Q!("    cbz             " k!() ", " Label!("Lbignum_ctz_end", 2, After)),

        // Use w = a[i] to store nonzero words in a top-down sweep
        // Set the initial default to be as if we had a 1 word directly above

        Q!("    mov             " i!() ", " k!()),
        Q!("    mov             " w!() ", #1"),

        Q!(Label!("Lbignum_ctz_loop", 3) ":"),
        Q!("    sub             " k!() ", " k!() ", #1"),
        Q!("    ldr             " a!() ", [" x!() ", " k!() ", lsl #3]"),
        Q!("    cmp             " a!() ", #0"),
        Q!("    csel            " i!() ", " k!() ", " i!() ", ne"),
        Q!("    csel            " w!() ", " a!() ", " w!() ", ne"),
        Q!("    cbnz            " k!() ", " Label!("Lbignum_ctz_loop", 3, Before)),

        // Now w = a[i] is the lowest nonzero word, or in the zero case the
        // default of the "extra" 1 = a[k]. We now want 64*i + ctz(w).
        //
        // ARM doesn't have a direct word ctz instruction, so we emulate it via
        // ctz(w) = 64 - clz(~w & (w-1)). This is depending, for cases of the form
        // ctz(....1), on the behavior clz(0) = 64, which is guaranteed according
        // to the ARM manual.

        Q!("    mvn             " a!() ", " w!()),
        Q!("    sub             " w!() ", " w!() ", #1"),
        Q!("    add             " i!() ", " i!() ", #1"),
        Q!("    and             " w!() ", " w!() ", " a!()),
        Q!("    lsl             " i!() ", " i!() ", #6"),
        Q!("    clz             " a!() ", " w!()),
        Q!("    sub             " "x0, " i!() ", " a!()),

        Q!(Label!("Lbignum_ctz_end", 2) ":"),
        inout("x0") x.len() => ret,
        inout("x1") x.as_ptr() => _,
        // clobbers
        out("x2") _,
        out("x3") _,
        out("x4") _,
            )
    };
    ret as usize
}
