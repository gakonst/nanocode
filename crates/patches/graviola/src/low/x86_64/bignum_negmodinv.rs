// generated source. do not edit.
#![allow(non_upper_case_globals, unused_macros, unused_imports)]
use crate::low::macros::*;

// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

// ----------------------------------------------------------------------------
// Negated modular inverse, z := (-1/x) mod 2^{64k}
// Input x[k]; output z[k]
//
//    extern void bignum_negmodinv(uint64_t k, uint64_t *z, const uint64_t *x);
//
// Assuming x is odd (otherwise nothing makes sense) the result satisfies
//
//       x * z + 1 == 0 (mod 2^{64 * k})
//
// but is not necessarily reduced mod x.
//
// Standard x86-64 ABI: RDI = k, RSI = z, RDX = x
// Microsoft x64 ABI:   RCX = k, RDX = z, R8 = x
// ----------------------------------------------------------------------------

macro_rules! k {
    () => {
        "rdi"
    };
}
macro_rules! z {
    () => {
        "rsi"
    };
}
// Moved from initial location to free rdx
macro_rules! x {
    () => {
        "rcx"
    };
}

macro_rules! a {
    () => {
        "rax"
    };
}
macro_rules! d {
    () => {
        "rdx"
    };
}
macro_rules! i {
    () => {
        "r8"
    };
}
macro_rules! m {
    () => {
        "r9"
    };
}
macro_rules! h {
    () => {
        "r10"
    };
}
macro_rules! w {
    () => {
        "r11"
    };
}
macro_rules! t {
    () => {
        "r12"
    };
}
macro_rules! e {
    () => {
        "rbx"
    };
}

macro_rules! ashort {
    () => {
        "eax"
    };
}
macro_rules! ishort {
    () => {
        "r8d"
    };
}

/// Negated modular inverse, z := (-1/x) mod 2^{64k}
///
/// Input x[k]; output z[k]
///
/// Assuming x is odd (otherwise nothing makes sense) the result satisfies
///
/// x * z + 1 == 0 (mod 2^{64 * k})
///
/// but is not necessarily reduced mod x.
pub(crate) fn bignum_negmodinv(z: &mut [u64], x: &[u64]) {
    debug_assert!(z.len() == x.len());
    // SAFETY: inline assembly. see [crate::low::inline_assembly_safety] for safety info.
    unsafe {
        core::arch::asm!(

        Q!("    endbr64         " ),

        Q!("    push            " "rbx"),
        Q!("    push            " "r12"),

        // If k = 0 do nothing (actually we could have avoiding the pushes and pops)

        Q!("    test            " k!() ", " k!()),
        Q!("    jz              " Label!("Lbignum_negmodinv_end", 2, After)),

        // Move the x pointer into its permanent home (rdx is needed for muls)

        Q!("    mov             " x!() ", rdx"),

        // Compute word-level negated modular inverse w for x[0].

        Q!("    mov             " a!() ", [" x!() "]"),

        Q!("    mov             " d!() ", " a!()),
        Q!("    mov             " w!() ", " a!()),
        Q!("    shl             " d!() ", 2"),
        Q!("    sub             " w!() ", " d!()),
        Q!("    xor             " w!() ", 2"),

        Q!("    mov             " d!() ", " w!()),
        Q!("    imul            " d!() ", " a!()),
        Q!("    mov             " ashort!() ", 2"),
        Q!("    add             " a!() ", " d!()),
        Q!("    add             " d!() ", 1"),

        Q!("    imul            " w!() ", " a!()),

        Q!("    imul            " d!() ", " d!()),
        Q!("    mov             " ashort!() ", 1"),
        Q!("    add             " a!() ", " d!()),
        Q!("    imul            " w!() ", " a!()),

        Q!("    imul            " d!() ", " d!()),
        Q!("    mov             " ashort!() ", 1"),
        Q!("    add             " a!() ", " d!()),
        Q!("    imul            " w!() ", " a!()),

        Q!("    imul            " d!() ", " d!()),
        Q!("    mov             " ashort!() ", 1"),
        Q!("    add             " a!() ", " d!()),
        Q!("    imul            " w!() ", " a!()),

        // Write that as lowest word of the output, then if k = 1 we're finished

        Q!("    mov             " "[" z!() "], " w!()),
        Q!("    cmp             " k!() ", 1"),
        Q!("    jz              " Label!("Lbignum_negmodinv_end", 2, After)),

        // Otherwise compute and write the other digits (1..k-1) of w * x + 1

        Q!("    mov             " a!() ", [" x!() "]"),
        Q!("    xor             " h!() ", " h!()),
        Q!("    mul             " w!()),
        Q!("    add             " a!() ", 1"),
        Q!("    adc             " h!() ", " d!()),
        Q!("    mov             " ishort!() ", 1"),
        Q!(Label!("Lbignum_negmodinv_initloop", 3) ":"),
        Q!("    mov             " a!() ", [" x!() "+ 8 * " i!() "]"),
        Q!("    mul             " w!()),
        Q!("    add             " a!() ", " h!()),
        Q!("    adc             " d!() ", 0"),
        Q!("    mov             " "[" z!() "+ 8 * " i!() "], " a!()),
        Q!("    mov             " h!() ", " d!()),
        Q!("    inc             " i!()),
        Q!("    cmp             " i!() ", " k!()),
        Q!("    jc              " Label!("Lbignum_negmodinv_initloop", 3, Before)),

        // For simpler indexing, z := z + 8 and k := k - 1 per outer iteration
        // Then we can use the same index for x and for z and effective size k.
        //
        // But we also offset k by 1 so the "real" size is k + 1; after doing
        // the special zeroth bit we count with t through k more digits, so
        // getting k + 1 total as required.
        //
        // This lets us avoid some special cases inside the loop at the cost
        // of needing the additional "finale" tail for the final iteration
        // since we do one outer loop iteration too few.

        Q!("    sub             " k!() ", 2"),
        Q!("    jz              " Label!("Lbignum_negmodinv_finale", 4, After)),

        Q!(Label!("Lbignum_negmodinv_outerloop", 5) ":"),
        Q!("    add             " z!() ", 8"),

        Q!("    mov             " h!() ", [" z!() "]"),
        Q!("    mov             " m!() ", " w!()),
        Q!("    imul            " m!() ", " h!()),
        Q!("    mov             " "[" z!() "], " m!()),
        Q!("    mov             " a!() ", [" x!() "]"),
        Q!("    mul             " m!()),
        Q!("    add             " a!() ", " h!()),
        Q!("    adc             " d!() ", 0"),
        Q!("    mov             " h!() ", " d!()),
        Q!("    mov             " ishort!() ", 1"),
        Q!("    mov             " t!() ", " k!()),
        Q!(Label!("Lbignum_negmodinv_innerloop", 6) ":"),
        Q!("    adc             " h!() ", [" z!() "+ 8 * " i!() "]"),
        Q!("    sbb             " e!() ", " e!()),
        Q!("    mov             " a!() ", [" x!() "+ 8 * " i!() "]"),
        Q!("    mul             " m!()),
        Q!("    sub             " d!() ", " e!()),
        Q!("    add             " a!() ", " h!()),
        Q!("    mov             " "[" z!() "+ 8 * " i!() "], " a!()),
        Q!("    mov             " h!() ", " d!()),
        Q!("    inc             " i!()),
        Q!("    dec             " t!()),
        Q!("    jnz             " Label!("Lbignum_negmodinv_innerloop", 6, Before)),

        Q!("    dec             " k!()),
        Q!("    jnz             " Label!("Lbignum_negmodinv_outerloop", 5, Before)),

        Q!(Label!("Lbignum_negmodinv_finale", 4) ":"),
        Q!("    mov             " a!() ", [" z!() "+ 8]"),
        Q!("    imul            " a!() ", " w!()),
        Q!("    mov             " "[" z!() "+ 8], " a!()),

        Q!(Label!("Lbignum_negmodinv_end", 2) ":"),
        Q!("    pop             " "r12"),
        Q!("    pop             " "rbx"),
        inout("rdi") z.len() => _,
        inout("rsi") z.as_mut_ptr() => _,
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
