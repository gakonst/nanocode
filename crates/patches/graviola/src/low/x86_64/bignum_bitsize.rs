// generated source. do not edit.
#![allow(non_upper_case_globals, unused_macros, unused_imports)]
use crate::low::macros::*;

// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

// ----------------------------------------------------------------------------
// Return size of bignum in bits
// Input x[k]; output function return
//
//    extern uint64_t bignum_bitsize(uint64_t k, const uint64_t *x);
//
// In the case of a zero bignum as input the result is 0
//
// In principle this has a precondition k < 2^58, but obviously that
// is always true in practice because of address space limitations.
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
macro_rules! w {
    () => {
        "rdx"
    };
}
macro_rules! a {
    () => {
        "rcx"
    };
}
macro_rules! j {
    () => {
        "r8"
    };
}

/// Return size of bignum in bits
///
/// Input x[k]; output function return
///
/// In the case of a zero bignum as input the result is 0
///
/// In principle this has a precondition k < 2^58, but obviously that
/// is always true in practice because of address space limitations.
pub(crate) fn bignum_bitsize(x: &[u64]) -> usize {
    let ret: u64;
    // SAFETY: inline assembly. see [crate::low::inline_assembly_safety] for safety info.
    unsafe {
        core::arch::asm!(

        Q!("    endbr64         " ),

        // Initialize the index i and also prepare default return value of 0 (i = rax)

        Q!("    xor             " i!() ", " i!()),

        // If the bignum is zero-length, just return 0

        Q!("    test            " k!() ", " k!()),
        Q!("    jz              " Label!("Lbignum_bitsize_end", 2, After)),

        // Use w = a[i-1] to store nonzero words in a bottom-up sweep
        // Set the initial default to be as if we had a 11...11 word directly below

        Q!("    mov             " w!() ", -1"),
        Q!("    xor             " j!() ", " j!()),
        Q!(Label!("Lbignum_bitsize_loop", 3) ":"),
        Q!("    mov             " a!() ", [" x!() "+ 8 * " j!() "]"),
        Q!("    inc             " j!()),
        Q!("    test            " a!() ", " a!()),
        Q!("    cmovnz          " i!() ", " j!()),
        Q!("    cmovnz          " w!() ", " a!()),
        Q!("    cmp             " j!() ", " k!()),
        Q!("    jnz             " Label!("Lbignum_bitsize_loop", 3, Before)),

        // Now w = a[i-1] is the highest nonzero word, or in the zero case the
        // default of the "extra" 11...11 = a[0-1]. We now want 64* i - clz(w) =
        // 64 * i - (63 - bsr(w)) = (64 * i - 63) + bsr(w). Note that this code
        // does not rely on the behavior of the bsr instruction for zero inputs,
        // which is undefined.

        Q!("    shl             " i!() ", 6"),
        Q!("    sub             " i!() ", 63"),
        Q!("    bsr             " w!() ", " w!()),
        Q!("    add             " "rax, " w!()),

        Q!(Label!("Lbignum_bitsize_end", 2) ":"),
        inout("rdi") x.len() => _,
        inout("rsi") x.as_ptr() => _,
        out("rax") ret,
        // clobbers
        out("r8") _,
        out("rcx") _,
        out("rdx") _,
            )
    };
    ret as usize
}
