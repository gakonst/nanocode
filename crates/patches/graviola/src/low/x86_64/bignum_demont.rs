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
// Standard x86-64 ABI: RDI = k, RSI = z, RDX = x, RCX = m
// Microsoft x64 ABI:   RCX = k, RDX = z, R8 = x, R9 = m
// ----------------------------------------------------------------------------

macro_rules! k {
    () => {
        "rdi"
    };
}
macro_rules! z {
    () => {
        "rsi"
    };
}
macro_rules! x {
    () => {
        "rdx"
    };
}
macro_rules! m {
    () => {
        "rcx"
    };
}

// General temp, low part of product and mul input
macro_rules! a {
    () => {
        "rax"
    };
}
// General temp, high part of product (no longer x)
macro_rules! b {
    () => {
        "rdx"
    };
}
// Negated modular inverse
macro_rules! w {
    () => {
        "r8"
    };
}
// Outer loop counter
macro_rules! i {
    () => {
        "r9"
    };
}
// Inner loop counter
macro_rules! j {
    () => {
        "rbx"
    };
}
// Home for Montgomery multiplier
macro_rules! d {
    () => {
        "rbp"
    };
}
macro_rules! h {
    () => {
        "r10"
    };
}
macro_rules! e {
    () => {
        "r11"
    };
}
macro_rules! n {
    () => {
        "r12"
    };
}

// A temp reg in the initial word-level negmodinv, same as j

macro_rules! t {
    () => {
        "rbx"
    };
}

macro_rules! ashort {
    () => {
        "eax"
    };
}
macro_rules! jshort {
    () => {
        "ebx"
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

        Q!("    endbr64         " ),

        // Save registers

        Q!("    push            " "rbx"),
        Q!("    push            " "rbp"),
        Q!("    push            " "r12"),

        // If k = 0 the whole operation is trivial

        Q!("    test            " k!() ", " k!()),
        Q!("    jz              " Label!("Lbignum_demont_end", 2, After)),

        // Compute word-level negated modular inverse w for m == m[0].

        Q!("    mov             " a!() ", [" m!() "]"),

        Q!("    mov             " t!() ", " a!()),
        Q!("    mov             " w!() ", " a!()),
        Q!("    shl             " t!() ", 2"),
        Q!("    sub             " w!() ", " t!()),
        Q!("    xor             " w!() ", 2"),

        Q!("    mov             " t!() ", " w!()),
        Q!("    imul            " t!() ", " a!()),
        Q!("    mov             " ashort!() ", 2"),
        Q!("    add             " a!() ", " t!()),
        Q!("    add             " t!() ", 1"),

        Q!("    imul            " w!() ", " a!()),

        Q!("    imul            " t!() ", " t!()),
        Q!("    mov             " ashort!() ", 1"),
        Q!("    add             " a!() ", " t!()),
        Q!("    imul            " w!() ", " a!()),

        Q!("    imul            " t!() ", " t!()),
        Q!("    mov             " ashort!() ", 1"),
        Q!("    add             " a!() ", " t!()),
        Q!("    imul            " w!() ", " a!()),

        Q!("    imul            " t!() ", " t!()),
        Q!("    mov             " ashort!() ", 1"),
        Q!("    add             " a!() ", " t!()),
        Q!("    imul            " w!() ", " a!()),

        // Initially just copy the input to the output. It would be a little more
        // efficient but somewhat fiddlier to tweak the zeroth iteration below instead.
        // After this we never use x again and can safely recycle RDX for muls

        Q!("    xor             " j!() ", " j!()),
        Q!(Label!("Lbignum_demont_iloop", 3) ":"),
        Q!("    mov             " a!() ", [" x!() "+ 8 * " j!() "]"),
        Q!("    mov             " "[" z!() "+ 8 * " j!() "], " a!()),
        Q!("    inc             " j!()),
        Q!("    cmp             " j!() ", " k!()),
        Q!("    jc              " Label!("Lbignum_demont_iloop", 3, Before)),

        // Outer loop, just doing a standard Montgomery reduction on z

        Q!("    xor             " i!() ", " i!()),

        Q!(Label!("Lbignum_demont_outerloop", 4) ":"),
        Q!("    mov             " e!() ", [" z!() "]"),
        Q!("    mov             " d!() ", " w!()),
        Q!("    imul            " d!() ", " e!()),
        Q!("    mov             " a!() ", [" m!() "]"),
        Q!("    mul             " d!()),
        Q!("    add             " a!() ", " e!()),
        Q!("    mov             " h!() ", rdx"),
        Q!("    mov             " jshort!() ", 1"),
        Q!("    mov             " n!() ", " k!()),
        Q!("    dec             " n!()),
        Q!("    jz              " Label!("Lbignum_demont_montend", 5, After)),

        Q!(Label!("Lbignum_demont_montloop", 6) ":"),
        Q!("    adc             " h!() ", [" z!() "+ 8 * " j!() "]"),
        Q!("    sbb             " e!() ", " e!()),
        Q!("    mov             " a!() ", [" m!() "+ 8 * " j!() "]"),
        Q!("    mul             " d!()),
        Q!("    sub             " "rdx, " e!()),
        Q!("    add             " a!() ", " h!()),
        Q!("    mov             " "[" z!() "+ 8 * " j!() "-8], " a!()),
        Q!("    mov             " h!() ", rdx"),
        Q!("    inc             " j!()),
        Q!("    dec             " n!()),
        Q!("    jnz             " Label!("Lbignum_demont_montloop", 6, Before)),

        Q!(Label!("Lbignum_demont_montend", 5) ":"),
        Q!("    adc             " h!() ", 0"),
        Q!("    mov             " "[" z!() "+ 8 * " j!() "-8], " h!()),

        // End of outer loop.

        Q!("    inc             " i!()),
        Q!("    cmp             " i!() ", " k!()),
        Q!("    jc              " Label!("Lbignum_demont_outerloop", 4, Before)),

        // Now do a comparison of z with m to set a final correction mask
        // indicating that z >= m and so we need to subtract m.

        Q!("    xor             " j!() ", " j!()),
        Q!("    mov             " n!() ", " k!()),
        Q!(Label!("Lbignum_demont_cmploop", 7) ":"),
        Q!("    mov             " a!() ", [" z!() "+ 8 * " j!() "]"),
        Q!("    sbb             " a!() ", [" m!() "+ 8 * " j!() "]"),
        Q!("    inc             " j!()),
        Q!("    dec             " n!()),
        Q!("    jnz             " Label!("Lbignum_demont_cmploop", 7, Before)),
        Q!("    sbb             " d!() ", " d!()),
        Q!("    not             " d!()),

        // Now do a masked subtraction of m for the final reduced result.

        Q!("    xor             " e!() ", " e!()),
        Q!("    xor             " j!() ", " j!()),
        Q!(Label!("Lbignum_demont_corrloop", 8) ":"),
        Q!("    mov             " a!() ", [" m!() "+ 8 * " j!() "]"),
        Q!("    and             " a!() ", " d!()),
        Q!("    neg             " e!()),
        Q!("    sbb             " "[" z!() "+ 8 * " j!() "], " a!()),
        Q!("    sbb             " e!() ", " e!()),
        Q!("    inc             " j!()),
        Q!("    cmp             " j!() ", " k!()),
        Q!("    jc              " Label!("Lbignum_demont_corrloop", 8, Before)),

        Q!(Label!("Lbignum_demont_end", 2) ":"),
        Q!("    pop             " "r12"),
        Q!("    pop             " "rbp"),
        Q!("    pop             " "rbx"),

        inout("rdi") m.len() => _,
        inout("rsi") z.as_mut_ptr() => _,
        inout("rdx") x.as_ptr() => _,
        inout("rcx") m.as_ptr() => _,
        // clobbers
        out("r10") _,
        out("r11") _,
        out("r12") _,
        out("r8") _,
        out("r9") _,
        out("rax") _,
            )
    };
}
