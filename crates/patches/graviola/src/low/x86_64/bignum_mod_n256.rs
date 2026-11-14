// generated source. do not edit.
#![allow(non_upper_case_globals, unused_macros, unused_imports)]
use crate::low::macros::*;

// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

// ----------------------------------------------------------------------------
// Reduce modulo group order, z := x mod n_256
// Input x[4]; output z[4]
//
//    extern void bignum_mod_n256_4(uint64_t z[static 4], const uint64_t x[static 4]);
//
// Reduction is modulo the group order of the NIST curve P-256.
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

macro_rules! d0 {
    () => {
        "rdx"
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

macro_rules! n0 {
    () => {
        "rax"
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

macro_rules! n3short {
    () => {
        "r11d"
    };
}

// Can re-use this as a temporary once we've loaded the input

macro_rules! c {
    () => {
        "rsi"
    };
}

/// Reduce modulo group order, z := x mod n_256
///
/// Input x[4]; output z[4]
///
/// Reduction is modulo the group order of the NIST curve P-256.
pub(crate) fn bignum_mod_n256(z: &mut [u64; 4], x: &[u64; 4]) {
    // SAFETY: inline assembly. see [crate::low::inline_assembly_safety] for safety info.
    unsafe {
        core::arch::asm!(

        Q!("    endbr64         " ),

        // Load a set of registers [n3; 0; n1; n0] = 2^256 - n_256

        Q!("    mov             " n0!() ", 0x0c46353d039cdaaf"),
        Q!("    mov             " n1!() ", 0x4319055258e8617b"),
        Q!("    mov             " n3short!() ", 0x00000000ffffffff"),

        // Load the input and compute x + (2^256 - n_256)

        Q!("    mov             " d0!() ", [" x!() "]"),
        Q!("    add             " d0!() ", " n0!()),
        Q!("    mov             " d1!() ", [" x!() "+ 8]"),
        Q!("    adc             " d1!() ", " n1!()),
        Q!("    mov             " d2!() ", [" x!() "+ 16]"),
        Q!("    adc             " d2!() ", 0"),
        Q!("    mov             " d3!() ", [" x!() "+ 24]"),
        Q!("    adc             " d3!() ", " n3!()),

        // Now CF is set iff 2^256 <= x + (2^256 - n_256), i.e. iff n_256 <= x.
        // Create a mask for the condition x < n, and mask the three nontrivial digits
        // ready to undo the previous addition with a compensating subtraction

        Q!("    sbb             " c!() ", " c!()),
        Q!("    not             " c!()),
        Q!("    and             " n0!() ", " c!()),
        Q!("    and             " n1!() ", " c!()),
        Q!("    and             " n3!() ", " c!()),

        // Now subtract mask * (2^256 - n_256) again and store

        Q!("    sub             " d0!() ", " n0!()),
        Q!("    mov             " "[" z!() "], " d0!()),
        Q!("    sbb             " d1!() ", " n1!()),
        Q!("    mov             " "[" z!() "+ 8], " d1!()),
        Q!("    sbb             " d2!() ", 0"),
        Q!("    mov             " "[" z!() "+ 16], " d2!()),
        Q!("    sbb             " d3!() ", " n3!()),
        Q!("    mov             " "[" z!() "+ 24], " d3!()),

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
