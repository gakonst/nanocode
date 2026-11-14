// generated source. do not edit.
#![allow(non_upper_case_globals, unused_macros, unused_imports)]
use crate::low::macros::*;

// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

// ----------------------------------------------------------------------------
// Subtract modulo m, z := (x - y) mod m, assuming x and y reduced
// Inputs x[k], y[k], m[k]; output z[k]
//
//    extern void bignum_modsub(uint64_t k, uint64_t *z, const uint64_t *x,
//                              const uint64_t *y, const uint64_t *m);
//
// Standard ARM ABI: X0 = k, X1 = z, X2 = x, X3 = y, X4 = m
// ----------------------------------------------------------------------------

macro_rules! k {
    () => {
        "x0"
    };
}
macro_rules! z {
    () => {
        "x1"
    };
}
macro_rules! x {
    () => {
        "x2"
    };
}
macro_rules! y {
    () => {
        "x3"
    };
}
macro_rules! m {
    () => {
        "x4"
    };
}
macro_rules! i {
    () => {
        "x5"
    };
}
macro_rules! j {
    () => {
        "x6"
    };
}
macro_rules! a {
    () => {
        "x7"
    };
}
macro_rules! b {
    () => {
        "x8"
    };
}
macro_rules! c {
    () => {
        "x9"
    };
}

/// Subtract modulo m, z := (x - y) mod m, assuming x and y reduced
///
/// Inputs x[k], y[k], m[k]; output z[k]
pub(crate) fn bignum_modsub(z: &mut [u64], x: &[u64], y: &[u64], m: &[u64]) {
    debug_assert!(z.len() == x.len());
    debug_assert!(z.len() == y.len());
    debug_assert!(z.len() == m.len());
    // SAFETY: inline assembly. see [crate::low::inline_assembly_safety] for safety info.
    unsafe {
        core::arch::asm!(

        Q!("    adds            " j!() ", " k!() ", xzr"),
        Q!("    beq             " Label!("Lbignum_modsub_end", 2, After)),
        Q!("    subs            " i!() ", xzr, xzr"),

        // Subtract z := x - y and record a mask for the carry x - y < 0

        Q!(Label!("Lbignum_modsub_subloop", 3) ":"),
        Q!("    ldr             " a!() ", [" x!() ", " i!() "]"),
        Q!("    ldr             " b!() ", [" y!() ", " i!() "]"),
        Q!("    sbcs            " a!() ", " a!() ", " b!()),
        Q!("    str             " a!() ", [" z!() ", " i!() "]"),
        Q!("    add             " i!() ", " i!() ", #8"),
        Q!("    sub             " j!() ", " j!() ", #1"),
        Q!("    cbnz            " j!() ", " Label!("Lbignum_modsub_subloop", 3, Before)),
        Q!("    csetm           " c!() ", cc"),

        // Now do a masked addition z := z + [c] * m

        Q!("    mov             " j!() ", " k!()),
        Q!("    adds            " i!() ", xzr, xzr"),
        Q!(Label!("Lbignum_modsub_addloop", 4) ":"),
        Q!("    ldr             " a!() ", [" z!() ", " i!() "]"),
        Q!("    ldr             " b!() ", [" m!() ", " i!() "]"),
        Q!("    and             " b!() ", " b!() ", " c!()),
        Q!("    adcs            " a!() ", " a!() ", " b!()),
        Q!("    str             " a!() ", [" z!() ", " i!() "]"),
        Q!("    add             " i!() ", " i!() ", #8"),
        Q!("    sub             " j!() ", " j!() ", #1"),
        Q!("    cbnz            " j!() ", " Label!("Lbignum_modsub_addloop", 4, Before)),

        Q!(Label!("Lbignum_modsub_end", 2) ":"),
        inout("x0") z.len() => _,
        inout("x1") z.as_mut_ptr() => _,
        inout("x2") x.as_ptr() => _,
        inout("x3") y.as_ptr() => _,
        inout("x4") m.as_ptr() => _,
        // clobbers
        out("x5") _,
        out("x6") _,
        out("x7") _,
        out("x8") _,
        out("x9") _,
            )
    };
}
