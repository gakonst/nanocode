// generated source. do not edit.
#![allow(non_upper_case_globals, unused_macros, unused_imports)]
use crate::low::macros::*;

// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

// ----------------------------------------------------------------------------
// Reduce modulo group order, z := x mod n_384
// Input x[6]; output z[6]
//
//    extern void bignum_mod_n384_6(uint64_t z[static 6], const uint64_t x[static 6]);
//
// Reduction is modulo the group order of the NIST curve P-384.
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
macro_rules! n4 {
    () => {
        "x6"
    };
}
macro_rules! n5 {
    () => {
        "x7"
    };
}

macro_rules! d0 {
    () => {
        "x8"
    };
}
macro_rules! d1 {
    () => {
        "x9"
    };
}
macro_rules! d2 {
    () => {
        "x10"
    };
}
macro_rules! d3 {
    () => {
        "x11"
    };
}
macro_rules! d4 {
    () => {
        "x12"
    };
}
macro_rules! d5 {
    () => {
        "x13"
    };
}

macro_rules! movbig {
    ($nn:expr, $n3:expr, $n2:expr, $n1:expr, $n0:expr) => { Q!(
        "movz " $nn ", " $n0 ";\n"
        "movk " $nn ", " $n1 ", lsl #16;\n"
        "movk " $nn ", " $n2 ", lsl #32;\n"
        "movk " $nn ", " $n3 ", lsl #48"
    )}
}

/// Reduce modulo group order, z := x mod n_384
///
/// Input x[6]; output z[6]
///
/// Reduction is modulo the group order of the NIST curve P-384.
pub(crate) fn bignum_mod_n384(z: &mut [u64; 6], x: &[u64; 6]) {
    // SAFETY: inline assembly. see [crate::low::inline_assembly_safety] for safety info.
    unsafe {
        core::arch::asm!(

        // Load the complicated lower three words of n_384

        movbig!(n0!(), "#0xecec", "#0x196a", "#0xccc5", "#0x2973"),
        movbig!(n1!(), "#0x581a", "#0x0db2", "#0x48b0", "#0xa77a"),
        movbig!(n2!(), "#0xc763", "#0x4d81", "#0xf437", "#0x2ddf"),

        // Load the input number

        Q!("    ldp             " d0!() ", " d1!() ", [" x!() "]"),
        Q!("    ldp             " d2!() ", " d3!() ", [" x!() ", #16]"),
        Q!("    ldp             " d4!() ", " d5!() ", [" x!() ", #32]"),

        // Do the subtraction. Since the top three words of n_384 are all 1s
        // we can devolve the top to adding 0, thanks to the inverted carry.

        Q!("    subs            " n0!() ", " d0!() ", " n0!()),
        Q!("    sbcs            " n1!() ", " d1!() ", " n1!()),
        Q!("    sbcs            " n2!() ", " d2!() ", " n2!()),
        Q!("    adcs            " n3!() ", " d3!() ", xzr"),
        Q!("    adcs            " n4!() ", " d4!() ", xzr"),
        Q!("    adcs            " n5!() ", " d5!() ", xzr"),

        // Now if the carry is *clear* (inversion at work) the subtraction carried
        // and hence we should have done nothing, so we reset each n_i = d_i

        Q!("    csel            " n0!() ", " d0!() ", " n0!() ", cc"),
        Q!("    csel            " n1!() ", " d1!() ", " n1!() ", cc"),
        Q!("    csel            " n2!() ", " d2!() ", " n2!() ", cc"),
        Q!("    csel            " n3!() ", " d3!() ", " n3!() ", cc"),
        Q!("    csel            " n4!() ", " d4!() ", " n4!() ", cc"),
        Q!("    csel            " n5!() ", " d5!() ", " n5!() ", cc"),

        // Store the end result

        Q!("    stp             " n0!() ", " n1!() ", [" z!() "]"),
        Q!("    stp             " n2!() ", " n3!() ", [" z!() ", #16]"),
        Q!("    stp             " n4!() ", " n5!() ", [" z!() ", #32]"),

        inout("x0") z.as_mut_ptr() => _,
        inout("x1") x.as_ptr() => _,
        // clobbers
        out("x10") _,
        out("x11") _,
        out("x12") _,
        out("x13") _,
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
