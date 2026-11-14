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
// Standard ARM ABI: X0 = m, X1 = x, X2 = n, X3 = y, X4 = t, returns X0
// ----------------------------------------------------------------------------

macro_rules! CHUNKSIZE {
    () => {
        "58"
    };
}

// Pervasive variables

macro_rules! k {
    () => {
        "x9"
    };
}
macro_rules! m {
    () => {
        "x4"
    };
}
macro_rules! n {
    () => {
        "x5"
    };
}

// Used via parameters in copy-in loop, then re-used as outer loop
// counter t and adaptive precision digit size l, which becomes a
// reduced version of k in later iterations but starts at l = k

macro_rules! x {
    () => {
        "x1"
    };
}
macro_rules! y {
    () => {
        "x3"
    };
}

macro_rules! t {
    () => {
        "x2"
    };
}
macro_rules! l {
    () => {
        "x3"
    };
}

// The matrix of update factors to apply to m and n
// Also used a couple of additional temporary variables for the swapping loop
// Also used as an extra down-counter in corrective negation loops

macro_rules! m_m {
    () => {
        "x6"
    };
}
macro_rules! m_n {
    () => {
        "x7"
    };
}
macro_rules! n_m {
    () => {
        "x8"
    };
}
macro_rules! n_n {
    () => {
        "x1"
    };
}

macro_rules! t3 {
    () => {
        "x6"
    };
}
macro_rules! t4 {
    () => {
        "x7"
    };
}

macro_rules! j {
    () => {
        "x6"
    };
}

// General temporary variables and loop counters

macro_rules! i {
    () => {
        "x10"
    };
}
macro_rules! t1 {
    () => {
        "x11"
    };
}
macro_rules! t2 {
    () => {
        "x12"
    };
}

// High and low proxies for the inner loop
// Then re-used for high and carry words during actual cross-multiplications

macro_rules! m_hi {
    () => {
        "x13"
    };
}
macro_rules! n_hi {
    () => {
        "x14"
    };
}
macro_rules! m_lo {
    () => {
        "x15"
    };
}
macro_rules! n_lo {
    () => {
        "x16"
    };
}

macro_rules! h1 {
    () => {
        "x13"
    };
}
macro_rules! h2 {
    () => {
        "x14"
    };
}
macro_rules! l1 {
    () => {
        "x15"
    };
}
macro_rules! l2 {
    () => {
        "x16"
    };
}

macro_rules! c1 {
    () => {
        "x17"
    };
}
macro_rules! c2 {
    () => {
        "x19"
    };
}
macro_rules! tt {
    () => {
        "x20"
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

        // We make use of just a couple of additional registers

        Q!("    stp             " "x19, x20, [sp, #-16] !"),

        // Compute k = max(m,n), and if this is zero skip to the end. Note that
        // in this case x0 = m = 0 so we return the right answer of "false"

        Q!("    cmp             " "x0, x2"),
        Q!("    csel            " k!() ", x2, x0, cc"),
        Q!("    cbz             " k!() ", " Label!("Lbignum_coprime_end", 2, After)),

        // Set up inside w two size-k buffers m and n

        Q!("    lsl             " i!() ", " k!() ", #3"),
        Q!("    add             " n!() ", " m!() ", " i!()),

        // Copy the input x into the buffer m, padding with zeros as needed

        Q!("    mov             " i!() ", xzr"),
        Q!("    cbz             " "x0, " Label!("Lbignum_coprime_xpadloop", 3, After)),
        Q!(Label!("Lbignum_coprime_xloop", 4) ":"),
        Q!("    ldr             " t1!() ", [x1, " i!() ", lsl #3]"),
        Q!("    str             " t1!() ", [" m!() ", " i!() ", lsl #3]"),
        Q!("    add             " i!() ", " i!() ", #1"),
        Q!("    cmp             " i!() ", x0"),
        Q!("    bcc             " Label!("Lbignum_coprime_xloop", 4, Before)),
        Q!("    cmp             " i!() ", " k!()),
        Q!("    bcs             " Label!("Lbignum_coprime_xskip", 5, After)),
        Q!(Label!("Lbignum_coprime_xpadloop", 3) ":"),
        Q!("    str             " "xzr, [" m!() ", " i!() ", lsl #3]"),
        Q!("    add             " i!() ", " i!() ", #1"),
        Q!("    cmp             " i!() ", " k!()),
        Q!("    bcc             " Label!("Lbignum_coprime_xpadloop", 3, Before)),
        Q!(Label!("Lbignum_coprime_xskip", 5) ":"),

        // Copy the input y into the buffer n, padding with zeros as needed

        Q!("    mov             " i!() ", xzr"),
        Q!("    cbz             " "x2, " Label!("Lbignum_coprime_ypadloop", 6, After)),
        Q!(Label!("Lbignum_coprime_yloop", 7) ":"),
        Q!("    ldr             " t1!() ", [x3, " i!() ", lsl #3]"),
        Q!("    str             " t1!() ", [" n!() ", " i!() ", lsl #3]"),
        Q!("    add             " i!() ", " i!() ", #1"),
        Q!("    cmp             " i!() ", x2"),
        Q!("    bcc             " Label!("Lbignum_coprime_yloop", 7, Before)),
        Q!("    cmp             " i!() ", " k!()),
        Q!("    bcs             " Label!("Lbignum_coprime_yskip", 8, After)),
        Q!(Label!("Lbignum_coprime_ypadloop", 6) ":"),
        Q!("    str             " "xzr, [" n!() ", " i!() ", lsl #3]"),
        Q!("    add             " i!() ", " i!() ", #1"),
        Q!("    cmp             " i!() ", " k!()),
        Q!("    bcc             " Label!("Lbignum_coprime_ypadloop", 6, Before)),
        Q!(Label!("Lbignum_coprime_yskip", 8) ":"),

        // Set up the outer loop count of 64 * sum of input sizes.
        // The invariant is that m * n < 2^t at all times.

        Q!("    add             " t!() ", x0, x2"),
        Q!("    lsl             " t!() ", " t!() ", #6"),

        // Record for the very end the OR of the lowest words.
        // If the bottom bit is zero we know both are even so the answer is false.
        // But since this is constant-time code we still execute all the main part.

        Q!("    ldr             " "x0, [" m!() "]"),
        Q!("    ldr             " t3!() ", [" n!() "]"),
        Q!("    orr             " "x0, x0, " t3!()),

        // Now if n is even trigger a swap of m and n. This ensures that if
        // one or other of m and n is odd then we make sure now that n is,
        // as expected by our invariant later on.

        Q!("    and             " t3!() ", " t3!() ", #1"),
        Q!("    sub             " t3!() ", " t3!() ", #1"),

        Q!("    mov             " i!() ", xzr"),
        Q!(Label!("Lbignum_coprime_swaploop", 9) ":"),
        Q!("    ldr             " t1!() ", [" m!() ", " i!() ", lsl #3]"),
        Q!("    ldr             " t2!() ", [" n!() ", " i!() ", lsl #3]"),
        Q!("    eor             " t4!() ", " t1!() ", " t2!()),
        Q!("    and             " t4!() ", " t4!() ", " t3!()),
        Q!("    eor             " t1!() ", " t1!() ", " t4!()),
        Q!("    eor             " t2!() ", " t2!() ", " t4!()),
        Q!("    str             " t1!() ", [" m!() ", " i!() ", lsl #3]"),
        Q!("    str             " t2!() ", [" n!() ", " i!() ", lsl #3]"),
        Q!("    add             " i!() ", " i!() ", #1"),
        Q!("    cmp             " i!() ", " k!()),
        Q!("    bcc             " Label!("Lbignum_coprime_swaploop", 9, Before)),

        // Start of the main outer loop iterated t / CHUNKSIZE times

        Q!(Label!("Lbignum_coprime_outerloop", 12) ":"),

        // We need only bother with sharper l = min k (ceil(t/64)) digits
        // Either both m and n fit in l digits, or m has become zero and so
        // nothing happens in the loop anyway and this makes no difference.

        Q!("    add             " i!() ", " t!() ", #63"),
        Q!("    lsr             " l!() ", " i!() ", #6"),
        Q!("    cmp             " l!() ", " k!()),
        Q!("    csel            " l!() ", " k!() ", " l!() ", cs"),

        // Select upper and lower proxies for both m and n to drive the inner
        // loop. The lower proxies are simply the lowest digits themselves,
        // m_lo = m[0] and n_lo = n[0], while the upper proxies are bitfields
        // of the two inputs selected so their top bit (63) aligns with the
        // most significant bit of *either* of the two inputs.

        Q!("    mov             " h1!() ", xzr"),
        Q!("    mov             " l1!() ", xzr"),
        Q!("    mov             " h2!() ", xzr"),
        Q!("    mov             " l2!() ", xzr"),
        Q!("    mov             " c2!() ", xzr"),
        // and in this case h1 and h2 are those words

        Q!("    mov             " i!() ", xzr"),
        Q!(Label!("Lbignum_coprime_toploop", 13) ":"),
        Q!("    ldr             " t1!() ", [" m!() ", " i!() ", lsl #3]"),
        Q!("    ldr             " t2!() ", [" n!() ", " i!() ", lsl #3]"),
        Q!("    orr             " c1!() ", " t1!() ", " t2!()),
        Q!("    cmp             " c1!() ", xzr"),
        Q!("    and             " c1!() ", " c2!() ", " h1!()),
        Q!("    csel            " l1!() ", " c1!() ", " l1!() ", ne"),
        Q!("    and             " c1!() ", " c2!() ", " h2!()),
        Q!("    csel            " l2!() ", " c1!() ", " l2!() ", ne"),
        Q!("    csel            " h1!() ", " t1!() ", " h1!() ", ne"),
        Q!("    csel            " h2!() ", " t2!() ", " h2!() ", ne"),
        Q!("    csetm           " c2!() ", ne"),
        Q!("    add             " i!() ", " i!() ", #1"),
        Q!("    cmp             " i!() ", " l!()),
        Q!("    bcc             " Label!("Lbignum_coprime_toploop", 13, Before)),

        Q!("    orr             " t1!() ", " h1!() ", " h2!()),
        Q!("    clz             " t2!() ", " t1!()),
        Q!("    negs            " c1!() ", " t2!()),
        Q!("    lsl             " h1!() ", " h1!() ", " t2!()),
        Q!("    csel            " l1!() ", " l1!() ", xzr, ne"),
        Q!("    lsl             " h2!() ", " h2!() ", " t2!()),
        Q!("    csel            " l2!() ", " l2!() ", xzr, ne"),
        Q!("    lsr             " l1!() ", " l1!() ", " c1!()),
        Q!("    lsr             " l2!() ", " l2!() ", " c1!()),
        Q!("    orr             " m_hi!() ", " h1!() ", " l1!()),
        Q!("    orr             " n_hi!() ", " h2!() ", " l2!()),

        Q!("    ldr             " m_lo!() ", [" m!() "]"),
        Q!("    ldr             " n_lo!() ", [" n!() "]"),

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

        Q!("    mov             " m_m!() ", #1"),
        Q!("    mov             " m_n!() ", xzr"),
        Q!("    mov             " n_m!() ", xzr"),
        Q!("    mov             " n_n!() ", #1"),

        Q!("    mov             " i!() ", # " CHUNKSIZE!()),

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
        // then corrects as needed. It also maintains the invariant ~ZF <=> odd(m_lo),
        // since it seems to reduce the dependent latency. Set that up first.

        Q!("    ands            " "xzr, " m_lo!() ", #1"),

        Q!(Label!("Lbignum_coprime_innerloop", 14) ":"),

        // At the start of the loop ~ZF <=> m_lo is odd; mask values accordingly
        // Set the flags for m_hi - [~ZF] * n_hi so we know to flip things.

        Q!("    csel            " t1!() ", " n_hi!() ", xzr, ne"),
        Q!("    csel            " t2!() ", " n_lo!() ", xzr, ne"),
        Q!("    csel            " c1!() ", " n_m!() ", xzr, ne"),
        Q!("    csel            " c2!() ", " n_n!() ", xzr, ne"),
        Q!("    ccmp            " m_hi!() ", " n_hi!() ", #0x2, ne"),

        // Compute subtractive updates, trivial in the case ZF <=> even(m_lo).

        Q!("    sub             " t1!() ", " m_hi!() ", " t1!()),
        Q!("    sub             " t2!() ", " m_lo!() ", " t2!()),

        // If the subtraction borrows, swap things appropriately, negating where
        // we've already subtracted so things are as if we actually swapped first.

        Q!("    csel            " n_hi!() ", " n_hi!() ", " m_hi!() ", cs"),
        Q!("    cneg            " t1!() ", " t1!() ", cc"),
        Q!("    csel            " n_lo!() ", " n_lo!() ", " m_lo!() ", cs"),
        Q!("    cneg            " m_lo!() ", " t2!() ", cc"),
        Q!("    csel            " n_m!() ", " n_m!() ", " m_m!() ", cs"),
        Q!("    csel            " n_n!() ", " n_n!() ", " m_n!() ", cs"),

        // Update and shift while setting oddness flag for next iteration
        // We look at bit 1 of t2 (m_lo before possible negation), which is
        // safe because it is even.

        Q!("    ands            " "xzr, " t2!() ", #2"),
        Q!("    add             " m_m!() ", " m_m!() ", " c1!()),
        Q!("    add             " m_n!() ", " m_n!() ", " c2!()),
        Q!("    lsr             " m_hi!() ", " t1!() ", #1"),
        Q!("    lsr             " m_lo!() ", " m_lo!() ", #1"),
        Q!("    add             " n_m!() ", " n_m!() ", " n_m!()),
        Q!("    add             " n_n!() ", " n_n!() ", " n_n!()),

        // Next iteration; don't disturb the flags since they are used at entry

        Q!("    sub             " i!() ", " i!() ", #1"),
        Q!("    cbnz            " i!() ", " Label!("Lbignum_coprime_innerloop", 14, Before)),

        // Now actually compute the updates to m and n corresponding to that matrix,
        // and correct the signs if they have gone negative. First we compute the
        // (k+1)-sized updates
        //
        //    c1::h1::m = m_m * m - m_n * n
        //    c2::h2::n = n_m * m - n_n * n
        //
        // then for each one, sign-correct and shift by CHUNKSIZE

        Q!("    mov             " h1!() ", xzr"),
        Q!("    mov             " h2!() ", xzr"),
        Q!("    mov             " c1!() ", xzr"),
        Q!("    mov             " c2!() ", xzr"),
        Q!("    mov             " i!() ", xzr"),
        Q!(Label!("Lbignum_coprime_crossloop", 15) ":"),
        Q!("    ldr             " t1!() ", [" m!() ", " i!() ", lsl #3]"),
        Q!("    ldr             " t2!() ", [" n!() ", " i!() ", lsl #3]"),

        Q!("    mul             " l1!() ", " m_m!() ", " t1!()),
        Q!("    mul             " l2!() ", " m_n!() ", " t2!()),
        Q!("    adds            " l1!() ", " l1!() ", " h1!()),
        Q!("    umulh           " h1!() ", " m_m!() ", " t1!()),
        Q!("    adc             " h1!() ", " h1!() ", xzr"),
        Q!("    umulh           " tt!() ", " m_n!() ", " t2!()),
        Q!("    sub             " c1!() ", " tt!() ", " c1!()),
        Q!("    subs            " l1!() ", " l1!() ", " l2!()),
        Q!("    str             " l1!() ", [" m!() ", " i!() ", lsl #3]"),
        Q!("    sbcs            " h1!() ", " h1!() ", " c1!()),
        Q!("    csetm           " c1!() ", cc"),

        Q!("    mul             " l1!() ", " n_m!() ", " t1!()),
        Q!("    mul             " l2!() ", " n_n!() ", " t2!()),
        Q!("    adds            " l1!() ", " l1!() ", " h2!()),
        Q!("    umulh           " h2!() ", " n_m!() ", " t1!()),
        Q!("    adc             " h2!() ", " h2!() ", xzr"),
        Q!("    umulh           " tt!() ", " n_n!() ", " t2!()),
        Q!("    sub             " c2!() ", " tt!() ", " c2!()),
        Q!("    subs            " l1!() ", " l1!() ", " l2!()),
        Q!("    str             " l1!() ", [" n!() ", " i!() ", lsl #3]"),
        Q!("    sbcs            " h2!() ", " h2!() ", " c2!()),
        Q!("    csetm           " c2!() ", cc"),

        Q!("    add             " i!() ", " i!() ", #1"),
        Q!("    cmp             " i!() ", " l!()),
        Q!("    bcc             " Label!("Lbignum_coprime_crossloop", 15, Before)),

        // Write back m optionally negated and shifted right CHUNKSIZE bits

        Q!("    adds            " "xzr, " c1!() ", " c1!()),

        Q!("    ldr             " l1!() ", [" m!() "]"),
        Q!("    mov             " i!() ", xzr"),
        Q!("    sub             " j!() ", " l!() ", #1"),
        Q!("    cbz             " j!() ", " Label!("Lbignum_coprime_negskip1", 16, After)),

        Q!(Label!("Lbignum_coprime_negloop1", 17) ":"),
        Q!("    add             " t1!() ", " i!() ", #8"),
        Q!("    ldr             " t2!() ", [" m!() ", " t1!() "]"),
        Q!("    extr            " l1!() ", " t2!() ", " l1!() ", # " CHUNKSIZE!()),
        Q!("    eor             " l1!() ", " l1!() ", " c1!()),
        Q!("    adcs            " l1!() ", " l1!() ", xzr"),
        Q!("    str             " l1!() ", [" m!() ", " i!() "]"),
        Q!("    mov             " l1!() ", " t2!()),
        Q!("    add             " i!() ", " i!() ", #8"),
        Q!("    sub             " j!() ", " j!() ", #1"),
        Q!("    cbnz            " j!() ", " Label!("Lbignum_coprime_negloop1", 17, Before)),
        Q!(Label!("Lbignum_coprime_negskip1", 16) ":"),
        Q!("    extr            " l1!() ", " h1!() ", " l1!() ", # " CHUNKSIZE!()),
        Q!("    eor             " l1!() ", " l1!() ", " c1!()),
        Q!("    adcs            " l1!() ", " l1!() ", xzr"),
        Q!("    str             " l1!() ", [" m!() ", " i!() "]"),

        // Write back n optionally negated and shifted right CHUNKSIZE bits

        Q!("    adds            " "xzr, " c2!() ", " c2!()),

        Q!("    ldr             " l1!() ", [" n!() "]"),
        Q!("    mov             " i!() ", xzr"),
        Q!("    sub             " j!() ", " l!() ", #1"),
        Q!("    cbz             " j!() ", " Label!("Lbignum_coprime_negskip2", 18, After)),
        Q!(Label!("Lbignum_coprime_negloop2", 19) ":"),
        Q!("    add             " t1!() ", " i!() ", #8"),
        Q!("    ldr             " t2!() ", [" n!() ", " t1!() "]"),
        Q!("    extr            " l1!() ", " t2!() ", " l1!() ", # " CHUNKSIZE!()),
        Q!("    eor             " l1!() ", " l1!() ", " c2!()),
        Q!("    adcs            " l1!() ", " l1!() ", xzr"),
        Q!("    str             " l1!() ", [" n!() ", " i!() "]"),
        Q!("    mov             " l1!() ", " t2!()),
        Q!("    add             " i!() ", " i!() ", #8"),
        Q!("    sub             " j!() ", " j!() ", #1"),
        Q!("    cbnz            " j!() ", " Label!("Lbignum_coprime_negloop2", 19, Before)),
        Q!(Label!("Lbignum_coprime_negskip2", 18) ":"),
        Q!("    extr            " l1!() ", " h2!() ", " l1!() ", # " CHUNKSIZE!()),
        Q!("    eor             " l1!() ", " l1!() ", " c2!()),
        Q!("    adcs            " l1!() ", " l1!() ", xzr"),
        Q!("    str             " l1!() ", [" n!() ", " i!() "]"),

        // End of main loop. We can stop if t' <= 0 since then m * n < 2^0, which
        // since n is odd (in the main cases where we had one or other input odd)
        // means that m = 0 and n is the final gcd. Moreover we do in fact need to
        // maintain strictly t > 0 in the main loop, or the computation of the
        // optimized digit bound l could collapse to 0.

        Q!("    subs            " t!() ", " t!() ", # " CHUNKSIZE!()),
        Q!("    bhi             " Label!("Lbignum_coprime_outerloop", 12, Before)),

        // Now compare n with 1 (OR of the XORs in t1)

        Q!("    ldr             " t1!() ", [" n!() "]"),
        Q!("    eor             " t1!() ", " t1!() ", #1"),
        Q!("    cmp             " k!() ", #1"),
        Q!("    beq             " Label!("Lbignum_coprime_finalcomb", 20, After)),
        Q!("    mov             " i!() ", #1"),
        Q!(Label!("Lbignum_coprime_compareloop", 21) ":"),
        Q!("    ldr             " t2!() ", [" n!() ", " i!() ", lsl #3]"),
        Q!("    orr             " t1!() ", " t1!() ", " t2!()),
        Q!("    add             " i!() ", " i!() ", #1"),
        Q!("    cmp             " i!() ", " k!()),
        Q!("    bcc             " Label!("Lbignum_coprime_compareloop", 21, Before)),

        // Now combine that with original oddness flag, which is still in x0

        Q!(Label!("Lbignum_coprime_finalcomb", 20) ":"),
        Q!("    cmp             " t1!() ", xzr"),
        Q!("    cset            " t1!() ", eq"),
        Q!("    and             " "x0, x0, " t1!()),

        Q!(Label!("Lbignum_coprime_end", 2) ":"),
        Q!("    ldp             " "x19, x20, [sp], #16"),

        inout("x0") x.len() => ret,
        inout("x1") x.as_ptr() => _,
        inout("x2") y.len() => _,
        inout("x3") y.as_ptr() => _,
        inout("x4") t.as_mut_ptr() => _,
        // clobbers
        out("x10") _,
        out("x11") _,
        out("x12") _,
        out("x13") _,
        out("x14") _,
        out("x15") _,
        out("x16") _,
        out("x17") _,
        out("x20") _,
        out("x5") _,
        out("x6") _,
        out("x7") _,
        out("x8") _,
        out("x9") _,
            )
    };
    ret > 0
}
