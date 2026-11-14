// generated source. do not edit.
#![allow(non_upper_case_globals, unused_macros, unused_imports)]
use crate::low::macros::*;

// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

// ----------------------------------------------------------------------------
// Negate modulo p_25519, z := (-x) mod p_25519, assuming x reduced
// Input x[4]; output z[4]
//
//    extern void bignum_neg_p25519(uint64_t z[static 4], const uint64_t x[static 4]);
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
macro_rules! c {
    () => {
        "x6"
    };
}
macro_rules! d {
    () => {
        "x7"
    };
}

/// Negate modulo p_25519, z := (-x) mod p_25519, assuming x reduced
///
/// Input x[4]; output z[4]
pub(crate) fn bignum_neg_p25519(z: &mut [u64; 4], x: &[u64; 4]) {
    // SAFETY: inline assembly. see [crate::low::inline_assembly_safety] for safety info.
    unsafe {
        core::arch::asm!(

        // Load the digits of x and compute [d3;d2;d1;d0] = (2^255 - 19) - x
        // while also computing c = the OR of the digits of x

        Q!("    ldp             " d0!() ", " d1!() ", [" x!() "]"),
        Q!("    mov             " d!() ", #-19"),
        Q!("    orr             " c!() ", " d0!() ", " d1!()),
        Q!("    subs            " d0!() ", " d!() ", " d0!()),
        Q!("    mov             " d!() ", #-1"),
        Q!("    sbcs            " d1!() ", " d!() ", " d1!()),
        Q!("    ldp             " d2!() ", " d3!() ", [" x!() ", #16]"),
        Q!("    orr             " c!() ", " c!() ", " d2!()),
        Q!("    sbcs            " d2!() ", " d!() ", " d2!()),
        Q!("    mov             " d!() ", #0x7FFFFFFFFFFFFFFF"),
        Q!("    orr             " c!() ", " c!() ", " d3!()),
        Q!("    sbc             " d3!() ", " d!() ", " d3!()),

        // If in fact c = 0 then the result is zero, otherwise the main result

        Q!("    cmp             " c!() ", xzr"),
        Q!("    csel            " d0!() ", " d0!() ", xzr, ne"),
        Q!("    csel            " d1!() ", " d1!() ", xzr, ne"),
        Q!("    csel            " d2!() ", " d2!() ", xzr, ne"),
        Q!("    csel            " d3!() ", " d3!() ", xzr, ne"),

        // Write back result and return

        Q!("    stp             " d0!() ", " d1!() ", [" z!() "]"),
        Q!("    stp             " d2!() ", " d3!() ", [" z!() ", #16]"),

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
