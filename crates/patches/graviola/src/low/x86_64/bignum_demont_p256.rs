// generated source. do not edit.
#![allow(non_upper_case_globals, unused_macros, unused_imports)]
use crate::low::macros::*;

// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

// ----------------------------------------------------------------------------
// Convert from Montgomery form z := (x / 2^256) mod p_256, assuming x reduced
// Input x[4]; output z[4]
//
//    extern void bignum_demont_p256(uint64_t z[static 4],
//                                   const uint64_t x[static 4]);
//
// This assumes the input is < p_256 for correctness. If this is not the case,
// use the variant "bignum_deamont_p256" instead.
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

// Add rdx * m into a register-pair (high,low)
// maintaining consistent double-carrying with adcx and adox,
// using rax and rcx as temporaries

macro_rules! mulpadd {
    ($high:expr, $low:expr, $m:expr) => { Q!(
        "mulx rcx, rax, " $m ";\n"
        "adcx " $low ", rax;\n"
        "adox " $high ", rcx"
    )}
}

/// Convert from Montgomery form z := (x / 2^256) mod p_256, assuming x reduced
///
/// Input x[4]; output z[4]
///
/// This assumes the input is < p_256 for correctness. If this is not the case,
/// use the variant "bignum_deamont_p256" instead.
pub(crate) fn bignum_demont_p256(z: &mut [u64; 4], x: &[u64; 4]) {
    // SAFETY: inline assembly. see [crate::low::inline_assembly_safety] for safety info.
    unsafe {
        core::arch::asm!(

        Q!("    endbr64         " ),

        // Save one more register to play with

        Q!("    push            " "rbx"),

        // Set up an initial 4-word window [r11,r10,r9,r8] = x

        Q!("    mov             " "r8, [" x!() "]"),
        Q!("    mov             " "r9, [" x!() "+ 8]"),
        Q!("    mov             " "r10, [" x!() "+ 16]"),
        Q!("    mov             " "r11, [" x!() "+ 24]"),

        // Fill in two zeros to the left

        Q!("    xor             " "rbx, rbx"),
        Q!("    xor             " "rsi, rsi"),

        // Montgomery reduce windows 0 and 1 together

        Q!("    mov             " "rdx, 0x0000000100000000"),
        mulpadd!("r10", "r9", "r8"),
        mulpadd!("r11", "r10", "r9"),
        Q!("    mov             " "rdx, 0xffffffff00000001"),
        mulpadd!("rbx", "r11", "r8"),
        mulpadd!("rsi", "rbx", "r9"),
        Q!("    mov             " "r8d, 0"),
        Q!("    adcx            " "rsi, r8"),

        // Append just one more leading zero (by the above r8 = 0 already).

        Q!("    xor             " "r9, r9"),

        // Montgomery reduce windows 2 and 3 together

        Q!("    mov             " "rdx, 0x0000000100000000"),
        mulpadd!("rbx", "r11", "r10"),
        mulpadd!("rsi", "rbx", "r11"),
        Q!("    mov             " "rdx, 0xffffffff00000001"),
        mulpadd!("r8", "rsi", "r10"),
        mulpadd!("r9", "r8", "r11"),
        Q!("    mov             " "r10d, 0"),
        Q!("    adcx            " "r9, r10"),

        // Since the input was assumed reduced modulo, i.e. < p, we actually know that
        // 2^256 * [carries; r9;r8;rsi;rbx] is <= (p - 1) + (2^256 - 1) p
        // and hence [carries; r9;r8;rsi;rbx] < p. This means in fact carries = 0
        // and [r9;r8;rsi;rbx] is already our answer, without further correction.
        // Write that back.

        Q!("    mov             " "[" z!() "], rbx"),
        Q!("    mov             " "[" z!() "+ 8], rsi"),
        Q!("    mov             " "[" z!() "+ 16], r8"),
        Q!("    mov             " "[" z!() "+ 24], r9"),

        // Restore saved register and return

        Q!("    pop             " "rbx"),

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
