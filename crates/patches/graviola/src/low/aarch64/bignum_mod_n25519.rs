// generated source. do not edit.
#![allow(non_upper_case_globals, unused_macros, unused_imports)]
use crate::low::macros::*;

// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

// ----------------------------------------------------------------------------
// Reduce modulo basepoint order, z := x mod n_25519
// Input x[k]; output z[4]
//
//    extern void bignum_mod_n25519(uint64_t z[static 4], uint64_t k,
//                                  const uint64_t *x);
//
// Reduction is modulo the order of the curve25519/edwards25519 basepoint,
// which is n_25519 = 2^252 + 27742317777372353535851937790883648493
//
// Standard ARM ABI: X0 = z, X1 = k, X2 = x
// ----------------------------------------------------------------------------

macro_rules! z {
    () => {
        "x0"
    };
}
macro_rules! k {
    () => {
        "x1"
    };
}
macro_rules! x {
    () => {
        "x2"
    };
}

macro_rules! m0 {
    () => {
        "x3"
    };
}
macro_rules! m1 {
    () => {
        "x4"
    };
}
macro_rules! m2 {
    () => {
        "x5"
    };
}
macro_rules! m3 {
    () => {
        "x6"
    };
}

macro_rules! t0 {
    () => {
        "x7"
    };
}
macro_rules! t1 {
    () => {
        "x8"
    };
}
macro_rules! t2 {
    () => {
        "x9"
    };
}
macro_rules! t3 {
    () => {
        "x10"
    };
}

macro_rules! n0 {
    () => {
        "x11"
    };
}
macro_rules! n1 {
    () => {
        "x12"
    };
}

// These two are aliased: we only load d when finished with q

macro_rules! q {
    () => {
        "x13"
    };
}
macro_rules! d {
    () => {
        "x13"
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

/// Reduce modulo basepoint order, z := x mod n_25519
///
/// Input x[k]; output z[4]
///
/// Reduction is modulo the order of the curve25519/edwards25519 basepoint,
/// which is n_25519 = 2^252 + 27742317777372353535851937790883648493
pub(crate) fn bignum_mod_n25519(z: &mut [u64; 4], x: &[u64]) {
    // SAFETY: inline assembly. see [crate::low::inline_assembly_safety] for safety info.
    unsafe {
        core::arch::asm!(

        // If the input is already <= 3 words long, go to a trivial "copy" path

        Q!("    cmp             " k!() ", #4"),
        Q!("    bcc             " Label!("Lbignum_mod_n25519_short", 2, After)),

        // Otherwise load the top 4 digits (top-down) and reduce k by 4
        // This [m3;m2;m1;m0] is the initial x where we begin reduction.

        Q!("    sub             " k!() ", " k!() ", #4"),
        Q!("    lsl             " t0!() ", " k!() ", #3"),
        Q!("    add             " t0!() ", " t0!() ", " x!()),
        Q!("    ldp             " m2!() ", " m3!() ", [" t0!() ", #16]"),
        Q!("    ldp             " m0!() ", " m1!() ", [" t0!() "]"),

        // Load the complicated two words of n_25519 = 2^252 + [n1; n0]

        movbig!(n0!(), "#0x5812", "#0x631a", "#0x5cf5", "#0xd3ed"),
        movbig!(n1!(), "#0x14de", "#0xf9de", "#0xa2f7", "#0x9cd6"),

        // Get the quotient estimate q = floor(x/2^252).
        // Also delete it from m3, in effect doing x' = x - q * 2^252

        Q!("    lsr             " q!() ", " m3!() ", #60"),
        Q!("    and             " m3!() ", " m3!() ", #0x0FFFFFFFFFFFFFFF"),

        // Multiply [t2;t1;t0] = q * [n1;n0]

        Q!("    mul             " t0!() ", " n0!() ", " q!()),
        Q!("    mul             " t1!() ", " n1!() ", " q!()),
        Q!("    umulh           " t2!() ", " n0!() ", " q!()),
        Q!("    adds            " t1!() ", " t1!() ", " t2!()),
        Q!("    umulh           " t2!() ", " n1!() ", " q!()),
        Q!("    adc             " t2!() ", " t2!() ", xzr"),

        // Subtract [m3;m2;m1;m0] = x' - q * [n1;n0] = x - q * n_25519

        Q!("    subs            " m0!() ", " m0!() ", " t0!()),
        Q!("    sbcs            " m1!() ", " m1!() ", " t1!()),
        Q!("    sbcs            " m2!() ", " m2!() ", " t2!()),
        Q!("    sbcs            " m3!() ", " m3!() ", xzr"),

        // If this borrows (CF = 0 because of inversion), add back n_25519.
        // The masked n3 digit exploits the fact that bit 60 of n0 is set.

        Q!("    csel            " t0!() ", " n0!() ", xzr, cc"),
        Q!("    csel            " t1!() ", " n1!() ", xzr, cc"),
        Q!("    adds            " m0!() ", " m0!() ", " t0!()),
        Q!("    adcs            " m1!() ", " m1!() ", " t1!()),
        Q!("    and             " t0!() ", " t0!() ", #0x1000000000000000"),
        Q!("    adcs            " m2!() ", " m2!() ", xzr"),
        Q!("    adc             " m3!() ", " m3!() ", " t0!()),

        // Now do (k-4) iterations of 5->4 word modular reduction. Each one
        // is similar to the sequence above except for the more refined quotient
        // estimation process.

        Q!("    cbz             " k!() ", " Label!("Lbignum_mod_n25519_writeback", 3, After)),

        Q!(Label!("Lbignum_mod_n25519_loop", 4) ":"),

        // Assume that the new 5-digit x is 2^64 * previous_x + next_digit.
        // Get the quotient estimate q = max (floor(x/2^252)) (2^64 - 1)
        // and first compute x' = x - 2^252 * q.

        Q!("    extr            " q!() ", " m3!() ", " m2!() ", #60"),
        Q!("    and             " m2!() ", " m2!() ", #0x0FFFFFFFFFFFFFFF"),
        Q!("    sub             " q!() ", " q!() ", " m3!() ", lsr #60"),
        Q!("    and             " m3!() ", " m3!() ", #0xF000000000000000"),
        Q!("    add             " m2!() ", " m2!() ", " m3!()),

        // Multiply [t2;t1;t0] = q * [n1;n0]

        Q!("    mul             " t0!() ", " n0!() ", " q!()),
        Q!("    mul             " t1!() ", " n1!() ", " q!()),
        Q!("    umulh           " t2!() ", " n0!() ", " q!()),
        Q!("    adds            " t1!() ", " t1!() ", " t2!()),
        Q!("    umulh           " t2!() ", " n1!() ", " q!()),
        Q!("    adc             " t2!() ", " t2!() ", xzr"),

        // Decrement k and load the next digit (note that d aliases to q)

        Q!("    sub             " k!() ", " k!() ", #1"),
        Q!("    ldr             " d!() ", [" x!() ", " k!() ", lsl #3]"),

        // Subtract [t3;t2;t1;t0] = x' - q * [n1;n0] = x - q * n_25519

        Q!("    subs            " t0!() ", " d!() ", " t0!()),
        Q!("    sbcs            " t1!() ", " m0!() ", " t1!()),
        Q!("    sbcs            " t2!() ", " m1!() ", " t2!()),
        Q!("    sbcs            " t3!() ", " m2!() ", xzr"),

        // If this borrows (CF = 0 because of inversion), add back n_25519.
        // The masked n3 digit exploits the fact that bit 60 of n1 is set.

        Q!("    csel            " m0!() ", " n0!() ", xzr, cc"),
        Q!("    csel            " m1!() ", " n1!() ", xzr, cc"),
        Q!("    adds            " m0!() ", " t0!() ", " m0!()),
        Q!("    and             " m3!() ", " m1!() ", #0x1000000000000000"),
        Q!("    adcs            " m1!() ", " t1!() ", " m1!()),
        Q!("    adcs            " m2!() ", " t2!() ", xzr"),
        Q!("    adc             " m3!() ", " t3!() ", " m3!()),

        Q!("    cbnz            " k!() ", " Label!("Lbignum_mod_n25519_loop", 4, Before)),

        // Finally write back [m3;m2;m1;m0] and return

        Q!(Label!("Lbignum_mod_n25519_writeback", 3) ":"),
        Q!("    stp             " m0!() ", " m1!() ", [" z!() "]"),
        Q!("    stp             " m2!() ", " m3!() ", [" z!() ", #16]"),
        // linear hoisting in -> b after Lbignum_mod_n25519_short
        Q!("    b               " Label!("hoist_finish", 5, After)),

        // Short case: just copy the input with zero-padding

        Q!(Label!("Lbignum_mod_n25519_short", 2) ":"),
        Q!("    mov             " m0!() ", xzr"),
        Q!("    mov             " m1!() ", xzr"),
        Q!("    mov             " m2!() ", xzr"),
        Q!("    mov             " m3!() ", xzr"),

        Q!("    cbz             " k!() ", " Label!("Lbignum_mod_n25519_writeback", 3, Before)),
        Q!("    ldr             " m0!() ", [" x!() "]"),
        Q!("    subs            " k!() ", " k!() ", #1"),
        Q!("    beq             " Label!("Lbignum_mod_n25519_writeback", 3, Before)),
        Q!("    ldr             " m1!() ", [" x!() ", #8]"),
        Q!("    subs            " k!() ", " k!() ", #1"),
        Q!("    beq             " Label!("Lbignum_mod_n25519_writeback", 3, Before)),
        Q!("    ldr             " m2!() ", [" x!() ", #16]"),
        Q!("    b               " Label!("Lbignum_mod_n25519_writeback", 3, Before)),
        Q!(Label!("hoist_finish", 5) ":"),
        inout("x0") z.as_mut_ptr() => _,
        inout("x1") x.len() => _,
        inout("x2") x.as_ptr() => _,
        // clobbers
        out("x10") _,
        out("x11") _,
        out("x12") _,
        out("x13") _,
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
