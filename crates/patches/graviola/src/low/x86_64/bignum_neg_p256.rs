// generated source. do not edit.
#![allow(non_upper_case_globals, unused_macros, unused_imports)]
use crate::low::macros::*;

// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

// ----------------------------------------------------------------------------
// Negate modulo p_256, z := (-x) mod p_256, assuming x reduced
// Input x[4]; output z[4]
//
//    extern void bignum_neg_p256(uint64_t z[static 4], const uint64_t x[static 4]);
//
// Standard x86-64 ABI: RDI = z, RSI = x
// Microsoft x64 ABI:   RCX = z, RDX = x
// ----------------------------------------------------------------------------

macro_rules! z {
    () => {
        "rdi"
    };
}
macro_rules! x {
    () => {
        "rsi"
    };
}

macro_rules! q {
    () => {
        "rdx"
    };
}

macro_rules! d0 {
    () => {
        "rax"
    };
}
macro_rules! d1 {
    () => {
        "rcx"
    };
}
macro_rules! d2 {
    () => {
        "r8"
    };
}
macro_rules! d3 {
    () => {
        "r9"
    };
}

macro_rules! n1 {
    () => {
        "r10"
    };
}
macro_rules! n3 {
    () => {
        "r11"
    };
}

macro_rules! d0short {
    () => {
        "eax"
    };
}
macro_rules! n1short {
    () => {
        "r10d"
    };
}

/// Negate modulo p_256, z := (-x) mod p_256, assuming x reduced
///
/// Input x[4]; output z[4]
pub(crate) fn bignum_neg_p256(z: &mut [u64; 4], x: &[u64; 4]) {
    // SAFETY: inline assembly. see [crate::low::inline_assembly_safety] for safety info.
    unsafe {
        core::arch::asm!(

        Q!("    endbr64         " ),

        // Load the input digits as [d3;d2;d1;d0] and also set a bitmask q
        // for the input being nonzero, so that we avoid doing -0 = p_256
        // and hence maintain strict modular reduction

        Q!("    mov             " d0!() ", [" x!() "]"),
        Q!("    mov             " d1!() ", [" x!() "+ 8]"),
        Q!("    mov             " n1!() ", " d0!()),
        Q!("    or              " n1!() ", " d1!()),
        Q!("    mov             " d2!() ", [" x!() "+ 16]"),
        Q!("    mov             " d3!() ", [" x!() "+ 24]"),
        Q!("    mov             " n3!() ", " d2!()),
        Q!("    or              " n3!() ", " d3!()),
        Q!("    or              " n3!() ", " n1!()),
        Q!("    neg             " n3!()),
        Q!("    sbb             " q!() ", " q!()),

        // Load the non-trivial words of p_256 = [n3;0;n1;-1] and mask them with q

        Q!("    mov             " n1short!() ", 0x00000000ffffffff"),
        Q!("    mov             " n3!() ", 0xffffffff00000001"),
        Q!("    and             " n1!() ", " q!()),
        Q!("    and             " n3!() ", " q!()),

        // Do the subtraction, getting it as [n3;d0;n1;q] to avoid moves

        Q!("    sub             " q!() ", " d0!()),
        Q!("    mov             " d0short!() ", 0"),
        Q!("    sbb             " n1!() ", " d1!()),
        Q!("    sbb             " d0!() ", " d2!()),
        Q!("    sbb             " n3!() ", " d3!()),

        // Write back

        Q!("    mov             " "[" z!() "], " q!()),
        Q!("    mov             " "[" z!() "+ 8], " n1!()),
        Q!("    mov             " "[" z!() "+ 16], " d0!()),
        Q!("    mov             " "[" z!() "+ 24], " n3!()),

        inout("rdi") z.as_mut_ptr() => _,
        inout("rsi") x.as_ptr() => _,
        // clobbers
        out("r10") _,
        out("r11") _,
        out("r8") _,
        out("r9") _,
        out("rax") _,
        out("rcx") _,
        out("rdx") _,
            )
    };
}
