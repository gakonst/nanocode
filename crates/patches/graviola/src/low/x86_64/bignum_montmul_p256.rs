// generated source. do not edit.
#![allow(non_upper_case_globals, unused_macros, unused_imports)]
use crate::low::macros::*;

// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

// ----------------------------------------------------------------------------
// Montgomery multiply, z := (x * y / 2^256) mod p_256
// Inputs x[4], y[4]; output z[4]
//
//    extern void bignum_montmul_p256(uint64_t z[static 4],
//                                    const uint64_t x[static 4],
//                                    const uint64_t y[static 4]);
//
// Does z := (2^{-256} * x * y) mod p_256, assuming that the inputs x and y
// satisfy x * y <= 2^256 * p_256 (in particular this is true if we are in
// the "usual" case x < p_256 and y < p_256).
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

// We move the y argument here so we can use rdx for multipliers

macro_rules! y {
    () => {
        "rcx"
    };
}

// Add rdx * m into a register-pair (high,low)
// maintaining consistent double-carrying with adcx and adox,
// using rax and rbx as temporaries

macro_rules! mulpadd {
    ($high:expr, $low:expr, $m:expr) => { Q!(
        "mulx rbx, rax, " $m ";\n"
        "adcx " $low ", rax;\n"
        "adox " $high ", rbx"
    )}
}

/// Montgomery multiply, z := (x * y / 2^256) mod p_256
///
/// Inputs x[4], y[4]; output z[4]
///
/// Does z := (2^{-256} * x * y) mod p_256, assuming that the inputs x and y
/// satisfy x * y <= 2^256 * p_256 (in particular this is true if we are in
/// the "usual" case x < p_256 and y < p_256).
pub(crate) fn bignum_montmul_p256(z: &mut [u64; 4], x: &[u64; 4], y: &[u64; 4]) {
    // SAFETY: inline assembly. see [crate::low::inline_assembly_safety] for safety info.
    unsafe {
        core::arch::asm!(

        Q!("    endbr64         " ),

        // Save more registers to play with

        Q!("    push            " "rbx"),
        Q!("    push            " "r12"),
        Q!("    push            " "r13"),
        Q!("    push            " "r14"),
        Q!("    push            " "r15"),

        // Copy y into a safe register to start with

        Q!("    mov             " y!() ", rdx"),

        // Do row 0 computation, which is a bit different:
        // set up initial window [r12,r11,r10,r9,r8] = y[0] * x
        // Unlike later, we only need a single carry chain

        Q!("    xor             " "r13d, r13d"),
        Q!("    mov             " "rdx, [" y!() "]"),
        Q!("    mulx            " "r9, r8, [" x!() "]"),
        Q!("    mulx            " "r10, rbx, [" x!() "+ 8]"),
        Q!("    adc             " "r9, rbx"),
        Q!("    mulx            " "r11, rbx, [" x!() "+ 16]"),
        Q!("    adc             " "r10, rbx"),
        Q!("    mulx            " "r12, rbx, [" x!() "+ 24]"),
        Q!("    adc             " "r11, rbx"),
        Q!("    adc             " "r12, r13"),

        // Add row 1

        Q!("    mov             " "rdx, [" y!() "+ 8]"),
        Q!("    xor             " "r14d, r14d"),
        mulpadd!("r10", "r9", Q!("[" x!() "]")),
        mulpadd!("r11", "r10", Q!("[" x!() "+" "8" "]")),
        mulpadd!("r12", "r11", Q!("[" x!() "+" "16" "]")),
        mulpadd!("r13", "r12", Q!("[" x!() "+" "24" "]")),
        Q!("    adc             " "r13, r14"),

        // Montgomery reduce windows 0 and 1 together

        Q!("    xor             " "r15d, r15d"),
        Q!("    mov             " "rdx, 0x0000000100000000"),
        mulpadd!("r10", "r9", "r8"),
        mulpadd!("r11", "r10", "r9"),
        Q!("    not             " "rdx"),
        Q!("    lea             " "rdx, [rdx + 2]"),
        mulpadd!("r12", "r11", "r8"),
        mulpadd!("r13", "r12", "r9"),
        Q!("    adcx            " "r13, r15"),
        Q!("    adox            " "r14, r15"),
        Q!("    adc             " "r14, r15"),

        // Add row 2

        Q!("    mov             " "rdx, [" y!() "+ 16]"),
        Q!("    xor             " "r8d, r8d"),
        mulpadd!("r11", "r10", Q!("[" x!() "]")),
        mulpadd!("r12", "r11", Q!("[" x!() "+" "8" "]")),
        mulpadd!("r13", "r12", Q!("[" x!() "+" "16" "]")),
        Q!("    adox            " "r14, r8"),
        Q!("    mulx            " "rbx, rax, [" x!() "+ 24]"),
        Q!("    adc             " "r13, rax"),
        Q!("    adc             " "r14, rbx"),
        Q!("    adc             " "r15, r8"),

        // Add row 3

        Q!("    mov             " "rdx, [" y!() "+ 24]"),
        Q!("    xor             " "r9d, r9d"),
        mulpadd!("r12", "r11", Q!("[" x!() "]")),
        mulpadd!("r13", "r12", Q!("[" x!() "+" "8" "]")),
        mulpadd!("r14", "r13", Q!("[" x!() "+" "16" "]")),
        Q!("    adox            " "r15, r9"),
        Q!("    mulx            " "rbx, rax, [" x!() "+ 24]"),
        Q!("    adc             " "r14, rax"),
        Q!("    adc             " "r15, rbx"),
        Q!("    adc             " "r8, r9"),

        // Montgomery reduce windows 2 and 3 together

        Q!("    xor             " "r9d, r9d"),
        Q!("    mov             " "rdx, 0x0000000100000000"),
        mulpadd!("r12", "r11", "r10"),
        mulpadd!("r13", "r12", "r11"),
        Q!("    not             " "rdx"),
        Q!("    lea             " "rdx, [rdx + 2]"),
        mulpadd!("r14", "r13", "r10"),
        mulpadd!("r15", "r14", "r11"),
        Q!("    adcx            " "r15, r9"),
        Q!("    adox            " "r8, r9"),
        Q!("    adc             " "r8, r9"),

        // We now have a pre-reduced 5-word form [r8; r15;r14;r13;r12]
        // Load [rax;r11;rbx;rdx;rcx] = 2^320 - p_256, re-using earlier numbers a bit
        // Do [rax;r11;rbx;rdx;rcx] = [r8;r15;r14;r13;r12] + (2^320 - p_256)

        Q!("    mov             " "ecx, 1"),
        Q!("    add             " "rcx, r12"),
        Q!("    dec             " "rdx"),
        Q!("    adc             " "rdx, r13"),
        Q!("    dec             " "r9"),
        Q!("    mov             " "rax, r9"),
        Q!("    adc             " "r9, r14"),
        Q!("    mov             " "r11d, 0x00000000fffffffe"),
        Q!("    adc             " "r11, r15"),
        Q!("    adc             " "rax, r8"),

        // Now carry is set if r + (2^320 - p_256) >= 2^320, i.e. r >= p_256
        // where r is the pre-reduced form. So conditionally select the
        // output accordingly.

        Q!("    cmovc           " "r12, rcx"),
        Q!("    cmovc           " "r13, rdx"),
        Q!("    cmovc           " "r14, r9"),
        Q!("    cmovc           " "r15, r11"),

        // Write back reduced value

        Q!("    mov             " "[" z!() "], r12"),
        Q!("    mov             " "[" z!() "+ 8], r13"),
        Q!("    mov             " "[" z!() "+ 16], r14"),
        Q!("    mov             " "[" z!() "+ 24], r15"),

        // Restore registers and return

        Q!("    pop             " "r15"),
        Q!("    pop             " "r14"),
        Q!("    pop             " "r13"),
        Q!("    pop             " "r12"),
        Q!("    pop             " "rbx"),

        inout("rdi") z.as_mut_ptr() => _,
        inout("rsi") x.as_ptr() => _,
        inout("rdx") y.as_ptr() => _,
        // clobbers
        out("r10") _,
        out("r11") _,
        out("r12") _,
        out("r13") _,
        out("r14") _,
        out("r15") _,
        out("r8") _,
        out("r9") _,
        out("rax") _,
        out("rcx") _,
            )
    };
}
