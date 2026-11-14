// generated source. do not edit.
#![allow(non_upper_case_globals, unused_macros, unused_imports)]
use crate::low::macros::*;

// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

// ----------------------------------------------------------------------------
// Montgomery multiply, z := (x * y / 2^384) mod p_384
// Inputs x[6], y[6]; output z[6]
//
//    extern void bignum_montmul_p384(uint64_t z[static 6],
//                                    const uint64_t x[static 6],
//                                    const uint64_t y[static 6]);
//
// Does z := (2^{-384} * x * y) mod p_384, assuming that the inputs x and y
// satisfy x * y <= 2^384 * p_384 (in particular this is true if we are in
// the "usual" case x < p_384 and y < p_384).
//
// Standard x86-64 ABI: RDI = z, RSI = x, RDX = y
// Microsoft x64 ABI:   RCX = z, RDX = x, R8 = y
// -----------------------------------------------------------------------------

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

// Some temp registers for the last correction stage

macro_rules! d {
    () => {
        "rax"
    };
}
macro_rules! u {
    () => {
        "rdx"
    };
}
macro_rules! v {
    () => {
        "rcx"
    };
}
macro_rules! w {
    () => {
        "rbx"
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

// Core one-step Montgomery reduction macro. Takes input in
// [d7;d6;d5;d4;d3;d2;d1;d0] and returns result in [d7;d6;d5;d4;d3;d2;d1],
// adding to the existing contents, re-using d0 as a temporary internally
//
// We want to add (2^384 - 2^128 - 2^96 + 2^32 - 1) * w
// where w = [d0 + (d0<<32)] mod 2^64
//
//       montredc(d7,d6,d5,d4,d3,d2,d1,d0)
//
// This particular variant, with its mix of addition and subtraction
// at the top, is not intended to maintain a coherent carry or borrow out.
// It is assumed the final result would fit in [d7;d6;d5;d4;d3;d2;d1].
// which is always the case here as the top word is even always in {0,1}

macro_rules! montredc {
    ($d7:expr, $d6:expr, $d5:expr, $d4:expr, $d3:expr, $d2:expr, $d1:expr, $d0:expr) => { Q!(
        /* Our correction multiplier is w = [d0 + (d0<<32)] mod 2^64 */
        "mov rdx, " $d0 ";\n"
        "shl rdx, 32;\n"
        "add rdx, " $d0 ";\n"
        /* Construct [rbp;rbx;rax;-] = (2^384 - p_384) * w */
        /* We know the lowest word will cancel so we can re-use d0 as a temp */
        "xor ebp, ebp;\n"
        "mov rax, 0xffffffff00000001;\n"
        "mulx rax, rbx, rax;\n"
        "mov ebx, 0x00000000ffffffff;\n"
        "mulx rbx, " $d0 ", rbx;\n"
        "adc rax, " $d0 ";\n"
        "adc rbx, rdx;\n"
        "adc ebp, ebp;\n"
        /*  Now subtract that and add 2^384 * w */
        "sub " $d1 ", rax;\n"
        "sbb " $d2 ", rbx;\n"
        "sbb " $d3 ", rbp;\n"
        "sbb " $d4 ", 0;\n"
        "sbb " $d5 ", 0;\n"
        "sbb rdx, 0;\n"
        "add " $d6 ", rdx;\n"
        "adc " $d7 ", 0"
    )}
}

/// Montgomery multiply, z := (x * y / 2^384) mod p_384
///
/// Inputs x[6], y[6]; output z[6]
///
/// Does z := (2^{-384} * x * y) mod p_384, assuming that the inputs x and y
/// satisfy x * y <= 2^384 * p_384 (in particular this is true if we are in
/// the "usual" case x < p_384 and y < p_384).
pub(crate) fn bignum_montmul_p384(z: &mut [u64; 6], x: &[u64; 6], y: &[u64; 6]) {
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

        // Copy y into a safe register to start with

        Q!("    mov             " y!() ", rdx"),

        // Do row 0 computation, which is a bit different:
        // set up initial window [r14,r13,r12,r11,r10,r9,r8] = y[0] * x
        // Unlike later, we only need a single carry chain

        Q!("    mov             " "rdx, [" y!() "]"),
        Q!("    xor             " "r15d, r15d"),
        Q!("    mulx            " "r9, r8, [" x!() "]"),
        Q!("    mulx            " "r10, rbx, [" x!() "+ 8]"),
        Q!("    add             " "r9, rbx"),
        Q!("    mulx            " "r11, rbx, [" x!() "+ 16]"),
        Q!("    adc             " "r10, rbx"),
        Q!("    mulx            " "r12, rbx, [" x!() "+ 24]"),
        Q!("    adc             " "r11, rbx"),
        Q!("    mulx            " "r13, rbx, [" x!() "+ 32]"),
        Q!("    adc             " "r12, rbx"),
        Q!("    mulx            " "r14, rbx, [" x!() "+ 40]"),
        Q!("    adc             " "r13, rbx"),
        Q!("    adc             " "r14, r15"),

        // Montgomery reduce the zeroth window

        montredc!("r15", "r14", "r13", "r12", "r11", "r10", "r9", "r8"),

        // Add row 1

        Q!("    mov             " "rdx, [" y!() "+ 8]"),
        Q!("    xor             " "r8d, r8d"),
        mulpadd!("r10", "r9", Q!("[" x!() "]")),
        mulpadd!("r11", "r10", Q!("[" x!() "+" "8" "]")),
        mulpadd!("r12", "r11", Q!("[" x!() "+" "16" "]")),
        mulpadd!("r13", "r12", Q!("[" x!() "+" "24" "]")),
        mulpadd!("r14", "r13", Q!("[" x!() "+" "32" "]")),
        Q!("    adox            " "r15, r8"),
        Q!("    mulx            " "rbx, rax, [" x!() "+ 40]"),
        Q!("    adc             " "r14, rax"),
        Q!("    adc             " "r15, rbx"),
        Q!("    adc             " "r8, r8"),

        // Montgomery reduce window 1

        montredc!("r8", "r15", "r14", "r13", "r12", "r11", "r10", "r9"),

        // Add row 2

        Q!("    mov             " "rdx, [" y!() "+ 16]"),
        Q!("    xor             " "r9d, r9d"),
        mulpadd!("r11", "r10", Q!("[" x!() "]")),
        mulpadd!("r12", "r11", Q!("[" x!() "+" "8" "]")),
        mulpadd!("r13", "r12", Q!("[" x!() "+" "16" "]")),
        mulpadd!("r14", "r13", Q!("[" x!() "+" "24" "]")),
        mulpadd!("r15", "r14", Q!("[" x!() "+" "32" "]")),
        Q!("    adox            " "r8, r9"),
        Q!("    mulx            " "rbx, rax, [" x!() "+ 40]"),
        Q!("    adc             " "r15, rax"),
        Q!("    adc             " "r8, rbx"),
        Q!("    adc             " "r9, r9"),

        // Montgomery reduce window 2

        montredc!("r9", "r8", "r15", "r14", "r13", "r12", "r11", "r10"),

        // Add row 3

        Q!("    mov             " "rdx, [" y!() "+ 24]"),
        Q!("    xor             " "r10d, r10d"),
        mulpadd!("r12", "r11", Q!("[" x!() "]")),
        mulpadd!("r13", "r12", Q!("[" x!() "+" "8" "]")),
        mulpadd!("r14", "r13", Q!("[" x!() "+" "16" "]")),
        mulpadd!("r15", "r14", Q!("[" x!() "+" "24" "]")),
        mulpadd!("r8", "r15", Q!("[" x!() "+" "32" "]")),
        Q!("    adox            " "r9, r10"),
        Q!("    mulx            " "rbx, rax, [" x!() "+ 40]"),
        Q!("    adc             " "r8, rax"),
        Q!("    adc             " "r9, rbx"),
        Q!("    adc             " "r10, r10"),

        // Montgomery reduce window 3

        montredc!("r10", "r9", "r8", "r15", "r14", "r13", "r12", "r11"),

        // Add row 4

        Q!("    mov             " "rdx, [" y!() "+ 32]"),
        Q!("    xor             " "r11d, r11d"),
        mulpadd!("r13", "r12", Q!("[" x!() "]")),
        mulpadd!("r14", "r13", Q!("[" x!() "+" "8" "]")),
        mulpadd!("r15", "r14", Q!("[" x!() "+" "16" "]")),
        mulpadd!("r8", "r15", Q!("[" x!() "+" "24" "]")),
        mulpadd!("r9", "r8", Q!("[" x!() "+" "32" "]")),
        Q!("    adox            " "r10, r11"),
        Q!("    mulx            " "rbx, rax, [" x!() "+ 40]"),
        Q!("    adc             " "r9, rax"),
        Q!("    adc             " "r10, rbx"),
        Q!("    adc             " "r11, r11"),

        // Montgomery reduce window 4

        montredc!("r11", "r10", "r9", "r8", "r15", "r14", "r13", "r12"),

        // Add row 5

        Q!("    mov             " "rdx, [" y!() "+ 40]"),
        Q!("    xor             " "r12d, r12d"),
        mulpadd!("r14", "r13", Q!("[" x!() "]")),
        mulpadd!("r15", "r14", Q!("[" x!() "+" "8" "]")),
        mulpadd!("r8", "r15", Q!("[" x!() "+" "16" "]")),
        mulpadd!("r9", "r8", Q!("[" x!() "+" "24" "]")),
        mulpadd!("r10", "r9", Q!("[" x!() "+" "32" "]")),
        Q!("    adox            " "r11, r12"),
        Q!("    mulx            " "rbx, rax, [" x!() "+ 40]"),
        Q!("    adc             " "r10, rax"),
        Q!("    adc             " "r11, rbx"),
        Q!("    adc             " "r12, r12"),

        // Montgomery reduce window 5

        montredc!("r12", "r11", "r10", "r9", "r8", "r15", "r14", "r13"),

        // We now have a pre-reduced 7-word form z = [r12; r11;r10;r9;r8;r15;r14]
        // Next, accumulate in different registers z - p_384, or more precisely
        //
        //   [r12; r13;rbp;rdx;rcx;rbx;rax] = z + (2^384 - p_384)

        Q!("    xor             " "edx, edx"),
        Q!("    xor             " "ebp, ebp"),
        Q!("    xor             " "r13d, r13d"),

        Q!("    mov             " "rax, 0xffffffff00000001"),
        Q!("    add             " "rax, r14"),
        Q!("    mov             " "ebx, 0x00000000ffffffff"),
        Q!("    adc             " "rbx, r15"),
        Q!("    mov             " "ecx, 0x0000000000000001"),
        Q!("    adc             " "rcx, r8"),
        Q!("    adc             " "rdx, r9"),
        Q!("    adc             " "rbp, r10"),
        Q!("    adc             " "r13, r11"),
        Q!("    adc             " "r12, 0"),

        // ~ZF <=> r12 >= 1 <=> z + (2^384 - p_384) >= 2^384 <=> z >= p_384, which
        // determines whether to use the further reduced argument or the original z.

        Q!("    cmovnz          " "r14, rax"),
        Q!("    cmovnz          " "r15, rbx"),
        Q!("    cmovnz          " "r8, rcx"),
        Q!("    cmovnz          " "r9, rdx"),
        Q!("    cmovnz          " "r10, rbp"),
        Q!("    cmovnz          " "r11, r13"),

        // Write back the result

        Q!("    mov             " "[" z!() "], r14"),
        Q!("    mov             " "[" z!() "+ 8], r15"),
        Q!("    mov             " "[" z!() "+ 16], r8"),
        Q!("    mov             " "[" z!() "+ 24], r9"),
        Q!("    mov             " "[" z!() "+ 32], r10"),
        Q!("    mov             " "[" z!() "+ 40], r11"),

        // Restore registers and return

        Q!("    pop             " "r15"),
        Q!("    pop             " "r14"),
        Q!("    pop             " "r13"),
        Q!("    pop             " "r12"),
        Q!("    pop             " "rbp"),
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
