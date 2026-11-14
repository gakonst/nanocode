// generated source. do not edit.
#![allow(non_upper_case_globals, unused_macros, unused_imports)]
use crate::low::macros::*;

// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

// ----------------------------------------------------------------------------
// Montgomery square, z := (x^2 / 2^256) mod p_256
// Input x[4]; output z[4]
//
//    extern void bignum_montsqr_p256(uint64_t z[static 4],
//                                    const uint64_t x[static 4]);
//
// Does z := (x^2 / 2^256) mod p_256, assuming x^2 <= 2^256 * p_256, which is
// guaranteed in particular if x < p_256 initially (the "intended" case).
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

// Use this fairly consistently for a zero

macro_rules! zero {
    () => {
        "rbp"
    };
}
macro_rules! zeroe {
    () => {
        "ebp"
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

/// Montgomery square, z := (x^2 / 2^256) mod p_256
///
/// Input x[4]; output z[4]
///
/// Does z := (x^2 / 2^256) mod p_256, assuming x^2 <= 2^256 * p_256, which is
/// guaranteed in particular if x < p_256 initially (the "intended" case).
pub(crate) fn bignum_montsqr_p256(z: &mut [u64; 4], x: &[u64; 4]) {
    // SAFETY: inline assembly. see [crate::low::inline_assembly_safety] for safety info.
    unsafe {
        core::arch::asm!(

        Q!("    endbr64         " ),

        // Save more registers to play with

        Q!("    push            " "rbx"),
        Q!("    push            " "rbp"),
        Q!("    push            " "r12"),
        Q!("    push            " "r13"),
        Q!("    push            " "r14"),
        Q!("    push            " "r15"),

        // Compute [r15;r8] = [00] which we use later, but mainly
        // set up an initial window [r14;...;r9] = [23;03;01]

        Q!("    mov             " "rdx, [" x!() "]"),
        Q!("    mulx            " "r15, r8, rdx"),
        Q!("    mulx            " "r10, r9, [" x!() "+ 8]"),
        Q!("    mulx            " "r12, r11, [" x!() "+ 24]"),
        Q!("    mov             " "rdx, [" x!() "+ 16]"),
        Q!("    mulx            " "r14, r13, [" x!() "+ 24]"),

        // Clear our zero register, and also initialize the flags for the carry chain

        Q!("    xor             " zeroe!() ", " zeroe!()),

        // Chain in the addition of 02 + 12 + 13 to that window (no carry-out possible)
        // This gives all the "heterogeneous" terms of the squaring ready to double

        mulpadd!("r11", "r10", Q!("[" x!() "]")),
        mulpadd!("r12", "r11", Q!("[" x!() "+" "8" "]")),
        Q!("    mov             " "rdx, [" x!() "+ 24]"),
        mulpadd!("r13", "r12", Q!("[" x!() "+" "8" "]")),
        Q!("    adcx            " "r13, " zero!()),
        Q!("    adox            " "r14, " zero!()),
        Q!("    adc             " "r14, " zero!()),

        // Double and add to the 00 + 11 + 22 + 33 terms

        Q!("    xor             " zeroe!() ", " zeroe!()),
        Q!("    adcx            " "r9, r9"),
        Q!("    adox            " "r9, r15"),
        Q!("    mov             " "rdx, [" x!() "+ 8]"),
        Q!("    mulx            " "rdx, rax, rdx"),
        Q!("    adcx            " "r10, r10"),
        Q!("    adox            " "r10, rax"),
        Q!("    adcx            " "r11, r11"),
        Q!("    adox            " "r11, rdx"),
        Q!("    mov             " "rdx, [" x!() "+ 16]"),
        Q!("    mulx            " "rdx, rax, rdx"),
        Q!("    adcx            " "r12, r12"),
        Q!("    adox            " "r12, rax"),
        Q!("    adcx            " "r13, r13"),
        Q!("    adox            " "r13, rdx"),
        Q!("    mov             " "rdx, [" x!() "+ 24]"),
        Q!("    mulx            " "r15, rax, rdx"),
        Q!("    adcx            " "r14, r14"),
        Q!("    adox            " "r14, rax"),
        Q!("    adcx            " "r15, " zero!()),
        Q!("    adox            " "r15, " zero!()),

        // First two waves of Montgomery reduction. Consolidate the double carries
        // in r9 and propagate up to the top in r8, which is no longer needed otherwise.

        Q!("    xor             " zeroe!() ", " zeroe!()),
        Q!("    mov             " "rdx, 0x0000000100000000"),
        mulpadd!("r10", "r9", "r8"),
        mulpadd!("r11", "r10", "r9"),
        Q!("    mov             " "rdx, 0xffffffff00000001"),
        mulpadd!("r12", "r11", "r8"),
        mulpadd!("r13", "r12", "r9"),
        Q!("    adcx            " "r13, " zero!()),
        Q!("    mov             " "r9d, " zeroe!()),
        Q!("    adox            " "r9, " zero!()),
        Q!("    adcx            " "r9, " zero!()),
        Q!("    add             " "r14, r9"),
        Q!("    adc             " "r15, " zero!()),
        Q!("    mov             " "r8d, " zeroe!()),
        Q!("    adc             " "r8, " zero!()),

        // Now two more steps of Montgomery reduction, again with r8 = top carry

        Q!("    xor             " zeroe!() ", " zeroe!()),
        Q!("    mov             " "rdx, 0x0000000100000000"),
        mulpadd!("r12", "r11", "r10"),
        mulpadd!("r13", "r12", "r11"),
        Q!("    mov             " "rdx, 0xffffffff00000001"),
        mulpadd!("r14", "r13", "r10"),
        mulpadd!("r15", "r14", "r11"),
        Q!("    adcx            " "r15, " zero!()),
        Q!("    adox            " "r8, " zero!()),
        Q!("    adc             " "r8, " zero!()),

        // Load [rax;r11;rbp;rdx;rcx] = 2^320 - p_256, re-using earlier numbers a bit
        // Do [rax;r11;rbp;rdx;rcx] = [r8;r15;r14;r13;r12] + (2^320 - p_256)

        Q!("    mov             " "ecx, 1"),
        Q!("    add             " "rcx, r12"),
        Q!("    lea             " "rdx, [rdx -1]"),
        Q!("    adc             " "rdx, r13"),
        Q!("    lea             " "rbp, [rbp -1]"),
        Q!("    mov             " "rax, rbp"),
        Q!("    adc             " "rbp, r14"),
        Q!("    mov             " "r11d, 0x00000000fffffffe"),
        Q!("    adc             " "r11, r15"),
        Q!("    adc             " "rax, r8"),

        // Now carry is set if r + (2^320 - p_256) >= 2^320, i.e. r >= p_256
        // where r is the pre-reduced form. So conditionally select the
        // output accordingly.

        Q!("    cmovc           " "r12, rcx"),
        Q!("    cmovc           " "r13, rdx"),
        Q!("    cmovc           " "r14, rbp"),
        Q!("    cmovc           " "r15, r11"),

        // Write back reduced value

        Q!("    mov             " "[" z!() "], r12"),
        Q!("    mov             " "[" z!() "+ 8], r13"),
        Q!("    mov             " "[" z!() "+ 16], r14"),
        Q!("    mov             " "[" z!() "+ 24], r15"),

        // Restore saved registers and return

        Q!("    pop             " "r15"),
        Q!("    pop             " "r14"),
        Q!("    pop             " "r13"),
        Q!("    pop             " "r12"),
        Q!("    pop             " "rbp"),
        Q!("    pop             " "rbx"),

        inout("rdi") z.as_mut_ptr() => _,
        inout("rsi") x.as_ptr() => _,
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
        out("rdx") _,
            )
    };
}
