// generated source. do not edit.
#![allow(non_upper_case_globals, unused_macros, unused_imports)]
use crate::low::macros::*;

// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

// ----------------------------------------------------------------------------
// Return size of bignum in digits (64-bit word)
// Input x[k]; output function return
//
//    extern uint64_t bignum_digitsize(uint64_t k, const uint64_t *x);
//
// In the case of a zero bignum as input the result is 0
//
// Standard x86-64 ABI: RDI = k, RSI = x, returns RAX
// Microsoft x64 ABI:   RCX = k, RDX = x, returns RAX
// ----------------------------------------------------------------------------

macro_rules! k {
    () => {
        "rdi"
    };
}
macro_rules! x {
    () => {
        "rsi"
    };
}
macro_rules! i {
    () => {
        "rax"
    };
}
macro_rules! a {
    () => {
        "rcx"
    };
}
macro_rules! j {
    () => {
        "rdx"
    };
}

/// Return size of bignum in digits (64-bit word)
///
/// Input x[k]; output function return
///
/// In the case of a zero bignum as input the result is 0
pub(crate) fn bignum_digitsize(z: &[u64]) -> usize {
    let ret: u64;
    // SAFETY: inline assembly. see [crate::low::inline_assembly_safety] for safety info.
    unsafe {
        core::arch::asm!(

        Q!("    endbr64         " ),

        // Initialize the index i and also prepare default return value of 0 (i = rax)

        Q!("    xor             " i!() ", " i!()),

        // If the bignum is zero-length, just return 0

        Q!("    test            " k!() ", " k!()),
        Q!("    jz              " Label!("Lbignum_digitsize_end", 2, After)),

        // Run over the words j = 0..i-1, and set i := j + 1 when hitting nonzero a[j]

        Q!("    xor             " j!() ", " j!()),
        Q!(Label!("Lbignum_digitsize_loop", 3) ":"),
        Q!("    mov             " a!() ", [" x!() "+ 8 * " j!() "]"),
        Q!("    inc             " j!()),
        Q!("    test            " a!() ", " a!()),
        Q!("    cmovnz          " i!() ", " j!()),
        Q!("    cmp             " j!() ", " k!()),
        Q!("    jnz             " Label!("Lbignum_digitsize_loop", 3, Before)),

        Q!(Label!("Lbignum_digitsize_end", 2) ":"),
        inout("rdi") z.len() => _,
        inout("rsi") z.as_ptr() => _,
        out("rax") ret,
        // clobbers
        out("rcx") _,
        out("rdx") _,
            )
    };
    ret as usize
}
