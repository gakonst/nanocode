// generated source. do not edit.
#![allow(non_upper_case_globals, unused_macros, unused_imports)]
use crate::low::macros::*;

// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

// ----------------------------------------------------------------------------
// Negate modulo p_25519, z := (-x) mod p_25519, assuming x reduced
// Input x[4]; output z[4]
//
//    extern void bignum_neg_p25519(uint64_t z[static 4], const uint64_t x[static 4]);
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
        "r8"
    };
}
macro_rules! n3 {
    () => {
        "r9"
    };
}

macro_rules! c {
    () => {
        "r10"
    };
}

macro_rules! qshort {
    () => {
        "esi"
    };
}

/// Negate modulo p_25519, z := (-x) mod p_25519, assuming x reduced
///
/// Input x[4]; output z[4]
pub(crate) fn bignum_neg_p25519(z: &mut [u64; 4], x: &[u64; 4]) {
    // SAFETY: inline assembly. see [crate::low::inline_assembly_safety] for safety info.
    unsafe {
        core::arch::asm!(

        Q!("    endbr64         " ),

        // Load the 4 digits of x and let q be an OR of all the digits

        Q!("    mov             " n0!() ", [" x!() "]"),
        Q!("    mov             " q!() ", " n0!()),
        Q!("    mov             " n1!() ", [" x!() "+ 8]"),
        Q!("    or              " q!() ", " n1!()),
        Q!("    mov             " n2!() ", [" x!() "+ 16]"),
        Q!("    or              " q!() ", " n2!()),
        Q!("    mov             " n3!() ", [" x!() "+ 24]"),
        Q!("    or              " q!() ", " n3!()),

        // Turn q into a strict x <> 0 bitmask, and c into a masked constant [-19]
        // so that [q;q;q;c] = [2^256 - 19], masked according to nonzeroness of x

        Q!("    neg             " q!()),
        Q!("    sbb             " q!() ", " q!()),
        Q!("    mov             " c!() ", -19"),
        Q!("    and             " c!() ", " q!()),

        // Now just do [2^256 - 19] - x and then mask to 255 bits,
        // which means in effect the required [2^255 - 19] - x

        Q!("    sub             " c!() ", " n0!()),
        Q!("    mov             " "[" z!() "], " c!()),
        Q!("    mov             " c!() ", " q!()),
        Q!("    sbb             " c!() ", " n1!()),
        Q!("    mov             " "[" z!() "+ 8], " c!()),
        Q!("    mov             " c!() ", " q!()),
        Q!("    sbb             " c!() ", " n2!()),
        Q!("    mov             " "[" z!() "+ 16], " c!()),
        Q!("    sbb             " q!() ", " n3!()),
        Q!("    btr             " q!() ", 63"),
        Q!("    mov             " "[" z!() "+ 24], " q!()),

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
