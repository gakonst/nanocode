// generated source. do not edit.
#![allow(non_upper_case_globals, unused_macros, unused_imports)]
use crate::low::macros::*;

// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

// ----------------------------------------------------------------------------
// Montgomery reduce, z := (x' / 2^{64p}) MOD m
// Inputs x[n], m[k], p; output z[k]
//
//    extern void bignum_montredc(uint64_t k, uint64_t *z, uint64_t n,
//                                const uint64_t *x, const uint64_t *m, uint64_t p);
//
// Does a := (x' / 2^{64p}) mod m where x' = x if n <= p + k and in general
// is the lowest (p+k) digits of x, assuming x' <= 2^{64p} * m. That is,
// p-fold Montgomery reduction w.r.t. a k-digit modulus m giving a k-digit
// answer.
//
// Standard ARM ABI: X0 = k, X1 = z, X2 = n, X3 = x, X4 = m, X5 = p
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
macro_rules! n {
    () => {
        "x2"
    };
}
macro_rules! x {
    () => {
        "x3"
    };
}
macro_rules! m {
    () => {
        "x4"
    };
}
macro_rules! p {
    () => {
        "x5"
    };
}

// Negated modular inverse
macro_rules! w {
    () => {
        "x6"
    };
}
// Outer loop counter
macro_rules! i {
    () => {
        "x7"
    };
}
// Inner loop counter
macro_rules! j {
    () => {
        "x8"
    };
}
// Home for Montgomery multiplier
macro_rules! d {
    () => {
        "x9"
    };
}
// Top carry for current window
macro_rules! c {
    () => {
        "x14"
    };
}

macro_rules! h {
    () => {
        "x10"
    };
}
macro_rules! e {
    () => {
        "x11"
    };
}
macro_rules! l {
    () => {
        "x12"
    };
}
macro_rules! a {
    () => {
        "x13"
    };
}

// Some more intuitive names for temp regs in initial word-level negmodinv.
// These just use i and j again, which aren't used early on.

macro_rules! one {
    () => {
        "x7"
    };
}
macro_rules! e1 {
    () => {
        "x7"
    };
}
macro_rules! e2 {
    () => {
        "x8"
    };
}
macro_rules! e4 {
    () => {
        "x7"
    };
}
macro_rules! e8 {
    () => {
        "x8"
    };
}

/// Montgomery reduce, z := (x' / 2^{64p}) MOD m
///
/// Inputs x[n], m[k], p; output z[k]
///
/// Does a := (x' / 2^{64p}) mod m where x' = x if n <= p + k and in general
/// is the lowest (p+k) digits of x, assuming x' <= 2^{64p} * m. That is,
/// p-fold Montgomery reduction w.r.t. a k-digit modulus m giving a k-digit
/// answer.
pub(crate) fn bignum_montredc(z: &mut [u64], x: &[u64], m: &[u64], p: u64) {
    debug_assert!(z.len() == m.len());
    // SAFETY: inline assembly. see [crate::low::inline_assembly_safety] for safety info.
    unsafe {
        core::arch::asm!(

        // If k = 0 the whole operation is trivial

        Q!("    cbz             " k!() ", " Label!("Lbignum_montredc_end", 2, After)),

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

        // Initialize z to the lowest k digits of the input, zero-padding if n < k.

        Q!("    cmp             " n!() ", " k!()),
        Q!("    csel            " j!() ", " k!() ", " n!() ", cs"),
        Q!("    mov             " i!() ", xzr"),
        Q!("    cbz             " j!() ", " Label!("Lbignum_montredc_padloop", 3, After)),
        Q!(Label!("Lbignum_montredc_copyloop", 4) ":"),
        Q!("    ldr             " a!() ", [" x!() ", " i!() ", lsl #3]"),
        Q!("    str             " a!() ", [" z!() ", " i!() ", lsl #3]"),
        Q!("    add             " i!() ", " i!() ", #1"),
        Q!("    cmp             " i!() ", " j!()),
        Q!("    bcc             " Label!("Lbignum_montredc_copyloop", 4, Before)),

        Q!("    cmp             " i!() ", " k!()),
        Q!("    bcs             " Label!("Lbignum_montredc_initialized", 5, After)),

        Q!(Label!("Lbignum_montredc_padloop", 3) ":"),
        Q!("    str             " "xzr, [" z!() ", " i!() ", lsl #3]"),
        Q!("    add             " i!() ", " i!() ", #1"),
        Q!("    cmp             " i!() ", " k!()),
        Q!("    bcc             " Label!("Lbignum_montredc_padloop", 3, Before)),

        Q!(Label!("Lbignum_montredc_initialized", 5) ":"),
        Q!("    mov             " c!() ", xzr"),

        // Now if p = 0 we just need the corrective tail, and even that is
        // only needed for the case when the input is exactly the modulus,
        // to maintain the <= 2^64p * n precondition

        Q!("    cbz             " p!() ", " Label!("Lbignum_montredc_corrective", 6, After)),

        // Outer loop, just doing a standard Montgomery reduction on z

        Q!("    mov             " i!() ", xzr"),
        Q!(Label!("Lbignum_montredc_outerloop", 7) ":"),

        Q!("    ldr             " e!() ", [" z!() "]"),
        Q!("    mul             " d!() ", " e!() ", " w!()),
        Q!("    ldr             " a!() ", [" m!() "]"),
        Q!("    mul             " l!() ", " d!() ", " a!()),
        Q!("    umulh           " h!() ", " d!() ", " a!()),
        Q!("    adds            " e!() ", " e!() ", " l!()),
        Q!("    mov             " j!() ", #1"),
        Q!("    sub             " a!() ", " k!() ", #1"),
        Q!("    cbz             " a!() ", " Label!("Lbignum_montredc_montend", 8, After)),
        Q!(Label!("Lbignum_montredc_montloop", 9) ":"),
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
        Q!("    cbnz            " a!() ", " Label!("Lbignum_montredc_montloop", 9, Before)),
        Q!(Label!("Lbignum_montredc_montend", 8) ":"),
        Q!("    adcs            " h!() ", " h!() ", " c!()),
        Q!("    adc             " c!() ", xzr, xzr"),
        Q!("    add             " j!() ", " j!() ", " i!()),
        Q!("    cmp             " j!() ", " n!()),
        Q!("    bcs             " Label!("Lbignum_montredc_offtheend", 12, After)),
        Q!("    ldr             " a!() ", [" x!() ", " j!() ", lsl #3]"),
        Q!("    adds            " h!() ", " h!() ", " a!()),
        Q!("    adc             " c!() ", " c!() ", xzr"),
        Q!(Label!("Lbignum_montredc_offtheend", 12) ":"),
        Q!("    sub             " j!() ", " k!() ", #1"),
        Q!("    str             " h!() ", [" z!() ", " j!() ", lsl #3]"),

        // End of outer loop

        Q!("    add             " i!() ", " i!() ", #1"),
        Q!("    cmp             " i!() ", " p!()),
        Q!("    bcc             " Label!("Lbignum_montredc_outerloop", 7, Before)),

        // Now do a comparison of (c::z) with (0::m) to set a final correction mask
        // indicating that (c::z) >= m and so we need to subtract m.

        Q!(Label!("Lbignum_montredc_corrective", 6) ":"),

        Q!("    subs            " j!() ", xzr, xzr"),
        Q!(Label!("Lbignum_montredc_cmploop", 13) ":"),
        Q!("    ldr             " a!() ", [" z!() ", " j!() ", lsl #3]"),
        Q!("    ldr             " e!() ", [" m!() ", " j!() ", lsl #3]"),
        Q!("    sbcs            " "xzr, " a!() ", " e!()),
        Q!("    add             " j!() ", " j!() ", #1"),
        Q!("    sub             " a!() ", " j!() ", " k!()),
        Q!("    cbnz            " a!() ", " Label!("Lbignum_montredc_cmploop", 13, Before)),

        Q!("    sbcs            " "xzr, " c!() ", xzr"),
        Q!("    csetm           " c!() ", cs"),

        // Now do a masked subtraction of m for the final reduced result.

        Q!("    subs            " j!() ", xzr, xzr"),
        Q!(Label!("Lbignum_montredc_corrloop", 14) ":"),
        Q!("    ldr             " a!() ", [" z!() ", " j!() ", lsl #3]"),
        Q!("    ldr             " e!() ", [" m!() ", " j!() ", lsl #3]"),
        Q!("    and             " e!() ", " e!() ", " c!()),
        Q!("    sbcs            " a!() ", " a!() ", " e!()),
        Q!("    str             " a!() ", [" z!() ", " j!() ", lsl #3]"),
        Q!("    add             " j!() ", " j!() ", #1"),
        Q!("    sub             " a!() ", " j!() ", " k!()),
        Q!("    cbnz            " a!() ", " Label!("Lbignum_montredc_corrloop", 14, Before)),

        Q!(Label!("Lbignum_montredc_end", 2) ":"),
        inout("x0") z.len() => _,
        inout("x1") z.as_mut_ptr() => _,
        inout("x2") x.len() => _,
        inout("x3") x.as_ptr() => _,
        inout("x4") m.as_ptr() => _,
        inout("x5") p => _,
        // clobbers
        out("x10") _,
        out("x11") _,
        out("x12") _,
        out("x13") _,
        out("x14") _,
        out("x6") _,
        out("x7") _,
        out("x8") _,
        out("x9") _,
            )
    };
}
