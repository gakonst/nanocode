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
// Standard ARM ABI: X0 = k, X1 = z, X2 = m, X3 = t
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
macro_rules! m {
    () => {
        "x2"
    };
}
macro_rules! t {
    () => {
        "x3"
    };
}

// Some variables
// Modular inverse w is aliased to i, but we never use them together

macro_rules! i {
    () => {
        "x4"
    };
}
macro_rules! w {
    () => {
        "x4"
    };
}
macro_rules! j {
    () => {
        "x5"
    };
}
macro_rules! h {
    () => {
        "x6"
    };
}
macro_rules! a {
    () => {
        "x7"
    };
}
macro_rules! l {
    () => {
        "x8"
    };
}
macro_rules! c {
    () => {
        "x9"
    };
}
macro_rules! b {
    () => {
        "x10"
    };
}
macro_rules! d {
    () => {
        "x11"
    };
}

// Some aliases for the values b and d

macro_rules! r {
    () => {
        "x10"
    };
}
macro_rules! q {
    () => {
        "x11"
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

        // If k = 0 the whole operation is trivial

        Q!("    cbz             " k!() ", " Label!("Lbignum_montifier_end", 2, After)),

        // Copy the input m into the temporary buffer t. The temporary register
        // c matters since we want it to hold the highest digit, ready for the
        // normalization phase.

        Q!("    mov             " i!() ", xzr"),
        Q!(Label!("Lbignum_montifier_copyinloop", 3) ":"),
        Q!("    ldr             " c!() ", [" m!() ", " i!() ", lsl #3]"),
        Q!("    str             " c!() ", [" t!() ", " i!() ", lsl #3]"),
        Q!("    add             " i!() ", " i!() ", #1"),
        Q!("    cmp             " i!() ", " k!()),
        Q!("    bcc             " Label!("Lbignum_montifier_copyinloop", 3, Before)),

        // Do a rather stupid but constant-time digit normalization, conditionally
        // shifting left (k-1) times based on whether the top word is zero.
        // With careful binary striding this could be O(k*log(k)) instead of O(k^2)
        // while still retaining the constant-time style.
        // The "cmp c, xzr" sets the zeroness predicate (ZF) for the entire inner loop

        Q!("    subs            " i!() ", " k!() ", #1"),
        Q!("    beq             " Label!("Lbignum_montifier_normalized", 4, After)),
        Q!(Label!("Lbignum_montifier_normloop", 5) ":"),
        Q!("    mov             " j!() ", xzr"),
        Q!("    cmp             " c!() ", xzr"),
        Q!("    mov             " a!() ", xzr"),
        Q!(Label!("Lbignum_montifier_shufloop", 6) ":"),
        Q!("    mov             " c!() ", " a!()),
        Q!("    ldr             " a!() ", [" t!() ", " j!() ", lsl #3]"),
        Q!("    csel            " c!() ", " c!() ", " a!() ", eq"),
        Q!("    str             " c!() ", [" t!() ", " j!() ", lsl #3]"),
        Q!("    add             " j!() ", " j!() ", #1"),
        Q!("    sub             " d!() ", " j!() ", " k!()),
        Q!("    cbnz            " d!() ", " Label!("Lbignum_montifier_shufloop", 6, Before)),
        Q!("    subs            " i!() ", " i!() ", #1"),
        Q!("    bne             " Label!("Lbignum_montifier_normloop", 5, Before)),

        // We now have the top digit nonzero, assuming the input was nonzero,
        // and as per the invariant of the loop above, c holds that digit. So
        // now just count c's leading zeros and shift t bitwise that many bits.

        Q!(Label!("Lbignum_montifier_normalized", 4) ":"),
        Q!("    clz             " c!() ", " c!()),

        Q!("    mov             " b!() ", xzr"),
        Q!("    mov             " i!() ", xzr"),
        Q!("    ands            " "xzr, " c!() ", #63"),
        Q!("    csetm           " l!() ", ne"),
        Q!("    neg             " d!() ", " c!()),
        Q!(Label!("Lbignum_montifier_bitloop", 7) ":"),
        Q!("    ldr             " j!() ", [" t!() ", " i!() ", lsl #3]"),
        Q!("    lsl             " a!() ", " j!() ", " c!()),
        Q!("    orr             " a!() ", " a!() ", " b!()),
        Q!("    lsr             " b!() ", " j!() ", " d!()),
        Q!("    and             " b!() ", " b!() ", " l!()),
        Q!("    str             " a!() ", [" t!() ", " i!() ", lsl #3]"),
        Q!("    add             " i!() ", " i!() ", #1"),
        Q!("    cmp             " i!() ", " k!()),
        Q!("    bcc             " Label!("Lbignum_montifier_bitloop", 7, Before)),

        // Let h be the high word of n, which in all the in-scope cases is >= 2^63.
        // Now successively form q = 2^i div h and r = 2^i mod h as i goes from
        // 64 to 126. We avoid just using division out of constant-time concerns
        // (at the least we would need to fix up h = 0 for out-of-scope inputs) and
        // don't bother with Newton-Raphson, since this stupid simple loop doesn't
        // contribute much of the overall runtime at typical sizes.

        Q!("    sub             " h!() ", " k!() ", #1"),
        Q!("    ldr             " h!() ", [" t!() ", " h!() ", lsl #3]"),
        Q!("    mov             " q!() ", #1"),
        Q!("    neg             " r!() ", " h!()),
        Q!("    mov             " i!() ", #62"),
        Q!(Label!("Lbignum_montifier_estloop", 8) ":"),
        Q!("    add             " q!() ", " q!() ", " q!()),
        Q!("    mov             " a!() ", " h!()),
        Q!("    sub             " a!() ", " a!() ", " r!()),
        Q!("    cmp             " r!() ", " a!()),
        Q!("    csetm           " a!() ", cs"),
        Q!("    sub             " q!() ", " q!() ", " a!()),
        Q!("    add             " r!() ", " r!() ", " r!()),
        Q!("    and             " a!() ", " a!() ", " h!()),
        Q!("    sub             " r!() ", " r!() ", " a!()),
        Q!("    subs            " i!() ", " i!() ", #1"),
        Q!("    bne             " Label!("Lbignum_montifier_estloop", 8, Before)),

        // Strictly speaking the above loop doesn't quite give the true remainder
        // and quotient in the special case r = h = 2^63, so fix it up. We get
        // q = 2^63 - 1 and r = 2^63 and really want q = 2^63 and r = 0. This is
        // supererogatory, because the main property of q used below still holds
        // in this case unless the initial m = 1, and then anyway the overall
        // specification (congruence modulo m) holds degenerately. But it seems
        // nicer to get a "true" quotient and remainder.

        Q!("    cmp             " r!() ", " h!()),
        Q!("    csinc           " q!() ", " q!() ", " q!() ", ne"),

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

        Q!("    mov             " c!() ", xzr"),
        Q!("    adds            " i!() ", xzr, xzr"),
        Q!(Label!("Lbignum_montifier_mulloop", 9) ":"),
        Q!("    ldr             " a!() ", [" t!() ", " i!() ", lsl #3]"),
        Q!("    mul             " l!() ", " q!() ", " a!()),
        Q!("    adcs            " l!() ", " l!() ", " c!()),
        Q!("    umulh           " c!() ", " q!() ", " a!()),
        Q!("    str             " l!() ", [" z!() ", " i!() ", lsl #3]"),
        Q!("    add             " i!() ", " i!() ", #1"),
        Q!("    sub             " a!() ", " i!() ", " k!()),
        Q!("    cbnz            " a!() ", " Label!("Lbignum_montifier_mulloop", 9, Before)),

        Q!("    adc             " c!() ", " c!() ", xzr"),
        Q!("    mov             " a!() ", #0x4000000000000000"),
        Q!("    subs            " c!() ", " c!() ", " a!()),
        Q!("    csetm           " q!() ", cs"),

        // Now do [c] * n - d for our final answer

        Q!("    subs            " i!() ", xzr, xzr"),
        Q!(Label!("Lbignum_montifier_remloop", 12) ":"),
        Q!("    ldr             " a!() ", [" t!() ", " i!() ", lsl #3]"),
        Q!("    ldr             " b!() ", [" z!() ", " i!() ", lsl #3]"),
        Q!("    and             " a!() ", " a!() ", " q!()),
        Q!("    sbcs            " a!() ", " a!() ", " b!()),
        Q!("    str             " a!() ", [" z!() ", " i!() ", lsl #3]"),
        Q!("    add             " i!() ", " i!() ", #1"),
        Q!("    sub             " a!() ", " i!() ", " k!()),
        Q!("    cbnz            " a!() ", " Label!("Lbignum_montifier_remloop", 12, Before)),

        // Now still need to do a couple of modular doublings to get us all the
        // way up to 2^{p+64} == r from the initial 2^{p+62} == r (mod n).

        Q!("    mov             " c!() ", xzr"),
        Q!("    subs            " j!() ", xzr, xzr"),
        Q!(Label!("Lbignum_montifier_dubloop1", 13) ":"),
        Q!("    ldr             " a!() ", [" z!() ", " j!() ", lsl #3]"),
        Q!("    extr            " c!() ", " a!() ", " c!() ", #63"),
        Q!("    ldr             " b!() ", [" t!() ", " j!() ", lsl #3]"),
        Q!("    sbcs            " c!() ", " c!() ", " b!()),
        Q!("    str             " c!() ", [" z!() ", " j!() ", lsl #3]"),
        Q!("    mov             " c!() ", " a!()),
        Q!("    add             " j!() ", " j!() ", #1"),
        Q!("    sub             " a!() ", " j!() ", " k!()),
        Q!("    cbnz            " a!() ", " Label!("Lbignum_montifier_dubloop1", 13, Before)),
        Q!("    lsr             " c!() ", " c!() ", #63"),
        Q!("    sbc             " c!() ", " c!() ", xzr"),
        Q!("    adds            " j!() ", xzr, xzr"),
        Q!(Label!("Lbignum_montifier_corrloop1", 14) ":"),
        Q!("    ldr             " a!() ", [" z!() ", " j!() ", lsl #3]"),
        Q!("    ldr             " b!() ", [" t!() ", " j!() ", lsl #3]"),
        Q!("    and             " b!() ", " b!() ", " c!()),
        Q!("    adcs            " a!() ", " a!() ", " b!()),
        Q!("    str             " a!() ", [" z!() ", " j!() ", lsl #3]"),
        Q!("    add             " j!() ", " j!() ", #1"),
        Q!("    sub             " a!() ", " j!() ", " k!()),
        Q!("    cbnz            " a!() ", " Label!("Lbignum_montifier_corrloop1", 14, Before)),

        // This is not exactly the same: we also copy output to t giving the
        // initialization t_1 = r == 2^{p+64} mod n for the main loop next.

        Q!("    mov             " c!() ", xzr"),
        Q!("    subs            " j!() ", xzr, xzr"),
        Q!(Label!("Lbignum_montifier_dubloop2", 15) ":"),
        Q!("    ldr             " a!() ", [" z!() ", " j!() ", lsl #3]"),
        Q!("    extr            " c!() ", " a!() ", " c!() ", #63"),
        Q!("    ldr             " b!() ", [" t!() ", " j!() ", lsl #3]"),
        Q!("    sbcs            " c!() ", " c!() ", " b!()),
        Q!("    str             " c!() ", [" z!() ", " j!() ", lsl #3]"),
        Q!("    mov             " c!() ", " a!()),
        Q!("    add             " j!() ", " j!() ", #1"),
        Q!("    sub             " a!() ", " j!() ", " k!()),
        Q!("    cbnz            " a!() ", " Label!("Lbignum_montifier_dubloop2", 15, Before)),
        Q!("    lsr             " c!() ", " c!() ", #63"),
        Q!("    sbc             " c!() ", " c!() ", xzr"),
        Q!("    adds            " j!() ", xzr, xzr"),
        Q!(Label!("Lbignum_montifier_corrloop2", 16) ":"),
        Q!("    ldr             " a!() ", [" z!() ", " j!() ", lsl #3]"),
        Q!("    ldr             " b!() ", [" t!() ", " j!() ", lsl #3]"),
        Q!("    and             " b!() ", " b!() ", " c!()),
        Q!("    adcs            " a!() ", " a!() ", " b!()),
        Q!("    str             " a!() ", [" z!() ", " j!() ", lsl #3]"),
        Q!("    str             " a!() ", [" t!() ", " j!() ", lsl #3]"),
        Q!("    add             " j!() ", " j!() ", #1"),
        Q!("    sub             " a!() ", " j!() ", " k!()),
        Q!("    cbnz            " a!() ", " Label!("Lbignum_montifier_corrloop2", 16, Before)),

        // We then successively generate (k+1)-digit values satisfying
        // t_i == 2^{p+64*i} mod n, each of which is stored in h::t. Finish
        // initialization by zeroing h initially

        Q!("    mov             " h!() ", xzr"),

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

        Q!("    add             " i!() ", " k!() ", " k!()),
        Q!(Label!("Lbignum_montifier_modloop", 17) ":"),
        Q!("    mov             " j!() ", xzr"),
        Q!("    mov             " b!() ", xzr"),
        Q!("    adds            " c!() ", xzr, xzr"),
        Q!(Label!("Lbignum_montifier_cmaloop", 18) ":"),
        Q!("    ldr             " a!() ", [" z!() ", " j!() ", lsl #3]"),
        Q!("    mul             " l!() ", " h!() ", " a!()),
        Q!("    adcs            " b!() ", " b!() ", " c!()),
        Q!("    umulh           " c!() ", " h!() ", " a!()),
        Q!("    adc             " c!() ", " c!() ", xzr"),
        Q!("    adds            " l!() ", " b!() ", " l!()),
        Q!("    ldr             " b!() ", [" t!() ", " j!() ", lsl #3]"),
        Q!("    str             " l!() ", [" t!() ", " j!() ", lsl #3]"),
        Q!("    add             " j!() ", " j!() ", #1"),
        Q!("    sub             " a!() ", " j!() ", " k!()),
        Q!("    cbnz            " a!() ", " Label!("Lbignum_montifier_cmaloop", 18, Before)),

        Q!("    adcs            " h!() ", " b!() ", " c!()),

        Q!("    csetm           " l!() ", cs"),

        Q!("    adds            " j!() ", xzr, xzr"),
        Q!(Label!("Lbignum_montifier_oaloop", 19) ":"),
        Q!("    ldr             " a!() ", [" t!() ", " j!() ", lsl #3]"),
        Q!("    ldr             " b!() ", [" z!() ", " j!() ", lsl #3]"),
        Q!("    and             " b!() ", " b!() ", " l!()),
        Q!("    adcs            " a!() ", " a!() ", " b!()),
        Q!("    str             " a!() ", [" t!() ", " j!() ", lsl #3]"),
        Q!("    add             " j!() ", " j!() ", #1"),
        Q!("    sub             " a!() ", " j!() ", " k!()),
        Q!("    cbnz            " a!() ", " Label!("Lbignum_montifier_oaloop", 19, Before)),
        Q!("    adc             " h!() ", " h!() ", xzr"),

        Q!("    subs            " i!() ", " i!() ", #1"),
        Q!("    bne             " Label!("Lbignum_montifier_modloop", 17, Before)),

        // Compute the negated modular inverse w (same register as i, not used again).

        Q!("    ldr             " a!() ", [" m!() "]"),
        Q!("    lsl             " w!() ", " a!() ", #2"),
        Q!("    sub             " w!() ", " a!() ", " w!()),
        Q!("    eor             " w!() ", " w!() ", #2"),
        Q!("    mov             " l!() ", #1"),
        Q!("    madd            " c!() ", " a!() ", " w!() ", " l!()),
        Q!("    mul             " b!() ", " c!() ", " c!()),
        Q!("    madd            " w!() ", " c!() ", " w!() ", " w!()),
        Q!("    mul             " c!() ", " b!() ", " b!()),
        Q!("    madd            " w!() ", " b!() ", " w!() ", " w!()),
        Q!("    mul             " b!() ", " c!() ", " c!()),
        Q!("    madd            " w!() ", " c!() ", " w!() ", " w!()),
        Q!("    madd            " w!() ", " b!() ", " w!() ", " w!()),

        // Now do one almost-Montgomery reduction w.r.t. the original m
        // which lops off one 2^64 from the congruence and, with the usual
        // almost-Montgomery correction, gets us back inside k digits for
        // the end result.

        Q!("    ldr             " b!() ", [" t!() "]"),
        Q!("    mul             " d!() ", " b!() ", " w!()),

        Q!("    mul             " l!() ", " d!() ", " a!()),
        Q!("    umulh           " c!() ", " d!() ", " a!()),
        Q!("    mov             " j!() ", #1"),
        Q!("    sub             " a!() ", " k!() ", #1"),
        Q!("    adds            " "xzr, " b!() ", " l!()),
        Q!("    cbz             " a!() ", " Label!("Lbignum_montifier_amontend", 20, After)),

        Q!(Label!("Lbignum_montifier_amontloop", 21) ":"),
        Q!("    ldr             " a!() ", [" m!() ", " j!() ", lsl #3]"),
        Q!("    ldr             " b!() ", [" t!() ", " j!() ", lsl #3]"),
        Q!("    mul             " l!() ", " d!() ", " a!()),
        Q!("    adcs            " b!() ", " b!() ", " c!()),
        Q!("    umulh           " c!() ", " d!() ", " a!()),
        Q!("    adc             " c!() ", " c!() ", xzr"),
        Q!("    adds            " b!() ", " b!() ", " l!()),
        Q!("    sub             " a!() ", " j!() ", #1"),
        Q!("    str             " b!() ", [" t!() ", " a!() ", lsl #3]"),
        Q!("    add             " j!() ", " j!() ", #1"),
        Q!("    sub             " a!() ", " j!() ", " k!()),
        Q!("    cbnz            " a!() ", " Label!("Lbignum_montifier_amontloop", 21, Before)),
        Q!(Label!("Lbignum_montifier_amontend", 20) ":"),
        Q!("    adcs            " h!() ", " h!() ", " c!()),
        Q!("    csetm           " l!() ", cs"),
        Q!("    sub             " a!() ", " k!() ", #1"),
        Q!("    str             " h!() ", [" t!() ", " a!() ", lsl #3]"),

        Q!("    subs            " j!() ", xzr, xzr"),
        Q!(Label!("Lbignum_montifier_osloop", 22) ":"),
        Q!("    ldr             " a!() ", [" t!() ", " j!() ", lsl #3]"),
        Q!("    ldr             " b!() ", [" m!() ", " j!() ", lsl #3]"),
        Q!("    and             " b!() ", " b!() ", " l!()),
        Q!("    sbcs            " a!() ", " a!() ", " b!()),
        Q!("    str             " a!() ", [" z!() ", " j!() ", lsl #3]"),
        Q!("    add             " j!() ", " j!() ", #1"),
        Q!("    sub             " a!() ", " j!() ", " k!()),
        Q!("    cbnz            " a!() ", " Label!("Lbignum_montifier_osloop", 22, Before)),

        // So far, the code(basically a variant of bignum_amontifier) has produced
        // a k-digit value z == 2^{192k} (mod m), not necessarily fully reduced mod m.
        // We now do a short Montgomery reduction (similar to bignum_demont) so that
        // we achieve full reduction mod m while lopping 2^{64k} off the congruence.
        // We recycle h as the somewhat strangely-named outer loop counter.

        Q!("    mov             " h!() ", " k!()),

        Q!(Label!("Lbignum_montifier_montouterloop", 23) ":"),
        Q!("    ldr             " b!() ", [" z!() "]"),
        Q!("    mul             " d!() ", " b!() ", " w!()),
        Q!("    ldr             " a!() ", [" m!() "]"),
        Q!("    mul             " l!() ", " d!() ", " a!()),
        Q!("    umulh           " c!() ", " d!() ", " a!()),
        Q!("    mov             " j!() ", #1"),
        Q!("    sub             " a!() ", " k!() ", #1"),
        Q!("    adds            " "xzr, " b!() ", " l!()),
        Q!("    cbz             " a!() ", " Label!("Lbignum_montifier_montend", 24, After)),
        Q!(Label!("Lbignum_montifier_montloop", 25) ":"),
        Q!("    ldr             " a!() ", [" m!() ", " j!() ", lsl #3]"),
        Q!("    ldr             " b!() ", [" z!() ", " j!() ", lsl #3]"),
        Q!("    mul             " l!() ", " d!() ", " a!()),
        Q!("    adcs            " b!() ", " b!() ", " c!()),
        Q!("    umulh           " c!() ", " d!() ", " a!()),
        Q!("    adc             " c!() ", " c!() ", xzr"),
        Q!("    adds            " b!() ", " b!() ", " l!()),
        Q!("    sub             " a!() ", " j!() ", #1"),
        Q!("    str             " b!() ", [" z!() ", " a!() ", lsl #3]"),
        Q!("    add             " j!() ", " j!() ", #1"),
        Q!("    sub             " a!() ", " j!() ", " k!()),
        Q!("    cbnz            " a!() ", " Label!("Lbignum_montifier_montloop", 25, Before)),
        Q!(Label!("Lbignum_montifier_montend", 24) ":"),
        Q!("    adc             " c!() ", " c!() ", xzr"),
        Q!("    sub             " a!() ", " k!() ", #1"),
        Q!("    str             " c!() ", [" z!() ", " a!() ", lsl #3]"),

        Q!("    subs            " h!() ", " h!() ", #1"),
        Q!("    bne             " Label!("Lbignum_montifier_montouterloop", 23, Before)),

        // Now do a comparison of z with m to set a final correction mask
        // indicating that z >= m and so we need to subtract m.

        Q!("    subs            " j!() ", xzr, xzr"),
        Q!(Label!("Lbignum_montifier_cmploop", 26) ":"),
        Q!("    ldr             " a!() ", [" z!() ", " j!() ", lsl #3]"),
        Q!("    ldr             " b!() ", [" m!() ", " j!() ", lsl #3]"),
        Q!("    sbcs            " "xzr, " a!() ", " b!()),
        Q!("    add             " j!() ", " j!() ", #1"),
        Q!("    sub             " a!() ", " j!() ", " k!()),
        Q!("    cbnz            " a!() ", " Label!("Lbignum_montifier_cmploop", 26, Before)),
        Q!("    csetm           " h!() ", cs"),

        // Now do a masked subtraction of m for the final reduced result.

        Q!("    subs            " j!() ", xzr, xzr"),
        Q!(Label!("Lbignum_montifier_corrloop", 27) ":"),
        Q!("    ldr             " a!() ", [" z!() ", " j!() ", lsl #3]"),
        Q!("    ldr             " b!() ", [" m!() ", " j!() ", lsl #3]"),
        Q!("    and             " b!() ", " b!() ", " h!()),
        Q!("    sbcs            " a!() ", " a!() ", " b!()),
        Q!("    str             " a!() ", [" z!() ", " j!() ", lsl #3]"),
        Q!("    add             " j!() ", " j!() ", #1"),
        Q!("    sub             " a!() ", " j!() ", " k!()),
        Q!("    cbnz            " a!() ", " Label!("Lbignum_montifier_corrloop", 27, Before)),

        Q!(Label!("Lbignum_montifier_end", 2) ":"),
        inout("x0") m.len() => _,
        inout("x1") z.as_mut_ptr() => _,
        inout("x2") m.as_ptr() => _,
        inout("x3") t.as_mut_ptr() => _,
        // clobbers
        out("x10") _,
        out("x11") _,
        out("x4") _,
        out("x5") _,
        out("x6") _,
        out("x7") _,
        out("x8") _,
        out("x9") _,
            )
    };
}
