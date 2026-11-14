// generated source. do not edit.
#![allow(non_upper_case_globals, unused_macros, unused_imports)]
use crate::low::macros::*;

// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

// ----------------------------------------------------------------------------
// Add modulo p_256, z := (x + y) mod p_256, assuming x and y reduced
// Inputs x[4], y[4]; output z[4]
//
//    extern void bignum_add_p256(uint64_t z[static 4], const uint64_t x[static 4],
//                                const uint64_t y[static 4]);
//
// Standard x86-64 ABI: RDI = z, RSI = x, RDX = y
// Microsoft x64 ABI:   RCX = z, RDX = x, R8 = y
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
macro_rules! y {
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
        "rdx"
    };
}
macro_rules! c {
    () => {
        "r11"
    };
}

macro_rules! n1short {
    () => {
        "r10d"
    };
}

/// Add modulo p_256, z := (x + y) mod p_256, assuming x and y reduced
///
/// Inputs x[4], y[4]; output z[4]
pub(crate) fn bignum_add_p256(z: &mut [u64; 4], x: &[u64; 4], y: &[u64; 4]) {
    // SAFETY: inline assembly. see [crate::low::inline_assembly_safety] for safety info.
    unsafe {
        core::arch::asm!(

        Q!("    endbr64         " ),

        // Load and add the two inputs as 2^256 * c + [d3;d2;d1;d0] = x + y

        Q!("    xor             " c!() ", " c!()),
        Q!("    mov             " d0!() ", [" x!() "]"),
        Q!("    add             " d0!() ", [" y!() "]"),
        Q!("    mov             " d1!() ", [" x!() "+ 8]"),
        Q!("    adc             " d1!() ", [" y!() "+ 8]"),
        Q!("    mov             " d2!() ", [" x!() "+ 16]"),
        Q!("    adc             " d2!() ", [" y!() "+ 16]"),
        Q!("    mov             " d3!() ", [" x!() "+ 24]"),
        Q!("    adc             " d3!() ", [" y!() "+ 24]"),
        Q!("    adc             " c!() ", " c!()),

        // Now subtract 2^256 * c + [d3;d3;d1;d1] = x + y - p_256
        // The constants n1 and n3 in [n3; 0; n1; -1] = p_256 are saved for later

        Q!("    sub             " d0!() ", -1"),
        Q!("    mov             " n1short!() ", 0x00000000ffffffff"),
        Q!("    sbb             " d1!() ", " n1!()),
        Q!("    sbb             " d2!() ", 0"),
        Q!("    mov             " n3!() ", 0xffffffff00000001"),
        Q!("    sbb             " d3!() ", " n3!()),

        // Since by hypothesis x < p_256 we know x + y - p_256 < 2^256, so the top
        // carry c actually gives us a bitmask for x + y - p_256 < 0, which we
        // now use to make a masked p_256' = [n3; 0; n1; c]

        Q!("    sbb             " c!() ", 0"),
        Q!("    and             " n1!() ", " c!()),
        Q!("    and             " n3!() ", " c!()),

        // Do the corrective addition and copy to output

        Q!("    add             " d0!() ", " c!()),
        Q!("    mov             " "[" z!() "], " d0!()),
        Q!("    adc             " d1!() ", " n1!()),
        Q!("    mov             " "[" z!() "+ 8], " d1!()),
        Q!("    adc             " d2!() ", 0"),
        Q!("    mov             " "[" z!() "+ 16], " d2!()),
        Q!("    adc             " d3!() ", " n3!()),
        Q!("    mov             " "[" z!() "+ 24], " d3!()),

        inout("rdi") z.as_mut_ptr() => _,
        inout("rsi") x.as_ptr() => _,
        inout("rdx") y.as_ptr() => _,
        // clobbers
        out("r10") _,
        out("r11") _,
        out("r8") _,
        out("r9") _,
        out("rax") _,
        out("rcx") _,
            )
    };
}
