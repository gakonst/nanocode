// generated source. do not edit.
#![allow(non_upper_case_globals, unused_macros, unused_imports)]
use crate::low::macros::*;

// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

// ----------------------------------------------------------------------------
// Convert to Montgomery form z := (2^384 * x) mod p_384
// Input x[6]; output z[6]
//
//    extern void bignum_tomont_p384(uint64_t z[static 6],
//                                   const uint64_t x[static 6]);
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

// Fairly consistently used as a zero register

macro_rules! zero {
    () => {
        "rbp"
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
        "rsi"
    };
}

macro_rules! vshort {
    () => {
        "ecx"
    };
}
macro_rules! wshort {
    () => {
        "esi"
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
        /* Construct [rbp;rcx;rax;-] = (2^384 - p_384) * w */
        /* We know the lowest word will cancel so we can re-use d0 as a temp */
        "xor ebp, ebp;\n"
        "mov rax, 0xffffffff00000001;\n"
        "mulx rax, rcx, rax;\n"
        "mov ecx, 0x00000000ffffffff;\n"
        "mulx rcx, " $d0 ", rcx;\n"
        "adc rax, " $d0 ";\n"
        "adc rcx, rdx;\n"
        "adc ebp, ebp;\n"
        /*  Now subtract that and add 2^384 * w */
        "sub " $d1 ", rax;\n"
        "sbb " $d2 ", rcx;\n"
        "sbb " $d3 ", rbp;\n"
        "sbb " $d4 ", 0;\n"
        "sbb " $d5 ", 0;\n"
        "sbb rdx, 0;\n"
        "add " $d6 ", rdx;\n"
        "adc " $d7 ", 0"
    )}
}

/// Convert to Montgomery form z := (2^384 * x) mod p_384
///
/// Input x[6]; output z[6]
pub(crate) fn bignum_tomont_p384(z: &mut [u64; 6], x: &[u64; 6]) {
    // SAFETY: inline assembly. see [crate::low::inline_assembly_safety] for safety info.
    unsafe {
        core::arch::asm!(

        Q!("    endbr64         " ),

        // We are essentially just doing a Montgomery multiplication of x and the
        // precomputed constant y = 2^768 mod p, so the code is almost the same
        // modulo a few registers and the change from loading y[i] to using constants,
        // plus the easy digits y[4] = 1 and y[5] = 0 being treated specially.
        // Because there is no y pointer to keep, we use one register less.

        Q!("    push            " "rbp"),
        Q!("    push            " "r12"),
        Q!("    push            " "r13"),
        Q!("    push            " "r14"),
        Q!("    push            " "r15"),

        // Do row 0 computation, which is a bit different:
        // set up initial window [r14,r13,r12,r11,r10,r9,r8] = y[0] * x
        // Unlike later, we only need a single carry chain

        Q!("    mov             " "rdx, 0xfffffffe00000001"),
        Q!("    mulx            " "r9, r8, [" x!() "]"),
        Q!("    mulx            " "r10, rcx, [" x!() "+ 8]"),
        Q!("    add             " "r9, rcx"),
        Q!("    mulx            " "r11, rcx, [" x!() "+ 16]"),
        Q!("    adc             " "r10, rcx"),
        Q!("    mulx            " "r12, rcx, [" x!() "+ 24]"),
        Q!("    adc             " "r11, rcx"),
        Q!("    mulx            " "r13, rcx, [" x!() "+ 32]"),
        Q!("    adc             " "r12, rcx"),
        Q!("    mulx            " "r14, rcx, [" x!() "+ 40]"),
        Q!("    adc             " "r13, rcx"),
        Q!("    adc             " "r14, 0"),

        // Montgomery reduce the zeroth window

        Q!("    xor             " "r15, r15"),
        montredc!("r15", "r14", "r13", "r12", "r11", "r10", "r9", "r8"),

        // Add row 1

        Q!("    xor             " zero!() ", " zero!()),
        Q!("    mov             " "rdx, 0x0000000200000000"),
        Q!("    xor             " "r8, r8"),
        mulpadd!("r10", "r9", Q!("[" x!() "]")),
        mulpadd!("r11", "r10", Q!("[" x!() "+" "8" "]")),
        mulpadd!("r12", "r11", Q!("[" x!() "+" "16" "]")),
        mulpadd!("r13", "r12", Q!("[" x!() "+" "24" "]")),
        mulpadd!("r14", "r13", Q!("[" x!() "+" "32" "]")),
        mulpadd!("r15", "r14", Q!("[" x!() "+" "40" "]")),
        Q!("    adcx            " "r15, " zero!()),
        Q!("    adox            " "r8, " zero!()),
        Q!("    adcx            " "r8, " zero!()),

        // Montgomery reduce window 1

        montredc!("r8", "r15", "r14", "r13", "r12", "r11", "r10", "r9"),

        // Add row 2

        Q!("    xor             " zero!() ", " zero!()),
        Q!("    mov             " "rdx, 0xfffffffe00000000"),
        Q!("    xor             " "r9, r9"),
        mulpadd!("r11", "r10", Q!("[" x!() "]")),
        mulpadd!("r12", "r11", Q!("[" x!() "+" "8" "]")),
        mulpadd!("r13", "r12", Q!("[" x!() "+" "16" "]")),
        mulpadd!("r14", "r13", Q!("[" x!() "+" "24" "]")),
        mulpadd!("r15", "r14", Q!("[" x!() "+" "32" "]")),
        mulpadd!("r8", "r15", Q!("[" x!() "+" "40" "]")),
        Q!("    adcx            " "r8, " zero!()),
        Q!("    adox            " "r9, " zero!()),
        Q!("    adcx            " "r9, " zero!()),

        // Montgomery reduce window 2

        montredc!("r9", "r8", "r15", "r14", "r13", "r12", "r11", "r10"),

        // Add row 3

        Q!("    xor             " zero!() ", " zero!()),
        Q!("    mov             " "rdx, 0x0000000200000000"),
        Q!("    xor             " "r10, r10"),
        mulpadd!("r12", "r11", Q!("[" x!() "]")),
        mulpadd!("r13", "r12", Q!("[" x!() "+" "8" "]")),
        mulpadd!("r14", "r13", Q!("[" x!() "+" "16" "]")),
        mulpadd!("r15", "r14", Q!("[" x!() "+" "24" "]")),
        mulpadd!("r8", "r15", Q!("[" x!() "+" "32" "]")),
        mulpadd!("r9", "r8", Q!("[" x!() "+" "40" "]")),
        Q!("    adcx            " "r9, " zero!()),
        Q!("    adox            " "r10, " zero!()),
        Q!("    adcx            " "r10, " zero!()),

        // Montgomery reduce window 3

        montredc!("r10", "r9", "r8", "r15", "r14", "r13", "r12", "r11"),

        // Add row 4. The multiplier y[4] = 1, so we just add x to the window
        // while extending it with one more digit, initially this carry

        Q!("    xor             " "r11, r11"),
        Q!("    add             " "r12, [" x!() "]"),
        Q!("    adc             " "r13, [" x!() "+ 8]"),
        Q!("    adc             " "r14, [" x!() "+ 16]"),
        Q!("    adc             " "r15, [" x!() "+ 24]"),
        Q!("    adc             " "r8, [" x!() "+ 32]"),
        Q!("    adc             " "r9, [" x!() "+ 40]"),
        Q!("    adc             " "r10, 0"),
        Q!("    adc             " "r11, 0"),

        // Montgomery reduce window 4

        montredc!("r11", "r10", "r9", "r8", "r15", "r14", "r13", "r12"),

        // Add row 5, The multiplier y[5] = 0, so this is trivial: all we do is
        // bring down another zero digit into the window.

        Q!("    xor             " "r12, r12"),

        // Montgomery reduce window 5

        montredc!("r12", "r11", "r10", "r9", "r8", "r15", "r14", "r13"),

        // We now have a pre-reduced 7-word form [r12;r11;r10;r9;r8;r15;r14]

        // We know, writing B = 2^{6*64} that the full implicit result is
        // B^2 c <= z + (B - 1) * p < B * p + (B - 1) * p < 2 * B * p,
        // so the top half is certainly < 2 * p. If c = 1 already, we know
        // subtracting p will give the reduced modulus. But now we do a
        // comparison to catch cases where the residue is >= p.
        // First set [0;0;0;w;v;u] = 2^384 - p_384

        Q!("    mov             " u!() ", 0xffffffff00000001"),
        Q!("    mov             " vshort!() ", 0x00000000ffffffff"),
        Q!("    mov             " wshort!() ", 0x0000000000000001"),

        // Let dd = [r11;r10;r9;r8;r15;r14] be the topless 6-word intermediate result.
        // Set CF if the addition dd + (2^384 - p_384) >= 2^384, hence iff dd >= p_384.

        Q!("    mov             " d!() ", r14"),
        Q!("    add             " d!() ", " u!()),
        Q!("    mov             " d!() ", r15"),
        Q!("    adc             " d!() ", " v!()),
        Q!("    mov             " d!() ", r8"),
        Q!("    adc             " d!() ", " w!()),
        Q!("    mov             " d!() ", r9"),
        Q!("    adc             " d!() ", 0"),
        Q!("    mov             " d!() ", r10"),
        Q!("    adc             " d!() ", 0"),
        Q!("    mov             " d!() ", r11"),
        Q!("    adc             " d!() ", 0"),

        // Now just add this new carry into the existing r12. It's easy to see they
        // can't both be 1 by our range assumptions, so this gives us a {0,1} flag

        Q!("    adc             " "r12, 0"),

        // Now convert it into a bitmask

        Q!("    neg             " "r12"),

        // Masked addition of 2^384 - p_384, hence subtraction of p_384

        Q!("    and             " u!() ", r12"),
        Q!("    and             " v!() ", r12"),
        Q!("    and             " w!() ", r12"),

        Q!("    add             " "r14, " u!()),
        Q!("    adc             " "r15, " v!()),
        Q!("    adc             " "r8, " w!()),
        Q!("    adc             " "r9, 0"),
        Q!("    adc             " "r10, 0"),
        Q!("    adc             " "r11, 0"),

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
