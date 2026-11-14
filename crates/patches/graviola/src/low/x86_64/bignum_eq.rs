// generated source. do not edit.
#![allow(non_upper_case_globals, unused_macros, unused_imports)]
use crate::low::macros::*;

// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

// ----------------------------------------------------------------------------
// Test bignums for equality, x = y
// Inputs x[m], y[n]; output function return
//
//    extern uint64_t bignum_eq(uint64_t m, const uint64_t *x, uint64_t n,
//                              const uint64_t *y);
//
// Standard x86-64 ABI: RDI = m, RSI = x, RDX = n, RCX = y, returns RAX
// Microsoft x64 ABI:   RCX = m, RDX = x, R8 = n, R9 = y, returns RAX
// ----------------------------------------------------------------------------

macro_rules! m {
    () => {
        "rdi"
    };
}
macro_rules! x {
    () => {
        "rsi"
    };
}
macro_rules! n {
    () => {
        "rdx"
    };
}
macro_rules! y {
    () => {
        "rcx"
    };
}
macro_rules! c {
    () => {
        "rax"
    };
}
// We can re-use n for this, not needed when d appears
macro_rules! d {
    () => {
        "rdx"
    };
}

/// Test bignums for equality, x = y
///
/// Inputs x[m], y[n]; output function return
pub(crate) fn bignum_eq(x: &[u64], y: &[u64]) -> bool {
    let ret: u64;
    // SAFETY: inline assembly. see [crate::low::inline_assembly_safety] for safety info.
    unsafe {
        core::arch::asm!(

        Q!("    endbr64         " ),

        // Initialize the accumulated OR of differences to zero

        Q!("    xor             " c!() ", " c!()),

        // If m >= n jump into the m > n loop at the final equality test
        // This will drop through for m = n

        Q!("    cmp             " m!() ", " n!()),
        Q!("    jnc             " Label!("Lbignum_eq_mtest", 2, After)),

        // Toploop for the case n > m

        Q!(Label!("Lbignum_eq_nloop", 3) ":"),
        Q!("    dec             " n!()),
        Q!("    or              " c!() ", [" y!() "+ 8 * " n!() "]"),
        Q!("    cmp             " m!() ", " n!()),
        Q!("    jnz             " Label!("Lbignum_eq_nloop", 3, Before)),
        Q!("    jmp             " Label!("Lbignum_eq_mmain", 4, After)),

        // Toploop for the case m > n (or n = m which enters at "mtest")

        Q!(Label!("Lbignum_eq_mloop", 5) ":"),
        Q!("    dec             " m!()),
        Q!("    or              " c!() ", [" x!() "+ 8 * " m!() "]"),
        Q!("    cmp             " m!() ", " n!()),
        Q!(Label!("Lbignum_eq_mtest", 2) ":"),
        Q!("    jnz             " Label!("Lbignum_eq_mloop", 5, Before)),

        // Combined main loop for the min(m,n) lower words

        Q!(Label!("Lbignum_eq_mmain", 4) ":"),
        Q!("    test            " m!() ", " m!()),
        Q!("    jz              " Label!("Lbignum_eq_end", 6, After)),

        Q!(Label!("Lbignum_eq_loop", 7) ":"),
        Q!("    mov             " d!() ", [" x!() "+ 8 * " m!() "-8]"),
        Q!("    xor             " d!() ", [" y!() "+ 8 * " m!() "-8]"),
        Q!("    or              " c!() ", " d!()),
        Q!("    dec             " m!()),
        Q!("    jnz             " Label!("Lbignum_eq_loop", 7, Before)),

        // Set a standard C condition based on whether c is nonzero

        Q!(Label!("Lbignum_eq_end", 6) ":"),
        Q!("    neg             " c!()),
        Q!("    sbb             " c!() ", " c!()),
        Q!("    inc             " c!()),
        inout("rdi") x.len() => _,
        inout("rsi") x.as_ptr() => _,
        inout("rdx") y.len() => _,
        inout("rcx") y.as_ptr() => _,
        out("rax") ret,
        // clobbers
            )
    };
    ret > 0
}
