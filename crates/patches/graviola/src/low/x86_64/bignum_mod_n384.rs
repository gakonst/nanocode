// generated source. do not edit.
#![allow(non_upper_case_globals, unused_macros, unused_imports)]
use crate::low::macros::*;

// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

// ----------------------------------------------------------------------------
// Reduce modulo group order, z := x mod n_384
// Input x[6]; output z[6]
//
//    extern void bignum_mod_n384_6(uint64_t z[static 6], const uint64_t x[static 6]);
//
// Reduction is modulo the group order of the NIST curve P-384.
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
macro_rules! d4 {
    () => {
        "r10"
    };
}
macro_rules! d5 {
    () => {
        "r11"
    };
}

macro_rules! a {
    () => {
        "rax"
    };
}

// Re-use the input pointer as a temporary once we're done

macro_rules! c {
    () => {
        "rsi"
    };
}

/// Reduce modulo group order, z := x mod n_384
///
/// Input x[6]; output z[6]
///
/// Reduction is modulo the group order of the NIST curve P-384.
pub(crate) fn bignum_mod_n384(z: &mut [u64; 6], x: &[u64; 6]) {
    // SAFETY: inline assembly. see [crate::low::inline_assembly_safety] for safety info.
    unsafe {
        core::arch::asm!(

        Q!("    endbr64         " ),

        // Load the input and compute x + (2^384 - n_384)

        Q!("    mov             " a!() ", 0x1313e695333ad68d"),
        Q!("    mov             " d0!() ", [" x!() "]"),
        Q!("    add             " d0!() ", " a!()),
        Q!("    mov             " d1!() ", 0xa7e5f24db74f5885"),
        Q!("    adc             " d1!() ", [" x!() "+ 8]"),
        Q!("    mov             " d2!() ", 0x389cb27e0bc8d220"),
        Q!("    adc             " d2!() ", [" x!() "+ 16]"),
        Q!("    mov             " d3!() ", [" x!() "+ 24]"),
        Q!("    adc             " d3!() ", 0"),
        Q!("    mov             " d4!() ", [" x!() "+ 32]"),
        Q!("    adc             " d4!() ", 0"),
        Q!("    mov             " d5!() ", [" x!() "+ 40]"),
        Q!("    adc             " d5!() ", 0"),

        // Now CF is set iff 2^384 <= x + (2^384 - n_384), i.e. iff n_384 <= x.
        // Create a mask for the condition x < n. We now want to subtract the
        // masked (2^384 - n_384), but because we're running out of registers
        // without using a save-restore sequence, we need some contortions.
        // Create the lowest digit (re-using a kept from above)

        Q!("    sbb             " c!() ", " c!()),
        Q!("    not             " c!()),
        Q!("    and             " a!() ", " c!()),

        // Do the first digit of addition and writeback

        Q!("    sub             " d0!() ", " a!()),
        Q!("    mov             " "[" z!() "], " d0!()),

        // Preserve carry chain and do the next digit

        Q!("    sbb             " d0!() ", " d0!()),
        Q!("    mov             " a!() ", 0xa7e5f24db74f5885"),
        Q!("    and             " a!() ", " c!()),
        Q!("    neg             " d0!()),
        Q!("    sbb             " d1!() ", " a!()),
        Q!("    mov             " "[" z!() "+ 8], " d1!()),

        // Preserve carry chain once more and do remaining digits

        Q!("    sbb             " d0!() ", " d0!()),
        Q!("    mov             " a!() ", 0x389cb27e0bc8d220"),
        Q!("    and             " a!() ", " c!()),
        Q!("    neg             " d0!()),
        Q!("    sbb             " d2!() ", " a!()),
        Q!("    mov             " "[" z!() "+ 16], " d2!()),
        Q!("    sbb             " d3!() ", 0"),
        Q!("    mov             " "[" z!() "+ 24], " d3!()),
        Q!("    sbb             " d4!() ", 0"),
        Q!("    mov             " "[" z!() "+ 32], " d4!()),
        Q!("    sbb             " d5!() ", 0"),
        Q!("    mov             " "[" z!() "+ 40], " d5!()),

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
