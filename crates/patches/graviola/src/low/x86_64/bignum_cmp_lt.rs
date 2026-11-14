// generated source. do not edit.
#![allow(non_upper_case_globals, unused_macros, unused_imports)]
use crate::low::macros::*;

// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

// ----------------------------------------------------------------------------
// Compare bignums, x < y
// Inputs x[m], y[n]; output function return
//
//    extern uint64_t bignum_lt(uint64_t m, const uint64_t *x, uint64_t n,
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
macro_rules! i {
    () => {
        "r8"
    };
}
macro_rules! a {
    () => {
        "rax"
    };
}

macro_rules! ashort {
    () => {
        "eax"
    };
}

/// Compare bignums, x < y
///
/// Inputs x[m], y[n]; output function return
pub(crate) fn bignum_cmp_lt(x: &[u64], y: &[u64]) -> u64 {
    let ret: u64;
    // SAFETY: inline assembly. see [crate::low::inline_assembly_safety] for safety info.
    unsafe {
        core::arch::asm!(

        Q!("    endbr64         " ),

        // Zero the main index counter for both branches

        Q!("    xor             " i!() ", " i!()),

        // Speculatively form m := m - n and do case split

        Q!("    sub             " m!() ", " n!()),
        Q!("    jc              " Label!("Lbignum_lt_ylonger", 2, After)),

        // The case where x is longer or of the same size (m >= n)

        Q!("    inc             " m!()),
        Q!("    test            " n!() ", " n!()),
        Q!("    jz              " Label!("Lbignum_lt_xtest", 3, After)),
        Q!(Label!("Lbignum_lt_xmainloop", 4) ":"),
        Q!("    mov             " a!() ", [" x!() "+ 8 * " i!() "]"),
        Q!("    sbb             " a!() ", [" y!() "+ 8 * " i!() "]"),
        Q!("    inc             " i!()),
        Q!("    dec             " n!()),
        Q!("    jnz             " Label!("Lbignum_lt_xmainloop", 4, Before)),
        Q!("    jmp             " Label!("Lbignum_lt_xtest", 3, After)),
        Q!(Label!("Lbignum_lt_xtoploop", 5) ":"),
        Q!("    mov             " a!() ", [" x!() "+ 8 * " i!() "]"),
        Q!("    sbb             " a!() ", 0"),
        Q!("    inc             " i!()),
        Q!(Label!("Lbignum_lt_xtest", 3) ":"),
        Q!("    dec             " m!()),
        Q!("    jnz             " Label!("Lbignum_lt_xtoploop", 5, Before)),
        Q!("    sbb             " a!() ", " a!()),
        Q!("    neg             " a!()),
        // linear hoisting in -> ret after Lbignum_lt_ytoploop
        Q!("    jmp             " Label!("hoist_finish", 6, After)),

        // The case where y is longer (n > m)

        Q!(Label!("Lbignum_lt_ylonger", 2) ":"),
        Q!("    add             " m!() ", " n!()),
        Q!("    sub             " n!() ", " m!()),
        Q!("    test            " m!() ", " m!()),
        Q!("    jz              " Label!("Lbignum_lt_ytoploop", 7, After)),
        Q!(Label!("Lbignum_lt_ymainloop", 8) ":"),
        Q!("    mov             " a!() ", [" x!() "+ 8 * " i!() "]"),
        Q!("    sbb             " a!() ", [" y!() "+ 8 * " i!() "]"),
        Q!("    inc             " i!()),
        Q!("    dec             " m!()),
        Q!("    jnz             " Label!("Lbignum_lt_ymainloop", 8, Before)),
        Q!(Label!("Lbignum_lt_ytoploop", 7) ":"),
        Q!("    mov             " ashort!() ", 0"),
        Q!("    sbb             " a!() ", [" y!() "+ 8 * " i!() "]"),
        Q!("    inc             " i!()),
        Q!("    dec             " n!()),
        Q!("    jnz             " Label!("Lbignum_lt_ytoploop", 7, Before)),

        Q!("    sbb             " a!() ", " a!()),
        Q!("    neg             " a!()),
        Q!(Label!("hoist_finish", 6) ":"),
        inout("rdi") x.len() => _,
        inout("rsi") x.as_ptr() => _,
        inout("rdx") y.len() => _,
        inout("rcx") y.as_ptr() => _,
        out("rax") ret,
        // clobbers
        out("r8") _,
            )
    };
    ret
}
