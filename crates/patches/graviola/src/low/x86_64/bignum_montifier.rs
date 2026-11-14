// generated source. do not edit.
#![allow(non_upper_case_globals, unused_macros, unused_imports)]
use crate::low::macros::*;

// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

// ----------------------------------------------------------------------------
// Compute "montification" constant z := 2^{128k} mod m
// Input m[k]; output z[k]; temporary buffer t[>=k]
//
//    extern void bignum_montifier(uint64_t k, uint64_t *z, const uint64_t *m,
//                                 uint64_t *t);
//
// The last argument points to a temporary buffer t that should have size >= k.
// This is called "montifier" because given any other k-digit number x,
// whether or not it's reduced modulo m, it can be mapped to its Montgomery
// representation (2^{64k} * x) mod m just by Montgomery multiplication by z.
//
// Standard x86-64 ABI: RDI = k, RSI = z, RDX = m, RCX = t
// Microsoft x64 ABI:   RCX = k, RDX = z, R8 = m, R9 = t
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

// These two inputs get moved to different places since RCX and RDX are special

macro_rules! m {
    () => {
        "r12"
    };
}
macro_rules! t {
    () => {
        "r13"
    };
}

// Other variables

macro_rules! i {
    () => {
        "rbx"
    };
}
// Modular inverse; aliased to i, but we never use them together
macro_rules! w {
    () => {
        "rbx"
    };
}
macro_rules! j {
    () => {
        "rbp"
    };
}
// Matters that this is RAX for special use in multiplies
macro_rules! a {
    () => {
        "rax"
    };
}
// Matters that this is RDX for special use in multiplies
macro_rules! d {
    () => {
        "rdx"
    };
}
// Matters that this is RCX as CL=lo(c) is assumed in shifts
macro_rules! c {
    () => {
        "rcx"
    };
}
macro_rules! h {
    () => {
        "r11"
    };
}
macro_rules! l {
    () => {
        "r10"
    };
}
macro_rules! b {
    () => {
        "r9"
    };
}
macro_rules! n {
    () => {
        "r8"
    };
}

// Some aliases for the values b and n

macro_rules! q {
    () => {
        "r8"
    };
}
macro_rules! r {
    () => {
        "r9"
    };
}

macro_rules! ashort {
    () => {
        "eax"
    };
}
macro_rules! ishort {
    () => {
        "ebx"
    };
}
macro_rules! jshort {
    () => {
        "ebp"
    };
}
macro_rules! qshort {
    () => {
        "r8d"
    };
}

/// Compute "montification" constant z := 2^{128k} mod m
///
/// Input m[k]; output z[k]; temporary buffer t[>=k]
///
/// The last argument points to a temporary buffer t that should have size >= k.
/// This is called "montifier" because given any other k-digit number x,
/// whether or not it's reduced modulo m, it can be mapped to its Montgomery
/// representation (2^{64k} * x) mod m just by Montgomery multiplication by z.
pub(crate) fn bignum_montifier(z: &mut [u64], m: &[u64], t: &mut [u64]) {
    debug_assert!(z.len() == m.len());
    debug_assert!(z.len() <= t.len());
    // SAFETY: inline assembly. see [crate::low::inline_assembly_safety] for safety info.
    unsafe {
        core::arch::asm!(

        Q!("    endbr64         " ),

        // Save some additional registers for use, copy args out of RCX and RDX

        Q!("    push            " "rbp"),
        Q!("    push            " "rbx"),
        Q!("    push            " "r12"),
        Q!("    push            " "r13"),

        Q!("    mov             " m!() ", rdx"),
        Q!("    mov             " t!() ", rcx"),

        // If k = 0 the whole operation is trivial

        Q!("    test            " k!() ", " k!()),
        Q!("    jz              " Label!("Lbignum_montifier_end", 2, After)),

        // Copy the input m into the temporary buffer t. The temporary register
        // c matters since we want it to hold the highest digit, ready for the
        // normalization phase.

        Q!("    xor             " i!() ", " i!()),
        Q!(Label!("Lbignum_montifier_copyinloop", 3) ":"),
        Q!("    mov             " c!() ", [" m!() "+ 8 * " i!() "]"),
        Q!("    mov             " "[" t!() "+ 8 * " i!() "], " c!()),
        Q!("    inc             " i!()),
        Q!("    cmp             " i!() ", " k!()),
        Q!("    jc              " Label!("Lbignum_montifier_copyinloop", 3, Before)),

        // Do a rather stupid but constant-time digit normalization, conditionally
        // shifting left (k-1) times based on whether the top word is zero.
        // With careful binary striding this could be O(k*log(k)) instead of O(k^2)
        // while still retaining the constant-time style.
        // The "neg c" sets the zeroness predicate (~CF) for the entire inner loop

        Q!("    mov             " i!() ", " k!()),
        Q!("    dec             " i!()),
        Q!("    jz              " Label!("Lbignum_montifier_normalized", 4, After)),
        Q!(Label!("Lbignum_montifier_normloop", 5) ":"),
        Q!("    xor             " j!() ", " j!()),
        Q!("    mov             " h!() ", " k!()),
        Q!("    neg             " c!()),
        Q!("    mov             " ashort!() ", 0"),
        Q!(Label!("Lbignum_montifier_shufloop", 6) ":"),
        Q!("    mov             " c!() ", " a!()),
        Q!("    mov             " a!() ", [" t!() "+ 8 * " j!() "]"),
        Q!("    cmovc           " c!() ", " a!()),
        Q!("    mov             " "[" t!() "+ 8 * " j!() "], " c!()),
        Q!("    inc             " j!()),
        Q!("    dec             " h!()),
        Q!("    jnz             " Label!("Lbignum_montifier_shufloop", 6, Before)),
        Q!("    dec             " i!()),
        Q!("    jnz             " Label!("Lbignum_montifier_normloop", 5, Before)),

        // We now have the top digit nonzero, assuming the input was nonzero,
        // and as per the invariant of the loop above, c holds that digit. So
        // now just count c's leading zeros and shift t bitwise that many bits.
        // Note that we don't care about the result of bsr for zero inputs so
        // the simple xor-ing with 63 is safe.

        Q!(Label!("Lbignum_montifier_normalized", 4) ":"),

        Q!("    bsr             " c!() ", " c!()),
        Q!("    xor             " c!() ", 63"),

        Q!("    xor             " b!() ", " b!()),
        Q!("    xor             " i!() ", " i!()),
        Q!(Label!("Lbignum_montifier_bitloop", 7) ":"),
        Q!("    mov             " a!() ", [" t!() "+ 8 * " i!() "]"),
        Q!("    mov             " j!() ", " a!()),
        Q!("    shld            " a!() ", " b!() ", cl"),
        Q!("    mov             " "[" t!() "+ 8 * " i!() "], " a!()),
        Q!("    mov             " b!() ", " j!()),
        Q!("    inc             " i!()),
        Q!("    cmp             " i!() ", " k!()),
        Q!("    jc              " Label!("Lbignum_montifier_bitloop", 7, Before)),

        // Let h be the high word of n, which in all the in-scope cases is >= 2^63.
        // Now successively form q = 2^i div h and r = 2^i mod h as i goes from
        // 64 to 126. We avoid just using division out of constant-time concerns
        // (at the least we would need to fix up h = 0 for out-of-scope inputs) and
        // don't bother with Newton-Raphson, since this stupid simple loop doesn't
        // contribute much of the overall runtime at typical sizes.

        Q!("    mov             " h!() ", [" t!() "+ 8 * " k!() "-8]"),
        Q!("    mov             " qshort!() ", 1"),
        Q!("    mov             " r!() ", " h!()),
        Q!("    neg             " r!()),
        Q!("    mov             " ishort!() ", 62"),
        Q!(Label!("Lbignum_montifier_estloop", 8) ":"),

        Q!("    add             " q!() ", " q!()),
        Q!("    mov             " a!() ", " h!()),
        Q!("    sub             " a!() ", " r!()),
        Q!("    cmp             " r!() ", " a!()),
        Q!("    sbb             " a!() ", " a!()),
        Q!("    not             " a!()),
        Q!("    sub             " q!() ", " a!()),
        Q!("    add             " r!() ", " r!()),
        Q!("    and             " a!() ", " h!()),
        Q!("    sub             " r!() ", " a!()),
        Q!("    dec             " i!()),
        Q!("    jnz             " Label!("Lbignum_montifier_estloop", 8, Before)),

        // Strictly speaking the above loop doesn't quite give the true remainder
        // and quotient in the special case r = h = 2^63, so fix it up. We get
        // q = 2^63 - 1 and r = 2^63 and really want q = 2^63 and r = 0. This is
        // supererogatory, because the main property of q used below still holds
        // in this case unless the initial m = 1, and then anyway the overall
        // specification (congruence modulo m) holds degenerately. But it seems
        // nicer to get a "true" quotient and remainder.

        Q!("    inc             " r!()),
        Q!("    cmp             " h!() ", " r!()),
        Q!("    adc             " q!() ", 0"),

        // So now we have q and r with 2^126 = q * h + r (imagining r = 0 in the
        // fixed-up case above: note that we never actually use the computed
        // value of r below and so didn't adjust it). And we can assume the ranges
        // q <= 2^63 and r < h < 2^64.
        //
        // The idea is to use q as a first quotient estimate for a remainder
        // of 2^{p+62} mod n, where p = 64 * k. We have, splitting n into the
        // high and low parts h and l:
        //
        // 2^{p+62} - q * n = 2^{p+62} - q * (2^{p-64} * h + l)
        //                  = 2^{p+62} - (2^{p-64} * (q * h) + q * l)
        //                  = 2^{p+62} - 2^{p-64} * (2^126 - r) - q * l
        //                  = 2^{p-64} * r - q * l
        //
        // Note that 2^{p-64} * r < 2^{p-64} * h <= n
        // and also  q * l < 2^63 * 2^{p-64} = 2^{p-1} <= n
        // so |diff| = |2^{p-64} * r - q * l| < n.
        //
        // If in fact diff >= 0 then it is already 2^{p+62} mod n.
        // otherwise diff + n is the right answer.
        //
        // To (maybe?) make the computation slightly easier we actually flip
        // the sign and compute d = q * n - 2^{p+62}. Then the answer is either
        // -d (when negative) or n - d; in either case we effectively negate d.
        // This negating tweak in fact spoils the result for cases where
        // 2^{p+62} mod n = 0, when we get n instead. However the only case
        // where this can happen is m = 1, when the whole spec holds trivially,
        // and actually the remainder of the logic below works anyway since
        // the latter part of the code only needs a congruence for the k-digit
        // result, not strict modular reduction (the doublings will maintain
        // the non-strict inequality).

        Q!("    xor             " c!() ", " c!()),
        Q!("    xor             " i!() ", " i!()),
        Q!(Label!("Lbignum_montifier_mulloop", 9) ":"),
        Q!("    mov             " a!() ", [" t!() "+ 8 * " i!() "]"),
        Q!("    mul             " q!()),
        Q!("    add             " a!() ", " c!()),
        Q!("    adc             " d!() ", 0"),
        Q!("    mov             " "[" z!() "+ 8 * " i!() "], " a!()),
        Q!("    mov             " c!() ", " d!()),
        Q!("    inc             " i!()),
        Q!("    cmp             " i!() ", " k!()),
        Q!("    jc              " Label!("Lbignum_montifier_mulloop", 9, Before)),

        // Now c is the high word of the product, so subtract 2^62
        // and then turn it into a bitmask in q = h

        Q!("    mov             " a!() ", 0x4000000000000000"),
        Q!("    sub             " c!() ", " a!()),
        Q!("    sbb             " q!() ", " q!()),
        Q!("    not             " q!()),

        // Now do [c] * n - d for our final answer

        Q!("    xor             " c!() ", " c!()),
        Q!("    xor             " i!() ", " i!()),
        Q!(Label!("Lbignum_montifier_remloop", 12) ":"),
        Q!("    mov             " a!() ", [" t!() "+ 8 * " i!() "]"),
        Q!("    and             " a!() ", " q!()),
        Q!("    neg             " c!()),
        Q!("    sbb             " a!() ", [" z!() "+ 8 * " i!() "]"),
        Q!("    sbb             " c!() ", " c!()),
        Q!("    mov             " "[" z!() "+ 8 * " i!() "], " a!()),
        Q!("    inc             " i!()),
        Q!("    cmp             " i!() ", " k!()),
        Q!("    jc              " Label!("Lbignum_montifier_remloop", 12, Before)),

        // Now still need to do a couple of modular doublings to get us all the
        // way up to 2^{p+64} == r from initial 2^{p+62} == r (mod n).

        Q!("    xor             " c!() ", " c!()),
        Q!("    xor             " j!() ", " j!()),
        Q!("    xor             " b!() ", " b!()),
        Q!(Label!("Lbignum_montifier_dubloop1", 13) ":"),
        Q!("    mov             " a!() ", [" z!() "+ 8 * " j!() "]"),
        Q!("    shrd            " c!() ", " a!() ", 63"),
        Q!("    neg             " b!()),
        Q!("    sbb             " c!() ", [" t!() "+ 8 * " j!() "]"),
        Q!("    sbb             " b!() ", " b!()),
        Q!("    mov             " "[" z!() "+ 8 * " j!() "], " c!()),
        Q!("    mov             " c!() ", " a!()),
        Q!("    inc             " j!()),
        Q!("    cmp             " j!() ", " k!()),
        Q!("    jc              " Label!("Lbignum_montifier_dubloop1", 13, Before)),
        Q!("    shr             " c!() ", 63"),
        Q!("    add             " c!() ", " b!()),
        Q!("    xor             " j!() ", " j!()),
        Q!("    xor             " b!() ", " b!()),
        Q!(Label!("Lbignum_montifier_corrloop1", 14) ":"),
        Q!("    mov             " a!() ", [" t!() "+ 8 * " j!() "]"),
        Q!("    and             " a!() ", " c!()),
        Q!("    neg             " b!()),
        Q!("    adc             " a!() ", [" z!() "+ 8 * " j!() "]"),
        Q!("    sbb             " b!() ", " b!()),
        Q!("    mov             " "[" z!() "+ 8 * " j!() "], " a!()),
        Q!("    inc             " j!()),
        Q!("    cmp             " j!() ", " k!()),
        Q!("    jc              " Label!("Lbignum_montifier_corrloop1", 14, Before)),

        // This is not exactly the same: we also copy output to t giving the
        // initialization t_1 = r == 2^{p+64} mod n for the main loop next.

        Q!("    xor             " c!() ", " c!()),
        Q!("    xor             " j!() ", " j!()),
        Q!("    xor             " b!() ", " b!()),
        Q!(Label!("Lbignum_montifier_dubloop2", 15) ":"),
        Q!("    mov             " a!() ", [" z!() "+ 8 * " j!() "]"),
        Q!("    shrd            " c!() ", " a!() ", 63"),
        Q!("    neg             " b!()),
        Q!("    sbb             " c!() ", [" t!() "+ 8 * " j!() "]"),
        Q!("    sbb             " b!() ", " b!()),
        Q!("    mov             " "[" z!() "+ 8 * " j!() "], " c!()),
        Q!("    mov             " c!() ", " a!()),
        Q!("    inc             " j!()),
        Q!("    cmp             " j!() ", " k!()),
        Q!("    jc              " Label!("Lbignum_montifier_dubloop2", 15, Before)),
        Q!("    shr             " c!() ", 63"),
        Q!("    add             " c!() ", " b!()),
        Q!("    xor             " j!() ", " j!()),
        Q!("    xor             " b!() ", " b!()),
        Q!(Label!("Lbignum_montifier_corrloop2", 16) ":"),
        Q!("    mov             " a!() ", [" t!() "+ 8 * " j!() "]"),
        Q!("    and             " a!() ", " c!()),
        Q!("    neg             " b!()),
        Q!("    adc             " a!() ", [" z!() "+ 8 * " j!() "]"),
        Q!("    sbb             " b!() ", " b!()),
        Q!("    mov             " "[" z!() "+ 8 * " j!() "], " a!()),
        Q!("    mov             " "[" t!() "+ 8 * " j!() "], " a!()),
        Q!("    inc             " j!()),
        Q!("    cmp             " j!() ", " k!()),
        Q!("    jc              " Label!("Lbignum_montifier_corrloop2", 16, Before)),

        // We then successively generate (k+1)-digit values satisfying
        // t_i == 2^{p+64*i} mod n, each of which is stored in h::t. Finish
        // initialization by zeroing h initially

        Q!("    xor             " h!() ", " h!()),

        // Then if t_i = 2^{p} * h + l
        // we have t_{i+1} == 2^64 * t_i
        //         = (2^{p+64} * h) + (2^64 * l)
        //        == r * h + l<<64
        // Do this 2*k more times so we end up == 2^{192*k+64}, one more than we want
        //
        // Writing B = 2^{64k}, the possible correction of adding r, which for
        // a (k+1)-digit result is equivalent to subtracting q = 2^{64*(k+1)} - r
        // would give the overall worst-case value minus q of
        // [ B * (B^k - 1) + (B - 1) * r ] - [B^{k+1} - r]
        // = B * (r - 1) < B^{k+1} so we keep inside k+1 digits as required.
        //
        // This implementation makes the shift implicit by starting b with the
        // "previous" digit (initially 0) to offset things by 1.

        Q!("    lea             " i!() ", [" k!() "+ " k!() "]"),
        Q!(Label!("Lbignum_montifier_modloop", 17) ":"),
        Q!("    xor             " b!() ", " b!()),
        Q!("    mov             " n!() ", " k!()),
        Q!("    xor             " j!() ", " j!()),
        Q!("    xor             " c!() ", " c!()),
        Q!(Label!("Lbignum_montifier_cmaloop", 18) ":"),
        Q!("    adc             " c!() ", " b!()),
        Q!("    sbb             " l!() ", " l!()),
        Q!("    mov             " a!() ", [" z!() "+ 8 * " j!() "]"),
        Q!("    mul             " h!()),
        Q!("    sub             " d!() ", " l!()),
        Q!("    add             " a!() ", " c!()),
        Q!("    mov             " b!() ", [" t!() "+ 8 * " j!() "]"),
        Q!("    mov             " "[" t!() "+ 8 * " j!() "], " a!()),
        Q!("    mov             " c!() ", " d!()),
        Q!("    inc             " j!()),
        Q!("    dec             " n!()),
        Q!("    jnz             " Label!("Lbignum_montifier_cmaloop", 18, Before)),
        Q!("    adc             " b!() ", " c!()),
        Q!("    mov             " h!() ", " b!()),

        Q!("    sbb             " l!() ", " l!()),

        Q!("    xor             " j!() ", " j!()),
        Q!("    xor             " c!() ", " c!()),
        Q!(Label!("Lbignum_montifier_oaloop", 19) ":"),
        Q!("    mov             " a!() ", [" t!() "+ 8 * " j!() "]"),
        Q!("    mov             " b!() ", [" z!() "+ 8 * " j!() "]"),
        Q!("    and             " b!() ", " l!()),
        Q!("    neg             " c!()),
        Q!("    adc             " a!() ", " b!()),
        Q!("    sbb             " c!() ", " c!()),
        Q!("    mov             " "[" t!() "+ 8 * " j!() "], " a!()),
        Q!("    inc             " j!()),
        Q!("    cmp             " j!() ", " k!()),
        Q!("    jc              " Label!("Lbignum_montifier_oaloop", 19, Before)),
        Q!("    sub             " h!() ", " c!()),

        Q!("    dec             " i!()),
        Q!("    jnz             " Label!("Lbignum_montifier_modloop", 17, Before)),

        // Compute the negated modular inverse w (same register as i, not used again).

        Q!("    mov             " a!() ", [" m!() "]"),
        Q!("    mov             " c!() ", " a!()),
        Q!("    mov             " w!() ", " a!()),
        Q!("    shl             " c!() ", 2"),
        Q!("    sub             " w!() ", " c!()),
        Q!("    xor             " w!() ", 2"),
        Q!("    mov             " c!() ", " w!()),
        Q!("    imul            " c!() ", " a!()),
        Q!("    mov             " ashort!() ", 2"),
        Q!("    add             " a!() ", " c!()),
        Q!("    add             " c!() ", 1"),
        Q!("    imul            " w!() ", " a!()),
        Q!("    imul            " c!() ", " c!()),
        Q!("    mov             " ashort!() ", 1"),
        Q!("    add             " a!() ", " c!()),
        Q!("    imul            " w!() ", " a!()),
        Q!("    imul            " c!() ", " c!()),
        Q!("    mov             " ashort!() ", 1"),
        Q!("    add             " a!() ", " c!()),
        Q!("    imul            " w!() ", " a!()),
        Q!("    imul            " c!() ", " c!()),
        Q!("    mov             " ashort!() ", 1"),
        Q!("    add             " a!() ", " c!()),
        Q!("    imul            " w!() ", " a!()),

        // Now do one almost-Montgomery reduction w.r.t. the original m
        // which lops off one 2^64 from the congruence and, with the usual
        // almost-Montgomery correction, gets us back inside k digits

        Q!("    mov             " c!() ", [" t!() "]"),
        Q!("    mov             " b!() ", " w!()),
        Q!("    imul            " b!() ", " c!()),

        Q!("    mov             " a!() ", [" m!() "]"),
        Q!("    mul             " b!()),
        Q!("    add             " a!() ", " c!()),
        Q!("    mov             " c!() ", " d!()),
        Q!("    mov             " jshort!() ", 1"),
        Q!("    mov             " n!() ", " k!()),
        Q!("    dec             " n!()),
        Q!("    jz              " Label!("Lbignum_montifier_amontend", 20, After)),
        Q!(Label!("Lbignum_montifier_amontloop", 21) ":"),
        Q!("    adc             " c!() ", [" t!() "+ 8 * " j!() "]"),
        Q!("    sbb             " l!() ", " l!()),
        Q!("    mov             " a!() ", [" m!() "+ 8 * " j!() "]"),
        Q!("    mul             " b!()),
        Q!("    sub             " d!() ", " l!()),
        Q!("    add             " a!() ", " c!()),
        Q!("    mov             " "[" t!() "+ 8 * " j!() "-8], " a!()),
        Q!("    mov             " c!() ", " d!()),
        Q!("    inc             " j!()),
        Q!("    dec             " n!()),
        Q!("    jnz             " Label!("Lbignum_montifier_amontloop", 21, Before)),
        Q!(Label!("Lbignum_montifier_amontend", 20) ":"),
        Q!("    adc             " h!() ", " c!()),
        Q!("    sbb             " l!() ", " l!()),
        Q!("    mov             " "[" t!() "+ 8 * " k!() "-8], " h!()),

        Q!("    xor             " j!() ", " j!()),
        Q!("    xor             " c!() ", " c!()),
        Q!(Label!("Lbignum_montifier_aosloop", 22) ":"),
        Q!("    mov             " a!() ", [" t!() "+ 8 * " j!() "]"),
        Q!("    mov             " b!() ", [" m!() "+ 8 * " j!() "]"),
        Q!("    and             " b!() ", " l!()),
        Q!("    neg             " c!()),
        Q!("    sbb             " a!() ", " b!()),
        Q!("    sbb             " c!() ", " c!()),
        Q!("    mov             " "[" z!() "+ 8 * " j!() "], " a!()),
        Q!("    inc             " j!()),
        Q!("    cmp             " j!() ", " k!()),
        Q!("    jc              " Label!("Lbignum_montifier_aosloop", 22, Before)),

        // So far, the code (basically a variant of bignum_amontifier) has produced
        // a k-digit value z == 2^{192k} (mod m), not necessarily fully reduced mod m.
        // We now do a short Montgomery reduction (similar to bignum_demont) so that
        // we achieve full reduction mod m while lopping 2^{64k} off the congruence.
        // We recycle h as the somewhat strangely-named outer loop counter.

        Q!("    mov             " h!() ", " k!()),

        Q!(Label!("Lbignum_montifier_montouterloop", 23) ":"),
        Q!("    mov             " c!() ", [" z!() "]"),
        Q!("    mov             " b!() ", " w!()),
        Q!("    imul            " b!() ", " c!()),
        Q!("    mov             " a!() ", [" m!() "]"),
        Q!("    mul             " b!()),
        Q!("    add             " a!() ", " c!()),
        Q!("    mov             " c!() ", " d!()),
        Q!("    mov             " jshort!() ", 1"),
        Q!("    mov             " n!() ", " k!()),
        Q!("    dec             " n!()),
        Q!("    jz              " Label!("Lbignum_montifier_montend", 24, After)),
        Q!(Label!("Lbignum_montifier_montloop", 25) ":"),
        Q!("    adc             " c!() ", [" z!() "+ 8 * " j!() "]"),
        Q!("    sbb             " l!() ", " l!()),
        Q!("    mov             " a!() ", [" m!() "+ 8 * " j!() "]"),
        Q!("    mul             " b!()),
        Q!("    sub             " d!() ", " l!()),
        Q!("    add             " a!() ", " c!()),
        Q!("    mov             " "[" z!() "+ 8 * " j!() "-8], " a!()),
        Q!("    mov             " c!() ", " d!()),
        Q!("    inc             " j!()),
        Q!("    dec             " n!()),
        Q!("    jnz             " Label!("Lbignum_montifier_montloop", 25, Before)),
        Q!(Label!("Lbignum_montifier_montend", 24) ":"),
        Q!("    adc             " c!() ", 0"),
        Q!("    mov             " "[" z!() "+ 8 * " k!() "-8], " c!()),

        Q!("    dec             " h!()),
        Q!("    jnz             " Label!("Lbignum_montifier_montouterloop", 23, Before)),

        // Now do a comparison of z with m to set a final correction mask
        // indicating that z >= m and so we need to subtract m.

        Q!("    xor             " j!() ", " j!()),
        Q!("    mov             " n!() ", " k!()),
        Q!(Label!("Lbignum_montifier_cmploop", 26) ":"),
        Q!("    mov             " a!() ", [" z!() "+ 8 * " j!() "]"),
        Q!("    sbb             " a!() ", [" m!() "+ 8 * " j!() "]"),
        Q!("    inc             " j!()),
        Q!("    dec             " n!()),
        Q!("    jnz             " Label!("Lbignum_montifier_cmploop", 26, Before)),
        Q!("    sbb             " d!() ", " d!()),
        Q!("    not             " d!()),

        // Now do a masked subtraction of m for the final reduced result.

        Q!("    xor             " l!() ", " l!()),
        Q!("    xor             " j!() ", " j!()),
        Q!(Label!("Lbignum_montifier_corrloop", 27) ":"),
        Q!("    mov             " a!() ", [" m!() "+ 8 * " j!() "]"),
        Q!("    and             " a!() ", " d!()),
        Q!("    neg             " l!()),
        Q!("    sbb             " "[" z!() "+ 8 * " j!() "], " a!()),
        Q!("    sbb             " l!() ", " l!()),
        Q!("    inc             " j!()),
        Q!("    cmp             " j!() ", " k!()),
        Q!("    jc              " Label!("Lbignum_montifier_corrloop", 27, Before)),

        Q!(Label!("Lbignum_montifier_end", 2) ":"),
        Q!("    pop             " "r13"),
        Q!("    pop             " "r12"),
        Q!("    pop             " "rbx"),
        Q!("    pop             " "rbp"),

        inout("rdi") m.len() => _,
        inout("rsi") z.as_mut_ptr() => _,
        inout("rdx") m.as_ptr() => _,
        inout("rcx") t.as_mut_ptr() => _,
        // clobbers
        out("r10") _,
        out("r11") _,
        out("r12") _,
        out("r13") _,
        out("r8") _,
        out("r9") _,
        out("rax") _,
            )
    };
}
