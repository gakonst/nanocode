// generated source. do not edit.
#![allow(non_upper_case_globals, unused_macros, unused_imports)]
use crate::low::macros::*;

// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

// ----------------------------------------------------------------------------
// Negate modulo p_384, z := (-x) mod p_384, assuming x reduced
// Input x[6]; output z[6]
//
//    extern void bignum_neg_p384(uint64_t z[static 6], const uint64_t x[static 6]);
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

macro_rules! n0 {
    () => {
        "rax"
    };
}
macro_rules! n1 {
    () => {
        "rcx"
    };
}
macro_rules! n2 {
    () => {
        "rdx"
    };
}
macro_rules! n3 {
    () => {
        "r8"
    };
}
macro_rules! n4 {
    () => {
        "r9"
    };
}
macro_rules! q {
    () => {
        "r10"
    };
}

macro_rules! n0short {
    () => {
        "eax"
    };
}

/// Negate modulo p_384, z := (-x) mod p_384, assuming x reduced
///
/// Input x[6]; output z[6]
pub(crate) fn bignum_neg_p384(z: &mut [u64; 6], x: &[u64; 6]) {
    // SAFETY: inline assembly. see [crate::low::inline_assembly_safety] for safety info.
    unsafe {
        core::arch::asm!(

        Q!("    endbr64         " ),

        // Or together the input digits and create a bitmask q if this is nonzero, so
        // that we avoid doing -0 = p_384 and hence maintain strict modular reduction

        Q!("    mov             " n0!() ", [" x!() "]"),
        Q!("    or              " n0!() ", [" x!() "+ 8]"),
        Q!("    mov             " n1!() ", [" x!() "+ 16]"),
        Q!("    or              " n1!() ", [" x!() "+ 24]"),
        Q!("    mov             " n2!() ", [" x!() "+ 32]"),
        Q!("    or              " n2!() ", [" x!() "+ 40]"),
        Q!("    or              " n0!() ", " n1!()),
        Q!("    or              " n0!() ", " n2!()),
        Q!("    neg             " n0!()),
        Q!("    sbb             " q!() ", " q!()),

        // Let [q;n4;n3;n2;n1;n0] = if q then p_384 else 0

        Q!("    mov             " n0short!() ", 0x00000000ffffffff"),
        Q!("    and             " n0!() ", " q!()),
        Q!("    mov             " n1!() ", 0xffffffff00000000"),
        Q!("    and             " n1!() ", " q!()),
        Q!("    mov             " n2!() ", 0xfffffffffffffffe"),
        Q!("    and             " n2!() ", " q!()),
        Q!("    mov             " n3!() ", " q!()),
        Q!("    mov             " n4!() ", " q!()),

        // Do the subtraction

        Q!("    sub             " n0!() ", [" x!() "]"),
        Q!("    sbb             " n1!() ", [" x!() "+ 8]"),
        Q!("    sbb             " n2!() ", [" x!() "+ 16]"),
        Q!("    sbb             " n3!() ", [" x!() "+ 24]"),
        Q!("    sbb             " n4!() ", [" x!() "+ 32]"),
        Q!("    sbb             " q!() ", [" x!() "+ 40]"),

        // Write back

        Q!("    mov             " "[" z!() "], " n0!()),
        Q!("    mov             " "[" z!() "+ 8], " n1!()),
        Q!("    mov             " "[" z!() "+ 16], " n2!()),
        Q!("    mov             " "[" z!() "+ 24], " n3!()),
        Q!("    mov             " "[" z!() "+ 32], " n4!()),
        Q!("    mov             " "[" z!() "+ 40], " q!()),

        inout("rdi") z.as_mut_ptr() => _,
        inout("rsi") x.as_ptr() => _,
        // clobbers
        out("r10") _,
        out("r8") _,
        out("r9") _,
        out("rax") _,
        out("rcx") _,
        out("rdx") _,
            )
    };
}
