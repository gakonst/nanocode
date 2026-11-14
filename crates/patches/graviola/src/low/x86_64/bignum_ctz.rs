// generated source. do not edit.
#![allow(non_upper_case_globals, unused_macros, unused_imports)]
use crate::low::macros::*;

// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

// ----------------------------------------------------------------------------
// Count trailing zero bits
// Input x[k]; output function return
//
//    extern uint64_t bignum_ctz(uint64_t k, const uint64_t *x);
//
//
// In the case of a zero bignum as input the result is 64 * k
//
// In principle this has a precondition k < 2^58, but obviously that
// is always true in practice because of address space limitations
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
        "rdx"
    };
}
macro_rules! w {
    () => {
        "rcx"
    };
}
macro_rules! a {
    () => {
        "rax"
    };
}

macro_rules! wshort {
    () => {
        "ecx"
    };
}

/// Count trailing zero bits
///
/// Input x[k]; output function return
///
///
/// In the case of a zero bignum as input the result is 64 * k
///
/// In principle this has a precondition k < 2^58, but obviously that
/// is always true in practice because of address space limitations
pub(crate) fn bignum_ctz(x: &[u64]) -> usize {
    let ret: u64;
    // SAFETY: inline assembly. see [crate::low::inline_assembly_safety] for safety info.
    unsafe {
        core::arch::asm!(

        Q!("    endbr64         " ),

        // If the bignum is zero-length, just return 0

        Q!("    xor             " "rax, rax"),
        Q!("    test            " k!() ", " k!()),
        Q!("    jz              " Label!("Lbignum_ctz_end", 2, After)),

        // Use w = a[i-1] to store nonzero words in a top-down sweep
        // Set the initial default to be as if we had a 1 word directly above

        Q!("    mov             " i!() ", " k!()),
        Q!("    inc             " i!()),
        Q!("    mov             " wshort!() ", 1"),

        Q!(Label!("Lbignum_ctz_loop", 3) ":"),
        Q!("    mov             " a!() ", [" x!() "+ 8 * " k!() "-8]"),
        Q!("    test            " a!() ", " a!()),
        Q!("    cmovne          " i!() ", " k!()),
        Q!("    cmovne          " w!() ", " a!()),
        Q!("    dec             " k!()),
        Q!("    jnz             " Label!("Lbignum_ctz_loop", 3, Before)),

        // Now w = a[i-1] is the lowest nonzero word, or in the zero case the
        // default of the "extra" 1 = a[k]. We now want 64*(i-1) + ctz(w).
        // Note that this code does not rely on the behavior of the BSF instruction
        // for zero inputs, which is undefined according to the manual.

        Q!("    dec             " i!()),
        Q!("    shl             " i!() ", 6"),
        Q!("    bsf             " "rax, " w!()),
        Q!("    add             " "rax, " i!()),

        Q!(Label!("Lbignum_ctz_end", 2) ":"),
        inout("rdi") x.len() => _,
        inout("rsi") x.as_ptr() => _,
        out("rax") ret,
        // clobbers
        out("rcx") _,
        out("rdx") _,
            )
    };
    ret as usize
}
