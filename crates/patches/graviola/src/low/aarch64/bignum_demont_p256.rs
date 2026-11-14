// generated source. do not edit.
#![allow(non_upper_case_globals, unused_macros, unused_imports)]
use crate::low::macros::*;

// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

// ----------------------------------------------------------------------------
// Convert from Montgomery form z := (x / 2^256) mod p_256, assuming x reduced
// Input x[4]; output z[4]
//
//    extern void bignum_demont_p256(uint64_t z[static 4],
//                                   const uint64_t x[static 4]);
//
// This assumes the input is < p_256 for correctness. If this is not the case,
// use the variant "bignum_deamont_p256" instead.
//
// Standard ARM ABI: X0 = z, X1 = x
// ----------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Core one-step "short" Montgomery reduction macro. Takes input in
// [d3;d2;d1;d0] and returns result in [d4;d3;d2;d1], adding to the
// existing contents of [d3;d2;d1] and generating d4 from zero, re-using
// d0 as a temporary internally together with t0, t1 and t2.
// It is fine for d4 to be the same register as d0, and it often is.
// ---------------------------------------------------------------------------

macro_rules! montreds {
    ($d4:expr, $d3:expr, $d2:expr, $d1:expr, $d0:expr, $t2:expr, $t1:expr, $t0:expr) => { Q!(
        /* Let w = d0, the original word we use as offset; d0 gets recycled      */
        /* First let [t2;t1] = 2^32 * w                                          */
        /* then let [d0;t0] = (2^64 - 2^32 + 1) * w (overwrite old d0)           */
        "lsl " $t1 ", " $d0 ", #32;\n"
        "subs " $t0 ", " $d0 ", " $t1 ";\n"
        "lsr " $t2 ", " $d0 ", #32;\n"
        "sbc " $d0 ", " $d0 ", " $t2 ";\n"
        /* Hence [d4;..;d1] := [d3;d2;d1;0] + (2^256 - 2^224 + 2^192 + 2^96) * w */
        "adds " $d1 ", " $d1 ", " $t1 ";\n"
        "adcs " $d2 ", " $d2 ", " $t2 ";\n"
        "adcs " $d3 ", " $d3 ", " $t0 ";\n"
        "adc " $d4 ", " $d0 ", xzr"
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

// Rotating registers for the intermediate windows (with repetitions)

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

// Other temporaries

macro_rules! u {
    () => {
        "x6"
    };
}
macro_rules! v {
    () => {
        "x7"
    };
}
macro_rules! w {
    () => {
        "x8"
    };
}

/// Convert from Montgomery form z := (x / 2^256) mod p_256, assuming x reduced
///
/// Input x[4]; output z[4]
///
/// This assumes the input is < p_256 for correctness. If this is not the case,
/// use the variant "bignum_deamont_p256" instead.
pub(crate) fn bignum_demont_p256(z: &mut [u64; 4], x: &[u64; 4]) {
    // SAFETY: inline assembly. see [crate::low::inline_assembly_safety] for safety info.
    unsafe {
        core::arch::asm!(

        Q!(Label!("bignum_demont_p256_alt", 2) ":"),

        // Set up an initial window with the input x and an extra leading zero

        Q!("    ldp             " d0!() ", " d1!() ", [" x!() "]"),
        Q!("    ldp             " d2!() ", " d3!() ", [" x!() ", #16]"),

        // Systematically scroll left doing 1-step reductions

        montreds!(d0!(), d3!(), d2!(), d1!(), d0!(), u!(), v!(), w!()),

        montreds!(d1!(), d0!(), d3!(), d2!(), d1!(), u!(), v!(), w!()),

        montreds!(d2!(), d1!(), d0!(), d3!(), d2!(), u!(), v!(), w!()),

        montreds!(d3!(), d2!(), d1!(), d0!(), d3!(), u!(), v!(), w!()),

        // Write back result

        Q!("    stp             " d0!() ", " d1!() ", [" z!() "]"),
        Q!("    stp             " d2!() ", " d3!() ", [" z!() ", #16]"),

        inout("x0") z.as_mut_ptr() => _,
        inout("x1") x.as_ptr() => _,
        // clobbers
        out("v4") _,
        out("x2") _,
        out("x3") _,
        out("x4") _,
        out("x5") _,
        out("x6") _,
        out("x7") _,
        out("x8") _,
            )
    };
}
