// generated source. do not edit.
#![allow(non_upper_case_globals, unused_macros, unused_imports)]
use crate::low::macros::*;

// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

// ----------------------------------------------------------------------------
// Invert modulo m, z = (1/a) mod b, assuming b is an odd number > 1, coprime a
// Inputs a[k], b[k]; output z[k]; temporary buffer t[>=3*k]
//
//    extern void bignum_modinv(uint64_t k, uint64_t *z, const uint64_t *a,
//                              const uint64_t *b, uint64_t *t);
//
// k-digit (digit=64 bits) "z := a^-1 mod b" (modular inverse of a modulo b)
// using t as a temporary buffer (t at least 3*k words = 24*k bytes), and
// assuming that a and b are coprime *and* that b is an odd number > 1.
//
// Standard x86-64 ABI: RDI = k, RSI = z, RDX = a, RCX = b, R8 = t
// Microsoft x64 ABI:   RCX = k, RDX = z, R8 = a, R9 = b, [RSP+40] = t
// ----------------------------------------------------------------------------

// We get CHUNKSIZE bits per outer iteration, 64 minus a few for proxy errors

macro_rules! CHUNKSIZE {
    () => {
        "58"
    };
}

// These variables are so fundamental we keep them consistently in registers.
// k actually stays where it was at the beginning, while l gets set up  later

macro_rules! k {
    () => {
        "rdi"
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
// Modular inverse
macro_rules! v {
    () => {
        "QWORD PTR [rsp + 40]"
    };
}
// We reconstruct n as m + 8*k as needed
macro_rules! m {
    () => {
        "QWORD PTR [rsp + 48]"
    };
}
macro_rules! w {
    () => {
        "QWORD PTR [rsp + 56]"
    };
}
macro_rules! z {
    () => {
        "QWORD PTR [rsp + 64]"
    };
}
// Original b pointer, not b the temp
macro_rules! bm {
    () => {
        "QWORD PTR [rsp + 72]"
    };
}

macro_rules! STACKVARSIZE {
    () => {
        "80"
    };
}

// These get set to m/n or w/z during the cross-multiplications etc.
// Otherwise they can be used as additional temporaries

macro_rules! p1 {
    () => {
        "r8"
    };
}
macro_rules! p2 {
    () => {
        "r15"
    };
}

// These are shorthands for common temporary registers

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
        "r14"
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
        "r14"
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

macro_rules! ashort {
    () => {
        "eax"
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

/// Invert modulo m, z = (1/a) mod b, assuming b is an odd number > 1, coprime a
///
/// Inputs a[k], b[k]; output z[k]; temporary buffer t[>=3*k]
///
/// k-digit (digit=64 bits) "z := a^-1 mod b" (modular inverse of a modulo b)
/// using t as a temporary buffer (t at least 3*k words = 24*k bytes), and
/// assuming that a and b are coprime *and* that b is an odd number > 1.
pub(crate) fn bignum_modinv(z: &mut [u64], a: &[u64], b: &[u64], t: &mut [u64]) {
    debug_assert!(z.len() == a.len());
    debug_assert!(z.len() == b.len());
    debug_assert!(z.len() * 3 <= t.len());
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

        // If k = 0 then do nothing (this is out of scope anyway)

        Q!("    test            " k!() ", " k!()),
        Q!("    jz              " Label!("Lbignum_modinv_end", 2, After)),

        // Set up the additional two buffers m and n beyond w in temp space
        // and record all pointers m, n, w and z in stack-based variables

        Q!("    mov             " z!() ", rsi"),
        Q!("    mov             " w!() ", r8"),
        Q!("    mov             " bm!() ", rcx"),
        Q!("    lea             " "r10, [r8 + 8 * " k!() "]"),
        Q!("    mov             " m!() ", r10"),
        Q!("    lea             " p2!() ", [r10 + 8 * " k!() "]"),

        // Initialize the main buffers with their starting values:
        // m = a, n = b, w = b (to be tweaked to b - 1) and z = 0

        Q!("    xor             " "r11, r11"),
        Q!("    xor             " i!() ", " i!()),
        Q!(Label!("Lbignum_modinv_copyloop", 3) ":"),
        Q!("    mov             " a!() ", [rdx + 8 * " i!() "]"),
        Q!("    mov             " b!() ", [rcx + 8 * " i!() "]"),
        Q!("    mov             " "[r10 + 8 * " i!() "], " a!()),
        Q!("    mov             " "[" p2!() "+ 8 * " i!() "], " b!()),
        Q!("    mov             " "[r8 + 8 * " i!() "], " b!()),
        Q!("    mov             " "[rsi + 8 * " i!() "], r11"),
        Q!("    inc             " i!()),
        Q!("    cmp             " i!() ", " k!()),
        Q!("    jc              " Label!("Lbignum_modinv_copyloop", 3, Before)),

        // Tweak down w to b - 1 (this crude approach is safe as b needs to be odd
        // for it to be in scope). We have then established the congruence invariant:
        //
        //   a * w == -m (mod b)
        //   a * z == n (mod b)
        //
        // This, with the bounds w <= b and z <= b, is maintained round the outer loop

        Q!("    mov             " a!() ", [r8]"),
        Q!("    mov             " b!() ", " a!()),
        Q!("    dec             " b!()),
        Q!("    mov             " "[r8], " b!()),

        // Compute v = negated modular inverse of b mod 2^64, reusing a from above
        // This is used for Montgomery reduction operations each time round the loop

        Q!("    mov             " h2!() ", " a!()),
        Q!("    mov             " h1!() ", " a!()),
        Q!("    shl             " h2!() ", 2"),
        Q!("    sub             " h1!() ", " h2!()),
        Q!("    xor             " h1!() ", 2"),

        Q!("    mov             " h2!() ", " h1!()),
        Q!("    imul            " h2!() ", " a!()),
        Q!("    mov             " ashort!() ", 2"),
        Q!("    add             " a!() ", " h2!()),
        Q!("    add             " h2!() ", 1"),

        Q!("    imul            " h1!() ", " a!()),

        Q!("    imul            " h2!() ", " h2!()),
        Q!("    mov             " ashort!() ", 1"),
        Q!("    add             " a!() ", " h2!()),
        Q!("    imul            " h1!() ", " a!()),

        Q!("    imul            " h2!() ", " h2!()),
        Q!("    mov             " ashort!() ", 1"),
        Q!("    add             " a!() ", " h2!()),
        Q!("    imul            " h1!() ", " a!()),

        Q!("    imul            " h2!() ", " h2!()),
        Q!("    mov             " ashort!() ", 1"),
        Q!("    add             " a!() ", " h2!()),
        Q!("    imul            " h1!() ", " a!()),

        Q!("    mov             " v!() ", " h1!()),

        // Set up the outer loop count of 128 * k
        // The invariant is that m * n < 2^t at all times.

        Q!("    mov             " a!() ", " k!()),
        Q!("    shl             " a!() ", 7"),
        Q!("    mov             " t!() ", " a!()),

        // Start of the main outer loop iterated t / CHUNKSIZE times

        Q!(Label!("Lbignum_modinv_outerloop", 4) ":"),

        // We need only bother with sharper l = min k (ceil(t/64)) digits
        // for the computations on m and n (but we still need k for w and z).
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

        Q!("    mov             " p1!() ", " m!()),
        Q!("    lea             " p2!() ", [" p1!() "+ 8 * " k!() "]"),
        Q!("    xor             " i!() ", " i!()),
        Q!(Label!("Lbignum_modinv_toploop", 5) ":"),
        Q!("    mov             " b!() ", [" p1!() "+ 8 * " i!() "]"),
        Q!("    mov             " c!() ", [" p2!() "+ 8 * " i!() "]"),
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
        Q!("    jc              " Label!("Lbignum_modinv_toploop", 5, Before)),

        Q!("    mov             " a!() ", " h1!()),
        Q!("    or              " a!() ", " h2!()),
        Q!("    bsr             " c!() ", " a!()),
        Q!("    xor             " c!() ", 63"),
        Q!("    shld            " h1!() ", " l1!() ", cl"),
        Q!("    shld            " h2!() ", " l2!() ", cl"),

        // m_lo = m[0], n_lo = n[0];

        Q!("    mov             " "rax, [" p1!() "]"),
        Q!("    mov             " m_lo!() ", rax"),

        Q!("    mov             " "rax, [" p2!() "]"),
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
        Q!("    mov             " mat_mm!() ", " p1!()),
        Q!("    mov             " mat_nn!() ", " p2!()),

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

        Q!(Label!("Lbignum_modinv_innerloop", 6) ":"),

        Q!("    xor             " "eax, eax"),
        Q!("    xor             " "ebx, ebx"),
        Q!("    xor             " p1!() ", " p1!()),
        Q!("    xor             " p2!() ", " p2!()),
        Q!("    bt              " m_lo!() ", 0"),

        Q!("    cmovc           " "rax, " n_hi!()),
        Q!("    cmovc           " "rbx, " n_lo!()),
        Q!("    cmovc           " p1!() ", " n_m!()),
        Q!("    cmovc           " p2!() ", " n_n!()),

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
        Q!("    add             " m_m!() ", " p1!()),
        Q!("    add             " m_n!() ", " p2!()),
        Q!("    shr             " m_hi!() ", 1"),
        Q!("    add             " n_m!() ", " n_m!()),
        Q!("    add             " n_n!() ", " n_n!()),

        // End of the inner for-loop

        Q!("    dec             " i!()),
        Q!("    jnz             " Label!("Lbignum_modinv_innerloop", 6, Before)),

        // Unstash the temporary variables

        Q!("    mov             " k!() ", " mat_mn!()),
        Q!("    mov             " l!() ", " mat_nm!()),
        Q!("    mov             " p1!() ", " mat_mm!()),
        Q!("    mov             " p2!() ", " mat_nn!()),

        // Put the matrix entries in memory since we're out of registers
        // We pull them out repeatedly in the next loop

        Q!("    mov             " mat_mm!() ", " m_m!()),
        Q!("    mov             " mat_mn!() ", " m_n!()),
        Q!("    mov             " mat_nm!() ", " n_m!()),
        Q!("    mov             " mat_nn!() ", " n_n!()),

        // Apply the update to w and z, using addition in this case, and also take
        // the chance to shift an additional 6 = 64-CHUNKSIZE bits to be ready for a
        // Montgomery multiplication. Because we know that m_m + m_n <= 2^58 and
        // w, z <= b < 2^{64k}, we know that both of these fit in k+1 words.
        // We do this before the m-n update to allow us to play with c1 and c2 here.
        //
        //    l1::w = 2^6 * (m_m * w + m_n * z)
        //    l2::z = 2^6 * (n_m * w + n_n * z)
        //
        // with c1 and c2 recording previous words for the shifting part

        Q!("    mov             " p1!() ", " w!()),
        Q!("    mov             " p2!() ", " z!()),
        Q!("    xor             " l1!() ", " l1!()),
        Q!("    xor             " l2!() ", " l2!()),
        Q!("    xor             " c1!() ", " c1!()),
        Q!("    xor             " c2!() ", " c2!()),
        Q!("    xor             " i!() ", " i!()),
        Q!(Label!("Lbignum_modinv_congloop", 7) ":"),

        Q!("    mov             " c!() ", [" p1!() "+ 8 * " i!() "]"),
        Q!("    mov             " a!() ", " mat_mm!()),
        Q!("    mul             " c!()),
        Q!("    add             " l1!() ", " a!()),
        Q!("    adc             " d!() ", 0"),
        Q!("    mov             " h1!() ", " d!()),

        Q!("    mov             " a!() ", " mat_nm!()),
        Q!("    mul             " c!()),
        Q!("    add             " l2!() ", " a!()),
        Q!("    adc             " d!() ", 0"),
        Q!("    mov             " h2!() ", " d!()),

        Q!("    mov             " c!() ", [" p2!() "+ 8 * " i!() "]"),
        Q!("    mov             " a!() ", " mat_mn!()),
        Q!("    mul             " c!()),
        Q!("    add             " l1!() ", " a!()),
        Q!("    adc             " h1!() ", " d!()),
        Q!("    shrd            " c1!() ", " l1!() ", " CHUNKSIZE!()),
        Q!("    mov             " "[" p1!() "+ 8 * " i!() "], " c1!()),
        Q!("    mov             " c1!() ", " l1!()),
        Q!("    mov             " l1!() ", " h1!()),

        Q!("    mov             " a!() ", " mat_nn!()),
        Q!("    mul             " c!()),
        Q!("    add             " l2!() ", " a!()),
        Q!("    adc             " h2!() ", " d!()),
        Q!("    shrd            " c2!() ", " l2!() ", " CHUNKSIZE!()),
        Q!("    mov             " "[" p2!() "+ 8 * " i!() "], " c2!()),
        Q!("    mov             " c2!() ", " l2!()),
        Q!("    mov             " l2!() ", " h2!()),

        Q!("    inc             " i!()),
        Q!("    cmp             " i!() ", " k!()),
        Q!("    jc              " Label!("Lbignum_modinv_congloop", 7, Before)),

        Q!("    shld            " l1!() ", " c1!() ", 64 - " CHUNKSIZE!()),
        Q!("    shld            " l2!() ", " c2!() ", 64 - " CHUNKSIZE!()),

        // Do a Montgomery reduction of l1::w

        Q!("    mov             " p2!() ", " bm!()),

        Q!("    mov             " b!() ", [" p1!() "]"),
        Q!("    mov             " h1!() ", " v!()),
        Q!("    imul            " h1!() ", " b!()),
        Q!("    mov             " a!() ", [" p2!() "]"),
        Q!("    mul             " h1!()),
        Q!("    add             " a!() ", " b!()),
        Q!("    mov             " c1!() ", rdx"),
        Q!("    mov             " ishort!() ", 1"),
        Q!("    mov             " c!() ", " k!()),
        Q!("    dec             " c!()),
        Q!("    jz              " Label!("Lbignum_modinv_wmontend", 8, After)),

        Q!(Label!("Lbignum_modinv_wmontloop", 9) ":"),
        Q!("    adc             " c1!() ", [" p1!() "+ 8 * " i!() "]"),
        Q!("    sbb             " b!() ", " b!()),
        Q!("    mov             " a!() ", [" p2!() "+ 8 * " i!() "]"),
        Q!("    mul             " h1!()),
        Q!("    sub             " "rdx, " b!()),
        Q!("    add             " a!() ", " c1!()),
        Q!("    mov             " "[" p1!() "+ 8 * " i!() "-8], " a!()),
        Q!("    mov             " c1!() ", rdx"),
        Q!("    inc             " i!()),
        Q!("    dec             " c!()),
        Q!("    jnz             " Label!("Lbignum_modinv_wmontloop", 9, Before)),

        Q!(Label!("Lbignum_modinv_wmontend", 8) ":"),
        Q!("    adc             " c1!() ", " l1!()),
        Q!("    mov             " "[" p1!() "+ 8 * " k!() "-8], " c1!()),
        Q!("    sbb             " c1!() ", " c1!()),
        Q!("    neg             " c1!()),

        Q!("    mov             " c!() ", " k!()),
        Q!("    xor             " i!() ", " i!()),
        Q!(Label!("Lbignum_modinv_wcmploop", 12) ":"),
        Q!("    mov             " a!() ", [" p1!() "+ 8 * " i!() "]"),
        Q!("    sbb             " a!() ", [" p2!() "+ 8 * " i!() "]"),
        Q!("    inc             " i!()),
        Q!("    dec             " c!()),
        Q!("    jnz             " Label!("Lbignum_modinv_wcmploop", 12, Before)),
        Q!("    sbb             " c1!() ", 0"),
        Q!("    sbb             " c1!() ", " c1!()),
        Q!("    not             " c1!()),

        Q!("    xor             " c!() ", " c!()),
        Q!("    xor             " i!() ", " i!()),
        Q!(Label!("Lbignum_modinv_wcorrloop", 13) ":"),
        Q!("    mov             " a!() ", [" p1!() "+ 8 * " i!() "]"),
        Q!("    mov             " b!() ", [" p2!() "+ 8 * " i!() "]"),
        Q!("    and             " b!() ", " c1!()),
        Q!("    neg             " c!()),
        Q!("    sbb             " a!() ", " b!()),
        Q!("    sbb             " c!() ", " c!()),
        Q!("    mov             " "[" p1!() "+ 8 * " i!() "], " a!()),
        Q!("    inc             " i!()),
        Q!("    cmp             " i!() ", " k!()),
        Q!("    jc              " Label!("Lbignum_modinv_wcorrloop", 13, Before)),

        // Do a Montgomery reduction of l2::z

        Q!("    mov             " p1!() ", " z!()),

        Q!("    mov             " b!() ", [" p1!() "]"),
        Q!("    mov             " h2!() ", " v!()),
        Q!("    imul            " h2!() ", " b!()),
        Q!("    mov             " a!() ", [" p2!() "]"),
        Q!("    mul             " h2!()),
        Q!("    add             " a!() ", " b!()),
        Q!("    mov             " c2!() ", rdx"),
        Q!("    mov             " ishort!() ", 1"),
        Q!("    mov             " c!() ", " k!()),
        Q!("    dec             " c!()),
        Q!("    jz              " Label!("Lbignum_modinv_zmontend", 14, After)),

        Q!(Label!("Lbignum_modinv_zmontloop", 15) ":"),
        Q!("    adc             " c2!() ", [" p1!() "+ 8 * " i!() "]"),
        Q!("    sbb             " b!() ", " b!()),
        Q!("    mov             " a!() ", [" p2!() "+ 8 * " i!() "]"),
        Q!("    mul             " h2!()),
        Q!("    sub             " "rdx, " b!()),
        Q!("    add             " a!() ", " c2!()),
        Q!("    mov             " "[" p1!() "+ 8 * " i!() "-8], " a!()),
        Q!("    mov             " c2!() ", rdx"),
        Q!("    inc             " i!()),
        Q!("    dec             " c!()),
        Q!("    jnz             " Label!("Lbignum_modinv_zmontloop", 15, Before)),

        Q!(Label!("Lbignum_modinv_zmontend", 14) ":"),
        Q!("    adc             " c2!() ", " l2!()),
        Q!("    mov             " "[" p1!() "+ 8 * " k!() "-8], " c2!()),
        Q!("    sbb             " c2!() ", " c2!()),
        Q!("    neg             " c2!()),

        Q!("    mov             " c!() ", " k!()),
        Q!("    xor             " i!() ", " i!()),
        Q!(Label!("Lbignum_modinv_zcmploop", 16) ":"),
        Q!("    mov             " a!() ", [" p1!() "+ 8 * " i!() "]"),
        Q!("    sbb             " a!() ", [" p2!() "+ 8 * " i!() "]"),
        Q!("    inc             " i!()),
        Q!("    dec             " c!()),
        Q!("    jnz             " Label!("Lbignum_modinv_zcmploop", 16, Before)),
        Q!("    sbb             " c2!() ", 0"),
        Q!("    sbb             " c2!() ", " c2!()),
        Q!("    not             " c2!()),

        Q!("    xor             " c!() ", " c!()),
        Q!("    xor             " i!() ", " i!()),
        Q!(Label!("Lbignum_modinv_zcorrloop", 17) ":"),
        Q!("    mov             " a!() ", [" p1!() "+ 8 * " i!() "]"),
        Q!("    mov             " b!() ", [" p2!() "+ 8 * " i!() "]"),
        Q!("    and             " b!() ", " c2!()),
        Q!("    neg             " c!()),
        Q!("    sbb             " a!() ", " b!()),
        Q!("    sbb             " c!() ", " c!()),
        Q!("    mov             " "[" p1!() "+ 8 * " i!() "], " a!()),
        Q!("    inc             " i!()),
        Q!("    cmp             " i!() ", " k!()),
        Q!("    jc              " Label!("Lbignum_modinv_zcorrloop", 17, Before)),

        // Now actually compute the updates to m and n corresponding to the matrix,
        // and correct the signs if they have gone negative. First we compute the
        // (k+1)-sized updates with the following invariant (here h1 and h2 are in
        // fact carry bitmasks, either 0 or -1):
        //
        //    h1::l1::m = m_m * m - m_n * n
        //    h2::l2::n = n_m * m - n_n * n

        Q!("    mov             " p1!() ", " m!()),
        Q!("    lea             " p2!() ", [" p1!() "+ 8 * " k!() "]"),
        Q!("    xor             " i!() ", " i!()),
        Q!("    xor             " h1!() ", " h1!()),
        Q!("    xor             " l1!() ", " l1!()),
        Q!("    xor             " h2!() ", " h2!()),
        Q!("    xor             " l2!() ", " l2!()),
        Q!(Label!("Lbignum_modinv_crossloop", 18) ":"),

        Q!("    mov             " c!() ", [" p1!() "+ 8 * " i!() "]"),
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

        Q!("    mov             " c!() ", [" p2!() "+ 8 * " i!() "]"),
        Q!("    mov             " a!() ", " mat_mn!()),
        Q!("    mul             " c!()),
        Q!("    sub             " d!() ", " h1!()),

        Q!("    sub             " l1!() ", " a!()),
        Q!("    sbb             " c1!() ", " d!()),
        Q!("    sbb             " h1!() ", " h1!()),
        Q!("    mov             " "[" p1!() "+ 8 * " i!() "], " l1!()),
        Q!("    mov             " l1!() ", " c1!()),

        Q!("    mov             " a!() ", " mat_nn!()),
        Q!("    mul             " c!()),
        Q!("    sub             " d!() ", " h2!()),

        Q!("    sub             " l2!() ", " a!()),
        Q!("    sbb             " c2!() ", " d!()),
        Q!("    sbb             " h2!() ", " h2!()),
        Q!("    mov             " "[" p2!() "+ 8 * " i!() "], " l2!()),
        Q!("    mov             " l2!() ", " c2!()),

        Q!("    inc             " i!()),
        Q!("    cmp             " i!() ", " l!()),
        Q!("    jc              " Label!("Lbignum_modinv_crossloop", 18, Before)),

        // Now fix the signs of m and n if they have gone negative

        Q!("    xor             " i!() ", " i!()),
        Q!("    mov             " c1!() ", " h1!()),
        Q!("    mov             " c2!() ", " h2!()),
        Q!("    xor             " l1!() ", " h1!()),
        Q!("    xor             " l2!() ", " h2!()),
        Q!(Label!("Lbignum_modinv_optnegloop", 19) ":"),
        Q!("    mov             " a!() ", [" p1!() "+ 8 * " i!() "]"),
        Q!("    xor             " a!() ", " h1!()),
        Q!("    neg             " c1!()),
        Q!("    adc             " a!() ", 0"),
        Q!("    sbb             " c1!() ", " c1!()),
        Q!("    mov             " "[" p1!() "+ 8 * " i!() "], " a!()),
        Q!("    mov             " a!() ", [" p2!() "+ 8 * " i!() "]"),
        Q!("    xor             " a!() ", " h2!()),
        Q!("    neg             " c2!()),
        Q!("    adc             " a!() ", 0"),
        Q!("    sbb             " c2!() ", " c2!()),
        Q!("    mov             " "[" p2!() "+ 8 * " i!() "], " a!()),
        Q!("    inc             " i!()),
        Q!("    cmp             " i!() ", " l!()),
        Q!("    jc              " Label!("Lbignum_modinv_optnegloop", 19, Before)),
        Q!("    sub             " l1!() ", " c1!()),
        Q!("    sub             " l2!() ", " c2!()),

        // Now shift them right CHUNKSIZE bits

        Q!("    mov             " i!() ", " l!()),
        Q!(Label!("Lbignum_modinv_shiftloop", 20) ":"),
        Q!("    mov             " a!() ", [" p1!() "+ 8 * " i!() "-8]"),
        Q!("    mov             " c1!() ", " a!()),
        Q!("    shrd            " a!() ", " l1!() ", " CHUNKSIZE!()),
        Q!("    mov             " "[" p1!() "+ 8 * " i!() "-8], " a!()),
        Q!("    mov             " l1!() ", " c1!()),
        Q!("    mov             " a!() ", [" p2!() "+ 8 * " i!() "-8]"),
        Q!("    mov             " c2!() ", " a!()),
        Q!("    shrd            " a!() ", " l2!() ", " CHUNKSIZE!()),
        Q!("    mov             " "[" p2!() "+ 8 * " i!() "-8], " a!()),
        Q!("    mov             " l2!() ", " c2!()),
        Q!("    dec             " i!()),
        Q!("    jnz             " Label!("Lbignum_modinv_shiftloop", 20, Before)),

        // Finally, use the signs h1 and h2 to do optional modular negations of
        // w and z respectively, flipping h2 to make signs work. We don't make
        // any checks for zero values, but we certainly retain w <= b and z <= b.
        // This is enough for the Montgomery step in the next iteration to give
        // strict reduction w < b amd z < b, and anyway when we terminate we
        // could not have z = b since it violates the coprimality assumption for
        // in-scope cases.

        Q!("    not             " h2!()),
        Q!("    mov             " c!() ", " bm!()),
        Q!("    mov             " p1!() ", " w!()),
        Q!("    mov             " p2!() ", " z!()),
        Q!("    mov             " c1!() ", " h1!()),
        Q!("    mov             " c2!() ", " h2!()),
        Q!("    xor             " i!() ", " i!()),
        Q!(Label!("Lbignum_modinv_fliploop", 21) ":"),
        Q!("    mov             " d!() ", " h2!()),
        Q!("    mov             " a!() ", [" c!() "+ 8 * " i!() "]"),
        Q!("    and             " d!() ", " a!()),
        Q!("    and             " a!() ", " h1!()),
        Q!("    mov             " b!() ", [" p1!() "+ 8 * " i!() "]"),
        Q!("    xor             " b!() ", " h1!()),
        Q!("    neg             " c1!()),
        Q!("    adc             " a!() ", " b!()),
        Q!("    sbb             " c1!() ", " c1!()),
        Q!("    mov             " "[" p1!() "+ 8 * " i!() "], " a!()),
        Q!("    mov             " b!() ", [" p2!() "+ 8 * " i!() "]"),
        Q!("    xor             " b!() ", " h2!()),
        Q!("    neg             " c2!()),
        Q!("    adc             " d!() ", " b!()),
        Q!("    sbb             " c2!() ", " c2!()),
        Q!("    mov             " "[" p2!() "+ 8 * " i!() "], " d!()),
        Q!("    inc             " i!()),
        Q!("    cmp             " i!() ", " k!()),
        Q!("    jc              " Label!("Lbignum_modinv_fliploop", 21, Before)),

        // End of main loop. We can stop if t' <= 0 since then m * n < 2^0, which
        // since n is odd and m and n are coprime (in the in-scope cases) means
        // m = 0, n = 1 and hence from the congruence invariant a * z == 1 (mod b).
        // Moreover we do in fact need to maintain strictly t > 0 in the main loop,
        // or the computation of the optimized digit bound l could collapse to 0.

        Q!("    sub             " t!() ", " CHUNKSIZE!()),
        Q!("    jnbe            " Label!("Lbignum_modinv_outerloop", 4, Before)),

        Q!(Label!("Lbignum_modinv_end", 2) ":"),
        Q!("    add             " "rsp, " STACKVARSIZE!()),
        Q!("    pop             " "r15"),
        Q!("    pop             " "r14"),
        Q!("    pop             " "r13"),
        Q!("    pop             " "r12"),
        Q!("    pop             " "rbx"),
        Q!("    pop             " "rbp"),

        inout("rdi") b.len() => _,
        inout("rsi") z.as_mut_ptr() => _,
        inout("rdx") a.as_ptr() => _,
        inout("rcx") b.as_ptr() => _,
        inout("r8") t.as_mut_ptr() => _,
        // clobbers
        out("r10") _,
        out("r11") _,
        out("r12") _,
        out("r13") _,
        out("r14") _,
        out("r15") _,
        out("r9") _,
        out("rax") _,
            )
    };
}
