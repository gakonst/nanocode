// generated source. do not edit.
#![allow(non_upper_case_globals, unused_macros, unused_imports)]
use crate::low::macros::*;

// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

// ----------------------------------------------------------------------------
// Negate modulo p_384, z := (-x) mod p_384, assuming x reduced
// Input x[6]; output z[6]
//
//    extern void bignum_neg_p384(uint64_t z[static 6], const uint64_t x[static 6]);
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

macro_rules! p {
    () => {
        "x2"
    };
}
macro_rules! t {
    () => {
        "x3"
    };
}

macro_rules! d0 {
    () => {
        "x4"
    };
}
macro_rules! d1 {
    () => {
        "x5"
    };
}
macro_rules! d2 {
    () => {
        "x6"
    };
}
macro_rules! d3 {
    () => {
        "x7"
    };
}
macro_rules! d4 {
    () => {
        "x8"
    };
}
macro_rules! d5 {
    () => {
        "x9"
    };
}

/// Negate modulo p_384, z := (-x) mod p_384, assuming x reduced
///
/// Input x[6]; output z[6]
pub(crate) fn bignum_neg_p384(z: &mut [u64; 6], x: &[u64; 6]) {
    // SAFETY: inline assembly. see [crate::low::inline_assembly_safety] for safety info.
    unsafe {
        core::arch::asm!(

        // Load the 6 digits of x

        Q!("    ldp             " d0!() ", " d1!() ", [" x!() "]"),
        Q!("    ldp             " d2!() ", " d3!() ", [" x!() ", #16]"),
        Q!("    ldp             " d4!() ", " d5!() ", [" x!() ", #32]"),

        // Set a bitmask p for the input being nonzero, so that we avoid doing
        // -0 = p_384 and hence maintain strict modular reduction

        Q!("    orr             " p!() ", " d0!() ", " d1!()),
        Q!("    orr             " t!() ", " d2!() ", " d3!()),
        Q!("    orr             " p!() ", " p!() ", " t!()),
        Q!("    orr             " t!() ", " d4!() ", " d5!()),
        Q!("    orr             " p!() ", " p!() ", " t!()),
        Q!("    cmp             " p!() ", #0"),
        Q!("    csetm           " p!() ", ne"),

        // Mask the complicated lower three words of p_384 = [-1;-1;-1;n2;n1;n0]
        // and subtract, using mask itself for upper digits

        Q!("    and             " t!() ", " p!() ", #0x00000000ffffffff"),
        Q!("    subs            " d0!() ", " t!() ", " d0!()),
        Q!("    and             " t!() ", " p!() ", #0xffffffff00000000"),
        Q!("    sbcs            " d1!() ", " t!() ", " d1!()),
        Q!("    and             " t!() ", " p!() ", #0xfffffffffffffffe"),
        Q!("    sbcs            " d2!() ", " t!() ", " d2!()),
        Q!("    sbcs            " d3!() ", " p!() ", " d3!()),
        Q!("    sbcs            " d4!() ", " p!() ", " d4!()),
        Q!("    sbc             " d5!() ", " p!() ", " d5!()),

        // Write back the result

        Q!("    stp             " d0!() ", " d1!() ", [" z!() "]"),
        Q!("    stp             " d2!() ", " d3!() ", [" z!() ", #16]"),
        Q!("    stp             " d4!() ", " d5!() ", [" z!() ", #32]"),

        // Return

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
