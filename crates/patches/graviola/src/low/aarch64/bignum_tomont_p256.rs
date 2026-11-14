// generated source. do not edit.
#![allow(non_upper_case_globals, unused_macros, unused_imports)]
use crate::low::macros::*;

// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

// ----------------------------------------------------------------------------
// Convert to Montgomery form z := (2^256 * x) mod p_256
// Input x[4]; output z[4]
//
//    extern void bignum_tomont_p256(uint64_t z[static 4],
//                                   const uint64_t x[static 4]);
//
// Standard ARM ABI: X0 = z, X1 = x
// ----------------------------------------------------------------------------

// ----------------------------------------------------------------------------
// Core "x |-> (2^64 * x) mod p_256" macro, with x assumed to be < p_256.
// Input is in [d4;d3;d2;d1] and output in [d3;d2;d1;d0]
// using d4 as well as t1, t2, t3 as temporaries.
// ----------------------------------------------------------------------------

macro_rules! modstep_p256 {
    ($d4:expr, $d3:expr, $d2:expr, $d1:expr, $d0:expr, $t1:expr, $t2:expr, $t3:expr) => { Q!(
        /* Writing the input as z = 2^256 * h + 2^192 * l + t = 2^192 * hl + t,  */
        /* our quotient approximation is MIN ((hl + hl>>32 + 1)>>64) (2^64 - 1). */
        "subs xzr, xzr, xzr;\n"
        /* Set carry flag for +1 */
        "extr " $t3 ", " $d4 ", " $d3 ", #32;\n"
        "adcs xzr, " $d3 ", " $t3 ";\n"
        "lsr " $t3 ", " $d4 ", #32;\n"
        "adcs " $t3 ", " $d4 ", " $t3 ";\n"
        "csetm " $d0 ", cs;\n"
        "orr " $t3 ", " $t3 ", " $d0 ";\n"
        /* First do [t2;t1] = 2^32 * q, which we use twice                       */
        "lsl " $t1 ", " $t3 ", #32;\n"
        "lsr " $t2 ", " $t3 ", #32;\n"
        /* Add 2^224 * q to sum                                                  */
        "adds " $d3 ", " $d3 ", " $t1 ";\n"
        "adc " $d4 ", " $d4 ", " $t2 ";\n"
        /* Accumulate [t2;t1;d0] = (2^96 - 1) * q                                */
        "subs " $d0 ", xzr, " $t3 ";\n"
        "sbcs " $t1 ", " $t1 ", xzr;\n"
        "sbc " $t2 ", " $t2 ", xzr;\n"
        /* Subtract (2^256 + 2^192 + 2^96 - 1) * q                               */
        "subs " $d0 ", xzr, " $d0 ";\n"
        "sbcs " $d1 ", " $d1 ", " $t1 ";\n"
        "sbcs " $d2 ", " $d2 ", " $t2 ";\n"
        "sbcs " $d3 ", " $d3 ", " $t3 ";\n"
        "sbcs " $d4 ", " $d4 ", " $t3 ";\n"
        /* Use top word as mask to correct                                       */
        "adds " $d0 ", " $d0 ", " $d4 ";\n"
        "mov " $t1 ", #0x00000000ffffffff;\n"
        "and " $t1 ", " $t1 ", " $d4 ";\n"
        "adcs " $d1 ", " $d1 ", " $t1 ";\n"
        "adcs " $d2 ", " $d2 ", xzr;\n"
        "mov " $t1 ", #0xffffffff00000001;\n"
        "and " $t1 ", " $t1 ", " $d4 ";\n"
        "adc " $d3 ", " $d3 ", " $t1
    )}
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
macro_rules! d4 {
    () => {
        "x6"
    };
}

macro_rules! t0 {
    () => {
        "x1"
    };
}
macro_rules! t1 {
    () => {
        "x7"
    };
}
macro_rules! t2 {
    () => {
        "x8"
    };
}
macro_rules! t3 {
    () => {
        "x9"
    };
}

/// Convert to Montgomery form z := (2^256 * x) mod p_256
///
/// Input x[4]; output z[4]
pub(crate) fn bignum_tomont_p256(z: &mut [u64; 4], x: &[u64; 4]) {
    // SAFETY: inline assembly. see [crate::low::inline_assembly_safety] for safety info.
    unsafe {
        core::arch::asm!(

        Q!(Label!("bignum_tomont_p256_alt", 2) ":"),

        // Load the input

        Q!("    ldp             " d0!() ", " d1!() ", [x1]"),
        Q!("    ldp             " d2!() ", " d3!() ", [x1, #16]"),

        // Do an initial reduction to make sure this is < p_256, using just
        // a copy of the bignum_mod_p256_4 code. This is needed to set up the
        // invariant "input < p_256" for the main modular reduction steps.

        Q!("    mov             " t0!() ", #0xffffffffffffffff"),
        Q!("    mov             " t1!() ", #0x00000000ffffffff"),
        Q!("    mov             " t3!() ", #0xffffffff00000001"),
        Q!("    subs            " t0!() ", " d0!() ", " t0!()),
        Q!("    sbcs            " t1!() ", " d1!() ", " t1!()),
        Q!("    sbcs            " t2!() ", " d2!() ", xzr"),
        Q!("    sbcs            " t3!() ", " d3!() ", " t3!()),
        Q!("    csel            " d0!() ", " d0!() ", " t0!() ", cc"),
        Q!("    csel            " d1!() ", " d1!() ", " t1!() ", cc"),
        Q!("    csel            " d2!() ", " d2!() ", " t2!() ", cc"),
        Q!("    csel            " d3!() ", " d3!() ", " t3!() ", cc"),

        // Successively multiply by 2^64 and reduce

        modstep_p256!(d3!(), d2!(), d1!(), d0!(), d4!(), t1!(), t2!(), t3!()),
        modstep_p256!(d2!(), d1!(), d0!(), d4!(), d3!(), t1!(), t2!(), t3!()),
        modstep_p256!(d1!(), d0!(), d4!(), d3!(), d2!(), t1!(), t2!(), t3!()),
        modstep_p256!(d0!(), d4!(), d3!(), d2!(), d1!(), t1!(), t2!(), t3!()),

        // Store the result and return

        Q!("    stp             " d1!() ", " d2!() ", [x0]"),
        Q!("    stp             " d3!() ", " d4!() ", [x0, #16]"),

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
