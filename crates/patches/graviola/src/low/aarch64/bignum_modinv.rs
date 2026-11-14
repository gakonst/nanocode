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
// Standard ARM ABI: X0 = k, X1 = z, X2 = a, X3 = b, X4 = t
// ----------------------------------------------------------------------------

// We get CHUNKSIZE bits per outer iteration, 64 minus a few for proxy errors

macro_rules! CHUNKSIZE {
    () => {
        "58"
    };
}

// Pervasive variables

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
macro_rules! b {
    () => {
        "x3"
    };
}
macro_rules! w {
    () => {
        "x4"
    };
}

// This one is recycled after initial copying in of a as outer loop counter

macro_rules! a {
    () => {
        "x2"
    };
}
macro_rules! t {
    () => {
        "x2"
    };
}

// Additional variables; later ones are currently rather high regs

macro_rules! l {
    () => {
        "x5"
    };
}

macro_rules! m {
    () => {
        "x21"
    };
}
macro_rules! n {
    () => {
        "x22"
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
        "x9"
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

// Negated modular inverse for Montgomery

macro_rules! v {
    () => {
        "x20"
    };
}

// Some more intuitive names for temp regs in initial word-level negmodinv.
// These just use t1 and t2 again, though carefully since t1 = initial b[0]

macro_rules! one {
    () => {
        Q!(t2!())
    };
}
macro_rules! e1 {
    () => {
        Q!(t2!())
    };
}
macro_rules! e2 {
    () => {
        Q!(t1!())
    };
}
macro_rules! e4 {
    () => {
        Q!(t2!())
    };
}
macro_rules! e8 {
    () => {
        Q!(t1!())
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

        // We make use of registers beyond the modifiable

        Q!("    stp             " "x19, x20, [sp, #-16] !"),
        Q!("    stp             " "x21, x22, [sp, #-16] !"),

        // If k = 0 then do nothing (this is out of scope anyway)

        Q!("    cbz             " k!() ", " Label!("Lbignum_modinv_end", 2, After)),

        // Set up the additional two buffers m and n beyond w in temp space

        Q!("    lsl             " i!() ", " k!() ", #3"),
        Q!("    add             " m!() ", " w!() ", " i!()),
        Q!("    add             " n!() ", " m!() ", " i!()),

        // Initialize the main buffers with their starting values:
        // m = a, n = b, w = b (to be tweaked to b - 1) and z = 0

        Q!("    mov             " i!() ", xzr"),
        Q!(Label!("Lbignum_modinv_copyloop", 3) ":"),
        Q!("    ldr             " t1!() ", [" a!() ", " i!() ", lsl #3]"),
        Q!("    ldr             " t2!() ", [" b!() ", " i!() ", lsl #3]"),
        Q!("    str             " t1!() ", [" m!() ", " i!() ", lsl #3]"),
        Q!("    str             " t2!() ", [" n!() ", " i!() ", lsl #3]"),
        Q!("    str             " t2!() ", [" w!() ", " i!() ", lsl #3]"),
        Q!("    str             " "xzr, [" z!() ", " i!() ", lsl #3]"),
        Q!("    add             " i!() ", " i!() ", #1"),
        Q!("    cmp             " i!() ", " k!()),
        Q!("    bcc             " Label!("Lbignum_modinv_copyloop", 3, Before)),

        // Tweak down w to b - 1 (this crude approach is safe as b needs to be odd
        // for it to be in scope). We have then established the congruence invariant:
        //
        //   a * w == -m (mod b)
        //   a * z == n (mod b)
        //
        // This, with the bound w <= b and z <= b, is maintained round the outer loop

        Q!("    ldr             " t1!() ", [" w!() "]"),
        Q!("    sub             " t2!() ", " t1!() ", #1"),
        Q!("    str             " t2!() ", [" w!() "]"),

        // Compute v = negated modular inverse of b mod 2^64, reusing t1 from above
        // This is used for Montgomery reduction operations each time round the loop

        Q!("    lsl             " v!() ", " t1!() ", #2"),
        Q!("    sub             " v!() ", " t1!() ", " v!()),
        Q!("    eor             " v!() ", " v!() ", #2"),
        Q!("    mov             " one!() ", #1"),
        Q!("    madd            " e1!() ", " t1!() ", " v!() ", " one!()),
        Q!("    mul             " e2!() ", " e1!() ", " e1!()),
        Q!("    madd            " v!() ", " e1!() ", " v!() ", " v!()),
        Q!("    mul             " e4!() ", " e2!() ", " e2!()),
        Q!("    madd            " v!() ", " e2!() ", " v!() ", " v!()),
        Q!("    mul             " e8!() ", " e4!() ", " e4!()),
        Q!("    madd            " v!() ", " e4!() ", " v!() ", " v!()),
        Q!("    madd            " v!() ", " e8!() ", " v!() ", " v!()),

        // Set up the outer loop count of 128 * k
        // The invariant is that m * n < 2^t at all times.

        Q!("    lsl             " t!() ", " k!() ", #7"),

        // Start of the main outer loop iterated t / CHUNKSIZE times

        Q!(Label!("Lbignum_modinv_outerloop", 4) ":"),

        // We need only bother with sharper l = min k (ceil(t/64)) digits
        // for the computations on m and n (but we still need k for w and z).
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
        Q!(Label!("Lbignum_modinv_toploop", 5) ":"),
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
        Q!("    bcc             " Label!("Lbignum_modinv_toploop", 5, Before)),

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

        Q!(Label!("Lbignum_modinv_innerloop", 6) ":"),

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
        Q!("    cbnz            " i!() ", " Label!("Lbignum_modinv_innerloop", 6, Before)),

        // Apply the update to w and z, using addition in this case, and also take
        // the chance to shift an additional 6 = 64-CHUNKSIZE bits to be ready for a
        // Montgomery multiplication. Because we know that m_m + m_n <= 2^58 and
        // w, z <= b < 2^{64k}, we know that both of these fit in k+1 words.
        // We do this before the m-n update to allow us to play with c1 and c2 here.
        //
        //    h1::w = 2^6 * (m_m * w + m_n * z)
        //    h2::z = 2^6 * (n_m * w + n_n * z)
        //
        // with c1 and c2 recording previous words for the shifting part

        Q!("    mov             " h1!() ", xzr"),
        Q!("    mov             " h2!() ", xzr"),
        Q!("    mov             " c1!() ", xzr"),
        Q!("    mov             " c2!() ", xzr"),

        Q!("    mov             " i!() ", xzr"),
        Q!(Label!("Lbignum_modinv_congloop", 7) ":"),
        Q!("    ldr             " t1!() ", [" w!() ", " i!() ", lsl #3]"),
        Q!("    ldr             " t2!() ", [" z!() ", " i!() ", lsl #3]"),

        Q!("    mul             " l1!() ", " m_m!() ", " t1!()),
        Q!("    mul             " l2!() ", " m_n!() ", " t2!()),
        Q!("    adds            " l1!() ", " l1!() ", " h1!()),
        Q!("    umulh           " h1!() ", " m_m!() ", " t1!()),
        Q!("    adc             " h1!() ", " h1!() ", xzr"),
        Q!("    adds            " l1!() ", " l1!() ", " l2!()),
        Q!("    extr            " c1!() ", " l1!() ", " c1!() ", # " CHUNKSIZE!()),
        Q!("    str             " c1!() ", [" w!() ", " i!() ", lsl #3]"),
        Q!("    mov             " c1!() ", " l1!()),
        Q!("    umulh           " l1!() ", " m_n!() ", " t2!()),
        Q!("    adc             " h1!() ", " h1!() ", " l1!()),

        Q!("    mul             " l1!() ", " n_m!() ", " t1!()),
        Q!("    mul             " l2!() ", " n_n!() ", " t2!()),
        Q!("    adds            " l1!() ", " l1!() ", " h2!()),
        Q!("    umulh           " h2!() ", " n_m!() ", " t1!()),
        Q!("    adc             " h2!() ", " h2!() ", xzr"),
        Q!("    adds            " l1!() ", " l1!() ", " l2!()),
        Q!("    extr            " c2!() ", " l1!() ", " c2!() ", # " CHUNKSIZE!()),
        Q!("    str             " c2!() ", [" z!() ", " i!() ", lsl #3]"),
        Q!("    mov             " c2!() ", " l1!()),
        Q!("    umulh           " l1!() ", " n_n!() ", " t2!()),
        Q!("    adc             " h2!() ", " h2!() ", " l1!()),

        Q!("    add             " i!() ", " i!() ", #1"),
        Q!("    cmp             " i!() ", " k!()),
        Q!("    bcc             " Label!("Lbignum_modinv_congloop", 7, Before)),

        Q!("    extr            " h1!() ", " h1!() ", " c1!() ", # " CHUNKSIZE!()),
        Q!("    extr            " h2!() ", " h2!() ", " c2!() ", # " CHUNKSIZE!()),

        // Do a Montgomery reduction of h1::w

        Q!("    ldr             " t1!() ", [" w!() "]"),
        Q!("    mul             " c1!() ", " t1!() ", " v!()),
        Q!("    ldr             " t2!() ", [" b!() "]"),
        Q!("    mul             " l1!() ", " c1!() ", " t2!()),
        Q!("    umulh           " l2!() ", " c1!() ", " t2!()),
        Q!("    adds            " t1!() ", " t1!() ", " l1!()),

        Q!("    mov             " i!() ", #1"),
        Q!("    sub             " t1!() ", " k!() ", #1"),
        Q!("    cbz             " t1!() ", " Label!("Lbignum_modinv_wmontend", 8, After)),
        Q!(Label!("Lbignum_modinv_wmontloop", 9) ":"),
        Q!("    ldr             " t1!() ", [" b!() ", " i!() ", lsl #3]"),
        Q!("    ldr             " t2!() ", [" w!() ", " i!() ", lsl #3]"),
        Q!("    mul             " l1!() ", " c1!() ", " t1!()),
        Q!("    adcs            " t2!() ", " t2!() ", " l2!()),
        Q!("    umulh           " l2!() ", " c1!() ", " t1!()),
        Q!("    adc             " l2!() ", " l2!() ", xzr"),
        Q!("    adds            " t2!() ", " t2!() ", " l1!()),
        Q!("    sub             " l1!() ", " i!() ", #1"),
        Q!("    str             " t2!() ", [" w!() ", " l1!() ", lsl #3]"),
        Q!("    add             " i!() ", " i!() ", #1"),
        Q!("    sub             " t1!() ", " i!() ", " k!()),
        Q!("    cbnz            " t1!() ", " Label!("Lbignum_modinv_wmontloop", 9, Before)),
        Q!(Label!("Lbignum_modinv_wmontend", 8) ":"),
        Q!("    adcs            " l2!() ", " l2!() ", " h1!()),
        Q!("    adc             " h1!() ", xzr, xzr"),
        Q!("    sub             " l1!() ", " i!() ", #1"),
        Q!("    str             " l2!() ", [" w!() ", " l1!() ", lsl #3]"),

        Q!("    subs            " i!() ", xzr, xzr"),
        Q!(Label!("Lbignum_modinv_wcmploop", 12) ":"),
        Q!("    ldr             " t1!() ", [" w!() ", " i!() ", lsl #3]"),
        Q!("    ldr             " t2!() ", [" b!() ", " i!() ", lsl #3]"),
        Q!("    sbcs            " "xzr, " t1!() ", " t2!()),
        Q!("    add             " i!() ", " i!() ", #1"),
        Q!("    sub             " t1!() ", " i!() ", " k!()),
        Q!("    cbnz            " t1!() ", " Label!("Lbignum_modinv_wcmploop", 12, Before)),

        Q!("    sbcs            " "xzr, " h1!() ", xzr"),
        Q!("    csetm           " h1!() ", cs"),

        Q!("    subs            " i!() ", xzr, xzr"),
        Q!(Label!("Lbignum_modinv_wcorrloop", 13) ":"),
        Q!("    ldr             " t1!() ", [" w!() ", " i!() ", lsl #3]"),
        Q!("    ldr             " t2!() ", [" b!() ", " i!() ", lsl #3]"),
        Q!("    and             " t2!() ", " t2!() ", " h1!()),
        Q!("    sbcs            " t1!() ", " t1!() ", " t2!()),
        Q!("    str             " t1!() ", [" w!() ", " i!() ", lsl #3]"),
        Q!("    add             " i!() ", " i!() ", #1"),
        Q!("    sub             " t1!() ", " i!() ", " k!()),
        Q!("    cbnz            " t1!() ", " Label!("Lbignum_modinv_wcorrloop", 13, Before)),

        // Do a Montgomery reduction of h2::z

        Q!("    ldr             " t1!() ", [" z!() "]"),
        Q!("    mul             " c1!() ", " t1!() ", " v!()),
        Q!("    ldr             " t2!() ", [" b!() "]"),
        Q!("    mul             " l1!() ", " c1!() ", " t2!()),
        Q!("    umulh           " l2!() ", " c1!() ", " t2!()),
        Q!("    adds            " t1!() ", " t1!() ", " l1!()),

        Q!("    mov             " i!() ", #1"),
        Q!("    sub             " t1!() ", " k!() ", #1"),
        Q!("    cbz             " t1!() ", " Label!("Lbignum_modinv_zmontend", 14, After)),
        Q!(Label!("Lbignum_modinv_zmontloop", 15) ":"),
        Q!("    ldr             " t1!() ", [" b!() ", " i!() ", lsl #3]"),
        Q!("    ldr             " t2!() ", [" z!() ", " i!() ", lsl #3]"),
        Q!("    mul             " l1!() ", " c1!() ", " t1!()),
        Q!("    adcs            " t2!() ", " t2!() ", " l2!()),
        Q!("    umulh           " l2!() ", " c1!() ", " t1!()),
        Q!("    adc             " l2!() ", " l2!() ", xzr"),
        Q!("    adds            " t2!() ", " t2!() ", " l1!()),
        Q!("    sub             " l1!() ", " i!() ", #1"),
        Q!("    str             " t2!() ", [" z!() ", " l1!() ", lsl #3]"),
        Q!("    add             " i!() ", " i!() ", #1"),
        Q!("    sub             " t1!() ", " i!() ", " k!()),
        Q!("    cbnz            " t1!() ", " Label!("Lbignum_modinv_zmontloop", 15, Before)),
        Q!(Label!("Lbignum_modinv_zmontend", 14) ":"),
        Q!("    adcs            " l2!() ", " l2!() ", " h2!()),
        Q!("    adc             " h2!() ", xzr, xzr"),
        Q!("    sub             " l1!() ", " i!() ", #1"),
        Q!("    str             " l2!() ", [" z!() ", " l1!() ", lsl #3]"),

        Q!("    subs            " i!() ", xzr, xzr"),
        Q!(Label!("Lbignum_modinv_zcmploop", 16) ":"),
        Q!("    ldr             " t1!() ", [" z!() ", " i!() ", lsl #3]"),
        Q!("    ldr             " t2!() ", [" b!() ", " i!() ", lsl #3]"),
        Q!("    sbcs            " "xzr, " t1!() ", " t2!()),
        Q!("    add             " i!() ", " i!() ", #1"),
        Q!("    sub             " t1!() ", " i!() ", " k!()),
        Q!("    cbnz            " t1!() ", " Label!("Lbignum_modinv_zcmploop", 16, Before)),

        Q!("    sbcs            " "xzr, " h2!() ", xzr"),
        Q!("    csetm           " h2!() ", cs"),

        Q!("    subs            " i!() ", xzr, xzr"),
        Q!(Label!("Lbignum_modinv_zcorrloop", 17) ":"),
        Q!("    ldr             " t1!() ", [" z!() ", " i!() ", lsl #3]"),
        Q!("    ldr             " t2!() ", [" b!() ", " i!() ", lsl #3]"),
        Q!("    and             " t2!() ", " t2!() ", " h2!()),
        Q!("    sbcs            " t1!() ", " t1!() ", " t2!()),
        Q!("    str             " t1!() ", [" z!() ", " i!() ", lsl #3]"),
        Q!("    add             " i!() ", " i!() ", #1"),
        Q!("    sub             " t1!() ", " i!() ", " k!()),
        Q!("    cbnz            " t1!() ", " Label!("Lbignum_modinv_zcorrloop", 17, Before)),

        // Now actually compute the updates to m and n corresponding to the matrix,
        // and correct the signs if they have gone negative. First we compute the
        // (k+1)-sized updates with the following invariant (here c1 and c2 are in
        // fact carry bitmasks, either 0 or -1):
        //
        //    c1::h1::m = m_m * m - m_n * n
        //    c2::h2::n = n_m * m - n_n * n

        Q!("    mov             " h1!() ", xzr"),
        Q!("    mov             " h2!() ", xzr"),
        Q!("    mov             " c1!() ", xzr"),
        Q!("    mov             " c2!() ", xzr"),
        Q!("    mov             " i!() ", xzr"),
        Q!(Label!("Lbignum_modinv_crossloop", 18) ":"),
        Q!("    ldr             " t1!() ", [" m!() ", " i!() ", lsl #3]"),
        Q!("    ldr             " t2!() ", [" n!() ", " i!() ", lsl #3]"),

        Q!("    mul             " l1!() ", " m_m!() ", " t1!()),
        Q!("    mul             " l2!() ", " m_n!() ", " t2!()),
        Q!("    adds            " l1!() ", " l1!() ", " h1!()),
        Q!("    umulh           " h1!() ", " m_m!() ", " t1!()),
        Q!("    adc             " h1!() ", " h1!() ", xzr"),
        Q!("    subs            " l1!() ", " l1!() ", " l2!()),
        Q!("    str             " l1!() ", [" m!() ", " i!() ", lsl #3]"),
        Q!("    umulh           " l1!() ", " m_n!() ", " t2!()),
        Q!("    sub             " c1!() ", " l1!() ", " c1!()),
        Q!("    sbcs            " h1!() ", " h1!() ", " c1!()),
        Q!("    csetm           " c1!() ", cc"),

        Q!("    mul             " l1!() ", " n_m!() ", " t1!()),
        Q!("    mul             " l2!() ", " n_n!() ", " t2!()),
        Q!("    adds            " l1!() ", " l1!() ", " h2!()),
        Q!("    umulh           " h2!() ", " n_m!() ", " t1!()),
        Q!("    adc             " h2!() ", " h2!() ", xzr"),
        Q!("    subs            " l1!() ", " l1!() ", " l2!()),
        Q!("    str             " l1!() ", [" n!() ", " i!() ", lsl #3]"),
        Q!("    umulh           " l1!() ", " n_n!() ", " t2!()),
        Q!("    sub             " c2!() ", " l1!() ", " c2!()),
        Q!("    sbcs            " h2!() ", " h2!() ", " c2!()),
        Q!("    csetm           " c2!() ", cc"),

        Q!("    add             " i!() ", " i!() ", #1"),
        Q!("    cmp             " i!() ", " l!()),
        Q!("    bcc             " Label!("Lbignum_modinv_crossloop", 18, Before)),

        // Write back m optionally negated and shifted right CHUNKSIZE bits

        Q!("    adds            " "xzr, " c1!() ", " c1!()),

        Q!("    ldr             " l1!() ", [" m!() "]"),
        Q!("    mov             " i!() ", xzr"),
        Q!("    sub             " j!() ", " l!() ", #1"),
        Q!("    cbz             " j!() ", " Label!("Lbignum_modinv_negskip1", 19, After)),

        Q!(Label!("Lbignum_modinv_negloop1", 20) ":"),
        Q!("    add             " t1!() ", " i!() ", #8"),
        Q!("    ldr             " t2!() ", [" m!() ", " t1!() "]"),
        Q!("    extr            " l1!() ", " t2!() ", " l1!() ", # " CHUNKSIZE!()),
        Q!("    eor             " l1!() ", " l1!() ", " c1!()),
        Q!("    adcs            " l1!() ", " l1!() ", xzr"),
        Q!("    str             " l1!() ", [" m!() ", " i!() "]"),
        Q!("    mov             " l1!() ", " t2!()),
        Q!("    add             " i!() ", " i!() ", #8"),
        Q!("    sub             " j!() ", " j!() ", #1"),
        Q!("    cbnz            " j!() ", " Label!("Lbignum_modinv_negloop1", 20, Before)),
        Q!(Label!("Lbignum_modinv_negskip1", 19) ":"),
        Q!("    extr            " l1!() ", " h1!() ", " l1!() ", # " CHUNKSIZE!()),
        Q!("    eor             " l1!() ", " l1!() ", " c1!()),
        Q!("    adcs            " l1!() ", " l1!() ", xzr"),
        Q!("    str             " l1!() ", [" m!() ", " i!() "]"),

        // Write back n optionally negated and shifted right CHUNKSIZE bits

        Q!("    adds            " "xzr, " c2!() ", " c2!()),

        Q!("    ldr             " l1!() ", [" n!() "]"),
        Q!("    mov             " i!() ", xzr"),
        Q!("    sub             " j!() ", " l!() ", #1"),
        Q!("    cbz             " j!() ", " Label!("Lbignum_modinv_negskip2", 21, After)),
        Q!(Label!("Lbignum_modinv_negloop2", 22) ":"),
        Q!("    add             " t1!() ", " i!() ", #8"),
        Q!("    ldr             " t2!() ", [" n!() ", " t1!() "]"),
        Q!("    extr            " l1!() ", " t2!() ", " l1!() ", # " CHUNKSIZE!()),
        Q!("    eor             " l1!() ", " l1!() ", " c2!()),
        Q!("    adcs            " l1!() ", " l1!() ", xzr"),
        Q!("    str             " l1!() ", [" n!() ", " i!() "]"),
        Q!("    mov             " l1!() ", " t2!()),
        Q!("    add             " i!() ", " i!() ", #8"),
        Q!("    sub             " j!() ", " j!() ", #1"),
        Q!("    cbnz            " j!() ", " Label!("Lbignum_modinv_negloop2", 22, Before)),
        Q!(Label!("Lbignum_modinv_negskip2", 21) ":"),
        Q!("    extr            " l1!() ", " h2!() ", " l1!() ", # " CHUNKSIZE!()),
        Q!("    eor             " l1!() ", " l1!() ", " c2!()),
        Q!("    adcs            " l1!() ", " l1!() ", xzr"),
        Q!("    str             " l1!() ", [" n!() ", " i!() "]"),

        // Finally, use the signs c1 and c2 to do optional modular negations of
        // w and z respectively, flipping c2 to make signs work. We don't make
        // any checks for zero values, but we certainly retain w <= b and z <= b.
        // This is enough for the Montgomery step in the next iteration to give
        // strict reduction w < b amd z < b, and anyway when we terminate we
        // could not have z = b since it violates the coprimality assumption for
        // in-scope cases.

        Q!("    mov             " i!() ", xzr"),
        Q!("    adds            " "xzr, " c1!() ", " c1!()),
        Q!(Label!("Lbignum_modinv_wfliploop", 23) ":"),
        Q!("    ldr             " t1!() ", [" b!() ", " i!() ", lsl #3]"),
        Q!("    ldr             " t2!() ", [" w!() ", " i!() ", lsl #3]"),
        Q!("    and             " t1!() ", " t1!() ", " c1!()),
        Q!("    eor             " t2!() ", " t2!() ", " c1!()),
        Q!("    adcs            " t1!() ", " t1!() ", " t2!()),
        Q!("    str             " t1!() ", [" w!() ", " i!() ", lsl #3]"),
        Q!("    add             " i!() ", " i!() ", #1"),
        Q!("    sub             " t1!() ", " i!() ", " k!()),
        Q!("    cbnz            " t1!() ", " Label!("Lbignum_modinv_wfliploop", 23, Before)),

        Q!("    mvn             " c2!() ", " c2!()),

        Q!("    mov             " i!() ", xzr"),
        Q!("    adds            " "xzr, " c2!() ", " c2!()),
        Q!(Label!("Lbignum_modinv_zfliploop", 24) ":"),
        Q!("    ldr             " t1!() ", [" b!() ", " i!() ", lsl #3]"),
        Q!("    ldr             " t2!() ", [" z!() ", " i!() ", lsl #3]"),
        Q!("    and             " t1!() ", " t1!() ", " c2!()),
        Q!("    eor             " t2!() ", " t2!() ", " c2!()),
        Q!("    adcs            " t1!() ", " t1!() ", " t2!()),
        Q!("    str             " t1!() ", [" z!() ", " i!() ", lsl #3]"),
        Q!("    add             " i!() ", " i!() ", #1"),
        Q!("    sub             " t1!() ", " i!() ", " k!()),
        Q!("    cbnz            " t1!() ", " Label!("Lbignum_modinv_zfliploop", 24, Before)),

        // End of main loop. We can stop if t' <= 0 since then m * n < 2^0, which
        // since n is odd and m and n are coprime (in the in-scope cases) means
        // m = 0, n = 1 and hence from the congruence invariant a * z == 1 (mod b).
        // Moreover we do in fact need to maintain strictly t > 0 in the main loop,
        // or the computation of the optimized digit bound l could collapse to 0.

        Q!("    subs            " t!() ", " t!() ", # " CHUNKSIZE!()),
        Q!("    bhi             " Label!("Lbignum_modinv_outerloop", 4, Before)),

        Q!(Label!("Lbignum_modinv_end", 2) ":"),
        Q!("    ldp             " "x21, x22, [sp], #16"),
        Q!("    ldp             " "x19, x20, [sp], #16"),

        inout("x0") b.len() => _,
        inout("x1") z.as_mut_ptr() => _,
        inout("x2") a.as_ptr() => _,
        inout("x3") b.as_ptr() => _,
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
        out("x21") _,
        out("x22") _,
        out("x5") _,
        out("x6") _,
        out("x7") _,
        out("x8") _,
        out("x9") _,
            )
    };
}
