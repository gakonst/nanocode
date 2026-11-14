// generated source. do not edit.
#![allow(non_upper_case_globals, unused_macros, unused_imports)]
use crate::low::macros::*;

// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

// ----------------------------------------------------------------------------
// Add modulo p_384, z := (x + y) mod p_384, assuming x and y reduced
// Inputs x[6], y[6]; output z[6]
//
//    extern void bignum_add_p384(uint64_t z[static 6], const uint64_t x[static 6],
//                                const uint64_t y[static 6]);
//
// Standard ARM ABI: X0 = z, X1 = x, X2 = y
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
macro_rules! y {
    () => {
        "x2"
    };
}
macro_rules! c {
    () => {
        "x3"
    };
}
macro_rules! l {
    () => {
        "x4"
    };
}
macro_rules! d0 {
    () => {
        "x5"
    };
}
macro_rules! d1 {
    () => {
        "x6"
    };
}
macro_rules! d2 {
    () => {
        "x7"
    };
}
macro_rules! d3 {
    () => {
        "x8"
    };
}
macro_rules! d4 {
    () => {
        "x9"
    };
}
macro_rules! d5 {
    () => {
        "x10"
    };
}

/// Add modulo p_384, z := (x + y) mod p_384, assuming x and y reduced
///
/// Inputs x[6], y[6]; output z[6]
pub(crate) fn bignum_add_p384(z: &mut [u64; 6], x: &[u64; 6], y: &[u64; 6]) {
    // SAFETY: inline assembly. see [crate::low::inline_assembly_safety] for safety info.
    unsafe {
        core::arch::asm!(

        // First just add the numbers as c + [d5; d4; d3; d2; d1; d0]

        Q!("    ldp             " d0!() ", " d1!() ", [" x!() "]"),
        Q!("    ldp             " l!() ", " c!() ", [" y!() "]"),
        Q!("    adds            " d0!() ", " d0!() ", " l!()),
        Q!("    adcs            " d1!() ", " d1!() ", " c!()),
        Q!("    ldp             " d2!() ", " d3!() ", [" x!() ", #16]"),
        Q!("    ldp             " l!() ", " c!() ", [" y!() ", #16]"),
        Q!("    adcs            " d2!() ", " d2!() ", " l!()),
        Q!("    adcs            " d3!() ", " d3!() ", " c!()),
        Q!("    ldp             " d4!() ", " d5!() ", [" x!() ", #32]"),
        Q!("    ldp             " l!() ", " c!() ", [" y!() ", #32]"),
        Q!("    adcs            " d4!() ", " d4!() ", " l!()),
        Q!("    adcs            " d5!() ", " d5!() ", " c!()),
        Q!("    adc             " c!() ", xzr, xzr"),

        // Now compare [d5; d4; d3; d2; d1; d0] with p_384

        Q!("    mov             " l!() ", #0x00000000ffffffff"),
        Q!("    subs            " "xzr, " d0!() ", " l!()),
        Q!("    mov             " l!() ", #0xffffffff00000000"),
        Q!("    sbcs            " "xzr, " d1!() ", " l!()),
        Q!("    mov             " l!() ", #0xfffffffffffffffe"),
        Q!("    sbcs            " "xzr, " d2!() ", " l!()),
        Q!("    adcs            " "xzr, " d3!() ", xzr"),
        Q!("    adcs            " "xzr, " d4!() ", xzr"),
        Q!("    adcs            " "xzr, " d5!() ", xzr"),

        // Now CF is set (because of inversion) if (x + y) % 2^384 >= p_384
        // Thus we want to correct if either this is set or the original carry c was

        Q!("    adcs            " c!() ", " c!() ", xzr"),
        Q!("    csetm           " c!() ", ne"),

        // Now correct by subtracting masked p_384

        Q!("    mov             " l!() ", #0x00000000ffffffff"),
        Q!("    and             " l!() ", " l!() ", " c!()),
        Q!("    subs            " d0!() ", " d0!() ", " l!()),
        Q!("    eor             " l!() ", " l!() ", " c!()),
        Q!("    sbcs            " d1!() ", " d1!() ", " l!()),
        Q!("    mov             " l!() ", #0xfffffffffffffffe"),
        Q!("    and             " l!() ", " l!() ", " c!()),
        Q!("    sbcs            " d2!() ", " d2!() ", " l!()),
        Q!("    sbcs            " d3!() ", " d3!() ", " c!()),
        Q!("    sbcs            " d4!() ", " d4!() ", " c!()),
        Q!("    sbc             " d5!() ", " d5!() ", " c!()),

        // Store the result

        Q!("    stp             " d0!() ", " d1!() ", [" z!() "]"),
        Q!("    stp             " d2!() ", " d3!() ", [" z!() ", #16]"),
        Q!("    stp             " d4!() ", " d5!() ", [" z!() ", #32]"),

        inout("x0") z.as_mut_ptr() => _,
        inout("x1") x.as_ptr() => _,
        inout("x2") y.as_ptr() => _,
        // clobbers
        out("x10") _,
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
