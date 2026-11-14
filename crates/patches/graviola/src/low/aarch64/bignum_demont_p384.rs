// generated source. do not edit.
#![allow(non_upper_case_globals, unused_macros, unused_imports)]
use crate::low::macros::*;

// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

// ----------------------------------------------------------------------------
// Convert from Montgomery form z := (x / 2^384) mod p_384, assuming x reduced
// Input x[6]; output z[6]
//
//    extern void bignum_demont_p384(uint64_t z[static 6],
//                                   const uint64_t x[static 6]);
//
// This assumes the input is < p_384 for correctness. If this is not the case,
// use the variant "bignum_deamont_p384" instead.
//
// Standard ARM ABI: X0 = z, X1 = x
// ----------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Core one-step "short" Montgomery reduction macro. Takes input in
// [d5;d4;d3;d2;d1;d0] and returns result in [d6;d5;d4;d3;d2;d1],
// adding to the existing contents of [d5;d4;d3;d2;d1]. It is fine
// for d6 to be the same register as d0.
//
// We want to add (2^384 - 2^128 - 2^96 + 2^32 - 1) * w
// where w = [d0 + (d0<<32)] mod 2^64
// ---------------------------------------------------------------------------

macro_rules! montreds {
    ($d6:expr, $d5:expr, $d4:expr, $d3:expr, $d2:expr, $d1:expr, $d0:expr, $t3:expr, $t2:expr, $t1:expr) => { Q!(
        /* Our correction multiplier is w = [d0 + (d0<<32)] mod 2^64            */
        /* Recycle d0 (which we know gets implicitly cancelled) to store it     */
        "lsl " $t1 ", " $d0 ", #32;\n"
        "add " $d0 ", " $t1 ", " $d0 ";\n"
        /* Now let [t2;t1] = 2^64 * w - w + w_hi where w_hi = floor(w/2^32)     */
        /* We need to subtract 2^32 * this, and we can ignore its lower 32      */
        /* bits since by design it will cancel anyway; we only need the w_hi    */
        /* part to get the carry propagation going.                             */
        "lsr " $t1 ", " $d0 ", #32;\n"
        "subs " $t1 ", " $t1 ", " $d0 ";\n"
        "sbc " $t2 ", " $d0 ", xzr;\n"
        /* Now select in t1 the field to subtract from d1                       */
        "extr " $t1 ", " $t2 ", " $t1 ", #32;\n"
        /* And now get the terms to subtract from d2 and d3                     */
        "lsr " $t2 ", " $t2 ", #32;\n"
        "adds " $t2 ", " $t2 ", " $d0 ";\n"
        "adc " $t3 ", xzr, xzr;\n"
        /* Do the subtraction of that portion                                   */
        "subs " $d1 ", " $d1 ", " $t1 ";\n"
        "sbcs " $d2 ", " $d2 ", " $t2 ";\n"
        "sbcs " $d3 ", " $d3 ", " $t3 ";\n"
        "sbcs " $d4 ", " $d4 ", xzr;\n"
        "sbcs " $d5 ", " $d5 ", xzr;\n"
        /* Now effectively add 2^384 * w by taking d0 as the input for last sbc */
        "sbc " $d6 ", " $d0 ", xzr"
    )}
}

// Input parameters

macro_rules! z {
    () => {
        "x0"
    };
}
macro_rules! x {
    () => {
        "x1"
    };
}

// Rotating registers for the intermediate windows

macro_rules! d0 {
    () => {
        "x2"
    };
}
macro_rules! d1 {
    () => {
        "x3"
    };
}
macro_rules! d2 {
    () => {
        "x4"
    };
}
macro_rules! d3 {
    () => {
        "x5"
    };
}
macro_rules! d4 {
    () => {
        "x6"
    };
}
macro_rules! d5 {
    () => {
        "x7"
    };
}

// Other temporaries

macro_rules! u {
    () => {
        "x8"
    };
}
macro_rules! v {
    () => {
        "x9"
    };
}
macro_rules! w {
    () => {
        "x10"
    };
}

/// Convert from Montgomery form z := (x / 2^384) mod p_384, assuming x reduced
///
/// Input x[6]; output z[6]
///
/// This assumes the input is < p_384 for correctness. If this is not the case,
/// use the variant "bignum_deamont_p384" instead.
pub(crate) fn bignum_demont_p384(z: &mut [u64; 6], x: &[u64; 6]) {
    // SAFETY: inline assembly. see [crate::low::inline_assembly_safety] for safety info.
    unsafe {
        core::arch::asm!(

        Q!(Label!("bignum_demont_p384_alt", 2) ":"),

        // Set up an initial window with the input x and an extra leading zero

        Q!("    ldp             " d0!() ", " d1!() ", [" x!() "]"),
        Q!("    ldp             " d2!() ", " d3!() ", [" x!() ", #16]"),
        Q!("    ldp             " d4!() ", " d5!() ", [" x!() ", #32]"),

        // Systematically scroll left doing 1-step reductions

        montreds!(d0!(), d5!(), d4!(), d3!(), d2!(), d1!(), d0!(), u!(), v!(), w!()),

        montreds!(d1!(), d0!(), d5!(), d4!(), d3!(), d2!(), d1!(), u!(), v!(), w!()),

        montreds!(d2!(), d1!(), d0!(), d5!(), d4!(), d3!(), d2!(), u!(), v!(), w!()),

        montreds!(d3!(), d2!(), d1!(), d0!(), d5!(), d4!(), d3!(), u!(), v!(), w!()),

        montreds!(d4!(), d3!(), d2!(), d1!(), d0!(), d5!(), d4!(), u!(), v!(), w!()),

        montreds!(d5!(), d4!(), d3!(), d2!(), d1!(), d0!(), d5!(), u!(), v!(), w!()),

        // This is already our answer with no correction needed

        Q!("    stp             " d0!() ", " d1!() ", [" z!() "]"),
        Q!("    stp             " d2!() ", " d3!() ", [" z!() ", #16]"),
        Q!("    stp             " d4!() ", " d5!() ", [" z!() ", #32]"),

        inout("x0") z.as_mut_ptr() => _,
        inout("x1") x.as_ptr() => _,
        // clobbers
        out("v6") _,
        out("x10") _,
        out("x2") _,
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
