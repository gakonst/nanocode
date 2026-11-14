// generated source. do not edit.
#![allow(non_upper_case_globals, unused_macros, unused_imports)]
use crate::low::macros::*;

// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

// ----------------------------------------------------------------------------
// Negate modulo p_256, z := (-x) mod p_256, assuming x reduced
// Input x[4]; output z[4]
//
//    extern void bignum_neg_p256(uint64_t z[static 4], const uint64_t x[static 4]);
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

/// Negate modulo p_256, z := (-x) mod p_256, assuming x reduced
///
/// Input x[4]; output z[4]
pub(crate) fn bignum_neg_p256(z: &mut [u64; 4], x: &[u64; 4]) {
    // SAFETY: inline assembly. see [crate::low::inline_assembly_safety] for safety info.
    unsafe {
        core::arch::asm!(

        // Load the 4 digits of x

        Q!("    ldp             " d0!() ", " d1!() ", [" x!() "]"),
        Q!("    ldp             " d2!() ", " d3!() ", [" x!() ", #16]"),

        // Set a bitmask p for the input being nonzero, so that we avoid doing
        // -0 = p_256 and hence maintain strict modular reduction

        Q!("    orr             " t!() ", " d0!() ", " d1!()),
        Q!("    orr             " p!() ", " d2!() ", " d3!()),
        Q!("    orr             " p!() ", " p!() ", " t!()),
        Q!("    cmp             " p!() ", #0"),
        Q!("    csetm           " p!() ", ne"),

        // Mask the nontrivial words of p_256 = [n3;0;n1;-1] and subtract

        Q!("    subs            " d0!() ", " p!() ", " d0!()),
        Q!("    and             " t!() ", " p!() ", #0x00000000ffffffff"),
        Q!("    sbcs            " d1!() ", " t!() ", " d1!()),
        Q!("    sbcs            " d2!() ", xzr, " d2!()),
        Q!("    and             " t!() ", " p!() ", #0xffffffff00000001"),
        Q!("    sbc             " d3!() ", " t!() ", " d3!()),

        // Write back the result

        Q!("    stp             " d0!() ", " d1!() ", [" z!() "]"),
        Q!("    stp             " d2!() ", " d3!() ", [" z!() ", #16]"),

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
            )
    };
}
