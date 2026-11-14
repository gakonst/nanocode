// generated source. do not edit.
#![allow(non_upper_case_globals, unused_macros, unused_imports)]
use crate::low::macros::*;

// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

// ----------------------------------------------------------------------------
// Convert to Montgomery form z := (2^384 * x) mod p_384
// Input x[6]; output z[6]
//
//    extern void bignum_tomont_p384(uint64_t z[static 6],
//                                   const uint64_t x[static 6]);
//
// Standard ARM ABI: X0 = z, X1 = x
// ----------------------------------------------------------------------------

// ----------------------------------------------------------------------------
// Core "x |-> (2^64 * x) mod p_384" macro, with x assumed to be < p_384.
// Input is in [d6;d5;d4;d3;d2;d1] and output in [d5;d4;d3;d2;d1;d0]
// using d6 as well as t1, t2, t3 as temporaries.
// ----------------------------------------------------------------------------

macro_rules! modstep_p384 {
    ($d6:expr, $d5:expr, $d4:expr, $d3:expr, $d2:expr, $d1:expr, $d0:expr, $t1:expr, $t2:expr, $t3:expr) => { Q!(
        /* Initial quotient approximation q = min (h + 1) (2^64 - 1) */
        "adds " $d6 ", " $d6 ", #1;\n"
        "csetm " $t3 ", cs;\n"
        "add " $d6 ", " $d6 ", " $t3 ";\n"
        "orn " $t3 ", xzr, " $t3 ";\n"
        "sub " $t2 ", " $d6 ", #1;\n"
        "sub " $t1 ", xzr, " $d6 ";\n"
        /* Correction term [d6;t2;t1;d0] = q * (2^384 - p_384) */
        "lsl " $d0 ", " $t1 ", #32;\n"
        "extr " $t1 ", " $t2 ", " $t1 ", #32;\n"
        "lsr " $t2 ", " $t2 ", #32;\n"
        "adds " $d0 ", " $d0 ", " $d6 ";\n"
        "adcs " $t1 ", " $t1 ", xzr;\n"
        "adcs " $t2 ", " $t2 ", " $d6 ";\n"
        "adc " $d6 ", xzr, xzr;\n"
        /* Addition to the initial value */
        "adds " $d1 ", " $d1 ", " $t1 ";\n"
        "adcs " $d2 ", " $d2 ", " $t2 ";\n"
        "adcs " $d3 ", " $d3 ", " $d6 ";\n"
        "adcs " $d4 ", " $d4 ", xzr;\n"
        "adcs " $d5 ", " $d5 ", xzr;\n"
        "adc " $t3 ", " $t3 ", xzr;\n"
        /* Use net top of the 7-word answer in t3 for masked correction */
        "mov " $t1 ", #0x00000000ffffffff;\n"
        "and " $t1 ", " $t1 ", " $t3 ";\n"
        "adds " $d0 ", " $d0 ", " $t1 ";\n"
        "eor " $t1 ", " $t1 ", " $t3 ";\n"
        "adcs " $d1 ", " $d1 ", " $t1 ";\n"
        "mov " $t1 ", #0xfffffffffffffffe;\n"
        "and " $t1 ", " $t1 ", " $t3 ";\n"
        "adcs " $d2 ", " $d2 ", " $t1 ";\n"
        "adcs " $d3 ", " $d3 ", " $t3 ";\n"
        "adcs " $d4 ", " $d4 ", " $t3 ";\n"
        "adc " $d5 ", " $d5 ", " $t3
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
macro_rules! d5 {
    () => {
        "x7"
    };
}
macro_rules! d6 {
    () => {
        "x8"
    };
}
macro_rules! t1 {
    () => {
        "x9"
    };
}
macro_rules! t2 {
    () => {
        "x10"
    };
}
macro_rules! t3 {
    () => {
        "x11"
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
macro_rules! n4 {
    () => {
        "x12"
    };
}
macro_rules! n5 {
    () => {
        "x1"
    };
}

/// Convert to Montgomery form z := (2^384 * x) mod p_384
///
/// Input x[6]; output z[6]
pub(crate) fn bignum_tomont_p384(z: &mut [u64; 6], x: &[u64; 6]) {
    // SAFETY: inline assembly. see [crate::low::inline_assembly_safety] for safety info.
    unsafe {
        core::arch::asm!(

        Q!(Label!("bignum_tomont_p384_alt", 2) ":"),

        // <macro definition d0 hoisted upwards>
        // <macro definition d1 hoisted upwards>
        // <macro definition d2 hoisted upwards>
        // <macro definition d3 hoisted upwards>
        // <macro definition d4 hoisted upwards>
        // <macro definition d5 hoisted upwards>
        // <macro definition d6 hoisted upwards>

        // <macro definition t1 hoisted upwards>
        // <macro definition t2 hoisted upwards>
        // <macro definition t3 hoisted upwards>

        // <macro definition n0 hoisted upwards>
        // <macro definition n1 hoisted upwards>
        // <macro definition n2 hoisted upwards>
        // <macro definition n3 hoisted upwards>
        // <macro definition n4 hoisted upwards>
        // <macro definition n5 hoisted upwards>

        // Load the inputs

        Q!("    ldp             " d0!() ", " d1!() ", [x1]"),
        Q!("    ldp             " d2!() ", " d3!() ", [x1, #16]"),
        Q!("    ldp             " d4!() ", " d5!() ", [x1, #32]"),

        // Do an initial reduction to make sure this is < p_384, using just
        // a copy of the bignum_mod_p384_6 code. This is needed to set up the
        // invariant "input < p_384" for the main modular reduction steps.

        Q!("    mov             " n0!() ", #0x00000000ffffffff"),
        Q!("    mov             " n1!() ", #0xffffffff00000000"),
        Q!("    mov             " n2!() ", #0xfffffffffffffffe"),
        Q!("    subs            " n0!() ", " d0!() ", " n0!()),
        Q!("    sbcs            " n1!() ", " d1!() ", " n1!()),
        Q!("    sbcs            " n2!() ", " d2!() ", " n2!()),
        Q!("    adcs            " n3!() ", " d3!() ", xzr"),
        Q!("    adcs            " n4!() ", " d4!() ", xzr"),
        Q!("    adcs            " n5!() ", " d5!() ", xzr"),
        Q!("    csel            " d0!() ", " d0!() ", " n0!() ", cc"),
        Q!("    csel            " d1!() ", " d1!() ", " n1!() ", cc"),
        Q!("    csel            " d2!() ", " d2!() ", " n2!() ", cc"),
        Q!("    csel            " d3!() ", " d3!() ", " n3!() ", cc"),
        Q!("    csel            " d4!() ", " d4!() ", " n4!() ", cc"),
        Q!("    csel            " d5!() ", " d5!() ", " n5!() ", cc"),

        // Successively multiply by 2^64 and reduce

        modstep_p384!(d5!(), d4!(), d3!(), d2!(), d1!(), d0!(), d6!(), t1!(), t2!(), t3!()),
        modstep_p384!(d4!(), d3!(), d2!(), d1!(), d0!(), d6!(), d5!(), t1!(), t2!(), t3!()),
        modstep_p384!(d3!(), d2!(), d1!(), d0!(), d6!(), d5!(), d4!(), t1!(), t2!(), t3!()),
        modstep_p384!(d2!(), d1!(), d0!(), d6!(), d5!(), d4!(), d3!(), t1!(), t2!(), t3!()),
        modstep_p384!(d1!(), d0!(), d6!(), d5!(), d4!(), d3!(), d2!(), t1!(), t2!(), t3!()),
        modstep_p384!(d0!(), d6!(), d5!(), d4!(), d3!(), d2!(), d1!(), t1!(), t2!(), t3!()),

        // Store the result and return

        Q!("    stp             " d1!() ", " d2!() ", [x0]"),
        Q!("    stp             " d3!() ", " d4!() ", [x0, #16]"),
        Q!("    stp             " d5!() ", " d6!() ", [x0, #32]"),

        inout("x0") z.as_mut_ptr() => _,
        inout("x1") x.as_ptr() => _,
        // clobbers
        out("x10") _,
        out("x11") _,
        out("x12") _,
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
