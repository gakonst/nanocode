// generated source. do not edit.
#![allow(non_upper_case_globals, unused_macros, unused_imports)]
use crate::low::macros::*;

// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

// ----------------------------------------------------------------------------
// Add, z := x + y
// Inputs x[m], y[n]; outputs function return (carry-out) and z[p]
//
//    extern uint64_t bignum_add(uint64_t p, uint64_t *z, uint64_t m,
//                               const uint64_t *x, uint64_t n, const uint64_t *y);
//
// Does the z := x + y operation, truncating modulo p words in general and
// returning a top carry (0 or 1) in the p'th place, only adding the input
// words below p (as well as m and n respectively) to get the sum and carry.
//
// Standard x86-64 ABI: RDI = p, RSI = z, RDX = m, RCX = x, R8 = n, R9 = y, returns RAX
// Microsoft x64 ABI:   RCX = p, RDX = z, R8 = m, R9 = x, [RSP+40] = n, [RSP+48] = y, returns RAX
// ----------------------------------------------------------------------------

macro_rules! p {
    () => {
        "rdi"
    };
}
macro_rules! z {
    () => {
        "rsi"
    };
}
macro_rules! m {
    () => {
        "rdx"
    };
}
macro_rules! x {
    () => {
        "rcx"
    };
}
macro_rules! n {
    () => {
        "r8"
    };
}
macro_rules! y {
    () => {
        "r9"
    };
}
macro_rules! i {
    () => {
        "r10"
    };
}
macro_rules! a {
    () => {
        "rax"
    };
}

macro_rules! ashort {
    () => {
        "eax"
    };
}

/// Add, z := x + y
///
/// Inputs x[m], y[n]; outputs function return (carry-out) and z[p]
///
/// Does the z := x + y operation, truncating modulo p words in general and
/// returning a top carry (0 or 1) in the p'th place, only adding the input
/// words below p (as well as m and n respectively) to get the sum and carry.
pub(crate) fn bignum_add(z: &mut [u64], x: &[u64], y: &[u64]) -> u64 {
    let ret: u64;
    // SAFETY: inline assembly. see [crate::low::inline_assembly_safety] for safety info.
    unsafe {
        core::arch::asm!(

        Q!("    endbr64         " ),

        // Zero the main index counter for both branches

        Q!("    xor             " i!() ", " i!()),

        // First clamp the two input sizes m := min(p,m) and n := min(p,n) since
        // we'll never need words past the p'th. Can now assume m <= p and n <= p.
        // Then compare the modified m and n and branch accordingly

        Q!("    cmp             " p!() ", " m!()),
        Q!("    cmovc           " m!() ", " p!()),
        Q!("    cmp             " p!() ", " n!()),
        Q!("    cmovc           " n!() ", " p!()),
        Q!("    cmp             " m!() ", " n!()),
        Q!("    jc              " Label!("Lbignum_add_ylonger", 2, After)),

        // The case where x is longer or of the same size (p >= m >= n)

        Q!("    sub             " p!() ", " m!()),
        Q!("    sub             " m!() ", " n!()),
        Q!("    inc             " m!()),
        Q!("    test            " n!() ", " n!()),
        Q!("    jz              " Label!("Lbignum_add_xtest", 3, After)),
        Q!(Label!("Lbignum_add_xmainloop", 4) ":"),
        Q!("    mov             " a!() ", [" x!() "+ 8 * " i!() "]"),
        Q!("    adc             " a!() ", [" y!() "+ 8 * " i!() "]"),
        Q!("    mov             " "[" z!() "+ 8 * " i!() "], " a!()),
        Q!("    inc             " i!()),
        Q!("    dec             " n!()),
        Q!("    jnz             " Label!("Lbignum_add_xmainloop", 4, Before)),
        Q!("    jmp             " Label!("Lbignum_add_xtest", 3, After)),
        Q!(Label!("Lbignum_add_xtoploop", 5) ":"),
        Q!("    mov             " a!() ", [" x!() "+ 8 * " i!() "]"),
        Q!("    adc             " a!() ", 0"),
        Q!("    mov             " "[" z!() "+ 8 * " i!() "], " a!()),
        Q!("    inc             " i!()),
        Q!(Label!("Lbignum_add_xtest", 3) ":"),
        Q!("    dec             " m!()),
        Q!("    jnz             " Label!("Lbignum_add_xtoploop", 5, Before)),
        Q!("    mov             " ashort!() ", 0"),
        Q!("    adc             " a!() ", 0"),
        Q!("    test            " p!() ", " p!()),
        Q!("    jnz             " Label!("Lbignum_add_tails", 6, After)),
        // linear hoisting in -> ret after Lbignum_add_tail
        Q!("    jmp             " Label!("hoist_finish", 7, After)),

        // The case where y is longer (p >= n > m)


        Q!(Label!("Lbignum_add_ylonger", 2) ":"),
        Q!("    sub             " p!() ", " n!()),
        Q!("    sub             " n!() ", " m!()),
        Q!("    test            " m!() ", " m!()),
        Q!("    jz              " Label!("Lbignum_add_ytoploop", 8, After)),
        Q!(Label!("Lbignum_add_ymainloop", 9) ":"),
        Q!("    mov             " a!() ", [" x!() "+ 8 * " i!() "]"),
        Q!("    adc             " a!() ", [" y!() "+ 8 * " i!() "]"),
        Q!("    mov             " "[" z!() "+ 8 * " i!() "], " a!()),
        Q!("    inc             " i!()),
        Q!("    dec             " m!()),
        Q!("    jnz             " Label!("Lbignum_add_ymainloop", 9, Before)),
        Q!(Label!("Lbignum_add_ytoploop", 8) ":"),
        Q!("    mov             " a!() ", [" y!() "+ 8 * " i!() "]"),
        Q!("    adc             " a!() ", 0"),
        Q!("    mov             " "[" z!() "+ 8 * " i!() "], " a!()),
        Q!("    inc             " i!()),
        Q!("    dec             " n!()),
        Q!("    jnz             " Label!("Lbignum_add_ytoploop", 8, Before)),
        Q!("    mov             " ashort!() ", 0"),
        Q!("    adc             " a!() ", 0"),
        Q!("    test            " p!() ", " p!()),
        Q!("    jnz             " Label!("Lbignum_add_tails", 6, After)),
        Q!("    jmp             " Label!("hoist_finish", 7, After)),

        // Adding a non-trivial tail, when p > max(m,n)

        Q!(Label!("Lbignum_add_tails", 6) ":"),
        Q!("    mov             " "[" z!() "+ 8 * " i!() "], " a!()),
        Q!("    xor             " a!() ", " a!()),
        Q!("    jmp             " Label!("Lbignum_add_tail", 12, After)),
        Q!(Label!("Lbignum_add_tailloop", 13) ":"),
        Q!("    mov             " "[" z!() "+ 8 * " i!() "], " a!()),
        Q!(Label!("Lbignum_add_tail", 12) ":"),
        Q!("    inc             " i!()),
        Q!("    dec             " p!()),
        Q!("    jnz             " Label!("Lbignum_add_tailloop", 13, Before)),
        Q!(Label!("hoist_finish", 7) ":"),
        inout("rdi") z.len() => _,
        inout("rsi") z.as_mut_ptr() => _,
        inout("rdx") x.len() => _,
        inout("rcx") x.as_ptr() => _,
        inout("r8") y.len() => _,
        inout("r9") y.as_ptr() => _,
        out("rax") ret,
        // clobbers
        out("r10") _,
            )
    };
    ret
}
