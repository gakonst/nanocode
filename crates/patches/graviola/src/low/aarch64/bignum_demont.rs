// generated source. do not edit.
#![allow(non_upper_case_globals, unused_macros, unused_imports)]
use crate::low::macros::*;

// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

// ----------------------------------------------------------------------------
// Convert from (almost-)Montgomery form z := (x / 2^{64k}) mod m
// Inputs x[k], m[k]; output z[k]
//
//    extern void bignum_demont(uint64_t k, uint64_t *z, const uint64_t *x,
//                              const uint64_t *m);
//
// Does z := (x / 2^{64k}) mod m, hence mapping out of Montgomery domain.
// In other words, this is a k-fold Montgomery reduction with same-size input.
// This can handle almost-Montgomery inputs, i.e. any k-digit bignum.
//
// Standard ARM ABI: X0 = k, X1 = z, X2 = x, X3 = m
// ----------------------------------------------------------------------------

macro_rules! k {
    () => {
        "x0"
    };
}
macro_rules! z {
    () => {
        "x1"
    };
}
macro_rules! x {
    () => {
        "x2"
    };
}
macro_rules! m {
    () => {
        "x3"
    };
}

// Negated modular inverse
macro_rules! w {
    () => {
        "x4"
    };
}
// Outer loop counter
macro_rules! i {
    () => {
        "x5"
    };
}
// Inner loop counter
macro_rules! j {
    () => {
        "x6"
    };
}
// Home for Montgomery multiplier
macro_rules! d {
    () => {
        "x7"
    };
}

macro_rules! h {
    () => {
        "x8"
    };
}
macro_rules! e {
    () => {
        "x9"
    };
}
macro_rules! l {
    () => {
        "x10"
    };
}
macro_rules! a {
    () => {
        "x11"
    };
}

// Some more intuitive names for temp regs in initial word-level negmodinv.
// These just use i and j again, which aren't used early on.

macro_rules! one {
    () => {
        "x5"
    };
}
macro_rules! e1 {
    () => {
        "x5"
    };
}
macro_rules! e2 {
    () => {
        "x6"
    };
}
macro_rules! e4 {
    () => {
        "x5"
    };
}
macro_rules! e8 {
    () => {
        "x6"
    };
}

/// Convert from (almost-)Montgomery form z := (x / 2^{64k}) mod m
///
/// Inputs x[k], m[k]; output z[k]
///
/// Does z := (x / 2^{64k}) mod m, hence mapping out of Montgomery domain.
/// In other words, this is a k-fold Montgomery reduction with same-size input.
/// This can handle almost-Montgomery inputs, i.e. any k-digit bignum.
pub(crate) fn bignum_demont(z: &mut [u64], x: &[u64], m: &[u64]) {
    debug_assert!(z.len() == x.len());
    debug_assert!(z.len() == m.len());
    // SAFETY: inline assembly. see [crate::low::inline_assembly_safety] for safety info.
    unsafe {
        core::arch::asm!(

        // If k = 0 the whole operation is trivial

        Q!("    cbz             " k!() ", " Label!("Lbignum_demont_end", 2, After)),

        // Compute word-level negated modular inverse w for m == m[0].
        // This is essentially the same as word_negmodinv.

        Q!("    ldr             " a!() ", [" m!() "]"),
        Q!("    lsl             " w!() ", " a!() ", #2"),
        Q!("    sub             " w!() ", " a!() ", " w!()),
        Q!("    eor             " w!() ", " w!() ", #2"),
        Q!("    mov             " one!() ", #1"),
        Q!("    madd            " e1!() ", " a!() ", " w!() ", " one!()),
        Q!("    mul             " e2!() ", " e1!() ", " e1!()),
        Q!("    madd            " w!() ", " e1!() ", " w!() ", " w!()),
        Q!("    mul             " e4!() ", " e2!() ", " e2!()),
        Q!("    madd            " w!() ", " e2!() ", " w!() ", " w!()),
        Q!("    mul             " e8!() ", " e4!() ", " e4!()),
        Q!("    madd            " w!() ", " e4!() ", " w!() ", " w!()),
        Q!("    madd            " w!() ", " e8!() ", " w!() ", " w!()),

        // Initially just copy the input to the output. It would be a little more
        // efficient but somewhat fiddlier to tweak the zeroth iteration below instead.

        Q!("    mov             " i!() ", xzr"),
        Q!(Label!("Lbignum_demont_iloop", 3) ":"),
        Q!("    ldr             " a!() ", [" x!() ", " i!() ", lsl #3]"),
        Q!("    str             " a!() ", [" z!() ", " i!() ", lsl #3]"),
        Q!("    add             " i!() ", " i!() ", #1"),
        Q!("    cmp             " i!() ", " k!()),
        Q!("    bcc             " Label!("Lbignum_demont_iloop", 3, Before)),

        // Outer loop, just doing a standard Montgomery reduction on z

        Q!("    mov             " i!() ", xzr"),
        Q!(Label!("Lbignum_demont_outerloop", 4) ":"),

        Q!("    ldr             " e!() ", [" z!() "]"),
        Q!("    mul             " d!() ", " e!() ", " w!()),
        Q!("    ldr             " a!() ", [" m!() "]"),
        Q!("    mul             " l!() ", " d!() ", " a!()),
        Q!("    umulh           " h!() ", " d!() ", " a!()),
        Q!("    adds            " e!() ", " e!() ", " l!()),
        Q!("    mov             " j!() ", #1"),
        Q!("    sub             " a!() ", " k!() ", #1"),
        Q!("    cbz             " a!() ", " Label!("Lbignum_demont_montend", 5, After)),
        Q!(Label!("Lbignum_demont_montloop", 6) ":"),
        Q!("    ldr             " a!() ", [" m!() ", " j!() ", lsl #3]"),
        Q!("    ldr             " e!() ", [" z!() ", " j!() ", lsl #3]"),
        Q!("    mul             " l!() ", " d!() ", " a!()),
        Q!("    adcs            " e!() ", " e!() ", " h!()),
        Q!("    umulh           " h!() ", " d!() ", " a!()),
        Q!("    adc             " h!() ", " h!() ", xzr"),
        Q!("    adds            " e!() ", " e!() ", " l!()),
        Q!("    sub             " l!() ", " j!() ", #1"),
        Q!("    str             " e!() ", [" z!() ", " l!() ", lsl #3]"),
        Q!("    add             " j!() ", " j!() ", #1"),
        Q!("    sub             " a!() ", " j!() ", " k!()),
        Q!("    cbnz            " a!() ", " Label!("Lbignum_demont_montloop", 6, Before)),
        Q!(Label!("Lbignum_demont_montend", 5) ":"),
        Q!("    adc             " h!() ", xzr, " h!()),
        Q!("    sub             " l!() ", " j!() ", #1"),
        Q!("    str             " h!() ", [" z!() ", " l!() ", lsl #3]"),

        // End of outer loop

        Q!("    add             " i!() ", " i!() ", #1"),
        Q!("    cmp             " i!() ", " k!()),
        Q!("    bcc             " Label!("Lbignum_demont_outerloop", 4, Before)),

        // Now do a comparison of z with m to set a final correction mask
        // indicating that z >= m and so we need to subtract m.

        Q!("    subs            " j!() ", xzr, xzr"),
        Q!(Label!("Lbignum_demont_cmploop", 7) ":"),
        Q!("    ldr             " a!() ", [" z!() ", " j!() ", lsl #3]"),
        Q!("    ldr             " e!() ", [" m!() ", " j!() ", lsl #3]"),
        Q!("    sbcs            " "xzr, " a!() ", " e!()),
        Q!("    add             " j!() ", " j!() ", #1"),
        Q!("    sub             " a!() ", " j!() ", " k!()),
        Q!("    cbnz            " a!() ", " Label!("Lbignum_demont_cmploop", 7, Before)),
        Q!("    csetm           " h!() ", cs"),

        // Now do a masked subtraction of m for the final reduced result.

        Q!("    subs            " j!() ", xzr, xzr"),
        Q!(Label!("Lbignum_demont_corrloop", 8) ":"),
        Q!("    ldr             " a!() ", [" z!() ", " j!() ", lsl #3]"),
        Q!("    ldr             " e!() ", [" m!() ", " j!() ", lsl #3]"),
        Q!("    and             " e!() ", " e!() ", " h!()),
        Q!("    sbcs            " a!() ", " a!() ", " e!()),
        Q!("    str             " a!() ", [" z!() ", " j!() ", lsl #3]"),
        Q!("    add             " j!() ", " j!() ", #1"),
        Q!("    sub             " a!() ", " j!() ", " k!()),
        Q!("    cbnz            " a!() ", " Label!("Lbignum_demont_corrloop", 8, Before)),

        Q!(Label!("Lbignum_demont_end", 2) ":"),
        inout("x0") m.len() => _,
        inout("x1") z.as_mut_ptr() => _,
        inout("x2") x.as_ptr() => _,
        inout("x3") m.as_ptr() => _,
        // clobbers
        out("x10") _,
        out("x11") _,
        out("x4") _,
        out("x5") _,
        out("x6") _,
        out("x7") _,
        out("x8") _,
        out("x9") _,
            )
    };
}
