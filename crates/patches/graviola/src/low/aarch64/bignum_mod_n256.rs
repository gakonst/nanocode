// generated source. do not edit.
#![allow(non_upper_case_globals, unused_macros, unused_imports)]
use crate::low::macros::*;

// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

// ----------------------------------------------------------------------------
// Reduce modulo group order, z := x mod n_256
// Input x[4]; output z[4]
//
//    extern void bignum_mod_n256_4(uint64_t z[static 4], const uint64_t x[static 4]);
//
// Reduction is modulo the group order of the NIST curve P-256.
//
// Standard ARM ABI: X0 = z, X1 = x
// ----------------------------------------------------------------------------

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

macro_rules! n0 {
    () => {
        "x2"
    };
}
macro_rules! n1 {
    () => {
        "x3"
    };
}
macro_rules! n2 {
    () => {
        "x4"
    };
}
macro_rules! n3 {
    () => {
        "x5"
    };
}

macro_rules! d0 {
    () => {
        "x6"
    };
}
macro_rules! d1 {
    () => {
        "x7"
    };
}
macro_rules! d2 {
    () => {
        "x8"
    };
}
macro_rules! d3 {
    () => {
        "x9"
    };
}

// Loading large constants

macro_rules! movbig {
    ($nn:expr, $n3:expr, $n2:expr, $n1:expr, $n0:expr) => { Q!(
        "movz " $nn ", " $n0 ";\n"
        "movk " $nn ", " $n1 ", lsl #16;\n"
        "movk " $nn ", " $n2 ", lsl #32;\n"
        "movk " $nn ", " $n3 ", lsl #48"
    )}
}

/// Reduce modulo group order, z := x mod n_256
///
/// Input x[4]; output z[4]
///
/// Reduction is modulo the group order of the NIST curve P-256.
pub(crate) fn bignum_mod_n256(z: &mut [u64; 4], x: &[u64; 4]) {
    // SAFETY: inline assembly. see [crate::low::inline_assembly_safety] for safety info.
    unsafe {
        core::arch::asm!(

        // Load the complicated three words of n_256, the other being all 1s

        movbig!(n0!(), "#0xf3b9", "#0xcac2", "#0xfc63", "#0x2551"),
        movbig!(n1!(), "#0xbce6", "#0xfaad", "#0xa717", "#0x9e84"),
        Q!("    mov             " n3!() ", #0xffffffff00000000"),

        // Load the input number

        Q!("    ldp             " d0!() ", " d1!() ", [" x!() "]"),
        Q!("    ldp             " d2!() ", " d3!() ", [" x!() ", #16]"),

        // Do the subtraction. Since word 2 of n_256 is all 1s, that can be
        // done by adding zero with carry, thanks to the inverted carry.

        Q!("    subs            " n0!() ", " d0!() ", " n0!()),
        Q!("    sbcs            " n1!() ", " d1!() ", " n1!()),
        Q!("    adcs            " n2!() ", " d2!() ", xzr"),
        Q!("    sbcs            " n3!() ", " d3!() ", " n3!()),

        // Now if the carry is *clear* (inversion at work) the subtraction carried
        // and hence we should have done nothing, so we reset each n_i = d_i

        Q!("    csel            " n0!() ", " d0!() ", " n0!() ", cc"),
        Q!("    csel            " n1!() ", " d1!() ", " n1!() ", cc"),
        Q!("    csel            " n2!() ", " d2!() ", " n2!() ", cc"),
        Q!("    csel            " n3!() ", " d3!() ", " n3!() ", cc"),

        // Store the end result

        Q!("    stp             " n0!() ", " n1!() ", [" z!() "]"),
        Q!("    stp             " n2!() ", " n3!() ", [" z!() ", #16]"),

        inout("x0") z.as_mut_ptr() => _,
        inout("x1") x.as_ptr() => _,
        // clobbers
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
