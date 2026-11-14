// generated source. do not edit.
#![allow(non_upper_case_globals, unused_macros, unused_imports)]
use crate::low::macros::*;

// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

// ----------------------------------------------------------------------------
// Shift bignum right by c < 64 bits z := floor(x / 2^c)
// Inputs x[n], c; outputs function return (bits shifted out) and z[k]
//
//    extern uint64_t bignum_shr_small(uint64_t k, uint64_t *z, uint64_t n,
//                                     const uint64_t *x, uint64_t c);
//
// Does the "z := x >> c" operation where x is n digits, result z is p.
// The shift count c is masked to 6 bits so it actually uses c' = c mod 64.
// The return value is the inout mod 2^c'.
//
// Standard x86-64 ABI: RDI = k, RSI = z, RDX = n, RCX = x, R8 = c, returns RAX
// Microsoft x64 ABI:   RCX = k, RDX = z, R8 = n, R9 = x, [RSP+40] = c, returns RAX
// ----------------------------------------------------------------------------

macro_rules! p {
    () => {
        "rdi"
    };
}
macro_rules! z {
    () => {
        "rsi"
    };
}
macro_rules! n {
    () => {
        "rdx"
    };
}

// These get moved from their initial positions

macro_rules! c {
    () => {
        "rcx"
    };
}
macro_rules! x {
    () => {
        "r9"
    };
}

// Other variables

macro_rules! b {
    () => {
        "rax"
    };
}
macro_rules! t {
    () => {
        "r8"
    };
}
macro_rules! a {
    () => {
        "r10"
    };
}

macro_rules! ashort {
    () => {
        "r10d"
    };
}

/// Shift bignum right by c < 64 bits z := floor(x / 2^c)
///
/// Inputs x[n], c; outputs function return (bits shifted out) and z[k]
///
/// Does the "z := x >> c" operation where x is n digits, result z is p.
/// The shift count c is masked to 6 bits so it actually uses c' = c mod 64.
/// The return value is the inout mod 2^c'.
pub(crate) fn bignum_shr_small(z: &mut [u64], x: &[u64], c: u8) {
    // SAFETY: inline assembly. see [crate::low::inline_assembly_safety] for safety info.
    unsafe {
        core::arch::asm!(

        Q!("    endbr64         " ),

        // Reshuffle registers to put the shift count into CL

        Q!("    mov             " x!() ", rcx"),
        Q!("    mov             " c!() ", r8"),

        // Set default carry-in word to 0, useful for other things too

        Q!("    xor             " b!() ", " b!()),

        // First, if p > n then pad output on the left with p-n zeros

        Q!("    cmp             " n!() ", " p!()),
        Q!("    jnc             " Label!("Lbignum_shr_small_nopad", 2, After)),
        Q!(Label!("Lbignum_shr_small_padloop", 3) ":"),
        Q!("    dec             " p!()),
        Q!("    mov             " "[" z!() "+ 8 * " p!() "], " b!()),
        Q!("    cmp             " n!() ", " p!()),
        Q!("    jc              " Label!("Lbignum_shr_small_padloop", 3, Before)),
        Q!(Label!("Lbignum_shr_small_nopad", 2) ":"),

        // We now know that p <= n. If in fact p < n let carry word = x[p] instead of 0

        Q!("    jz              " Label!("Lbignum_shr_small_shiftstart", 4, After)),
        Q!("    mov             " b!() ", [" x!() "+ 8 * " p!() "]"),
        Q!(Label!("Lbignum_shr_small_shiftstart", 4) ":"),
        Q!("    test            " p!() ", " p!()),
        Q!("    jz              " Label!("Lbignum_shr_small_trivial", 5, After)),

        // Now the main loop

        Q!(Label!("Lbignum_shr_small_loop", 6) ":"),
        Q!("    mov             " a!() ", [" x!() "+ 8 * " p!() "-8]"),
        Q!("    mov             " t!() ", " a!()),
        Q!("    shrd            " a!() ", " b!() ", cl"),
        Q!("    mov             " "[" z!() "+ 8 * " p!() "-8], " a!()),
        Q!("    mov             " b!() ", " t!()),
        Q!("    dec             " p!()),
        Q!("    jnz             " Label!("Lbignum_shr_small_loop", 6, Before)),

        // Mask the carry word and return with that as RAX = b

        Q!(Label!("Lbignum_shr_small_trivial", 5) ":"),
        Q!("    mov             " ashort!() ", 1"),
        Q!("    shl             " a!() ", cl"),
        Q!("    dec             " a!()),
        Q!("    and             " b!() ", " a!()),

        Q!(Label!("Lbignum_shr_small_end", 7) ":"),
        inout("rdi") z.len() => _,
        inout("rsi") z.as_ptr() => _,
        inout("rdx") x.len() => _,
        inout("rcx") x.as_ptr() => _,
        inout("r8") (c as u64) => _,
        // clobbers
        out("r10") _,
        out("r9") _,
        out("rax") _,
            )
    };
}
