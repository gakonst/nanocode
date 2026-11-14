// generated source. do not edit.
#![allow(non_upper_case_globals, unused_macros, unused_imports)]
use crate::low::macros::*;

// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

// ----------------------------------------------------------------------------
// Multiplex/select z := x (if p nonzero) or z := y (if p zero)
// Inputs p, x[k], y[k]; output z[k]
//
//    extern void bignum_mux(uint64_t p, uint64_t k, uint64_t *z, const uint64_t *x,
//                           const uint64_t *y);
//
// It is assumed that all numbers x, y and z have the same size k digits.
//
// Standard x86-64 ABI: RDI = p, RSI = k, RDX = z, RCX = x, R8 = y
// Microsoft x64 ABI:   RCX = p, RDX = k, R8 = z, R9 = x, [RSP+40] = y
// ----------------------------------------------------------------------------

macro_rules! b {
    () => {
        "rdi"
    };
}
macro_rules! k {
    () => {
        "rsi"
    };
}
macro_rules! z {
    () => {
        "rdx"
    };
}
macro_rules! x {
    () => {
        "rcx"
    };
}
macro_rules! y {
    () => {
        "r8"
    };
}
macro_rules! i {
    () => {
        "r9"
    };
}
macro_rules! a {
    () => {
        "rax"
    };
}

/// Multiplex/select z := x (if p nonzero) or z := y (if p zero)
///
/// Inputs p, x[k], y[k]; output z[k]
///
/// It is assumed that all numbers x, y and z have the same size k digits.
pub(crate) fn bignum_mux(p: u64, z: &mut [u64], x_if_p: &[u64], y_if_not_p: &[u64]) {
    debug_assert!(z.len() == x_if_p.len());
    debug_assert!(z.len() == y_if_not_p.len());
    // SAFETY: inline assembly. see [crate::low::inline_assembly_safety] for safety info.
    unsafe {
        core::arch::asm!(

        Q!("    endbr64         " ),

        Q!("    test            " k!() ", " k!()),
        Q!("    jz              " Label!("Lbignum_mux_end", 2, After)),

        Q!("    xor             " i!() ", " i!()),
        Q!("    neg             " b!()),
        Q!(Label!("Lbignum_mux_loop", 3) ":"),
        Q!("    mov             " a!() ", [" x!() "+ 8 * " i!() "]"),
        Q!("    mov             " b!() ", [" y!() "+ 8 * " i!() "]"),
        Q!("    cmovnc          " a!() ", " b!()),
        Q!("    mov             " "[" z!() "+ 8 * " i!() "], " a!()),
        Q!("    inc             " i!()),
        Q!("    dec             " k!()),
        Q!("    jnz             " Label!("Lbignum_mux_loop", 3, Before)),
        Q!(Label!("Lbignum_mux_end", 2) ":"),
        inout("rdi") p => _,
        inout("rsi") z.len() => _,
        inout("rdx") z.as_mut_ptr() => _,
        inout("rcx") x_if_p.as_ptr() => _,
        inout("r8") y_if_not_p.as_ptr() => _,
        // clobbers
        out("r9") _,
        out("rax") _,
            )
    };
}
