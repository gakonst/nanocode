// generated source. do not edit.
#![allow(non_upper_case_globals, unused_macros, unused_imports)]
use crate::low::macros::*;

// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

// ----------------------------------------------------------------------------
// Convert to Montgomery form z := (2^256 * x) mod p_256
// Input x[4]; output z[4]
//
//    extern void bignum_tomont_p256(uint64_t z[static 4],
//                                   const uint64_t x[static 4]);
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

macro_rules! dshort {
    () => {
        "eax"
    };
}
macro_rules! ushort {
    () => {
        "edx"
    };
}

// Add rdx * m into a register-pair (high,low)
// maintaining consistent double-carrying with adcx and adox,
// using rax and rbx as temporaries

macro_rules! mulpadd {
    ($high:expr, $low:expr, $m:expr) => { Q!(
        "mulx rcx, rax, " $m ";\n"
        "adcx " $low ", rax;\n"
        "adox " $high ", rcx"
    )}
}

/// Convert to Montgomery form z := (2^256 * x) mod p_256
///
/// Input x[4]; output z[4]
pub(crate) fn bignum_tomont_p256(z: &mut [u64; 4], x: &[u64; 4]) {
    // SAFETY: inline assembly. see [crate::low::inline_assembly_safety] for safety info.
    unsafe {
        core::arch::asm!(

        Q!("    endbr64         " ),

        // We are essentially just doing a Montgomery multiplication of x and the
        // precomputed constant y = 2^512 mod p, so the code is almost the same
        // modulo a few registers and the change from loading y[i] to using constants.
        // Because there is no y pointer to keep, we use one register less.

        Q!("    push            " "r12"),
        Q!("    push            " "r13"),
        Q!("    push            " "r14"),
        Q!("    push            " "r15"),

        // Do row 0 computation, which is a bit different:
        // set up initial window [r12,r11,r10,r9,r8] = y[0] * x
        // Unlike later, we only need a single carry chain

        Q!("    xor             " "r13, r13"),
        Q!("    mov             " "edx, 0x0000000000000003"),
        Q!("    mulx            " "r9, r8, [" x!() "]"),
        Q!("    mulx            " "r10, rcx, [" x!() "+ 8]"),
        Q!("    adcx            " "r9, rcx"),
        Q!("    mulx            " "r11, rcx, [" x!() "+ 16]"),
        Q!("    adcx            " "r10, rcx"),
        Q!("    mulx            " "r12, rcx, [" x!() "+ 24]"),
        Q!("    adcx            " "r11, rcx"),
        Q!("    adcx            " "r12, r13"),

        // Add row 1

        Q!("    mov             " "rdx, 0xfffffffbffffffff"),
        Q!("    xor             " "r14, r14"),
        mulpadd!("r10", "r9", Q!("[" x!() "]")),
        mulpadd!("r11", "r10", Q!("[" x!() "+" "8" "]")),
        mulpadd!("r12", "r11", Q!("[" x!() "+" "16" "]")),
        mulpadd!("r13", "r12", Q!("[" x!() "+" "24" "]")),
        Q!("    adc             " "r13, r14"),

        // Montgomery reduce windows 0 and 1 together

        Q!("    xor             " "r15, r15"),
        Q!("    mov             " "rdx, 0x0000000100000000"),
        mulpadd!("r10", "r9", "r8"),
        mulpadd!("r11", "r10", "r9"),
        Q!("    mov             " "rdx, 0xffffffff00000001"),
        mulpadd!("r12", "r11", "r8"),
        mulpadd!("r13", "r12", "r9"),
        Q!("    adcx            " "r13, r15"),
        Q!("    adox            " "r14, r15"),
        Q!("    adcx            " "r14, r15"),

        // Add row 2

        Q!("    mov             " "rdx, 0xfffffffffffffffe"),
        Q!("    xor             " "r8, r8"),
        mulpadd!("r11", "r10", Q!("[" x!() "]")),
        mulpadd!("r12", "r11", Q!("[" x!() "+" "8" "]")),
        mulpadd!("r13", "r12", Q!("[" x!() "+" "16" "]")),
        mulpadd!("r14", "r13", Q!("[" x!() "+" "24" "]")),
        Q!("    adcx            " "r14, r8"),
        Q!("    adox            " "r15, r8"),
        Q!("    adcx            " "r15, r8"),

        // Add row 3

        Q!("    mov             " "rdx, 0x00000004fffffffd"),
        Q!("    xor             " "r9, r9"),
        mulpadd!("r12", "r11", Q!("[" x!() "]")),
        mulpadd!("r13", "r12", Q!("[" x!() "+" "8" "]")),
        mulpadd!("r14", "r13", Q!("[" x!() "+" "16" "]")),
        mulpadd!("r15", "r14", Q!("[" x!() "+" "24" "]")),
        Q!("    adcx            " "r15, r9"),
        Q!("    adox            " "r8, r9"),
        Q!("    adcx            " "r8, r9"),

        // Montgomery reduce windows 2 and 3 together

        Q!("    xor             " "r9, r9"),
        Q!("    mov             " "rdx, 0x0000000100000000"),
        mulpadd!("r12", "r11", "r10"),
        mulpadd!("r13", "r12", "r11"),
        Q!("    mov             " "rdx, 0xffffffff00000001"),
        mulpadd!("r14", "r13", "r10"),
        mulpadd!("r15", "r14", "r11"),
        Q!("    adcx            " "r15, r9"),
        Q!("    adox            " "r8, r9"),
        Q!("    adcx            " "r8, r9"),

        // We now have a pre-reduced 5-word form [r8; r15;r14;r13;r12]
        // Load non-trivial digits of p_256 = [v; 0; u; -1]

        Q!("    mov             " ushort!() ", 0x00000000ffffffff"),
        Q!("    mov             " v!() ", 0xffffffff00000001"),

        // Now do the subtraction (0,p_256-1) - (r8,r15,r14,r13,r12) to get the carry

        Q!("    mov             " d!() ", -2"),
        Q!("    sub             " d!() ", r12"),
        Q!("    mov             " d!() ", " u!()),
        Q!("    sbb             " d!() ", r13"),
        Q!("    mov             " dshort!() ", 0"),
        Q!("    sbb             " d!() ", r14"),
        Q!("    mov             " d!() ", " v!()),
        Q!("    sbb             " d!() ", r15"),

        // This last last comparison in the chain will actually even set the mask
        // for us, so we don't need to separately create it from the carry.
        // This means p_256 - 1 < (c,d1,d0,d5,d4), i.e. we are so far >= p_256

        Q!("    mov             " dshort!() ", 0"),
        Q!("    sbb             " d!() ", r8"),
        Q!("    and             " u!() ", " d!()),
        Q!("    and             " v!() ", " d!()),

        // Do a masked subtraction of p_256 and write back

        Q!("    sub             " "r12, " d!()),
        Q!("    sbb             " "r13, " u!()),
        Q!("    sbb             " "r14, 0"),
        Q!("    sbb             " "r15, " v!()),

        Q!("    mov             " "[" z!() "], r12"),
        Q!("    mov             " "[" z!() "+ 8], r13"),
        Q!("    mov             " "[" z!() "+ 16], r14"),
        Q!("    mov             " "[" z!() "+ 24], r15"),

        // Restore registers and return

        Q!("    pop             " "r15"),
        Q!("    pop             " "r14"),
        Q!("    pop             " "r13"),
        Q!("    pop             " "r12"),

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
