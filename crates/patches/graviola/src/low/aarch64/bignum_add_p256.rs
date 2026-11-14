// generated source. do not edit.
#![allow(non_upper_case_globals, unused_macros, unused_imports)]
use crate::low::macros::*;

// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

// ----------------------------------------------------------------------------
// Add modulo p_256, z := (x + y) mod p_256, assuming x and y reduced
// Inputs x[4], y[4]; output z[4]
//
//    extern void bignum_add_p256(uint64_t z[static 4], const uint64_t x[static 4],
//                                const uint64_t y[static 4]);
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
macro_rules! n0 {
    () => {
        "x8"
    };
}
macro_rules! n1 {
    () => {
        "x9"
    };
}
macro_rules! n2 {
    () => {
        "x10"
    };
}
macro_rules! n3 {
    () => {
        "x11"
    };
}

/// Add modulo p_256, z := (x + y) mod p_256, assuming x and y reduced
///
/// Inputs x[4], y[4]; output z[4]
pub(crate) fn bignum_add_p256(z: &mut [u64; 4], x: &[u64; 4], y: &[u64; 4]) {
    // SAFETY: inline assembly. see [crate::low::inline_assembly_safety] for safety info.
    unsafe {
        core::arch::asm!(

        // First just add the numbers as [c;d3;d2;d1;d0]

        Q!("    ldp             " d0!() ", " d1!() ", [" x!() "]"),
        Q!("    ldp             " n0!() ", " n1!() ", [" y!() "]"),
        Q!("    adds            " d0!() ", " d0!() ", " n0!()),
        Q!("    adcs            " d1!() ", " d1!() ", " n1!()),
        Q!("    ldp             " d2!() ", " d3!() ", [" x!() ", #16]"),
        Q!("    ldp             " n2!() ", " n3!() ", [" y!() ", #16]"),
        Q!("    adcs            " d2!() ", " d2!() ", " n2!()),
        Q!("    adcs            " d3!() ", " d3!() ", " n3!()),
        Q!("    adc             " c!() ", xzr, xzr"),

        // Now let [c;n3;n2;n1;n0] = [c;d3;d2;d1;d0] - p_256

        Q!("    subs            " n0!() ", " d0!() ", #0xffffffffffffffff"),
        Q!("    mov             " n1!() ", #0x00000000ffffffff"),
        Q!("    sbcs            " n1!() ", " d1!() ", " n1!()),
        Q!("    sbcs            " n2!() ", " d2!() ", xzr"),
        Q!("    mov             " n3!() ", #0xffffffff00000001"),
        Q!("    sbcs            " n3!() ", " d3!() ", " n3!()),
        Q!("    sbcs            " c!() ", " c!() ", xzr"),

        // Select result according to whether (x + y) - p_256 < 0

        Q!("    csel            " d0!() ", " d0!() ", " n0!() ", cc"),
        Q!("    csel            " d1!() ", " d1!() ", " n1!() ", cc"),
        Q!("    csel            " d2!() ", " d2!() ", " n2!() ", cc"),
        Q!("    csel            " d3!() ", " d3!() ", " n3!() ", cc"),

        // Store the result

        Q!("    stp             " d0!() ", " d1!() ", [" z!() "]"),
        Q!("    stp             " d2!() ", " d3!() ", [" z!() ", #16]"),

        inout("x0") z.as_mut_ptr() => _,
        inout("x1") x.as_ptr() => _,
        inout("x2") y.as_ptr() => _,
        // clobbers
        out("x10") _,
        out("x11") _,
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
