// generated source. do not edit.
#![allow(non_upper_case_globals, unused_macros, unused_imports)]
use crate::low::macros::*;

// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

// ----------------------------------------------------------------------------
// Montgomery reduce, z := (x' / 2^{64p}) MOD m
// Inputs x[n], m[k], p; output z[k]
//
//    extern void bignum_montredc(uint64_t k, uint64_t *z, uint64_t n,
//                                const uint64_t *x, const uint64_t *m, uint64_t p);
//
// Does a := (x' / 2^{64p}) mod m where x' = x if n <= p + k and in general
// is the lowest (p+k) digits of x, assuming x' <= 2^{64p} * m. That is,
// p-fold Montgomery reduction w.r.t. a k-digit modulus m giving a k-digit
// answer.
//
// Standard x86-64 ABI: RDI = k, RSI = z, RDX = n, RCX = x, R8 = m, R9 = p
// Microsoft x64 ABI:   RCX = k, RDX = z, R8 = n, R9 = x, [RSP+40] = m, [RSP+48] = p
// ----------------------------------------------------------------------------

// We copy n into r10 but it comes in in rdx originally

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
macro_rules! n {
    () => {
        "r10"
    };
}
macro_rules! x {
    () => {
        "rcx"
    };
}
macro_rules! m {
    () => {
        "r8"
    };
}
macro_rules! p {
    () => {
        "r9"
    };
}

// General temp, low part of product and mul input
macro_rules! a {
    () => {
        "rax"
    };
}
// General temp, High part of product
macro_rules! b {
    () => {
        "rdx"
    };
}
// Negated modular inverse
macro_rules! w {
    () => {
        "QWORD PTR [rsp]"
    };
}
// Inner loop counter
macro_rules! j {
    () => {
        "rbx"
    };
}
// Home for i'th digit or Montgomery multiplier
macro_rules! d {
    () => {
        "rbp"
    };
}
macro_rules! h {
    () => {
        "r11"
    };
}
macro_rules! e {
    () => {
        "r12"
    };
}
macro_rules! t {
    () => {
        "r13"
    };
}
macro_rules! i {
    () => {
        "r14"
    };
}
macro_rules! c {
    () => {
        "r15"
    };
}

// Some more intuitive names for temp regs in initial word-level negmodinv.

macro_rules! t1 {
    () => {
        "rbx"
    };
}
macro_rules! t2 {
    () => {
        "r14"
    };
}

macro_rules! ashort {
    () => {
        "eax"
    };
}
macro_rules! cshort {
    () => {
        "r15d"
    };
}
macro_rules! jshort {
    () => {
        "ebx"
    };
}

/// Montgomery reduce, z := (x' / 2^{64p}) MOD m
///
/// Inputs x[n], m[k], p; output z[k]
///
/// Does a := (x' / 2^{64p}) mod m where x' = x if n <= p + k and in general
/// is the lowest (p+k) digits of x, assuming x' <= 2^{64p} * m. That is,
/// p-fold Montgomery reduction w.r.t. a k-digit modulus m giving a k-digit
/// answer.
pub(crate) fn bignum_montredc(z: &mut [u64], x: &[u64], m: &[u64], p: u64) {
    debug_assert!(z.len() == m.len());
    // SAFETY: inline assembly. see [crate::low::inline_assembly_safety] for safety info.
    unsafe {
        core::arch::asm!(

        Q!("    endbr64         " ),

        // Save registers and allocate space on stack for non-register variable w

        Q!("    push            " "rbx"),
        Q!("    push            " "rbp"),
        Q!("    push            " "r12"),
        Q!("    push            " "r13"),
        Q!("    push            " "r14"),
        Q!("    push            " "r15"),
        Q!("    sub             " "rsp, 8"),

        // If k = 0 the whole operation is trivial

        Q!("    test            " k!() ", " k!()),
        Q!("    jz              " Label!("Lbignum_montredc_end", 2, After)),

        // Move n input into its permanent home, since we need rdx for multiplications

        Q!("    mov             " n!() ", rdx"),

        // Compute word-level negated modular inverse w for m == m[0].

        Q!("    mov             " a!() ", [" m!() "]"),

        Q!("    mov             " t2!() ", " a!()),
        Q!("    mov             " t1!() ", " a!()),
        Q!("    shl             " t2!() ", 2"),
        Q!("    sub             " t1!() ", " t2!()),
        Q!("    xor             " t1!() ", 2"),

        Q!("    mov             " t2!() ", " t1!()),
        Q!("    imul            " t2!() ", " a!()),
        Q!("    mov             " ashort!() ", 2"),
        Q!("    add             " a!() ", " t2!()),
        Q!("    add             " t2!() ", 1"),

        Q!("    imul            " t1!() ", " a!()),

        Q!("    imul            " t2!() ", " t2!()),
        Q!("    mov             " ashort!() ", 1"),
        Q!("    add             " a!() ", " t2!()),
        Q!("    imul            " t1!() ", " a!()),

        Q!("    imul            " t2!() ", " t2!()),
        Q!("    mov             " ashort!() ", 1"),
        Q!("    add             " a!() ", " t2!()),
        Q!("    imul            " t1!() ", " a!()),

        Q!("    imul            " t2!() ", " t2!()),
        Q!("    mov             " ashort!() ", 1"),
        Q!("    add             " a!() ", " t2!()),
        Q!("    imul            " t1!() ", " a!()),

        Q!("    mov             " w!() ", " t1!()),

        // Initialize z to the lowest k digits of the input, zero-padding if n < k.

        Q!("    mov             " j!() ", " k!()),
        Q!("    cmp             " n!() ", " k!()),
        Q!("    cmovc           " j!() ", " n!()),
        Q!("    xor             " i!() ", " i!()),
        Q!("    test            " j!() ", " j!()),
        Q!("    jz              " Label!("Lbignum_montredc_padloop", 3, After)),
        Q!(Label!("Lbignum_montredc_copyloop", 4) ":"),
        Q!("    mov             " a!() ", [" x!() "+ 8 * " i!() "]"),
        Q!("    mov             " "[" z!() "+ 8 * " i!() "], " a!()),
        Q!("    inc             " i!()),
        Q!("    cmp             " i!() ", " j!()),
        Q!("    jc              " Label!("Lbignum_montredc_copyloop", 4, Before)),

        Q!("    cmp             " i!() ", " k!()),
        Q!("    jnc             " Label!("Lbignum_montredc_initialized", 5, After)),

        Q!("    xor             " j!() ", " j!()),
        Q!(Label!("Lbignum_montredc_padloop", 3) ":"),
        Q!("    mov             " "[" z!() "+ 8 * " i!() "], " j!()),
        Q!("    inc             " i!()),
        Q!("    cmp             " i!() ", " k!()),
        Q!("    jc              " Label!("Lbignum_montredc_padloop", 3, Before)),

        Q!(Label!("Lbignum_montredc_initialized", 5) ":"),
        Q!("    xor             " c!() ", " c!()),

        // Now if p = 0 we just need the corrective tail, and even that is
        // only needed for the case when the input is exactly the modulus,
        // to maintain the <= 2^64p * n precondition

        Q!("    test            " p!() ", " p!()),
        Q!("    jz              " Label!("Lbignum_montredc_corrective", 6, After)),

        // Outer loop, just doing a standard Montgomery reduction on z

        Q!("    xor             " i!() ", " i!()),
        Q!(Label!("Lbignum_montredc_outerloop", 7) ":"),
        Q!("    mov             " e!() ", [" z!() "]"),
        Q!("    mov             " d!() ", " w!()),
        Q!("    imul            " d!() ", " e!()),
        Q!("    mov             " a!() ", [" m!() "]"),
        Q!("    mul             " d!()),
        Q!("    add             " a!() ", " e!()),
        Q!("    mov             " h!() ", rdx"),
        Q!("    mov             " jshort!() ", 1"),
        Q!("    mov             " t!() ", " k!()),
        Q!("    dec             " t!()),
        Q!("    jz              " Label!("Lbignum_montredc_montend", 8, After)),

        Q!(Label!("Lbignum_montredc_montloop", 9) ":"),
        Q!("    adc             " h!() ", [" z!() "+ 8 * " j!() "]"),
        Q!("    sbb             " e!() ", " e!()),
        Q!("    mov             " a!() ", [" m!() "+ 8 * " j!() "]"),
        Q!("    mul             " d!()),
        Q!("    sub             " "rdx, " e!()),
        Q!("    add             " a!() ", " h!()),
        Q!("    mov             " "[" z!() "+ 8 * " j!() "-8], " a!()),
        Q!("    mov             " h!() ", rdx"),
        Q!("    inc             " j!()),
        Q!("    dec             " t!()),
        Q!("    jnz             " Label!("Lbignum_montredc_montloop", 9, Before)),

        Q!(Label!("Lbignum_montredc_montend", 8) ":"),
        Q!("    adc             " h!() ", " c!()),
        Q!("    mov             " cshort!() ", 0"),
        Q!("    adc             " c!() ", 0"),

        Q!("    add             " j!() ", " i!()),
        Q!("    cmp             " j!() ", " n!()),
        Q!("    jnc             " Label!("Lbignum_montredc_offtheend", 12, After)),
        Q!("    mov             " a!() ", [" x!() "+ 8 * " j!() "]"),
        Q!("    add             " h!() ", " a!()),
        Q!("    adc             " c!() ", 0"),
        Q!(Label!("Lbignum_montredc_offtheend", 12) ":"),
        Q!("    mov             " "[" z!() "+ 8 * " k!() "-8], " h!()),

        // End of outer loop.

        Q!("    inc             " i!()),
        Q!("    cmp             " i!() ", " p!()),
        Q!("    jc              " Label!("Lbignum_montredc_outerloop", 7, Before)),

        // Now do a comparison of (c::z) with (0::m) to set a final correction mask
        // indicating that (c::z) >= m and so we need to subtract m.

        Q!(Label!("Lbignum_montredc_corrective", 6) ":"),

        Q!("    xor             " j!() ", " j!()),
        Q!("    mov             " n!() ", " k!()),
        Q!(Label!("Lbignum_montredc_cmploop", 13) ":"),
        Q!("    mov             " a!() ", [" z!() "+ 8 * " j!() "]"),
        Q!("    sbb             " a!() ", [" m!() "+ 8 * " j!() "]"),
        Q!("    inc             " j!()),
        Q!("    dec             " n!()),
        Q!("    jnz             " Label!("Lbignum_montredc_cmploop", 13, Before)),

        Q!("    sbb             " c!() ", 0"),
        Q!("    sbb             " d!() ", " d!()),
        Q!("    not             " d!()),

        // Now do a masked subtraction of m for the final reduced result.

        Q!("    xor             " e!() ", " e!()),
        Q!("    xor             " j!() ", " j!()),
        Q!(Label!("Lbignum_montredc_corrloop", 14) ":"),
        Q!("    mov             " a!() ", [" m!() "+ 8 * " j!() "]"),
        Q!("    and             " a!() ", " d!()),
        Q!("    neg             " e!()),
        Q!("    sbb             " "[" z!() "+ 8 * " j!() "], " a!()),
        Q!("    sbb             " e!() ", " e!()),
        Q!("    inc             " j!()),
        Q!("    cmp             " j!() ", " k!()),
        Q!("    jc              " Label!("Lbignum_montredc_corrloop", 14, Before)),

        Q!(Label!("Lbignum_montredc_end", 2) ":"),
        Q!("    add             " "rsp, 8"),
        Q!("    pop             " "r15"),
        Q!("    pop             " "r14"),
        Q!("    pop             " "r13"),
        Q!("    pop             " "r12"),
        Q!("    pop             " "rbp"),
        Q!("    pop             " "rbx"),

        inout("rdi") z.len() => _,
        inout("rsi") z.as_mut_ptr() => _,
        inout("rdx") x.len() => _,
        inout("rcx") x.as_ptr() => _,
        inout("r8") m.as_ptr() => _,
        inout("r9") p => _,
        // clobbers
        out("r10") _,
        out("r11") _,
        out("r12") _,
        out("r13") _,
        out("r14") _,
        out("r15") _,
        out("rax") _,
            )
    };
}
