// generated source. do not edit.
#![allow(non_upper_case_globals, unused_macros, unused_imports)]
use crate::low::macros::*;

// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

// ----------------------------------------------------------------------------
// Convert from Montgomery form z := (x / 2^384) mod p_384, assuming x reduced
// Input x[6]; output z[6]
//
//    extern void bignum_demont_p384(uint64_t z[static 6],
//                                   const uint64_t x[static 6]);
//
// This assumes the input is < p_384 for correctness. If this is not the case,
// use the variant "bignum_deamont_p384" instead.
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

// Core one-step "short" Montgomery reduction macro. Takes input in
// [d5;d4;d3;d2;d1;d0] and returns result in [d6;d5;d4;d3;d2;d1],
// adding to the existing contents of [d5;d4;d3;d2;d1;d0]. This
// is intended only for 6-word inputs as in mapping out of Montgomery,
// not for the general case of Montgomery multiplication. It is fine
// for d6 to be the same register as d0.
//
// Parms:  montreds(d6,d5,d4,d3,d2,d1,d0)
//
// We want to add (2^384 - 2^128 - 2^96 + 2^32 - 1) * w
// where w = [d0 + (d0<<32)] mod 2^64

macro_rules! montreds {
    ($d6:expr, $d5:expr, $d4:expr, $d3:expr, $d2:expr, $d1:expr, $d0:expr) => { Q!(
        /* Our correction multiplier is w = [d0 + (d0<<32)] mod 2^64 */
        "mov rdx, " $d0 ";\n"
        "shl rdx, 32;\n"
        "add rdx, " $d0 ";\n"
        /* Construct [rsi;rcx;rax;-] = (2^384 - p_384) * w           */
        /* We know the lowest word will cancel so we can re-use d0   */
        /* as a temp.                                                */
        "xor rsi, rsi;\n"
        "mov rax, 0xffffffff00000001;\n"
        "mulx rax, rcx, rax;\n"
        "mov ecx, 0x00000000ffffffff;\n"
        "mulx rcx, " $d0 ", rcx;\n"
        "adc rax, " $d0 ";\n"
        "adc rcx, rdx;\n"
        "adc rsi, 0;\n"
        /* Now subtract that and add 2^384 * w                       */
        "sub " $d1 ", rax;\n"
        "sbb " $d2 ", rcx;\n"
        "sbb " $d3 ", rsi;\n"
        "sbb " $d4 ", 0;\n"
        "sbb " $d5 ", 0;\n"
        "mov " $d6 ", rdx;\n"
        "sbb " $d6 ", 0"
    )}
}

/// Convert from Montgomery form z := (x / 2^384) mod p_384, assuming x reduced
///
/// Input x[6]; output z[6]
///
/// This assumes the input is < p_384 for correctness. If this is not the case,
/// use the variant "bignum_deamont_p384" instead.
pub(crate) fn bignum_demont_p384(z: &mut [u64; 6], x: &[u64; 6]) {
    // SAFETY: inline assembly. see [crate::low::inline_assembly_safety] for safety info.
    unsafe {
        core::arch::asm!(

        Q!("    endbr64         " ),

        // Save more registers to play with

        Q!("    push            " "r12"),
        Q!("    push            " "r13"),

        // Set up an initial window [r13,r12,r11,r10,r9,r8] = x

        Q!("    mov             " "r8, [" x!() "]"),
        Q!("    mov             " "r9, [" x!() "+ 8]"),
        Q!("    mov             " "r10, [" x!() "+ 16]"),
        Q!("    mov             " "r11, [" x!() "+ 24]"),
        Q!("    mov             " "r12, [" x!() "+ 32]"),
        Q!("    mov             " "r13, [" x!() "+ 40]"),

        // Montgomery reduce window 0

        montreds!("r8", "r13", "r12", "r11", "r10", "r9", "r8"),

        // Montgomery reduce window 1

        montreds!("r9", "r8", "r13", "r12", "r11", "r10", "r9"),

        // Montgomery reduce window 2

        montreds!("r10", "r9", "r8", "r13", "r12", "r11", "r10"),

        // Montgomery reduce window 3

        montreds!("r11", "r10", "r9", "r8", "r13", "r12", "r11"),

        // Montgomery reduce window 4

        montreds!("r12", "r11", "r10", "r9", "r8", "r13", "r12"),

        // Montgomery reduce window 5

        montreds!("r13", "r12", "r11", "r10", "r9", "r8", "r13"),

        // Write back the result

        Q!("    mov             " "[" z!() "], r8"),
        Q!("    mov             " "[" z!() "+ 8], r9"),
        Q!("    mov             " "[" z!() "+ 16], r10"),
        Q!("    mov             " "[" z!() "+ 24], r11"),
        Q!("    mov             " "[" z!() "+ 32], r12"),
        Q!("    mov             " "[" z!() "+ 40], r13"),

        // Restore registers and return

        Q!("    pop             " "r13"),
        Q!("    pop             " "r12"),

        inout("rdi") z.as_mut_ptr() => _,
        inout("rsi") x.as_ptr() => _,
        // clobbers
        out("r10") _,
        out("r11") _,
        out("r12") _,
        out("r13") _,
        out("r8") _,
        out("r9") _,
        out("rax") _,
        out("rcx") _,
        out("rdx") _,
            )
    };
}
