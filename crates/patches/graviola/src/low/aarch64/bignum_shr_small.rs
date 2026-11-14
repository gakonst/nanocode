// generated source. do not edit.
#![allow(non_upper_case_globals, unused_macros, unused_imports)]
use crate::low::macros::*;

// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

// ----------------------------------------------------------------------------
// Shift bignum right by c < 64 bits z := floor(x / 2^c)
// Inputs x[n], c; outputs function return (bits shifted out) and z[k]
//
//    extern uint64_t bignum_shr_small(uint64_t k, uint64_t *z, uint64_t n,
//                                     const uint64_t *x, uint64_t c);
//
// Does the "z := x >> c" operation where x is n digits, result z is p.
// The shift count c is masked to 6 bits so it actually uses c' = c mod 64.
// The return value is the inout mod 2^c'.
//
// Standard ARM ABI: X0 = k, X1 = z, X2 = n, X3 = x, X4 = c, returns X0
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
macro_rules! n {
    () => {
        "x2"
    };
}
macro_rules! x {
    () => {
        "x3"
    };
}
macro_rules! c {
    () => {
        "x4"
    };
}

macro_rules! d {
    () => {
        "x5"
    };
}
macro_rules! a {
    () => {
        "x6"
    };
}
macro_rules! b {
    () => {
        "x7"
    };
}
macro_rules! m {
    () => {
        "x8"
    };
}
macro_rules! t {
    () => {
        "x9"
    };
}

/// Shift bignum right by c < 64 bits z := floor(x / 2^c)
///
/// Inputs x[n], c; outputs function return (bits shifted out) and z[k]
///
/// Does the "z := x >> c" operation where x is n digits, result z is p.
/// The shift count c is masked to 6 bits so it actually uses c' = c mod 64.
/// The return value is the inout mod 2^c'.
pub(crate) fn bignum_shr_small(z: &mut [u64], x: &[u64], c: u8) {
    // SAFETY: inline assembly. see [crate::low::inline_assembly_safety] for safety info.
    unsafe {
        core::arch::asm!(

        // Set default carry-in word to 0

        Q!("    mov             " b!() ", xzr"),

        // First, if p > n then pad output on the left with p-n zeros

        Q!("    cmp             " n!() ", " p!()),
        Q!("    bcs             " Label!("Lbignum_shr_small_nopad", 2, After)),
        Q!(Label!("Lbignum_shr_small_padloop", 3) ":"),
        Q!("    sub             " p!() ", " p!() ", #1"),
        Q!("    str             " "xzr, [" z!() ", " p!() ", lsl #3]"),
        Q!("    cmp             " n!() ", " p!()),
        Q!("    bcc             " Label!("Lbignum_shr_small_padloop", 3, Before)),

        // We now know that p <= n. If in fact p < n let carry word = x[p] instead of 0

        Q!(Label!("Lbignum_shr_small_nopad", 2) ":"),
        Q!("    beq             " Label!("Lbignum_shr_small_shiftstart", 4, After)),
        Q!("    ldr             " b!() ", [" x!() ", " p!() ", lsl #3]"),
        Q!(Label!("Lbignum_shr_small_shiftstart", 4) ":"),

        // Set up negated version of the shift and shift b in preparation.
        // Use a mask for nonzero shift to fake 64-bit left shift in zero case

        Q!("    neg             " d!() ", " c!()),
        Q!("    lsl             " b!() ", " b!() ", " d!()),
        Q!("    ands            " "xzr, " c!() ", #63"),
        Q!("    csetm           " m!() ", ne"),
        Q!("    and             " b!() ", " b!() ", " m!()),

        // Now the main loop

        Q!("    cbz             " p!() ", " Label!("Lbignum_shr_small_end", 5, After)),
        Q!(Label!("Lbignum_shr_small_loop", 6) ":"),
        Q!("    sub             " p!() ", " p!() ", #1"),
        Q!("    ldr             " t!() ", [" x!() ", " p!() ", lsl #3]"),
        Q!("    lsr             " a!() ", " t!() ", " c!()),
        Q!("    orr             " a!() ", " a!() ", " b!()),
        Q!("    lsl             " b!() ", " t!() ", " d!()),
        Q!("    and             " b!() ", " b!() ", " m!()),
        Q!("    str             " a!() ", [" z!() ", " p!() ", lsl #3]"),
        Q!("    cbnz            " p!() ", " Label!("Lbignum_shr_small_loop", 6, Before)),

        // Return top word, shifted back to be a modulus

        Q!(Label!("Lbignum_shr_small_end", 5) ":"),
        Q!("    lsr             " "x0, " b!() ", " d!()),
        inout("x0") z.len() => _,
        inout("x1") z.as_ptr() => _,
        inout("x2") x.len() => _,
        inout("x3") x.as_ptr() => _,
        inout("x4") (c as u64) => _,
        // clobbers
        out("x5") _,
        out("x6") _,
        out("x7") _,
        out("x8") _,
        out("x9") _,
            )
    };
}
