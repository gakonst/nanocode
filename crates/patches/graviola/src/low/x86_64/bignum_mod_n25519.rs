// generated source. do not edit.
#![allow(non_upper_case_globals, unused_macros, unused_imports)]
use crate::low::macros::*;

// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

// ----------------------------------------------------------------------------
// Reduce modulo basepoint order, z := x mod n_25519
// Input x[k]; output z[4]
//
//    extern void bignum_mod_n25519(uint64_t z[static 4], uint64_t k,
//                                  const uint64_t *x);
//
// Reduction is modulo the order of the curve25519/edwards25519 basepoint,
// which is n_25519 = 2^252 + 27742317777372353535851937790883648493
//
// Standard x86-64 ABI: RDI = z, RSI = k, RDX = x
// Microsoft x64 ABI:   RCX = z, RDX = k, R8 = x
// ----------------------------------------------------------------------------

macro_rules! z {
    () => {
        "rdi"
    };
}
macro_rules! k {
    () => {
        "rsi"
    };
}
macro_rules! x {
    () => {
        "rcx"
    };
}

macro_rules! m0 {
    () => {
        "r8"
    };
}
macro_rules! m1 {
    () => {
        "r9"
    };
}
macro_rules! m2 {
    () => {
        "r10"
    };
}
macro_rules! m3 {
    () => {
        "r11"
    };
}
macro_rules! d {
    () => {
        "r12"
    };
}

macro_rules! q {
    () => {
        "rbx"
    };
}

/// Reduce modulo basepoint order, z := x mod n_25519
///
/// Input x[k]; output z[4]
///
/// Reduction is modulo the order of the curve25519/edwards25519 basepoint,
/// which is n_25519 = 2^252 + 27742317777372353535851937790883648493
pub(crate) fn bignum_mod_n25519(z: &mut [u64; 4], x: &[u64]) {
    // SAFETY: inline assembly. see [crate::low::inline_assembly_safety] for safety info.
    unsafe {
        core::arch::asm!(

        Q!("    endbr64         " ),

        // Save extra registers

        Q!("    push            " "rbx"),
        Q!("    push            " "rbp"),
        Q!("    push            " "r12"),

        // If the input is already <= 3 words long, go to a trivial "copy" path

        Q!("    cmp             " k!() ", 4"),
        Q!("    jc              " Label!("Lbignum_mod_n25519_shortinput", 2, After)),

        // Otherwise load the top 4 digits (top-down) and reduce k by 4
        // This [m3;m2;m1;m0] is the initial x where we begin reduction.

        Q!("    sub             " k!() ", 4"),
        Q!("    mov             " m3!() ", [rdx + 8 * " k!() "+ 24]"),
        Q!("    mov             " m2!() ", [rdx + 8 * " k!() "+ 16]"),
        Q!("    mov             " m1!() ", [rdx + 8 * " k!() "+ 8]"),
        Q!("    mov             " m0!() ", [rdx + 8 * " k!() "]"),

        // Move x into another register to leave rdx free for multiplies

        Q!("    mov             " x!() ", rdx"),

        // Get the quotient estimate q = floor(x/2^252).
        // Also delete it from m3, in effect doing x' = x - q * 2^252

        Q!("    mov             " q!() ", " m3!()),
        Q!("    shr             " q!() ", 60"),

        Q!("    shl             " m3!() ", 4"),
        Q!("    shr             " m3!() ", 4"),

        // Let [rdx;d;rbp] = q * (n_25519 - 2^252)

        Q!("    mov             " "rax, 0x5812631a5cf5d3ed"),
        Q!("    mul             " q!()),
        Q!("    mov             " "rbp, rax"),
        Q!("    mov             " d!() ", rdx"),

        Q!("    mov             " "rax, 0x14def9dea2f79cd6"),
        Q!("    mul             " q!()),
        Q!("    add             " d!() ", rax"),
        Q!("    adc             " "rdx, 0"),

        // Subtract to get x' - q * (n_25519 - 2^252) = x - q * n_25519

        Q!("    sub             " m0!() ", rbp"),
        Q!("    sbb             " m1!() ", " d!()),
        Q!("    sbb             " m2!() ", rdx"),
        Q!("    sbb             " m3!() ", 0"),

        // Get a bitmask for the borrow and create a masked version of
        // non-trivial digits of [rbx;0;rdx;rax] = n_25519, then add it.
        // The masked n3 digit exploits the fact that bit 60 of n0 is set.

        Q!("    sbb             " "rbx, rbx"),

        Q!("    mov             " "rax, 0x5812631a5cf5d3ed"),
        Q!("    and             " "rax, rbx"),
        Q!("    mov             " "rdx, 0x14def9dea2f79cd6"),
        Q!("    and             " "rdx, rbx"),
        Q!("    mov             " "rbx, 0x1000000000000000"),
        Q!("    and             " "rbx, rax"),

        Q!("    add             " m0!() ", rax"),
        Q!("    adc             " m1!() ", rdx"),
        Q!("    adc             " m2!() ", 0"),
        Q!("    adc             " m3!() ", rbx"),

        // Now do (k-4) iterations of 5->4 word modular reduction. Each one
        // is similar to the sequence above except for the more refined quotient
        // estimation process.

        Q!("    test            " k!() ", " k!()),
        Q!("    jz              " Label!("Lbignum_mod_n25519_writeback", 3, After)),

        Q!(Label!("Lbignum_mod_n25519_loop", 4) ":"),

        // Assume that the new 5-digit x is 2^64 * previous_x + next_digit.
        // Get the quotient estimate q = max (floor(x/2^252)) (2^64 - 1)
        // and first compute x' = x - 2^252 * q.

        Q!("    mov             " q!() ", " m3!()),
        Q!("    shld            " q!() ", " m2!() ", 4"),
        Q!("    shr             " m3!() ", 60"),
        Q!("    sub             " q!() ", " m3!()),
        Q!("    shl             " m2!() ", 4"),
        Q!("    shrd            " m2!() ", " m3!() ", 4"),

        // Let [rdx;m3;rbp] = q * (n_25519 - 2^252)

        Q!("    mov             " "rax, 0x5812631a5cf5d3ed"),
        Q!("    mul             " q!()),
        Q!("    mov             " "rbp, rax"),
        Q!("    mov             " m3!() ", rdx"),

        Q!("    mov             " "rax, 0x14def9dea2f79cd6"),
        Q!("    mul             " q!()),
        Q!("    add             " m3!() ", rax"),
        Q!("    adc             " "rdx, 0"),

        // Load the next digit

        Q!("    mov             " d!() ", [" x!() "+ 8 * " k!() "-8]"),

        // Subtract to get x' - q * (n_25519 - 2^252) = x - q * n_25519

        Q!("    sub             " d!() ", rbp"),
        Q!("    sbb             " m0!() ", " m3!()),
        Q!("    sbb             " m1!() ", rdx"),
        Q!("    sbb             " m2!() ", 0"),

        // Get a bitmask for the borrow and create a masked version of
        // non-trivial digits of [rbx;0;rdx;rax] = n_25519, then add it.
        // The masked n3 digit exploits the fact that bit 60 of n0 is set.

        Q!("    sbb             " "rbx, rbx"),

        Q!("    mov             " "rax, 0x5812631a5cf5d3ed"),
        Q!("    and             " "rax, rbx"),
        Q!("    mov             " "rdx, 0x14def9dea2f79cd6"),
        Q!("    and             " "rdx, rbx"),
        Q!("    mov             " "rbx, 0x1000000000000000"),
        Q!("    and             " "rbx, rax"),

        Q!("    add             " d!() ", rax"),
        Q!("    adc             " m0!() ", rdx"),
        Q!("    adc             " m1!() ", 0"),
        Q!("    adc             " m2!() ", rbx"),

        // Now shuffle registers up and loop

        Q!("    mov             " m3!() ", " m2!()),
        Q!("    mov             " m2!() ", " m1!()),
        Q!("    mov             " m1!() ", " m0!()),
        Q!("    mov             " m0!() ", " d!()),

        Q!("    dec             " k!()),
        Q!("    jnz             " Label!("Lbignum_mod_n25519_loop", 4, Before)),
        Q!("    jmp             " Label!("Lbignum_mod_n25519_writeback", 3, After)),

        Q!(Label!("Lbignum_mod_n25519_shortinput", 2) ":"),

        Q!("    xor             " m0!() ", " m0!()),
        Q!("    xor             " m1!() ", " m1!()),
        Q!("    xor             " m2!() ", " m2!()),
        Q!("    xor             " m3!() ", " m3!()),

        Q!("    test            " k!() ", " k!()),
        Q!("    jz              " Label!("Lbignum_mod_n25519_writeback", 3, After)),
        Q!("    mov             " m0!() ", [rdx]"),
        Q!("    dec             " k!()),
        Q!("    jz              " Label!("Lbignum_mod_n25519_writeback", 3, After)),
        Q!("    mov             " m1!() ", [rdx + 8]"),
        Q!("    dec             " k!()),
        Q!("    jz              " Label!("Lbignum_mod_n25519_writeback", 3, After)),
        Q!("    mov             " m2!() ", [rdx + 16]"),

        // Write back

        Q!(Label!("Lbignum_mod_n25519_writeback", 3) ":"),

        Q!("    mov             " "[" z!() "], " m0!()),
        Q!("    mov             " "[" z!() "+ 8], " m1!()),
        Q!("    mov             " "[" z!() "+ 16], " m2!()),
        Q!("    mov             " "[" z!() "+ 24], " m3!()),

        // Restore registers and return

        Q!("    pop             " "r12"),
        Q!("    pop             " "rbp"),
        Q!("    pop             " "rbx"),
        inout("rdi") z.as_mut_ptr() => _,
        inout("rsi") x.len() => _,
        inout("rdx") x.as_ptr() => _,
        // clobbers
        out("r10") _,
        out("r11") _,
        out("r12") _,
        out("r8") _,
        out("r9") _,
        out("rax") _,
        out("rcx") _,
            )
    };
}
