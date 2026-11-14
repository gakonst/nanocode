// generated source. do not edit.
#![allow(non_upper_case_globals, unused_macros, unused_imports)]
use crate::low::macros::*;

// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

// ----------------------------------------------------------------------------
// Add modulo m, z := (x + y) mod m, assuming x and y reduced
// Inputs x[k], y[k], m[k]; output z[k]
//
//    extern void bignum_modadd(uint64_t k, uint64_t *z, const uint64_t *x,
//                              const uint64_t *y, const uint64_t *m);
//
// Standard x86-64 ABI: RDI = k, RSI = z, RDX = x, RCX = y, R8 = m
// Microsoft x64 ABI:   RCX = k, RDX = z, R8 = x, R9 = y, [RSP+40] = m
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
macro_rules! x {
    () => {
        "rdx"
    };
}
macro_rules! y {
    () => {
        "rcx"
    };
}
macro_rules! m {
    () => {
        "r8"
    };
}
macro_rules! i {
    () => {
        "r9"
    };
}
macro_rules! j {
    () => {
        "r10"
    };
}
macro_rules! a {
    () => {
        "rax"
    };
}
macro_rules! c {
    () => {
        "r11"
    };
}

/// Add modulo m, z := (x + y) mod m, assuming x and y reduced
///
/// Inputs x[k], y[k], m[k]; output z[k]
pub(crate) fn bignum_modadd(z: &mut [u64], x: &[u64], y: &[u64], m: &[u64]) {
    debug_assert!(z.len() == x.len());
    debug_assert!(z.len() == y.len());
    debug_assert!(z.len() == m.len());
    // SAFETY: inline assembly. see [crate::low::inline_assembly_safety] for safety info.
    unsafe {
        core::arch::asm!(

        Q!("    endbr64         " ),

        // If k = 0 do nothing

        Q!("    test            " k!() ", " k!()),
        Q!("    jz              " Label!("Lbignum_modadd_end", 2, After)),

        // First just add (c::z) := x + y

        Q!("    xor             " c!() ", " c!()),
        Q!("    mov             " j!() ", " k!()),
        Q!("    xor             " i!() ", " i!()),
        Q!(Label!("Lbignum_modadd_addloop", 3) ":"),
        Q!("    mov             " a!() ", [" x!() "+ 8 * " i!() "]"),
        Q!("    adc             " a!() ", [" y!() "+ 8 * " i!() "]"),
        Q!("    mov             " "[" z!() "+ 8 * " i!() "], " a!()),
        Q!("    inc             " i!()),
        Q!("    dec             " j!()),
        Q!("    jnz             " Label!("Lbignum_modadd_addloop", 3, Before)),
        Q!("    adc             " c!() ", 0"),

        // Now do a comparison subtraction (c::z) - m, recording mask for (c::z) >= m

        Q!("    mov             " j!() ", " k!()),
        Q!("    xor             " i!() ", " i!()),
        Q!(Label!("Lbignum_modadd_cmploop", 4) ":"),
        Q!("    mov             " a!() ", [" z!() "+ 8 * " i!() "]"),
        Q!("    sbb             " a!() ", [" m!() "+ 8 * " i!() "]"),
        Q!("    inc             " i!()),
        Q!("    dec             " j!()),
        Q!("    jnz             " Label!("Lbignum_modadd_cmploop", 4, Before)),
        Q!("    sbb             " c!() ", 0"),
        Q!("    not             " c!()),

        // Now do a masked subtraction z := z - [c] * m

        Q!("    xor             " i!() ", " i!()),
        Q!(Label!("Lbignum_modadd_subloop", 5) ":"),
        Q!("    mov             " a!() ", [" m!() "+ 8 * " i!() "]"),
        Q!("    and             " a!() ", " c!()),
        Q!("    neg             " j!()),
        Q!("    sbb             " "[" z!() "+ 8 * " i!() "], " a!()),
        Q!("    sbb             " j!() ", " j!()),
        Q!("    inc             " i!()),
        Q!("    cmp             " i!() ", " k!()),
        Q!("    jc              " Label!("Lbignum_modadd_subloop", 5, Before)),

        Q!(Label!("Lbignum_modadd_end", 2) ":"),
        inout("rdi") m.len() => _,
        inout("rsi") z.as_mut_ptr() => _,
        inout("rdx") x.as_ptr() => _,
        inout("rcx") y.as_ptr() => _,
        inout("r8") m.as_ptr() => _,
        // clobbers
        out("r10") _,
        out("r11") _,
        out("r9") _,
        out("rax") _,
            )
    };
}
