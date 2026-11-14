// generated source. do not edit.
#![allow(non_upper_case_globals, unused_macros, unused_imports)]
use crate::low::macros::*;

// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

// ----------------------------------------------------------------------------
// Test bignums for coprimality, gcd(x,y) = 1
// Inputs x[m], y[n]; output function return; temporary buffer t[>=2*max(m,n)]
//
//    extern uint64_t bignum_coprime(uint64_t m, const uint64_t *x, uint64_t n,
//                                   const uint64_t *y, uint64_t *t);
//
// Test for whether two bignums are coprime (no common factor besides 1).
// This is equivalent to testing if their gcd is 1, but a bit faster than
// doing those two computations separately.
//
// Here bignum x is m digits long, y is n digits long and the temporary
// buffer t needs to be 2 * max(m,n) digits long. The return value is
// 1 if coprime(x,y) and 0 otherwise.
//
// Standard x86-64 ABI: RDI = m, RSI = x, RDX = n, RCX = y, R8 = t, returns RAX
// Microsoft x64 ABI:   RCX = m, RDX = x, R8 = n, R9 = y, [RSP+40] = t, returns RAX
// ----------------------------------------------------------------------------

// We get CHUNKSIZE bits per outer iteration, 64 minus a bit for proxy errors

macro_rules! CHUNKSIZE {
    () => {
        "58"
    };
}

// These variables are so fundamental we keep them consistently in registers.
// m is in fact the temporary buffer argument w so use the same register

macro_rules! m {
    () => {
        "r8"
    };
}
macro_rules! n {
    () => {
        "r15"
    };
}
macro_rules! k {
    () => {
        "r14"
    };
}
macro_rules! l {
    () => {
        "r13"
    };
}

// These are kept on the stack since there aren't enough registers

macro_rules! mat_mm {
    () => {
        "QWORD PTR [rsp]"
    };
}
macro_rules! mat_mn {
    () => {
        "QWORD PTR [rsp + 8]"
    };
}
macro_rules! mat_nm {
    () => {
        "QWORD PTR [rsp + 16]"
    };
}
macro_rules! mat_nn {
    () => {
        "QWORD PTR [rsp + 24]"
    };
}
macro_rules! t {
    () => {
        "QWORD PTR [rsp + 32]"
    };
}
macro_rules! evenor {
    () => {
        "QWORD PTR [rsp + 40]"
    };
}

macro_rules! STACKVARSIZE {
    () => {
        "48"
    };
}

// These are shorthands for common temporary register

macro_rules! a {
    () => {
        "rax"
    };
}
macro_rules! b {
    () => {
        "rbx"
    };
}
macro_rules! c {
    () => {
        "rcx"
    };
}
macro_rules! d {
    () => {
        "rdx"
    };
}
macro_rules! i {
    () => {
        "r9"
    };
}

// Temporaries for the top proxy selection part

macro_rules! c1 {
    () => {
        "r10"
    };
}
macro_rules! c2 {
    () => {
        "r11"
    };
}
macro_rules! h1 {
    () => {
        "r12"
    };
}
macro_rules! h2 {
    () => {
        "rbp"
    };
}
macro_rules! l1 {
    () => {
        "rdi"
    };
}
macro_rules! l2 {
    () => {
        "rsi"
    };
}

// Re-use for the actual proxies; m_hi = h1 and n_hi = h2 are assumed

macro_rules! m_hi {
    () => {
        "r12"
    };
}
macro_rules! n_hi {
    () => {
        "rbp"
    };
}
macro_rules! m_lo {
    () => {
        "rdi"
    };
}
macro_rules! n_lo {
    () => {
        "rsi"
    };
}

// Re-use for the matrix entries in the inner loop, though they
// get spilled to the corresponding memory locations mat_...

macro_rules! m_m {
    () => {
        "r10"
    };
}
macro_rules! m_n {
    () => {
        "r11"
    };
}
macro_rules! n_m {
    () => {
        "rcx"
    };
}
macro_rules! n_n {
    () => {
        "rdx"
    };
}

macro_rules! ishort {
    () => {
        "r9d"
    };
}
macro_rules! m_mshort {
    () => {
        "r10d"
    };
}
macro_rules! m_nshort {
    () => {
        "r11d"
    };
}
macro_rules! n_mshort {
    () => {
        "ecx"
    };
}
macro_rules! n_nshort {
    () => {
        "edx"
    };
}

// Because they are so unmemorable

macro_rules! arg1 {
    () => {
        "rdi"
    };
}
macro_rules! arg2 {
    () => {
        "rsi"
    };
}
macro_rules! arg3 {
    () => {
        "rdx"
    };
}
macro_rules! arg4 {
    () => {
        "rcx"
    };
}

/// Test bignums for coprimality, gcd(x,y) = 1
///
/// Inputs x[m], y[n]; output function return; temporary buffer t[>=2*max(m,n)]
///
/// Test for whether two bignums are coprime (no common factor besides 1).
/// This is equivalent to testing if their gcd is 1, but a bit faster than
/// doing those two computations separately.
///
/// Here bignum x is m digits long, y is n digits long and the temporary
/// buffer t needs to be 2 * max(m,n) digits long. The return value is
/// 1 if coprime(x,y) and 0 otherwise.
pub(crate) fn bignum_coprime(x: &[u64], y: &[u64], t: &mut [u64]) -> bool {
    let ret: u64;
    debug_assert!(t.len() >= 2 * core::cmp::max(x.len(), y.len()));
    // SAFETY: inline assembly. see [crate::low::inline_assembly_safety] for safety info.
    unsafe {
        core::arch::asm!(

        Q!("    endbr64         " ),

        // Save all required registers and make room on stack for all the above vars

        Q!("    push            " "rbp"),
        Q!("    push            " "rbx"),
        Q!("    push            " "r12"),
        Q!("    push            " "r13"),
        Q!("    push            " "r14"),
        Q!("    push            " "r15"),
        Q!("    sub             " "rsp, " STACKVARSIZE!()),

        // Compute k = max(m,n), and if this is zero skip to the end. Note that
        // in this case k is also in rax so serves as the right answer of "false"

        Q!("    mov             " "rax, " arg1!()),
        Q!("    cmp             " "rax, " arg3!()),
        Q!("    cmovc           " "rax, " arg3!()),
        Q!("    mov             " k!() ", rax"),

        Q!("    test            " "rax, rax"),
        Q!("    jz              " Label!("Lbignum_coprime_end", 2, After)),

        // Set up inside w two size-k buffers m and n

        Q!("    lea             " n!() ", [" m!() "+ 8 * " k!() "]"),

        // Copy the input x into the buffer m, padding with zeros as needed

        Q!("    xor             " i!() ", " i!()),
        Q!("    test            " arg1!() ", " arg1!()),
        Q!("    jz              " Label!("Lbignum_coprime_xpadloop", 3, After)),
        Q!(Label!("Lbignum_coprime_xloop", 4) ":"),
        Q!("    mov             " a!() ", [" arg2!() "+ 8 * " i!() "]"),
        Q!("    mov             " "[" m!() "+ 8 * " i!() "], " a!()),
        Q!("    inc             " i!()),
        Q!("    cmp             " i!() ", " arg1!()),
        Q!("    jc              " Label!("Lbignum_coprime_xloop", 4, Before)),
        Q!("    cmp             " i!() ", " k!()),
        Q!("    jnc             " Label!("Lbignum_coprime_xskip", 5, After)),
        Q!(Label!("Lbignum_coprime_xpadloop", 3) ":"),
        Q!("    mov             " "QWORD PTR [" m!() "+ 8 * " i!() "], 0"),
        Q!("    inc             " i!()),
        Q!("    cmp             " i!() ", " k!()),
        Q!("    jc              " Label!("Lbignum_coprime_xpadloop", 3, Before)),
        Q!(Label!("Lbignum_coprime_xskip", 5) ":"),

        // Copy the input y into the buffer n, padding with zeros as needed

        Q!("    xor             " i!() ", " i!()),
        Q!("    test            " arg3!() ", " arg3!()),
        Q!("    jz              " Label!("Lbignum_coprime_ypadloop", 6, After)),
        Q!(Label!("Lbignum_coprime_yloop", 7) ":"),
        Q!("    mov             " a!() ", [" arg4!() "+ 8 * " i!() "]"),
        Q!("    mov             " "[" n!() "+ 8 * " i!() "], " a!()),
        Q!("    inc             " i!()),
        Q!("    cmp             " i!() ", " arg3!()),
        Q!("    jc              " Label!("Lbignum_coprime_yloop", 7, Before)),
        Q!("    cmp             " i!() ", " k!()),
        Q!("    jnc             " Label!("Lbignum_coprime_yskip", 8, After)),
        Q!(Label!("Lbignum_coprime_ypadloop", 6) ":"),
        Q!("    mov             " "QWORD PTR [" n!() "+ 8 * " i!() "], 0"),
        Q!("    inc             " i!()),
        Q!("    cmp             " i!() ", " k!()),
        Q!("    jc              " Label!("Lbignum_coprime_ypadloop", 6, Before)),
        Q!(Label!("Lbignum_coprime_yskip", 8) ":"),

        // Set up the outer loop count of 64 * sum of input sizes.
        // The invariant is that m * n < 2^t at all times.

        Q!("    lea             " a!() ", [" arg1!() "+ " arg3!() "]"),
        Q!("    shl             " a!() ", 6"),
        Q!("    mov             " t!() ", " a!()),

        // Record for the very end the OR of the lowest words.
        // If the bottom bit is zero we know both are even so the answer is false.
        // But since this is constant-time code we still execute all the main part.

        Q!("    mov             " a!() ", [" m!() "]"),
        Q!("    mov             " b!() ", [" n!() "]"),
        Q!("    or              " a!() ", " b!()),
        Q!("    mov             " evenor!() ", " a!()),

        // Now if n is even trigger a swap of m and n. This ensures that if
        // one or other of m and n is odd then we make sure now that n is,
        // as expected by our invariant later on.

        Q!("    and             " b!() ", 1"),
        Q!("    sub             " b!() ", 1"),

        Q!("    xor             " i!() ", " i!()),
        Q!(Label!("Lbignum_coprime_swaploop", 9) ":"),
        Q!("    mov             " a!() ", [" m!() "+ 8 * " i!() "]"),
        Q!("    mov             " c!() ", [" n!() "+ 8 * " i!() "]"),
        Q!("    mov             " d!() ", " a!()),
        Q!("    xor             " d!() ", " c!()),
        Q!("    and             " d!() ", " b!()),
        Q!("    xor             " a!() ", " d!()),
        Q!("    xor             " c!() ", " d!()),
        Q!("    mov             " "[" m!() "+ 8 * " i!() "], " a!()),
        Q!("    mov             " "[" n!() "+ 8 * " i!() "], " c!()),
        Q!("    inc             " i!()),
        Q!("    cmp             " i!() ", " k!()),
        Q!("    jnz             " Label!("Lbignum_coprime_swaploop", 9, Before)),

        // Start of the main outer loop iterated t / CHUNKSIZE times

        Q!(Label!("Lbignum_coprime_outerloop", 12) ":"),

        // We need only bother with sharper l = min k (ceil(t/64)) digits
        // Either both m and n fit in l digits, or m has become zero and so
        // nothing happens in the loop anyway and this makes no difference.

        Q!("    mov             " l!() ", " t!()),
        Q!("    add             " l!() ", 63"),
        Q!("    shr             " l!() ", 6"),
        Q!("    cmp             " l!() ", " k!()),
        Q!("    cmovnc          " l!() ", " k!()),

        // Select upper and lower proxies for both m and n to drive the inner
        // loop. The lower proxies are simply the lowest digits themselves,
        // m_lo = m[0] and n_lo = n[0], while the upper proxies are bitfields
        // of the two inputs selected so their top bit (63) aligns with the
        // most significant bit of *either* of the two inputs.

        Q!("    xor             " h1!() ", " h1!()),
        Q!("    xor             " l1!() ", " l1!()),
        Q!("    xor             " h2!() ", " h2!()),
        Q!("    xor             " l2!() ", " l2!()),
        Q!("    xor             " c2!() ", " c2!()),
        // and in this case h1 and h2 are those words

        Q!("    xor             " i!() ", " i!()),
        Q!(Label!("Lbignum_coprime_toploop", 13) ":"),
        Q!("    mov             " b!() ", [" m!() "+ 8 * " i!() "]"),
        Q!("    mov             " c!() ", [" n!() "+ 8 * " i!() "]"),
        Q!("    mov             " c1!() ", " c2!()),
        Q!("    and             " c1!() ", " h1!()),
        Q!("    and             " c2!() ", " h2!()),
        Q!("    mov             " a!() ", " b!()),
        Q!("    or              " a!() ", " c!()),
        Q!("    neg             " a!()),
        Q!("    cmovc           " l1!() ", " c1!()),
        Q!("    cmovc           " l2!() ", " c2!()),
        Q!("    cmovc           " h1!() ", " b!()),
        Q!("    cmovc           " h2!() ", " c!()),
        Q!("    sbb             " c2!() ", " c2!()),
        Q!("    inc             " i!()),
        Q!("    cmp             " i!() ", " l!()),
        Q!("    jc              " Label!("Lbignum_coprime_toploop", 13, Before)),

        Q!("    mov             " a!() ", " h1!()),
        Q!("    or              " a!() ", " h2!()),
        Q!("    bsr             " c!() ", " a!()),
        Q!("    xor             " c!() ", 63"),
        Q!("    shld            " h1!() ", " l1!() ", cl"),
        Q!("    shld            " h2!() ", " l2!() ", cl"),

        // m_lo = m[0], n_lo = n[0];

        Q!("    mov             " "rax, [" m!() "]"),
        Q!("    mov             " m_lo!() ", rax"),

        Q!("    mov             " "rax, [" n!() "]"),
        Q!("    mov             " n_lo!() ", rax"),

        // Now the inner loop, with i as loop counter from CHUNKSIZE down.
        // This records a matrix of updates to apply to the initial
        // values of m and n with, at stage j:
        //
        //     sgn * m' = (m_m * m - m_n * n) / 2^j
        //    -sgn * n' = (n_m * m - n_n * n) / 2^j
        //
        // where "sgn" is either +1 or -1, and we lose track of which except
        // that both instance above are the same. This throwing away the sign
        // costs nothing (since we have to correct in general anyway because
        // of the proxied comparison) and makes things a bit simpler. But it
        // is simply the parity of the number of times the first condition,
        // used as the swapping criterion, fires in this loop.

        Q!("    mov             " m_mshort!() ", 1"),
        Q!("    mov             " m_nshort!() ", 0"),
        Q!("    mov             " n_mshort!() ", 0"),
        Q!("    mov             " n_nshort!() ", 1"),
        Q!("    mov             " ishort!() ", " CHUNKSIZE!()),

        // Stash more variables over the inner loop to free up regs

        Q!("    mov             " mat_mn!() ", " k!()),
        Q!("    mov             " mat_nm!() ", " l!()),
        Q!("    mov             " mat_mm!() ", " m!()),
        Q!("    mov             " mat_nn!() ", " n!()),

        // Conceptually in the inner loop we follow these steps:
        //
        // * If m_lo is odd and m_hi < n_hi, then swap the four pairs
        //    (m_hi,n_hi); (m_lo,n_lo); (m_m,n_m); (m_n,n_n)
        //
        // * Now, if m_lo is odd (old or new, doesn't matter as initial n_lo is odd)
        //    m_hi := m_hi - n_hi, m_lo := m_lo - n_lo
        //    m_m  := m_m + n_m, m_n := m_n + n_n
        //
        // * Halve and double them
        //     m_hi := m_hi / 2, m_lo := m_lo / 2
        //     n_m := n_m * 2, n_n := n_n * 2
        //
        // The actual computation computes updates before actually swapping and
        // then corrects as needed.

        Q!(Label!("Lbignum_coprime_innerloop", 14) ":"),

        Q!("    xor             " "eax, eax"),
        Q!("    xor             " "ebx, ebx"),
        Q!("    xor             " m!() ", " m!()),
        Q!("    xor             " n!() ", " n!()),
        Q!("    bt              " m_lo!() ", 0"),

        Q!("    cmovc           " "rax, " n_hi!()),
        Q!("    cmovc           " "rbx, " n_lo!()),
        Q!("    cmovc           " m!() ", " n_m!()),
        Q!("    cmovc           " n!() ", " n_n!()),

        Q!("    mov             " l!() ", " m_lo!()),
        Q!("    sub             " m_lo!() ", rbx"),
        Q!("    sub             " "rbx, " l!()),
        Q!("    mov             " k!() ", " m_hi!()),
        Q!("    sub             " k!() ", rax"),
        Q!("    cmovc           " n_hi!() ", " m_hi!()),
        Q!("    lea             " m_hi!() ", [" k!() "-1]"),
        Q!("    cmovc           " m_lo!() ", rbx"),
        Q!("    cmovc           " n_lo!() ", " l!()),
        Q!("    not             " m_hi!()),
        Q!("    cmovc           " n_m!() ", " m_m!()),
        Q!("    cmovc           " n_n!() ", " m_n!()),
        Q!("    cmovnc          " m_hi!() ", " k!()),

        Q!("    shr             " m_lo!() ", 1"),
        Q!("    add             " m_m!() ", " m!()),
        Q!("    add             " m_n!() ", " n!()),
        Q!("    shr             " m_hi!() ", 1"),
        Q!("    add             " n_m!() ", " n_m!()),
        Q!("    add             " n_n!() ", " n_n!()),

        // End of the inner for-loop

        Q!("    dec             " i!()),
        Q!("    jnz             " Label!("Lbignum_coprime_innerloop", 14, Before)),

        // Unstash the temporary variables

        Q!("    mov             " k!() ", " mat_mn!()),
        Q!("    mov             " l!() ", " mat_nm!()),
        Q!("    mov             " m!() ", " mat_mm!()),
        Q!("    mov             " n!() ", " mat_nn!()),

        // Put the matrix entries in memory since we're out of registers
        // We pull them out repeatedly in the next loop

        Q!("    mov             " mat_mm!() ", " m_m!()),
        Q!("    mov             " mat_mn!() ", " m_n!()),
        Q!("    mov             " mat_nm!() ", " n_m!()),
        Q!("    mov             " mat_nn!() ", " n_n!()),

        // Now actually compute the updates to m and n corresponding to that matrix,
        // and correct the signs if they have gone negative. First we compute the
        // (k+1)-sized updates with the following invariant (here h1 and h2 are in
        // fact carry bitmasks, either 0 or -1):
        //
        //    h1::l1::m = m_m * m - m_n * n
        //    h2::l2::n = n_m * m - n_n * n

        Q!("    xor             " i!() ", " i!()),
        Q!("    xor             " h1!() ", " h1!()),
        Q!("    xor             " l1!() ", " l1!()),
        Q!("    xor             " h2!() ", " h2!()),
        Q!("    xor             " l2!() ", " l2!()),
        Q!(Label!("Lbignum_coprime_crossloop", 15) ":"),

        Q!("    mov             " c!() ", [" m!() "+ 8 * " i!() "]"),
        Q!("    mov             " a!() ", " mat_mm!()),
        Q!("    mul             " c!()),
        Q!("    add             " l1!() ", " a!()),
        Q!("    adc             " d!() ", 0"),
        Q!("    mov             " c1!() ", " d!()),

        Q!("    mov             " a!() ", " mat_nm!()),
        Q!("    mul             " c!()),
        Q!("    add             " l2!() ", " a!()),
        Q!("    adc             " d!() ", 0"),
        Q!("    mov             " c2!() ", " d!()),

        Q!("    mov             " c!() ", [" n!() "+ 8 * " i!() "]"),
        Q!("    mov             " a!() ", " mat_mn!()),
        Q!("    mul             " c!()),
        Q!("    sub             " d!() ", " h1!()),

        Q!("    sub             " l1!() ", " a!()),
        Q!("    sbb             " c1!() ", " d!()),
        Q!("    sbb             " h1!() ", " h1!()),
        Q!("    mov             " "[" m!() "+ 8 * " i!() "], " l1!()),
        Q!("    mov             " l1!() ", " c1!()),

        Q!("    mov             " a!() ", " mat_nn!()),
        Q!("    mul             " c!()),
        Q!("    sub             " d!() ", " h2!()),

        Q!("    sub             " l2!() ", " a!()),
        Q!("    sbb             " c2!() ", " d!()),
        Q!("    sbb             " h2!() ", " h2!()),
        Q!("    mov             " "[" n!() "+ 8 * " i!() "], " l2!()),
        Q!("    mov             " l2!() ", " c2!()),

        Q!("    inc             " i!()),
        Q!("    cmp             " i!() ", " l!()),
        Q!("    jc              " Label!("Lbignum_coprime_crossloop", 15, Before)),

        // Now fix the signs of m and n if they have gone negative

        Q!("    xor             " i!() ", " i!()),
        Q!("    mov             " c1!() ", " h1!()),
        Q!("    mov             " c2!() ", " h2!()),
        Q!("    xor             " l1!() ", " h1!()),
        Q!("    xor             " l2!() ", " h2!()),
        Q!(Label!("Lbignum_coprime_optnegloop", 16) ":"),
        Q!("    mov             " a!() ", [" m!() "+ 8 * " i!() "]"),
        Q!("    xor             " a!() ", " h1!()),
        Q!("    neg             " c1!()),
        Q!("    adc             " a!() ", 0"),
        Q!("    sbb             " c1!() ", " c1!()),
        Q!("    mov             " "[" m!() "+ 8 * " i!() "], " a!()),
        Q!("    mov             " a!() ", [" n!() "+ 8 * " i!() "]"),
        Q!("    xor             " a!() ", " h2!()),
        Q!("    neg             " c2!()),
        Q!("    adc             " a!() ", 0"),
        Q!("    sbb             " c2!() ", " c2!()),
        Q!("    mov             " "[" n!() "+ 8 * " i!() "], " a!()),
        Q!("    inc             " i!()),
        Q!("    cmp             " i!() ", " l!()),
        Q!("    jc              " Label!("Lbignum_coprime_optnegloop", 16, Before)),
        Q!("    sub             " l1!() ", " c1!()),
        Q!("    sub             " l2!() ", " c2!()),

        // Now shift them right CHUNKSIZE bits

        Q!("    mov             " i!() ", " l!()),
        Q!(Label!("Lbignum_coprime_shiftloop", 17) ":"),
        Q!("    mov             " a!() ", [" m!() "+ 8 * " i!() "-8]"),
        Q!("    mov             " h1!() ", " a!()),
        Q!("    shrd            " a!() ", " l1!() ", " CHUNKSIZE!()),
        Q!("    mov             " "[" m!() "+ 8 * " i!() "-8], " a!()),
        Q!("    mov             " l1!() ", " h1!()),
        Q!("    mov             " a!() ", [" n!() "+ 8 * " i!() "-8]"),
        Q!("    mov             " h2!() ", " a!()),
        Q!("    shrd            " a!() ", " l2!() ", " CHUNKSIZE!()),
        Q!("    mov             " "[" n!() "+ 8 * " i!() "-8], " a!()),
        Q!("    mov             " l2!() ", " h2!()),
        Q!("    dec             " i!()),
        Q!("    jnz             " Label!("Lbignum_coprime_shiftloop", 17, Before)),

        // End of main loop. We can stop if t' <= 0 since then m * n < 2^0, which
        // since n is odd (in the main cases where we had one or other input odd)
        // means that m = 0 and n is the final gcd. Moreover we do in fact need to
        // maintain strictly t > 0 in the main loop, or the computation of the
        // optimized digit bound l could collapse to 0.

        Q!("    sub             " t!() ", " CHUNKSIZE!()),
        Q!("    jnbe            " Label!("Lbignum_coprime_outerloop", 12, Before)),

        // Now compare n with 1 (OR of the XORs in a)

        Q!("    mov             " a!() ", [" n!() "]"),
        Q!("    xor             " a!() ", 1"),
        Q!("    cmp             " k!() ", 1"),
        Q!("    jz              " Label!("Lbignum_coprime_finalcomb", 18, After)),
        Q!("    mov             " ishort!() ", 1"),
        Q!(Label!("Lbignum_coprime_compareloop", 19) ":"),
        Q!("    or              " a!() ", [" n!() "+ 8 * " i!() "]"),
        Q!("    inc             " i!()),
        Q!("    cmp             " i!() ", " k!()),
        Q!("    jc              " Label!("Lbignum_coprime_compareloop", 19, Before)),

        // Now combine that with original "evenor" oddness flag
        // The final condition is lsb(evenor) = 1 AND a = 0

        Q!(Label!("Lbignum_coprime_finalcomb", 18) ":"),
        Q!("    neg             " a!()),
        Q!("    sbb             " a!() ", " a!()),
        Q!("    inc             " a!()),
        Q!("    and             " a!() ", " evenor!()),

        // The end

        Q!(Label!("Lbignum_coprime_end", 2) ":"),
        Q!("    add             " "rsp, " STACKVARSIZE!()),
        Q!("    pop             " "r15"),
        Q!("    pop             " "r14"),
        Q!("    pop             " "r13"),
        Q!("    pop             " "r12"),
        Q!("    pop             " "rbx"),
        Q!("    pop             " "rbp"),

        inout("rdi") x.len() => _,
        inout("rsi") x.as_ptr() => _,
        inout("rdx") y.len() => _,
        inout("rcx") y.as_ptr() => _,
        inout("r8") t.as_mut_ptr() => _,
        out("rax") ret,
        // clobbers
        out("r10") _,
        out("r11") _,
        out("r12") _,
        out("r13") _,
        out("r14") _,
        out("r15") _,
        out("r9") _,
            )
    };
    ret > 0
}
