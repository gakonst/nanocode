// generated source. do not edit.
#![allow(non_upper_case_globals, unused_macros, unused_imports)]
use crate::low::macros::*;

// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

// ----------------------------------------------------------------------------
// Montgomery square, z := (x^2 / 2^384) mod p_384
// Input x[6]; output z[6]
//
//    extern void bignum_montsqr_p384(uint64_t z[static 6],
//                                    const uint64_t x[static 6]);
//
// Does z := (x^2 / 2^384) mod p_384, assuming x^2 <= 2^384 * p_384, which is
// guaranteed in particular if x < p_384 initially (the "intended" case).
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
        "r10"
    };
}
macro_rules! w {
    () => {
        "r11"
    };
}

// A zero register, very often

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

// Core one-step "short" Montgomery reduction macro. Takes input in
// [d5;d4;d3;d2;d1;d0] and returns result in [d6;d5;d4;d3;d2;d1],
// adding to the existing [d5;d4;d3;d2;d1] and re-using d0 as a
// temporary internally, as well as rax, rbx and rdx.
// It is OK for d6 and d0 to be the same register (they often are)
//
// We want to add (2^384 - 2^128 - 2^96 + 2^32 - 1) * w
// where w = [d0 + (d0<<32)] mod 2^64
//
//       montreds(d6,d5,d4,d3,d2,d1,d0)

macro_rules! montreds {
    ($d6:expr, $d5:expr, $d4:expr, $d3:expr, $d2:expr, $d1:expr, $d0:expr) => { Q!(
        /* Our correction multiplier is w = [d0 + (d0<<32)] mod 2^64 */
        "mov rdx, " $d0 ";\n"
        "shl rdx, 32;\n"
        "add rdx, " $d0 ";\n"
        /* Construct [rbx;d0;rax;-] = (2^384 - p_384) * w            */
        /* We know the lowest word will cancel so we can re-use d0   */
        /* and rbx as temps.                                         */
        "mov rax, 0xffffffff00000001;\n"
        "mulx rax, " $d0 ", rax;\n"
        "mov ebx, 0x00000000ffffffff;\n"
        "mulx " $d0 ", rbx, rbx;\n"
        "add rax, rbx;\n"
        "adc " $d0 ", rdx;\n"
        "mov ebx, 0;\n"
        "adc rbx, rbx;\n"
        /* Now subtract that and add 2^384 * w                       */
        "sub " $d1 ", rax;\n"
        "sbb " $d2 ", " $d0 ";\n"
        "sbb " $d3 ", rbx;\n"
        "sbb " $d4 ", 0;\n"
        "sbb " $d5 ", 0;\n"
        "mov " $d6 ", rdx;\n"
        "sbb " $d6 ", 0"
    )}
}

/// Montgomery square, z := (x^2 / 2^384) mod p_384
///
/// Input x[6]; output z[6]
///
/// Does z := (x^2 / 2^384) mod p_384, assuming x^2 <= 2^384 * p_384, which is
/// guaranteed in particular if x < p_384 initially (the "intended" case).
pub(crate) fn bignum_montsqr_p384(z: &mut [u64; 6], x: &[u64; 6]) {
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

        // Set up an initial window [rcx;r15;...r9] = [34;05;03;01]
        // Note that we are using rcx as the first step past the rotating window

        Q!("    mov             " "rdx, [" x!() "]"),
        Q!("    mulx            " "r10, r9, [" x!() "+ 8]"),
        Q!("    mulx            " "r12, r11, [" x!() "+ 24]"),
        Q!("    mulx            " "r14, r13, [" x!() "+ 40]"),
        Q!("    mov             " "rdx, [" x!() "+ 24]"),
        Q!("    mulx            " "rcx, r15, [" x!() "+ 32]"),

        // Clear our zero register, and also initialize the flags for the carry chain

        Q!("    xor             " zeroe!() ", " zeroe!()),

        // Chain in the addition of 02 + 12 + 13 + 14 + 15 to that window
        // (no carry-out possible)

        Q!("    mov             " "rdx, [" x!() "+ 16]"),
        mulpadd!("r11", "r10", Q!("[" x!() "]")),
        mulpadd!("r12", "r11", Q!("[" x!() "+" "8" "]")),
        Q!("    mov             " "rdx, [" x!() "+ 8]"),
        mulpadd!("r13", "r12", Q!("[" x!() "+" "24" "]")),
        mulpadd!("r14", "r13", Q!("[" x!() "+" "32" "]")),
        mulpadd!("r15", "r14", Q!("[" x!() "+" "40" "]")),
        Q!("    adcx            " "r15, " zero!()),
        Q!("    adox            " "rcx, " zero!()),
        Q!("    adc             " "rcx, " zero!()),

        // Again zero out the flags. Actually they are already cleared but it may
        // help decouple these in the OOO engine not to wait for the chain above

        Q!("    xor             " zeroe!() ", " zeroe!()),

        // Now chain in the 04 + 23 + 24 + 25 + 35 + 45 terms
        // We are running out of registers in our rotating window, so we start
        // using rbx (and hence need care with using mulpadd after this). Thus
        // our result so far is in [rbp;rbx;rcx;r15;...r9]

        Q!("    mov             " "rdx, [" x!() "+ 32]"),
        mulpadd!("r13", "r12", Q!("[" x!() "]")),
        Q!("    mov             " "rdx, [" x!() "+ 16]"),
        mulpadd!("r14", "r13", Q!("[" x!() "+" "24" "]")),
        mulpadd!("r15", "r14", Q!("[" x!() "+" "32" "]")),
        Q!("    mulx            " "rdx, rax, [" x!() "+ 40]"),
        Q!("    adcx            " "r15, rax"),
        Q!("    adox            " "rcx, rdx"),

        // First set up the last couple of spots in our window, [rbp;rbx] = 45
        // then add the last other term 35

        Q!("    mov             " "rdx, [" x!() "+ 40]"),
        Q!("    mulx            " "rbp, rbx, [" x!() "+ 32]"),
        Q!("    mulx            " "rdx, rax, [" x!() "+ 24]"),
        Q!("    adcx            " "rcx, rax"),
        Q!("    adox            " "rbx, rdx"),
        Q!("    mov             " "eax, 0"),
        Q!("    adcx            " "rbx, rax"),
        Q!("    adox            " "rbp, rax"),
        Q!("    adc             " "rbp, rax"),

        // Just for a clear fresh start for the flags; we don't use the zero

        Q!("    xor             " "rax, rax"),

        // Double and add to the 00 + 11 + 22 + 33 + 44 + 55 terms
        // For one glorious moment the entire squaring result is all in the
        // register file as [rsi;rbp;rbx;rcx;r15;...;r8]
        // (since we've now finished with x we can re-use rsi)

        Q!("    mov             " "rdx, [" x!() "]"),
        Q!("    mulx            " "rax, r8, [" x!() "]"),
        Q!("    adcx            " "r9, r9"),
        Q!("    adox            " "r9, rax"),
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
        Q!("    mulx            " "rdx, rax, rdx"),
        Q!("    adcx            " "r14, r14"),
        Q!("    adox            " "r14, rax"),
        Q!("    adcx            " "r15, r15"),
        Q!("    adox            " "r15, rdx"),
        Q!("    mov             " "rdx, [" x!() "+ 32]"),
        Q!("    mulx            " "rdx, rax, rdx"),
        Q!("    adcx            " "rcx, rcx"),
        Q!("    adox            " "rcx, rax"),
        Q!("    adcx            " "rbx, rbx"),
        Q!("    adox            " "rbx, rdx"),
        Q!("    mov             " "rdx, [" x!() "+ 40]"),
        Q!("    mulx            " "rsi, rax, rdx"),
        Q!("    adcx            " "rbp, rbp"),
        Q!("    adox            " "rbp, rax"),
        Q!("    mov             " "eax, 0"),
        Q!("    adcx            " "rsi, rax"),
        Q!("    adox            " "rsi, rax"),

        // We need just *one* more register as a temp for the Montgomery steps.
        // Since we are writing to the z buffer anyway, make use of that to stash rbx.

        Q!("    mov             " "[" z!() "], rbx"),

        // Montgomery reduce the r13,...,r8 window 6 times

        montreds!("r8", "r13", "r12", "r11", "r10", "r9", "r8"),
        montreds!("r9", "r8", "r13", "r12", "r11", "r10", "r9"),
        montreds!("r10", "r9", "r8", "r13", "r12", "r11", "r10"),
        montreds!("r11", "r10", "r9", "r8", "r13", "r12", "r11"),
        montreds!("r12", "r11", "r10", "r9", "r8", "r13", "r12"),
        montreds!("r13", "r12", "r11", "r10", "r9", "r8", "r13"),

        // Now we can safely restore rbx before accumulating

        Q!("    mov             " "rbx, [" z!() "]"),

        Q!("    add             " "r14, r8"),
        Q!("    adc             " "r15, r9"),
        Q!("    adc             " "rcx, r10"),
        Q!("    adc             " "rbx, r11"),
        Q!("    adc             " "rbp, r12"),
        Q!("    adc             " "rsi, r13"),
        Q!("    mov             " "r8d, 0"),
        Q!("    adc             " "r8, r8"),

        // We now have a pre-reduced 7-word form z = [r8; rsi;rbp;rbx;rcx;r15;r14]
        // Next, accumulate in different registers z - p_384, or more precisely
        //
        //   [r8; r13;r12;r11;r10;r9;rax] = z + (2^384 - p_384)

        Q!("    xor             " "r11, r11"),
        Q!("    xor             " "r12, r12"),
        Q!("    xor             " "r13, r13"),
        Q!("    mov             " "rax, 0xffffffff00000001"),
        Q!("    add             " "rax, r14"),
        Q!("    mov             " "r9d, 0x00000000ffffffff"),
        Q!("    adc             " "r9, r15"),
        Q!("    mov             " "r10d, 0x0000000000000001"),
        Q!("    adc             " "r10, rcx"),
        Q!("    adc             " "r11, rbx"),
        Q!("    adc             " "r12, rbp"),
        Q!("    adc             " "r13, rsi"),
        Q!("    adc             " "r8, 0"),

        // ~ZF <=> r12 >= 1 <=> z + (2^384 - p_384) >= 2^384 <=> z >= p_384, which
        // determines whether to use the further reduced argument or the original z.

        Q!("    cmovnz          " "r14, rax"),
        Q!("    cmovnz          " "r15, r9"),
        Q!("    cmovnz          " "rcx, r10"),
        Q!("    cmovnz          " "rbx, r11"),
        Q!("    cmovnz          " "rbp, r12"),
        Q!("    cmovnz          " "rsi, r13"),

        // Write back the result

        Q!("    mov             " "[" z!() "], r14"),
        Q!("    mov             " "[" z!() "+ 8], r15"),
        Q!("    mov             " "[" z!() "+ 16], rcx"),
        Q!("    mov             " "[" z!() "+ 24], rbx"),
        Q!("    mov             " "[" z!() "+ 32], rbp"),
        Q!("    mov             " "[" z!() "+ 40], rsi"),

        // Restore registers and return

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
