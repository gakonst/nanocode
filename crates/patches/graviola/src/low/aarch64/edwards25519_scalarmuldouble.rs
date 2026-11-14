// generated source. do not edit.
#![allow(non_upper_case_globals, unused_macros, unused_imports)]
use crate::low::macros::*;

// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

// ----------------------------------------------------------------------------
// Double scalar multiplication for edwards25519, fresh and base point
// Input scalar[4], point[8], bscalar[4]; output res[8]
//
// extern void edwards25519_scalarmuldouble_alt
//   (uint64_t res[static 8],const uint64_t scalar[static 4],
//    const uint64_t point[static 8],const uint64_t bscalar[static 4]);
//
// Given scalar = n, point = P and bscalar = m, returns in res
// the point (X,Y) = n * P + m * B where B = (...,4/5) is
// the standard basepoint for the edwards25519 (Ed25519) curve.
//
// Both 256-bit coordinates of the input point P are implicitly
// reduced modulo 2^255-19 if they are not already in reduced form,
// but the conventional usage is that they *are* already reduced.
// The scalars can be arbitrary 256-bit numbers but may also be
// considered as implicitly reduced modulo the group order.
//
// Standard ARM ABI: X0 = res, X1 = scalar, X2 = point, X3 = bscalar
// ----------------------------------------------------------------------------

// Size of individual field elements

macro_rules! NUMSIZE {
    () => {
        "32"
    };
}

// Stable home for the input result argument during the whole body

macro_rules! res {
    () => {
        "x25"
    };
}

// Additional pointer variables for local subroutines

macro_rules! p0 {
    () => {
        "x22"
    };
}
macro_rules! p1 {
    () => {
        "x23"
    };
}
macro_rules! p2 {
    () => {
        "x24"
    };
}

// Other variables that are only needed prior to the modular inverse.

macro_rules! i {
    () => {
        "x19"
    };
}
macro_rules! bf {
    () => {
        "x20"
    };
}
macro_rules! cf {
    () => {
        "x21"
    };
}

// Pointer-offset pairs for result and temporaries on stack with some aliasing.

macro_rules! resx { () => { Q!(res!() ", # (0 * " NUMSIZE!() ")") } }
macro_rules! resy { () => { Q!(res!() ", # (1 * " NUMSIZE!() ")") } }

macro_rules! scalar { () => { Q!("sp, # (0 * " NUMSIZE!() ")") } }
macro_rules! bscalar { () => { Q!("sp, # (1 * " NUMSIZE!() ")") } }

macro_rules! btabent { () => { Q!("sp, # (2 * " NUMSIZE!() ")") } }
macro_rules! acc { () => { Q!("sp, # (5 * " NUMSIZE!() ")") } }
macro_rules! acc_x { () => { Q!("sp, # (5 * " NUMSIZE!() ")") } }
macro_rules! acc_y { () => { Q!("sp, # (6 * " NUMSIZE!() ")") } }
macro_rules! acc_z { () => { Q!("sp, # (7 * " NUMSIZE!() ")") } }
macro_rules! acc_w { () => { Q!("sp, # (8 * " NUMSIZE!() ")") } }

macro_rules! tabent { () => { Q!("sp, # (9 * " NUMSIZE!() ")") } }

macro_rules! tab { () => { Q!("sp, # (13 * " NUMSIZE!() ")") } }

// Total size to reserve on the stack (excluding local subroutines)

macro_rules! NSPACE { () => { Q!("45 * " NUMSIZE!()) } }

// Sub-references used in local subroutines with local stack

macro_rules! x_0 { () => { Q!(p0!() ", #0") } }
macro_rules! y_0 { () => { Q!(p0!() ", # " NUMSIZE!()) } }
macro_rules! z_0 { () => { Q!(p0!() ", # (2 * " NUMSIZE!() ")") } }
macro_rules! w_0 { () => { Q!(p0!() ", # (3 * " NUMSIZE!() ")") } }

macro_rules! x_1 { () => { Q!(p1!() ", #0") } }
macro_rules! y_1 { () => { Q!(p1!() ", # " NUMSIZE!()) } }
macro_rules! z_1 { () => { Q!(p1!() ", # (2 * " NUMSIZE!() ")") } }
macro_rules! w_1 { () => { Q!(p1!() ", # (3 * " NUMSIZE!() ")") } }

macro_rules! x_2 { () => { Q!(p2!() ", #0") } }
macro_rules! y_2 { () => { Q!(p2!() ", # " NUMSIZE!()) } }
macro_rules! z_2 { () => { Q!(p2!() ", # (2 * " NUMSIZE!() ")") } }
macro_rules! w_2 { () => { Q!(p2!() ", # (3 * " NUMSIZE!() ")") } }

macro_rules! t0 { () => { Q!("sp, # (0 * " NUMSIZE!() ")") } }
macro_rules! t1 { () => { Q!("sp, # (1 * " NUMSIZE!() ")") } }
macro_rules! t2 { () => { Q!("sp, # (2 * " NUMSIZE!() ")") } }
macro_rules! t3 { () => { Q!("sp, # (3 * " NUMSIZE!() ")") } }
macro_rules! t4 { () => { Q!("sp, # (4 * " NUMSIZE!() ")") } }
macro_rules! t5 { () => { Q!("sp, # (5 * " NUMSIZE!() ")") } }

// Load 64-bit immediate into a register

macro_rules! movbig {
    ($nn:expr, $n3:expr, $n2:expr, $n1:expr, $n0:expr) => { Q!(
        "movz " $nn ", " $n0 ";\n"
        "movk " $nn ", " $n1 ", lsl #16;\n"
        "movk " $nn ", " $n2 ", lsl #32;\n"
        "movk " $nn ", " $n3 ", lsl #48"
    )}
}

// Macro wrapping up the basic field operation bignum_mul_p25519_alt, only
// trivially different from a pure function call to that subroutine.

macro_rules! mul_p25519 {
    ($P0:expr, $P1:expr, $P2:expr) => { Q!(
        "ldp x3, x4, [" $P1 "];\n"
        "ldp x7, x8, [" $P2 "];\n"
        "mul x12, x3, x7;\n"
        "umulh x13, x3, x7;\n"
        "mul x11, x3, x8;\n"
        "umulh x14, x3, x8;\n"
        "adds x13, x13, x11;\n"
        "ldp x9, x10, [" $P2 "+ 16];\n"
        "mul x11, x3, x9;\n"
        "umulh x15, x3, x9;\n"
        "adcs x14, x14, x11;\n"
        "mul x11, x3, x10;\n"
        "umulh x16, x3, x10;\n"
        "adcs x15, x15, x11;\n"
        "adc x16, x16, xzr;\n"
        "ldp x5, x6, [" $P1 "+ 16];\n"
        "mul x11, x4, x7;\n"
        "adds x13, x13, x11;\n"
        "mul x11, x4, x8;\n"
        "adcs x14, x14, x11;\n"
        "mul x11, x4, x9;\n"
        "adcs x15, x15, x11;\n"
        "mul x11, x4, x10;\n"
        "adcs x16, x16, x11;\n"
        "umulh x3, x4, x10;\n"
        "adc x3, x3, xzr;\n"
        "umulh x11, x4, x7;\n"
        "adds x14, x14, x11;\n"
        "umulh x11, x4, x8;\n"
        "adcs x15, x15, x11;\n"
        "umulh x11, x4, x9;\n"
        "adcs x16, x16, x11;\n"
        "adc x3, x3, xzr;\n"
        "mul x11, x5, x7;\n"
        "adds x14, x14, x11;\n"
        "mul x11, x5, x8;\n"
        "adcs x15, x15, x11;\n"
        "mul x11, x5, x9;\n"
        "adcs x16, x16, x11;\n"
        "mul x11, x5, x10;\n"
        "adcs x3, x3, x11;\n"
        "umulh x4, x5, x10;\n"
        "adc x4, x4, xzr;\n"
        "umulh x11, x5, x7;\n"
        "adds x15, x15, x11;\n"
        "umulh x11, x5, x8;\n"
        "adcs x16, x16, x11;\n"
        "umulh x11, x5, x9;\n"
        "adcs x3, x3, x11;\n"
        "adc x4, x4, xzr;\n"
        "mul x11, x6, x7;\n"
        "adds x15, x15, x11;\n"
        "mul x11, x6, x8;\n"
        "adcs x16, x16, x11;\n"
        "mul x11, x6, x9;\n"
        "adcs x3, x3, x11;\n"
        "mul x11, x6, x10;\n"
        "adcs x4, x4, x11;\n"
        "umulh x5, x6, x10;\n"
        "adc x5, x5, xzr;\n"
        "umulh x11, x6, x7;\n"
        "adds x16, x16, x11;\n"
        "umulh x11, x6, x8;\n"
        "adcs x3, x3, x11;\n"
        "umulh x11, x6, x9;\n"
        "adcs x4, x4, x11;\n"
        "adc x5, x5, xzr;\n"
        "mov x7, #0x26;\n"
        "mul x11, x7, x16;\n"
        "umulh x9, x7, x16;\n"
        "adds x12, x12, x11;\n"
        "mul x11, x7, x3;\n"
        "umulh x3, x7, x3;\n"
        "adcs x13, x13, x11;\n"
        "mul x11, x7, x4;\n"
        "umulh x4, x7, x4;\n"
        "adcs x14, x14, x11;\n"
        "mul x11, x7, x5;\n"
        "umulh x5, x7, x5;\n"
        "adcs x15, x15, x11;\n"
        "cset x16, cs;\n"
        "adds x15, x15, x4;\n"
        "adc x16, x16, x5;\n"
        "cmn x15, x15;\n"
        "orr x15, x15, #0x8000000000000000;\n"
        "adc x8, x16, x16;\n"
        "mov x7, #0x13;\n"
        "madd x11, x7, x8, x7;\n"
        "adds x12, x12, x11;\n"
        "adcs x13, x13, x9;\n"
        "adcs x14, x14, x3;\n"
        "adcs x15, x15, xzr;\n"
        "csel x7, x7, xzr, cc;\n"
        "subs x12, x12, x7;\n"
        "sbcs x13, x13, xzr;\n"
        "sbcs x14, x14, xzr;\n"
        "sbc x15, x15, xzr;\n"
        "and x15, x15, #0x7fffffffffffffff;\n"
        "stp x12, x13, [" $P0 "];\n"
        "stp x14, x15, [" $P0 "+ 16]"
    )}
}

// A version of multiplication that only guarantees output < 2 * p_25519.
// This basically skips the +1 and final correction in quotient estimation.

macro_rules! mul_4 {
    ($P0:expr, $P1:expr, $P2:expr) => { Q!(
        "ldp x3, x4, [" $P1 "];\n"
        "ldp x7, x8, [" $P2 "];\n"
        "mul x12, x3, x7;\n"
        "umulh x13, x3, x7;\n"
        "mul x11, x3, x8;\n"
        "umulh x14, x3, x8;\n"
        "adds x13, x13, x11;\n"
        "ldp x9, x10, [" $P2 "+ 16];\n"
        "mul x11, x3, x9;\n"
        "umulh x15, x3, x9;\n"
        "adcs x14, x14, x11;\n"
        "mul x11, x3, x10;\n"
        "umulh x16, x3, x10;\n"
        "adcs x15, x15, x11;\n"
        "adc x16, x16, xzr;\n"
        "ldp x5, x6, [" $P1 "+ 16];\n"
        "mul x11, x4, x7;\n"
        "adds x13, x13, x11;\n"
        "mul x11, x4, x8;\n"
        "adcs x14, x14, x11;\n"
        "mul x11, x4, x9;\n"
        "adcs x15, x15, x11;\n"
        "mul x11, x4, x10;\n"
        "adcs x16, x16, x11;\n"
        "umulh x3, x4, x10;\n"
        "adc x3, x3, xzr;\n"
        "umulh x11, x4, x7;\n"
        "adds x14, x14, x11;\n"
        "umulh x11, x4, x8;\n"
        "adcs x15, x15, x11;\n"
        "umulh x11, x4, x9;\n"
        "adcs x16, x16, x11;\n"
        "adc x3, x3, xzr;\n"
        "mul x11, x5, x7;\n"
        "adds x14, x14, x11;\n"
        "mul x11, x5, x8;\n"
        "adcs x15, x15, x11;\n"
        "mul x11, x5, x9;\n"
        "adcs x16, x16, x11;\n"
        "mul x11, x5, x10;\n"
        "adcs x3, x3, x11;\n"
        "umulh x4, x5, x10;\n"
        "adc x4, x4, xzr;\n"
        "umulh x11, x5, x7;\n"
        "adds x15, x15, x11;\n"
        "umulh x11, x5, x8;\n"
        "adcs x16, x16, x11;\n"
        "umulh x11, x5, x9;\n"
        "adcs x3, x3, x11;\n"
        "adc x4, x4, xzr;\n"
        "mul x11, x6, x7;\n"
        "adds x15, x15, x11;\n"
        "mul x11, x6, x8;\n"
        "adcs x16, x16, x11;\n"
        "mul x11, x6, x9;\n"
        "adcs x3, x3, x11;\n"
        "mul x11, x6, x10;\n"
        "adcs x4, x4, x11;\n"
        "umulh x5, x6, x10;\n"
        "adc x5, x5, xzr;\n"
        "umulh x11, x6, x7;\n"
        "adds x16, x16, x11;\n"
        "umulh x11, x6, x8;\n"
        "adcs x3, x3, x11;\n"
        "umulh x11, x6, x9;\n"
        "adcs x4, x4, x11;\n"
        "adc x5, x5, xzr;\n"
        "mov x7, #0x26;\n"
        "mul x11, x7, x16;\n"
        "umulh x9, x7, x16;\n"
        "adds x12, x12, x11;\n"
        "mul x11, x7, x3;\n"
        "umulh x3, x7, x3;\n"
        "adcs x13, x13, x11;\n"
        "mul x11, x7, x4;\n"
        "umulh x4, x7, x4;\n"
        "adcs x14, x14, x11;\n"
        "mul x11, x7, x5;\n"
        "umulh x5, x7, x5;\n"
        "adcs x15, x15, x11;\n"
        "cset x16, cs;\n"
        "adds x15, x15, x4;\n"
        "adc x16, x16, x5;\n"
        "cmn x15, x15;\n"
        "bic x15, x15, #0x8000000000000000;\n"
        "adc x8, x16, x16;\n"
        "mov x7, #0x13;\n"
        "mul x11, x7, x8;\n"
        "adds x12, x12, x11;\n"
        "adcs x13, x13, x9;\n"
        "adcs x14, x14, x3;\n"
        "adc x15, x15, xzr;\n"
        "stp x12, x13, [" $P0 "];\n"
        "stp x14, x15, [" $P0 "+ 16]"
    )}
}

// Squaring just giving a result < 2 * p_25519, which is done by
// basically skipping the +1 in the quotient estimate and the final
// optional correction.

macro_rules! sqr_4 {
    ($P0:expr, $P1:expr) => { Q!(
        "ldp x2, x3, [" $P1 "];\n"
        "mul x9, x2, x3;\n"
        "umulh x10, x2, x3;\n"
        "ldp x4, x5, [" $P1 "+ 16];\n"
        "mul x11, x2, x5;\n"
        "umulh x12, x2, x5;\n"
        "mul x7, x2, x4;\n"
        "umulh x6, x2, x4;\n"
        "adds x10, x10, x7;\n"
        "adcs x11, x11, x6;\n"
        "mul x7, x3, x4;\n"
        "umulh x6, x3, x4;\n"
        "adc x6, x6, xzr;\n"
        "adds x11, x11, x7;\n"
        "mul x13, x4, x5;\n"
        "umulh x14, x4, x5;\n"
        "adcs x12, x12, x6;\n"
        "mul x7, x3, x5;\n"
        "umulh x6, x3, x5;\n"
        "adc x6, x6, xzr;\n"
        "adds x12, x12, x7;\n"
        "adcs x13, x13, x6;\n"
        "adc x14, x14, xzr;\n"
        "adds x9, x9, x9;\n"
        "adcs x10, x10, x10;\n"
        "adcs x11, x11, x11;\n"
        "adcs x12, x12, x12;\n"
        "adcs x13, x13, x13;\n"
        "adcs x14, x14, x14;\n"
        "cset x6, cs;\n"
        "umulh x7, x2, x2;\n"
        "mul x8, x2, x2;\n"
        "adds x9, x9, x7;\n"
        "mul x7, x3, x3;\n"
        "adcs x10, x10, x7;\n"
        "umulh x7, x3, x3;\n"
        "adcs x11, x11, x7;\n"
        "mul x7, x4, x4;\n"
        "adcs x12, x12, x7;\n"
        "umulh x7, x4, x4;\n"
        "adcs x13, x13, x7;\n"
        "mul x7, x5, x5;\n"
        "adcs x14, x14, x7;\n"
        "umulh x7, x5, x5;\n"
        "adc x6, x6, x7;\n"
        "mov x3, #0x26;\n"
        "mul x7, x3, x12;\n"
        "umulh x4, x3, x12;\n"
        "adds x8, x8, x7;\n"
        "mul x7, x3, x13;\n"
        "umulh x13, x3, x13;\n"
        "adcs x9, x9, x7;\n"
        "mul x7, x3, x14;\n"
        "umulh x14, x3, x14;\n"
        "adcs x10, x10, x7;\n"
        "mul x7, x3, x6;\n"
        "umulh x6, x3, x6;\n"
        "adcs x11, x11, x7;\n"
        "cset x12, cs;\n"
        "adds x11, x11, x14;\n"
        "adc x12, x12, x6;\n"
        "cmn x11, x11;\n"
        "bic x11, x11, #0x8000000000000000;\n"
        "adc x2, x12, x12;\n"
        "mov x3, #0x13;\n"
        "mul x7, x3, x2;\n"
        "adds x8, x8, x7;\n"
        "adcs x9, x9, x4;\n"
        "adcs x10, x10, x13;\n"
        "adc x11, x11, xzr;\n"
        "stp x8, x9, [" $P0 "];\n"
        "stp x10, x11, [" $P0 "+ 16]"
    )}
}

// Modular subtraction with double modulus 2 * p_25519 = 2^256 - 38

macro_rules! sub_twice4 {
    ($P0:expr, $P1:expr, $P2:expr) => { Q!(
        "ldp x5, x6, [" $P1 "];\n"
        "ldp x4, x3, [" $P2 "];\n"
        "subs x5, x5, x4;\n"
        "sbcs x6, x6, x3;\n"
        "ldp x7, x8, [" $P1 "+ 16];\n"
        "ldp x4, x3, [" $P2 "+ 16];\n"
        "sbcs x7, x7, x4;\n"
        "sbcs x8, x8, x3;\n"
        "mov x4, #38;\n"
        "csel x3, x4, xzr, lo;\n"
        "subs x5, x5, x3;\n"
        "sbcs x6, x6, xzr;\n"
        "sbcs x7, x7, xzr;\n"
        "sbc x8, x8, xzr;\n"
        "stp x5, x6, [" $P0 "];\n"
        "stp x7, x8, [" $P0 "+ 16]"
    )}
}

// Modular addition and doubling with double modulus 2 * p_25519 = 2^256 - 38.
// This only ensures that the result fits in 4 digits, not that it is reduced
// even w.r.t. double modulus. The result is always correct modulo provided
// the sum of the inputs is < 2^256 + 2^256 - 38, so in particular provided
// at least one of them is reduced double modulo.

macro_rules! add_twice4 {
    ($P0:expr, $P1:expr, $P2:expr) => { Q!(
        "ldp x3, x4, [" $P1 "];\n"
        "ldp x7, x8, [" $P2 "];\n"
        "adds x3, x3, x7;\n"
        "adcs x4, x4, x8;\n"
        "ldp x5, x6, [" $P1 "+ 16];\n"
        "ldp x7, x8, [" $P2 "+ 16];\n"
        "adcs x5, x5, x7;\n"
        "adcs x6, x6, x8;\n"
        "mov x9, #38;\n"
        "csel x9, x9, xzr, cs;\n"
        "adds x3, x3, x9;\n"
        "adcs x4, x4, xzr;\n"
        "adcs x5, x5, xzr;\n"
        "adc x6, x6, xzr;\n"
        "stp x3, x4, [" $P0 "];\n"
        "stp x5, x6, [" $P0 "+ 16]"
    )}
}

macro_rules! double_twice4 {
    ($P0:expr, $P1:expr) => { Q!(
        "ldp x3, x4, [" $P1 "];\n"
        "adds x3, x3, x3;\n"
        "adcs x4, x4, x4;\n"
        "ldp x5, x6, [" $P1 "+ 16];\n"
        "adcs x5, x5, x5;\n"
        "adcs x6, x6, x6;\n"
        "mov x9, #38;\n"
        "csel x9, x9, xzr, cs;\n"
        "adds x3, x3, x9;\n"
        "adcs x4, x4, xzr;\n"
        "adcs x5, x5, xzr;\n"
        "adc x6, x6, xzr;\n"
        "stp x3, x4, [" $P0 "];\n"
        "stp x5, x6, [" $P0 "+ 16]"
    )}
}

// Load the constant k_25519 = 2 * d_25519 using immediate operations

macro_rules! load_k25519 {
    ($P0:expr) => { Q!(
        "movz x0, #0xf159;\n"
        "movz x1, #0xb156;\n"
        "movz x2, #0xd130;\n"
        "movz x3, #0xfce7;\n"
        "movk x0, #0x26b2, lsl #16;\n"
        "movk x1, #0x8283, lsl #16;\n"
        "movk x2, #0xeef3, lsl #16;\n"
        "movk x3, #0x56df, lsl #16;\n"
        "movk x0, #0x9b94, lsl #32;\n"
        "movk x1, #0x149a, lsl #32;\n"
        "movk x2, #0x80f2, lsl #32;\n"
        "movk x3, #0xd9dc, lsl #32;\n"
        "movk x0, #0xebd6, lsl #48;\n"
        "movk x1, #0x00e0, lsl #48;\n"
        "movk x2, #0x198e, lsl #48;\n"
        "movk x3, #0x2406, lsl #48;\n"
        "stp x0, x1, [" $P0 "];\n"
        "stp x2, x3, [" $P0 "+ 16]"
    )}
}

/// Double scalar multiplication for edwards25519, fresh and base point
///
/// Input scalar[4], point[8], bscalar[4]; output res[8]
///
/// Given scalar = n, point = P and bscalar = m, returns in res
/// the point (X,Y) = n * P + m * B where B = (...,4/5) is
/// the standard basepoint for the edwards25519 (Ed25519) curve.
///
/// Both 256-bit coordinates of the input point P are implicitly
/// reduced modulo 2^255-19 if they are not already in reduced form,
/// but the conventional usage is that they *are* already reduced.
/// The scalars can be arbitrary 256-bit numbers but may also be
/// considered as implicitly reduced modulo the group order.
pub(crate) fn edwards25519_scalarmuldouble(
    res: &mut [u64; 8],
    scalar: &[u64; 4],
    point: &[u64; 8],
    bscalar: &[u64; 4],
) {
    // SAFETY: inline assembly. see [crate::low::inline_assembly_safety] for safety info.
    unsafe {
        core::arch::asm!(

        // Save regs and make room for temporaries

        Q!("    stp             " "x19, x20, [sp, #-16] !"),
        Q!("    stp             " "x21, x22, [sp, #-16] !"),
        Q!("    stp             " "x23, x24, [sp, #-16] !"),
        Q!("    stp             " "x25, x30, [sp, #-16] !"),
        Q!("    sub             " "sp, sp, # (" NSPACE!() "+ 0)"),

        // Move the output pointer to a stable place

        Q!("    mov             " res!() ", x0"),

        // Copy scalars while recoding all 4-bit nybbles except the top
        // one (bits 252..255) into signed 4-bit digits. This is essentially
        // done just by adding the recoding constant 0x0888..888, after
        // which all digits except the first have an implicit bias of -8,
        // so 0 -> -8, 1 -> -7, ... 7 -> -1, 8 -> 0, 9 -> 1, ... 15 -> 7.
        // (We could literally create 2s complement signed nybbles by
        // XORing with the same constant 0x0888..888 afterwards, but it
        // doesn't seem to make the end usage any simpler.)
        //
        // In order to ensure that the unrecoded top nybble (bits 252..255)
        // does not become > 8 as a result of carries lower down from the
        // recoding, we first (conceptually) subtract the group order iff
        // the top digit of the scalar is > 2^63. In the implementation the
        // reduction and recoding are combined by optionally using the
        // modified recoding constant 0x0888...888 + (2^256 - group_order).

        movbig!("x4", "#0xc7f5", "#0x6fb5", "#0xa0d9", "#0xe920"),
        movbig!("x5", "#0xe190", "#0xb993", "#0x70cb", "#0xa1d5"),
        Q!("    mov             " "x7, #0x8888888888888888"),
        Q!("    sub             " "x6, x7, #1"),
        Q!("    bic             " "x8, x7, #0xF000000000000000"),

        Q!("    ldp             " "x10, x11, [x3]"),
        Q!("    ldp             " "x12, x13, [x3, #16]"),
        Q!("    mov             " "x3, 0x8000000000000000"),
        Q!("    cmp             " "x3, x13"),
        Q!("    csel            " "x14, x7, x4, cs"),
        Q!("    csel            " "x15, x7, x5, cs"),
        Q!("    csel            " "x16, x7, x6, cs"),
        Q!("    csel            " "x17, x8, x7, cs"),
        Q!("    adds            " "x10, x10, x14"),
        Q!("    adcs            " "x11, x11, x15"),
        Q!("    adcs            " "x12, x12, x16"),
        Q!("    adc             " "x13, x13, x17"),
        Q!("    stp             " "x10, x11, [" bscalar!() "]"),
        Q!("    stp             " "x12, x13, [" bscalar!() "+ 16]"),

        Q!("    ldp             " "x10, x11, [x1]"),
        Q!("    ldp             " "x12, x13, [x1, #16]"),
        Q!("    mov             " "x3, 0x8000000000000000"),
        Q!("    cmp             " "x3, x13"),
        Q!("    csel            " "x14, x7, x4, cs"),
        Q!("    csel            " "x15, x7, x5, cs"),
        Q!("    csel            " "x16, x7, x6, cs"),
        Q!("    csel            " "x17, x8, x7, cs"),
        Q!("    adds            " "x10, x10, x14"),
        Q!("    adcs            " "x11, x11, x15"),
        Q!("    adcs            " "x12, x12, x16"),
        Q!("    adc             " "x13, x13, x17"),
        Q!("    stp             " "x10, x11, [" scalar!() "]"),
        Q!("    stp             " "x12, x13, [" scalar!() "+ 16]"),

        // Create table of multiples 1..8 of the general input point at "tab".
        // Reduce the input coordinates x and y modulo 2^256 - 38 first, for the
        // sake of definiteness; this is the reduction that will be maintained.
        // We could slightly optimize the additions because we know the input
        // point is affine (so Z = 1), but it doesn't seem worth the complication.

        Q!("    ldp             " "x10, x11, [x2]"),
        Q!("    ldp             " "x12, x13, [x2, #16]"),
        Q!("    adds            " "x14, x10, #38"),
        Q!("    adcs            " "x15, x11, xzr"),
        Q!("    adcs            " "x16, x12, xzr"),
        Q!("    adcs            " "x17, x13, xzr"),
        Q!("    csel            " "x10, x14, x10, cs"),
        Q!("    csel            " "x11, x15, x11, cs"),
        Q!("    csel            " "x12, x16, x12, cs"),
        Q!("    csel            " "x13, x17, x13, cs"),
        Q!("    stp             " "x10, x11, [" tab!() "]"),
        Q!("    stp             " "x12, x13, [" tab!() "+ 16]"),

        Q!("    ldp             " "x10, x11, [x2, #32]"),
        Q!("    ldp             " "x12, x13, [x2, #48]"),
        Q!("    adds            " "x14, x10, #38"),
        Q!("    adcs            " "x15, x11, xzr"),
        Q!("    adcs            " "x16, x12, xzr"),
        Q!("    adcs            " "x17, x13, xzr"),
        Q!("    csel            " "x10, x14, x10, cs"),
        Q!("    csel            " "x11, x15, x11, cs"),
        Q!("    csel            " "x12, x16, x12, cs"),
        Q!("    csel            " "x13, x17, x13, cs"),
        Q!("    stp             " "x10, x11, [" tab!() "+ 32]"),
        Q!("    stp             " "x12, x13, [" tab!() "+ 48]"),

        Q!("    mov             " "x1, #1"),
        Q!("    stp             " "x1, xzr, [" tab!() "+ 64]"),
        Q!("    stp             " "xzr, xzr, [" tab!() "+ 80]"),

        Q!("    add             " p0!() ", " tab!() "+ 96"),
        Q!("    add             " p1!() ", " tab!()),
        Q!("    add             " p2!() ", " tab!() "+ 32"),
        mul_4!(x_0!(), x_1!(), x_2!()),

        // Multiple 2

        Q!("    add             " p0!() ", " tab!() "+ 1 * 128"),
        Q!("    add             " p1!() ", " tab!()),
        Q!("    bl              " Label!("Ledwards25519_scalarmuldouble_alt_epdouble", 2, After)),

        // Multiple 3

        Q!("    add             " p0!() ", " tab!() "+ 2 * 128"),
        Q!("    add             " p1!() ", " tab!()),
        Q!("    add             " p2!() ", " tab!() "+ 1 * 128"),
        Q!("    bl              " Label!("Ledwards25519_scalarmuldouble_alt_epadd", 3, After)),

        // Multiple 4

        Q!("    add             " p0!() ", " tab!() "+ 3 * 128"),
        Q!("    add             " p1!() ", " tab!() "+ 1 * 128"),
        Q!("    bl              " Label!("Ledwards25519_scalarmuldouble_alt_epdouble", 2, After)),

        // Multiple 5

        Q!("    add             " p0!() ", " tab!() "+ 4 * 128"),
        Q!("    add             " p1!() ", " tab!()),
        Q!("    add             " p2!() ", " tab!() "+ 3 * 128"),
        Q!("    bl              " Label!("Ledwards25519_scalarmuldouble_alt_epadd", 3, After)),

        // Multiple 6

        Q!("    add             " p0!() ", " tab!() "+ 5 * 128"),
        Q!("    add             " p1!() ", " tab!() "+ 2 * 128"),
        Q!("    bl              " Label!("Ledwards25519_scalarmuldouble_alt_epdouble", 2, After)),

        // Multiple 7

        Q!("    add             " p0!() ", " tab!() "+ 6 * 128"),
        Q!("    add             " p1!() ", " tab!()),
        Q!("    add             " p2!() ", " tab!() "+ 5 * 128"),
        Q!("    bl              " Label!("Ledwards25519_scalarmuldouble_alt_epadd", 3, After)),

        // Multiple 8

        Q!("    add             " p0!() ", " tab!() "+ 7 * 128"),
        Q!("    add             " p1!() ", " tab!() "+ 3 * 128"),
        Q!("    bl              " Label!("Ledwards25519_scalarmuldouble_alt_epdouble", 2, After)),

        // Handle the initialization, starting the loop counter at i = 252
        // and initializing acc to the sum of the table entries for the
        // top nybbles of the scalars (the ones with no implicit -8 bias).

        Q!("    mov             " i!() ", #252"),

        // Index for btable entry...

        Q!("    ldr             " "x0, [" bscalar!() "+ 24]"),
        Q!("    lsr             " bf!() ", x0, #60"),

        // ...and constant-time indexing based on that index

        Q!("    adrp            " "x14, " PageRef!("edwards25519_scalarmuldouble_alt_constant")),
        Q!("    add             " "x14, x14, " PageOffRef!("edwards25519_scalarmuldouble_alt_constant")),

        Q!("    mov             " "x0, #1"),
        Q!("    mov             " "x1, xzr"),
        Q!("    mov             " "x2, xzr"),
        Q!("    mov             " "x3, xzr"),
        Q!("    mov             " "x4, #1"),
        Q!("    mov             " "x5, xzr"),
        Q!("    mov             " "x6, xzr"),
        Q!("    mov             " "x7, xzr"),
        Q!("    mov             " "x8, xzr"),
        Q!("    mov             " "x9, xzr"),
        Q!("    mov             " "x10, xzr"),
        Q!("    mov             " "x11, xzr"),

        Q!("    cmp             " bf!() ", #1"),
        Q!("    ldp             " "x12, x13, [x14]"),
        Q!("    csel            " "x0, x0, x12, ne"),
        Q!("    csel            " "x1, x1, x13, ne"),
        Q!("    ldp             " "x12, x13, [x14, #16]"),
        Q!("    csel            " "x2, x2, x12, ne"),
        Q!("    csel            " "x3, x3, x13, ne"),
        Q!("    ldp             " "x12, x13, [x14, #32]"),
        Q!("    csel            " "x4, x4, x12, ne"),
        Q!("    csel            " "x5, x5, x13, ne"),
        Q!("    ldp             " "x12, x13, [x14, #48]"),
        Q!("    csel            " "x6, x6, x12, ne"),
        Q!("    csel            " "x7, x7, x13, ne"),
        Q!("    ldp             " "x12, x13, [x14, #64]"),
        Q!("    csel            " "x8, x8, x12, ne"),
        Q!("    csel            " "x9, x9, x13, ne"),
        Q!("    ldp             " "x12, x13, [x14, #80]"),
        Q!("    csel            " "x10, x10, x12, ne"),
        Q!("    csel            " "x11, x11, x13, ne"),
        Q!("    add             " "x14, x14, #96"),

        Q!("    cmp             " bf!() ", #2"),
        Q!("    ldp             " "x12, x13, [x14]"),
        Q!("    csel            " "x0, x0, x12, ne"),
        Q!("    csel            " "x1, x1, x13, ne"),
        Q!("    ldp             " "x12, x13, [x14, #16]"),
        Q!("    csel            " "x2, x2, x12, ne"),
        Q!("    csel            " "x3, x3, x13, ne"),
        Q!("    ldp             " "x12, x13, [x14, #32]"),
        Q!("    csel            " "x4, x4, x12, ne"),
        Q!("    csel            " "x5, x5, x13, ne"),
        Q!("    ldp             " "x12, x13, [x14, #48]"),
        Q!("    csel            " "x6, x6, x12, ne"),
        Q!("    csel            " "x7, x7, x13, ne"),
        Q!("    ldp             " "x12, x13, [x14, #64]"),
        Q!("    csel            " "x8, x8, x12, ne"),
        Q!("    csel            " "x9, x9, x13, ne"),
        Q!("    ldp             " "x12, x13, [x14, #80]"),
        Q!("    csel            " "x10, x10, x12, ne"),
        Q!("    csel            " "x11, x11, x13, ne"),
        Q!("    add             " "x14, x14, #96"),

        Q!("    cmp             " bf!() ", #3"),
        Q!("    ldp             " "x12, x13, [x14]"),
        Q!("    csel            " "x0, x0, x12, ne"),
        Q!("    csel            " "x1, x1, x13, ne"),
        Q!("    ldp             " "x12, x13, [x14, #16]"),
        Q!("    csel            " "x2, x2, x12, ne"),
        Q!("    csel            " "x3, x3, x13, ne"),
        Q!("    ldp             " "x12, x13, [x14, #32]"),
        Q!("    csel            " "x4, x4, x12, ne"),
        Q!("    csel            " "x5, x5, x13, ne"),
        Q!("    ldp             " "x12, x13, [x14, #48]"),
        Q!("    csel            " "x6, x6, x12, ne"),
        Q!("    csel            " "x7, x7, x13, ne"),
        Q!("    ldp             " "x12, x13, [x14, #64]"),
        Q!("    csel            " "x8, x8, x12, ne"),
        Q!("    csel            " "x9, x9, x13, ne"),
        Q!("    ldp             " "x12, x13, [x14, #80]"),
        Q!("    csel            " "x10, x10, x12, ne"),
        Q!("    csel            " "x11, x11, x13, ne"),
        Q!("    add             " "x14, x14, #96"),

        Q!("    cmp             " bf!() ", #4"),
        Q!("    ldp             " "x12, x13, [x14]"),
        Q!("    csel            " "x0, x0, x12, ne"),
        Q!("    csel            " "x1, x1, x13, ne"),
        Q!("    ldp             " "x12, x13, [x14, #16]"),
        Q!("    csel            " "x2, x2, x12, ne"),
        Q!("    csel            " "x3, x3, x13, ne"),
        Q!("    ldp             " "x12, x13, [x14, #32]"),
        Q!("    csel            " "x4, x4, x12, ne"),
        Q!("    csel            " "x5, x5, x13, ne"),
        Q!("    ldp             " "x12, x13, [x14, #48]"),
        Q!("    csel            " "x6, x6, x12, ne"),
        Q!("    csel            " "x7, x7, x13, ne"),
        Q!("    ldp             " "x12, x13, [x14, #64]"),
        Q!("    csel            " "x8, x8, x12, ne"),
        Q!("    csel            " "x9, x9, x13, ne"),
        Q!("    ldp             " "x12, x13, [x14, #80]"),
        Q!("    csel            " "x10, x10, x12, ne"),
        Q!("    csel            " "x11, x11, x13, ne"),
        Q!("    add             " "x14, x14, #96"),

        Q!("    cmp             " bf!() ", #5"),
        Q!("    ldp             " "x12, x13, [x14]"),
        Q!("    csel            " "x0, x0, x12, ne"),
        Q!("    csel            " "x1, x1, x13, ne"),
        Q!("    ldp             " "x12, x13, [x14, #16]"),
        Q!("    csel            " "x2, x2, x12, ne"),
        Q!("    csel            " "x3, x3, x13, ne"),
        Q!("    ldp             " "x12, x13, [x14, #32]"),
        Q!("    csel            " "x4, x4, x12, ne"),
        Q!("    csel            " "x5, x5, x13, ne"),
        Q!("    ldp             " "x12, x13, [x14, #48]"),
        Q!("    csel            " "x6, x6, x12, ne"),
        Q!("    csel            " "x7, x7, x13, ne"),
        Q!("    ldp             " "x12, x13, [x14, #64]"),
        Q!("    csel            " "x8, x8, x12, ne"),
        Q!("    csel            " "x9, x9, x13, ne"),
        Q!("    ldp             " "x12, x13, [x14, #80]"),
        Q!("    csel            " "x10, x10, x12, ne"),
        Q!("    csel            " "x11, x11, x13, ne"),
        Q!("    add             " "x14, x14, #96"),

        Q!("    cmp             " bf!() ", #6"),
        Q!("    ldp             " "x12, x13, [x14]"),
        Q!("    csel            " "x0, x0, x12, ne"),
        Q!("    csel            " "x1, x1, x13, ne"),
        Q!("    ldp             " "x12, x13, [x14, #16]"),
        Q!("    csel            " "x2, x2, x12, ne"),
        Q!("    csel            " "x3, x3, x13, ne"),
        Q!("    ldp             " "x12, x13, [x14, #32]"),
        Q!("    csel            " "x4, x4, x12, ne"),
        Q!("    csel            " "x5, x5, x13, ne"),
        Q!("    ldp             " "x12, x13, [x14, #48]"),
        Q!("    csel            " "x6, x6, x12, ne"),
        Q!("    csel            " "x7, x7, x13, ne"),
        Q!("    ldp             " "x12, x13, [x14, #64]"),
        Q!("    csel            " "x8, x8, x12, ne"),
        Q!("    csel            " "x9, x9, x13, ne"),
        Q!("    ldp             " "x12, x13, [x14, #80]"),
        Q!("    csel            " "x10, x10, x12, ne"),
        Q!("    csel            " "x11, x11, x13, ne"),
        Q!("    add             " "x14, x14, #96"),

        Q!("    cmp             " bf!() ", #7"),
        Q!("    ldp             " "x12, x13, [x14]"),
        Q!("    csel            " "x0, x0, x12, ne"),
        Q!("    csel            " "x1, x1, x13, ne"),
        Q!("    ldp             " "x12, x13, [x14, #16]"),
        Q!("    csel            " "x2, x2, x12, ne"),
        Q!("    csel            " "x3, x3, x13, ne"),
        Q!("    ldp             " "x12, x13, [x14, #32]"),
        Q!("    csel            " "x4, x4, x12, ne"),
        Q!("    csel            " "x5, x5, x13, ne"),
        Q!("    ldp             " "x12, x13, [x14, #48]"),
        Q!("    csel            " "x6, x6, x12, ne"),
        Q!("    csel            " "x7, x7, x13, ne"),
        Q!("    ldp             " "x12, x13, [x14, #64]"),
        Q!("    csel            " "x8, x8, x12, ne"),
        Q!("    csel            " "x9, x9, x13, ne"),
        Q!("    ldp             " "x12, x13, [x14, #80]"),
        Q!("    csel            " "x10, x10, x12, ne"),
        Q!("    csel            " "x11, x11, x13, ne"),
        Q!("    add             " "x14, x14, #96"),

        Q!("    cmp             " bf!() ", #8"),
        Q!("    ldp             " "x12, x13, [x14]"),
        Q!("    csel            " "x0, x0, x12, ne"),
        Q!("    csel            " "x1, x1, x13, ne"),
        Q!("    ldp             " "x12, x13, [x14, #16]"),
        Q!("    csel            " "x2, x2, x12, ne"),
        Q!("    csel            " "x3, x3, x13, ne"),
        Q!("    ldp             " "x12, x13, [x14, #32]"),
        Q!("    csel            " "x4, x4, x12, ne"),
        Q!("    csel            " "x5, x5, x13, ne"),
        Q!("    ldp             " "x12, x13, [x14, #48]"),
        Q!("    csel            " "x6, x6, x12, ne"),
        Q!("    csel            " "x7, x7, x13, ne"),
        Q!("    ldp             " "x12, x13, [x14, #64]"),
        Q!("    csel            " "x8, x8, x12, ne"),
        Q!("    csel            " "x9, x9, x13, ne"),
        Q!("    ldp             " "x12, x13, [x14, #80]"),
        Q!("    csel            " "x10, x10, x12, ne"),
        Q!("    csel            " "x11, x11, x13, ne"),

        Q!("    stp             " "x0, x1, [" btabent!() "]"),
        Q!("    stp             " "x2, x3, [" btabent!() "+ 16]"),
        Q!("    stp             " "x4, x5, [" btabent!() "+ 32]"),
        Q!("    stp             " "x6, x7, [" btabent!() "+ 48]"),
        Q!("    stp             " "x8, x9, [" btabent!() "+ 64]"),
        Q!("    stp             " "x10, x11, [" btabent!() "+ 80]"),

        // Index for table entry...

        Q!("    ldr             " "x0, [" scalar!() "+ 24]"),
        Q!("    lsr             " bf!() ", x0, #60"),

        // ...and constant-time indexing based on that index

        Q!("    add             " p0!() ", " tab!()),

        Q!("    mov             " "x0, xzr"),
        Q!("    mov             " "x1, xzr"),
        Q!("    mov             " "x2, xzr"),
        Q!("    mov             " "x3, xzr"),
        Q!("    mov             " "x4, #1"),
        Q!("    mov             " "x5, xzr"),
        Q!("    mov             " "x6, xzr"),
        Q!("    mov             " "x7, xzr"),
        Q!("    mov             " "x8, #1"),
        Q!("    mov             " "x9, xzr"),
        Q!("    mov             " "x10, xzr"),
        Q!("    mov             " "x11, xzr"),
        Q!("    mov             " "x12, xzr"),
        Q!("    mov             " "x13, xzr"),
        Q!("    mov             " "x14, xzr"),
        Q!("    mov             " "x15, xzr"),

        Q!("    cmp             " bf!() ", #1"),
        Q!("    ldp             " "x16, x17, [" p0!() "]"),
        Q!("    csel            " "x0, x0, x16, ne"),
        Q!("    csel            " "x1, x1, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #16]"),
        Q!("    csel            " "x2, x2, x16, ne"),
        Q!("    csel            " "x3, x3, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #32]"),
        Q!("    csel            " "x4, x4, x16, ne"),
        Q!("    csel            " "x5, x5, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #48]"),
        Q!("    csel            " "x6, x6, x16, ne"),
        Q!("    csel            " "x7, x7, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #64]"),
        Q!("    csel            " "x8, x8, x16, ne"),
        Q!("    csel            " "x9, x9, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #80]"),
        Q!("    csel            " "x10, x10, x16, ne"),
        Q!("    csel            " "x11, x11, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #96]"),
        Q!("    csel            " "x12, x12, x16, ne"),
        Q!("    csel            " "x13, x13, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #112]"),
        Q!("    csel            " "x14, x14, x16, ne"),
        Q!("    csel            " "x15, x15, x17, ne"),
        Q!("    add             " p0!() ", " p0!() ", #128"),

        Q!("    cmp             " bf!() ", #2"),
        Q!("    ldp             " "x16, x17, [" p0!() "]"),
        Q!("    csel            " "x0, x0, x16, ne"),
        Q!("    csel            " "x1, x1, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #16]"),
        Q!("    csel            " "x2, x2, x16, ne"),
        Q!("    csel            " "x3, x3, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #32]"),
        Q!("    csel            " "x4, x4, x16, ne"),
        Q!("    csel            " "x5, x5, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #48]"),
        Q!("    csel            " "x6, x6, x16, ne"),
        Q!("    csel            " "x7, x7, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #64]"),
        Q!("    csel            " "x8, x8, x16, ne"),
        Q!("    csel            " "x9, x9, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #80]"),
        Q!("    csel            " "x10, x10, x16, ne"),
        Q!("    csel            " "x11, x11, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #96]"),
        Q!("    csel            " "x12, x12, x16, ne"),
        Q!("    csel            " "x13, x13, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #112]"),
        Q!("    csel            " "x14, x14, x16, ne"),
        Q!("    csel            " "x15, x15, x17, ne"),
        Q!("    add             " p0!() ", " p0!() ", #128"),

        Q!("    cmp             " bf!() ", #3"),
        Q!("    ldp             " "x16, x17, [" p0!() "]"),
        Q!("    csel            " "x0, x0, x16, ne"),
        Q!("    csel            " "x1, x1, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #16]"),
        Q!("    csel            " "x2, x2, x16, ne"),
        Q!("    csel            " "x3, x3, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #32]"),
        Q!("    csel            " "x4, x4, x16, ne"),
        Q!("    csel            " "x5, x5, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #48]"),
        Q!("    csel            " "x6, x6, x16, ne"),
        Q!("    csel            " "x7, x7, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #64]"),
        Q!("    csel            " "x8, x8, x16, ne"),
        Q!("    csel            " "x9, x9, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #80]"),
        Q!("    csel            " "x10, x10, x16, ne"),
        Q!("    csel            " "x11, x11, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #96]"),
        Q!("    csel            " "x12, x12, x16, ne"),
        Q!("    csel            " "x13, x13, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #112]"),
        Q!("    csel            " "x14, x14, x16, ne"),
        Q!("    csel            " "x15, x15, x17, ne"),
        Q!("    add             " p0!() ", " p0!() ", #128"),

        Q!("    cmp             " bf!() ", #4"),
        Q!("    ldp             " "x16, x17, [" p0!() "]"),
        Q!("    csel            " "x0, x0, x16, ne"),
        Q!("    csel            " "x1, x1, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #16]"),
        Q!("    csel            " "x2, x2, x16, ne"),
        Q!("    csel            " "x3, x3, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #32]"),
        Q!("    csel            " "x4, x4, x16, ne"),
        Q!("    csel            " "x5, x5, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #48]"),
        Q!("    csel            " "x6, x6, x16, ne"),
        Q!("    csel            " "x7, x7, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #64]"),
        Q!("    csel            " "x8, x8, x16, ne"),
        Q!("    csel            " "x9, x9, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #80]"),
        Q!("    csel            " "x10, x10, x16, ne"),
        Q!("    csel            " "x11, x11, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #96]"),
        Q!("    csel            " "x12, x12, x16, ne"),
        Q!("    csel            " "x13, x13, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #112]"),
        Q!("    csel            " "x14, x14, x16, ne"),
        Q!("    csel            " "x15, x15, x17, ne"),
        Q!("    add             " p0!() ", " p0!() ", #128"),

        Q!("    cmp             " bf!() ", #5"),
        Q!("    ldp             " "x16, x17, [" p0!() "]"),
        Q!("    csel            " "x0, x0, x16, ne"),
        Q!("    csel            " "x1, x1, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #16]"),
        Q!("    csel            " "x2, x2, x16, ne"),
        Q!("    csel            " "x3, x3, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #32]"),
        Q!("    csel            " "x4, x4, x16, ne"),
        Q!("    csel            " "x5, x5, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #48]"),
        Q!("    csel            " "x6, x6, x16, ne"),
        Q!("    csel            " "x7, x7, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #64]"),
        Q!("    csel            " "x8, x8, x16, ne"),
        Q!("    csel            " "x9, x9, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #80]"),
        Q!("    csel            " "x10, x10, x16, ne"),
        Q!("    csel            " "x11, x11, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #96]"),
        Q!("    csel            " "x12, x12, x16, ne"),
        Q!("    csel            " "x13, x13, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #112]"),
        Q!("    csel            " "x14, x14, x16, ne"),
        Q!("    csel            " "x15, x15, x17, ne"),
        Q!("    add             " p0!() ", " p0!() ", #128"),

        Q!("    cmp             " bf!() ", #6"),
        Q!("    ldp             " "x16, x17, [" p0!() "]"),
        Q!("    csel            " "x0, x0, x16, ne"),
        Q!("    csel            " "x1, x1, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #16]"),
        Q!("    csel            " "x2, x2, x16, ne"),
        Q!("    csel            " "x3, x3, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #32]"),
        Q!("    csel            " "x4, x4, x16, ne"),
        Q!("    csel            " "x5, x5, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #48]"),
        Q!("    csel            " "x6, x6, x16, ne"),
        Q!("    csel            " "x7, x7, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #64]"),
        Q!("    csel            " "x8, x8, x16, ne"),
        Q!("    csel            " "x9, x9, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #80]"),
        Q!("    csel            " "x10, x10, x16, ne"),
        Q!("    csel            " "x11, x11, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #96]"),
        Q!("    csel            " "x12, x12, x16, ne"),
        Q!("    csel            " "x13, x13, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #112]"),
        Q!("    csel            " "x14, x14, x16, ne"),
        Q!("    csel            " "x15, x15, x17, ne"),
        Q!("    add             " p0!() ", " p0!() ", #128"),

        Q!("    cmp             " bf!() ", #7"),
        Q!("    ldp             " "x16, x17, [" p0!() "]"),
        Q!("    csel            " "x0, x0, x16, ne"),
        Q!("    csel            " "x1, x1, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #16]"),
        Q!("    csel            " "x2, x2, x16, ne"),
        Q!("    csel            " "x3, x3, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #32]"),
        Q!("    csel            " "x4, x4, x16, ne"),
        Q!("    csel            " "x5, x5, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #48]"),
        Q!("    csel            " "x6, x6, x16, ne"),
        Q!("    csel            " "x7, x7, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #64]"),
        Q!("    csel            " "x8, x8, x16, ne"),
        Q!("    csel            " "x9, x9, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #80]"),
        Q!("    csel            " "x10, x10, x16, ne"),
        Q!("    csel            " "x11, x11, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #96]"),
        Q!("    csel            " "x12, x12, x16, ne"),
        Q!("    csel            " "x13, x13, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #112]"),
        Q!("    csel            " "x14, x14, x16, ne"),
        Q!("    csel            " "x15, x15, x17, ne"),
        Q!("    add             " p0!() ", " p0!() ", #128"),

        Q!("    cmp             " bf!() ", #8"),
        Q!("    ldp             " "x16, x17, [" p0!() "]"),
        Q!("    csel            " "x0, x0, x16, ne"),
        Q!("    csel            " "x1, x1, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #16]"),
        Q!("    csel            " "x2, x2, x16, ne"),
        Q!("    csel            " "x3, x3, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #32]"),
        Q!("    csel            " "x4, x4, x16, ne"),
        Q!("    csel            " "x5, x5, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #48]"),
        Q!("    csel            " "x6, x6, x16, ne"),
        Q!("    csel            " "x7, x7, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #64]"),
        Q!("    csel            " "x8, x8, x16, ne"),
        Q!("    csel            " "x9, x9, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #80]"),
        Q!("    csel            " "x10, x10, x16, ne"),
        Q!("    csel            " "x11, x11, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #96]"),
        Q!("    csel            " "x12, x12, x16, ne"),
        Q!("    csel            " "x13, x13, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #112]"),
        Q!("    csel            " "x14, x14, x16, ne"),
        Q!("    csel            " "x15, x15, x17, ne"),

        Q!("    stp             " "x0, x1, [" tabent!() "]"),
        Q!("    stp             " "x2, x3, [" tabent!() "+ 16]"),
        Q!("    stp             " "x4, x5, [" tabent!() "+ 32]"),
        Q!("    stp             " "x6, x7, [" tabent!() "+ 48]"),
        Q!("    stp             " "x8, x9, [" tabent!() "+ 64]"),
        Q!("    stp             " "x10, x11, [" tabent!() "+ 80]"),
        Q!("    stp             " "x12, x13, [" tabent!() "+ 96]"),
        Q!("    stp             " "x14, x15, [" tabent!() "+ 112]"),

        // Add those elements to initialize the accumulator for bit position 252

        Q!("    add             " p0!() ", " acc!()),
        Q!("    add             " p1!() ", " tabent!()),
        Q!("    add             " p2!() ", " btabent!()),
        Q!("    bl              " Label!("Ledwards25519_scalarmuldouble_alt_pepadd", 4, After)),

        // Main loop with acc = [scalar/2^i] * point + [bscalar/2^i] * basepoint
        // Start with i = 252 for bits 248..251 and go down four at a time to 3..0

        Q!(Label!("Ledwards25519_scalarmuldouble_alt_loop", 5) ":"),

        Q!("    sub             " i!() ", " i!() ", #4"),

        // Double to acc' = 2 * acc

        Q!("    add             " p0!() ", " acc!()),
        Q!("    add             " p1!() ", " acc!()),
        Q!("    bl              " Label!("Ledwards25519_scalarmuldouble_alt_pdouble", 6, After)),

        // Get btable entry, first getting the adjusted bitfield...

        Q!("    lsr             " "x0, " i!() ", #6"),
        Q!("    add             " "x1, " bscalar!()),
        Q!("    ldr             " "x2, [x1, x0, lsl #3]"),
        Q!("    lsr             " "x3, x2, " i!()),
        Q!("    and             " "x0, x3, #15"),
        Q!("    subs            " bf!() ", x0, #8"),
        Q!("    cneg            " bf!() ", " bf!() ", cc"),
        Q!("    csetm           " cf!() ", cc"),

        // ... then doing constant-time lookup with the appropriate index...

        Q!("    adrp            " "x14, " PageRef!("edwards25519_scalarmuldouble_alt_constant")),
        Q!("    add             " "x14, x14, " PageOffRef!("edwards25519_scalarmuldouble_alt_constant")),

        Q!("    mov             " "x0, #1"),
        Q!("    mov             " "x1, xzr"),
        Q!("    mov             " "x2, xzr"),
        Q!("    mov             " "x3, xzr"),
        Q!("    mov             " "x4, #1"),
        Q!("    mov             " "x5, xzr"),
        Q!("    mov             " "x6, xzr"),
        Q!("    mov             " "x7, xzr"),
        Q!("    mov             " "x8, xzr"),
        Q!("    mov             " "x9, xzr"),
        Q!("    mov             " "x10, xzr"),
        Q!("    mov             " "x11, xzr"),

        Q!("    cmp             " bf!() ", #1"),
        Q!("    ldp             " "x12, x13, [x14]"),
        Q!("    csel            " "x0, x0, x12, ne"),
        Q!("    csel            " "x1, x1, x13, ne"),
        Q!("    ldp             " "x12, x13, [x14, #16]"),
        Q!("    csel            " "x2, x2, x12, ne"),
        Q!("    csel            " "x3, x3, x13, ne"),
        Q!("    ldp             " "x12, x13, [x14, #32]"),
        Q!("    csel            " "x4, x4, x12, ne"),
        Q!("    csel            " "x5, x5, x13, ne"),
        Q!("    ldp             " "x12, x13, [x14, #48]"),
        Q!("    csel            " "x6, x6, x12, ne"),
        Q!("    csel            " "x7, x7, x13, ne"),
        Q!("    ldp             " "x12, x13, [x14, #64]"),
        Q!("    csel            " "x8, x8, x12, ne"),
        Q!("    csel            " "x9, x9, x13, ne"),
        Q!("    ldp             " "x12, x13, [x14, #80]"),
        Q!("    csel            " "x10, x10, x12, ne"),
        Q!("    csel            " "x11, x11, x13, ne"),
        Q!("    add             " "x14, x14, #96"),

        Q!("    cmp             " bf!() ", #2"),
        Q!("    ldp             " "x12, x13, [x14]"),
        Q!("    csel            " "x0, x0, x12, ne"),
        Q!("    csel            " "x1, x1, x13, ne"),
        Q!("    ldp             " "x12, x13, [x14, #16]"),
        Q!("    csel            " "x2, x2, x12, ne"),
        Q!("    csel            " "x3, x3, x13, ne"),
        Q!("    ldp             " "x12, x13, [x14, #32]"),
        Q!("    csel            " "x4, x4, x12, ne"),
        Q!("    csel            " "x5, x5, x13, ne"),
        Q!("    ldp             " "x12, x13, [x14, #48]"),
        Q!("    csel            " "x6, x6, x12, ne"),
        Q!("    csel            " "x7, x7, x13, ne"),
        Q!("    ldp             " "x12, x13, [x14, #64]"),
        Q!("    csel            " "x8, x8, x12, ne"),
        Q!("    csel            " "x9, x9, x13, ne"),
        Q!("    ldp             " "x12, x13, [x14, #80]"),
        Q!("    csel            " "x10, x10, x12, ne"),
        Q!("    csel            " "x11, x11, x13, ne"),
        Q!("    add             " "x14, x14, #96"),

        Q!("    cmp             " bf!() ", #3"),
        Q!("    ldp             " "x12, x13, [x14]"),
        Q!("    csel            " "x0, x0, x12, ne"),
        Q!("    csel            " "x1, x1, x13, ne"),
        Q!("    ldp             " "x12, x13, [x14, #16]"),
        Q!("    csel            " "x2, x2, x12, ne"),
        Q!("    csel            " "x3, x3, x13, ne"),
        Q!("    ldp             " "x12, x13, [x14, #32]"),
        Q!("    csel            " "x4, x4, x12, ne"),
        Q!("    csel            " "x5, x5, x13, ne"),
        Q!("    ldp             " "x12, x13, [x14, #48]"),
        Q!("    csel            " "x6, x6, x12, ne"),
        Q!("    csel            " "x7, x7, x13, ne"),
        Q!("    ldp             " "x12, x13, [x14, #64]"),
        Q!("    csel            " "x8, x8, x12, ne"),
        Q!("    csel            " "x9, x9, x13, ne"),
        Q!("    ldp             " "x12, x13, [x14, #80]"),
        Q!("    csel            " "x10, x10, x12, ne"),
        Q!("    csel            " "x11, x11, x13, ne"),
        Q!("    add             " "x14, x14, #96"),

        Q!("    cmp             " bf!() ", #4"),
        Q!("    ldp             " "x12, x13, [x14]"),
        Q!("    csel            " "x0, x0, x12, ne"),
        Q!("    csel            " "x1, x1, x13, ne"),
        Q!("    ldp             " "x12, x13, [x14, #16]"),
        Q!("    csel            " "x2, x2, x12, ne"),
        Q!("    csel            " "x3, x3, x13, ne"),
        Q!("    ldp             " "x12, x13, [x14, #32]"),
        Q!("    csel            " "x4, x4, x12, ne"),
        Q!("    csel            " "x5, x5, x13, ne"),
        Q!("    ldp             " "x12, x13, [x14, #48]"),
        Q!("    csel            " "x6, x6, x12, ne"),
        Q!("    csel            " "x7, x7, x13, ne"),
        Q!("    ldp             " "x12, x13, [x14, #64]"),
        Q!("    csel            " "x8, x8, x12, ne"),
        Q!("    csel            " "x9, x9, x13, ne"),
        Q!("    ldp             " "x12, x13, [x14, #80]"),
        Q!("    csel            " "x10, x10, x12, ne"),
        Q!("    csel            " "x11, x11, x13, ne"),
        Q!("    add             " "x14, x14, #96"),

        Q!("    cmp             " bf!() ", #5"),
        Q!("    ldp             " "x12, x13, [x14]"),
        Q!("    csel            " "x0, x0, x12, ne"),
        Q!("    csel            " "x1, x1, x13, ne"),
        Q!("    ldp             " "x12, x13, [x14, #16]"),
        Q!("    csel            " "x2, x2, x12, ne"),
        Q!("    csel            " "x3, x3, x13, ne"),
        Q!("    ldp             " "x12, x13, [x14, #32]"),
        Q!("    csel            " "x4, x4, x12, ne"),
        Q!("    csel            " "x5, x5, x13, ne"),
        Q!("    ldp             " "x12, x13, [x14, #48]"),
        Q!("    csel            " "x6, x6, x12, ne"),
        Q!("    csel            " "x7, x7, x13, ne"),
        Q!("    ldp             " "x12, x13, [x14, #64]"),
        Q!("    csel            " "x8, x8, x12, ne"),
        Q!("    csel            " "x9, x9, x13, ne"),
        Q!("    ldp             " "x12, x13, [x14, #80]"),
        Q!("    csel            " "x10, x10, x12, ne"),
        Q!("    csel            " "x11, x11, x13, ne"),
        Q!("    add             " "x14, x14, #96"),

        Q!("    cmp             " bf!() ", #6"),
        Q!("    ldp             " "x12, x13, [x14]"),
        Q!("    csel            " "x0, x0, x12, ne"),
        Q!("    csel            " "x1, x1, x13, ne"),
        Q!("    ldp             " "x12, x13, [x14, #16]"),
        Q!("    csel            " "x2, x2, x12, ne"),
        Q!("    csel            " "x3, x3, x13, ne"),
        Q!("    ldp             " "x12, x13, [x14, #32]"),
        Q!("    csel            " "x4, x4, x12, ne"),
        Q!("    csel            " "x5, x5, x13, ne"),
        Q!("    ldp             " "x12, x13, [x14, #48]"),
        Q!("    csel            " "x6, x6, x12, ne"),
        Q!("    csel            " "x7, x7, x13, ne"),
        Q!("    ldp             " "x12, x13, [x14, #64]"),
        Q!("    csel            " "x8, x8, x12, ne"),
        Q!("    csel            " "x9, x9, x13, ne"),
        Q!("    ldp             " "x12, x13, [x14, #80]"),
        Q!("    csel            " "x10, x10, x12, ne"),
        Q!("    csel            " "x11, x11, x13, ne"),
        Q!("    add             " "x14, x14, #96"),

        Q!("    cmp             " bf!() ", #7"),
        Q!("    ldp             " "x12, x13, [x14]"),
        Q!("    csel            " "x0, x0, x12, ne"),
        Q!("    csel            " "x1, x1, x13, ne"),
        Q!("    ldp             " "x12, x13, [x14, #16]"),
        Q!("    csel            " "x2, x2, x12, ne"),
        Q!("    csel            " "x3, x3, x13, ne"),
        Q!("    ldp             " "x12, x13, [x14, #32]"),
        Q!("    csel            " "x4, x4, x12, ne"),
        Q!("    csel            " "x5, x5, x13, ne"),
        Q!("    ldp             " "x12, x13, [x14, #48]"),
        Q!("    csel            " "x6, x6, x12, ne"),
        Q!("    csel            " "x7, x7, x13, ne"),
        Q!("    ldp             " "x12, x13, [x14, #64]"),
        Q!("    csel            " "x8, x8, x12, ne"),
        Q!("    csel            " "x9, x9, x13, ne"),
        Q!("    ldp             " "x12, x13, [x14, #80]"),
        Q!("    csel            " "x10, x10, x12, ne"),
        Q!("    csel            " "x11, x11, x13, ne"),
        Q!("    add             " "x14, x14, #96"),

        Q!("    cmp             " bf!() ", #8"),
        Q!("    ldp             " "x12, x13, [x14]"),
        Q!("    csel            " "x0, x0, x12, ne"),
        Q!("    csel            " "x1, x1, x13, ne"),
        Q!("    ldp             " "x12, x13, [x14, #16]"),
        Q!("    csel            " "x2, x2, x12, ne"),
        Q!("    csel            " "x3, x3, x13, ne"),
        Q!("    ldp             " "x12, x13, [x14, #32]"),
        Q!("    csel            " "x4, x4, x12, ne"),
        Q!("    csel            " "x5, x5, x13, ne"),
        Q!("    ldp             " "x12, x13, [x14, #48]"),
        Q!("    csel            " "x6, x6, x12, ne"),
        Q!("    csel            " "x7, x7, x13, ne"),
        Q!("    ldp             " "x12, x13, [x14, #64]"),
        Q!("    csel            " "x8, x8, x12, ne"),
        Q!("    csel            " "x9, x9, x13, ne"),
        Q!("    ldp             " "x12, x13, [x14, #80]"),
        Q!("    csel            " "x10, x10, x12, ne"),
        Q!("    csel            " "x11, x11, x13, ne"),

        // ... then optionally negating before storing. The table entry
        // is in precomputed form and we currently have
        //
        //      [x3;x2;x1;x0] = y - x
        //      [x7;x6;x5;x4] = x + y
        //      [x11;x10;x9;x8] = 2 * d * x * y
        //
        // Negation for Edwards curves is -(x,y) = (-x,y), which in this modified
        // form amounts to swapping the first two fields and negating the third.
        // The negation does not always fully reduce even mod 2^256-38 in the zero
        // case, instead giving -0 = 2^256-38. But that is fine since the result is
        // always fed to a multiplication inside the "pepadd" function below that
        // handles any 256-bit input.

        Q!("    cmp             " cf!() ", xzr"),

        Q!("    csel            " "x12, x0, x4, eq"),
        Q!("    csel            " "x4, x0, x4, ne"),
        Q!("    csel            " "x13, x1, x5, eq"),
        Q!("    csel            " "x5, x1, x5, ne"),
        Q!("    csel            " "x14, x2, x6, eq"),
        Q!("    csel            " "x6, x2, x6, ne"),
        Q!("    csel            " "x15, x3, x7, eq"),
        Q!("    csel            " "x7, x3, x7, ne"),

        Q!("    eor             " "x8, x8, " cf!()),
        Q!("    eor             " "x9, x9, " cf!()),
        Q!("    eor             " "x10, x10, " cf!()),
        Q!("    eor             " "x11, x11, " cf!()),
        Q!("    mov             " "x0, #37"),
        Q!("    and             " "x0, x0, " cf!()),
        Q!("    subs            " "x8, x8, x0"),
        Q!("    sbcs            " "x9, x9, xzr"),
        Q!("    sbcs            " "x10, x10, xzr"),
        Q!("    sbc             " "x11, x11, xzr"),

        Q!("    stp             " "x12, x13, [" btabent!() "]"),
        Q!("    stp             " "x14, x15, [" btabent!() "+ 16]"),
        Q!("    stp             " "x4, x5, [" btabent!() "+ 32]"),
        Q!("    stp             " "x6, x7, [" btabent!() "+ 48]"),
        Q!("    stp             " "x8, x9, [" btabent!() "+ 64]"),
        Q!("    stp             " "x10, x11, [" btabent!() "+ 80]"),

        // Get table entry, first getting the adjusted bitfield...

        Q!("    lsr             " "x0, " i!() ", #6"),
        Q!("    ldr             " "x1, [sp, x0, lsl #3]"),
        Q!("    lsr             " "x2, x1, " i!()),
        Q!("    and             " "x0, x2, #15"),
        Q!("    subs            " bf!() ", x0, #8"),
        Q!("    cneg            " bf!() ", " bf!() ", cc"),
        Q!("    csetm           " cf!() ", cc"),

        // ... then getting the unadjusted table entry

        Q!("    add             " p0!() ", " tab!()),

        Q!("    mov             " "x0, xzr"),
        Q!("    mov             " "x1, xzr"),
        Q!("    mov             " "x2, xzr"),
        Q!("    mov             " "x3, xzr"),
        Q!("    mov             " "x4, #1"),
        Q!("    mov             " "x5, xzr"),
        Q!("    mov             " "x6, xzr"),
        Q!("    mov             " "x7, xzr"),
        Q!("    mov             " "x8, #1"),
        Q!("    mov             " "x9, xzr"),
        Q!("    mov             " "x10, xzr"),
        Q!("    mov             " "x11, xzr"),
        Q!("    mov             " "x12, xzr"),
        Q!("    mov             " "x13, xzr"),
        Q!("    mov             " "x14, xzr"),
        Q!("    mov             " "x15, xzr"),

        Q!("    cmp             " bf!() ", #1"),
        Q!("    ldp             " "x16, x17, [" p0!() "]"),
        Q!("    csel            " "x0, x0, x16, ne"),
        Q!("    csel            " "x1, x1, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #16]"),
        Q!("    csel            " "x2, x2, x16, ne"),
        Q!("    csel            " "x3, x3, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #32]"),
        Q!("    csel            " "x4, x4, x16, ne"),
        Q!("    csel            " "x5, x5, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #48]"),
        Q!("    csel            " "x6, x6, x16, ne"),
        Q!("    csel            " "x7, x7, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #64]"),
        Q!("    csel            " "x8, x8, x16, ne"),
        Q!("    csel            " "x9, x9, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #80]"),
        Q!("    csel            " "x10, x10, x16, ne"),
        Q!("    csel            " "x11, x11, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #96]"),
        Q!("    csel            " "x12, x12, x16, ne"),
        Q!("    csel            " "x13, x13, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #112]"),
        Q!("    csel            " "x14, x14, x16, ne"),
        Q!("    csel            " "x15, x15, x17, ne"),
        Q!("    add             " p0!() ", " p0!() ", #128"),

        Q!("    cmp             " bf!() ", #2"),
        Q!("    ldp             " "x16, x17, [" p0!() "]"),
        Q!("    csel            " "x0, x0, x16, ne"),
        Q!("    csel            " "x1, x1, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #16]"),
        Q!("    csel            " "x2, x2, x16, ne"),
        Q!("    csel            " "x3, x3, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #32]"),
        Q!("    csel            " "x4, x4, x16, ne"),
        Q!("    csel            " "x5, x5, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #48]"),
        Q!("    csel            " "x6, x6, x16, ne"),
        Q!("    csel            " "x7, x7, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #64]"),
        Q!("    csel            " "x8, x8, x16, ne"),
        Q!("    csel            " "x9, x9, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #80]"),
        Q!("    csel            " "x10, x10, x16, ne"),
        Q!("    csel            " "x11, x11, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #96]"),
        Q!("    csel            " "x12, x12, x16, ne"),
        Q!("    csel            " "x13, x13, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #112]"),
        Q!("    csel            " "x14, x14, x16, ne"),
        Q!("    csel            " "x15, x15, x17, ne"),
        Q!("    add             " p0!() ", " p0!() ", #128"),

        Q!("    cmp             " bf!() ", #3"),
        Q!("    ldp             " "x16, x17, [" p0!() "]"),
        Q!("    csel            " "x0, x0, x16, ne"),
        Q!("    csel            " "x1, x1, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #16]"),
        Q!("    csel            " "x2, x2, x16, ne"),
        Q!("    csel            " "x3, x3, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #32]"),
        Q!("    csel            " "x4, x4, x16, ne"),
        Q!("    csel            " "x5, x5, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #48]"),
        Q!("    csel            " "x6, x6, x16, ne"),
        Q!("    csel            " "x7, x7, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #64]"),
        Q!("    csel            " "x8, x8, x16, ne"),
        Q!("    csel            " "x9, x9, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #80]"),
        Q!("    csel            " "x10, x10, x16, ne"),
        Q!("    csel            " "x11, x11, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #96]"),
        Q!("    csel            " "x12, x12, x16, ne"),
        Q!("    csel            " "x13, x13, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #112]"),
        Q!("    csel            " "x14, x14, x16, ne"),
        Q!("    csel            " "x15, x15, x17, ne"),
        Q!("    add             " p0!() ", " p0!() ", #128"),

        Q!("    cmp             " bf!() ", #4"),
        Q!("    ldp             " "x16, x17, [" p0!() "]"),
        Q!("    csel            " "x0, x0, x16, ne"),
        Q!("    csel            " "x1, x1, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #16]"),
        Q!("    csel            " "x2, x2, x16, ne"),
        Q!("    csel            " "x3, x3, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #32]"),
        Q!("    csel            " "x4, x4, x16, ne"),
        Q!("    csel            " "x5, x5, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #48]"),
        Q!("    csel            " "x6, x6, x16, ne"),
        Q!("    csel            " "x7, x7, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #64]"),
        Q!("    csel            " "x8, x8, x16, ne"),
        Q!("    csel            " "x9, x9, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #80]"),
        Q!("    csel            " "x10, x10, x16, ne"),
        Q!("    csel            " "x11, x11, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #96]"),
        Q!("    csel            " "x12, x12, x16, ne"),
        Q!("    csel            " "x13, x13, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #112]"),
        Q!("    csel            " "x14, x14, x16, ne"),
        Q!("    csel            " "x15, x15, x17, ne"),
        Q!("    add             " p0!() ", " p0!() ", #128"),

        Q!("    cmp             " bf!() ", #5"),
        Q!("    ldp             " "x16, x17, [" p0!() "]"),
        Q!("    csel            " "x0, x0, x16, ne"),
        Q!("    csel            " "x1, x1, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #16]"),
        Q!("    csel            " "x2, x2, x16, ne"),
        Q!("    csel            " "x3, x3, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #32]"),
        Q!("    csel            " "x4, x4, x16, ne"),
        Q!("    csel            " "x5, x5, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #48]"),
        Q!("    csel            " "x6, x6, x16, ne"),
        Q!("    csel            " "x7, x7, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #64]"),
        Q!("    csel            " "x8, x8, x16, ne"),
        Q!("    csel            " "x9, x9, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #80]"),
        Q!("    csel            " "x10, x10, x16, ne"),
        Q!("    csel            " "x11, x11, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #96]"),
        Q!("    csel            " "x12, x12, x16, ne"),
        Q!("    csel            " "x13, x13, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #112]"),
        Q!("    csel            " "x14, x14, x16, ne"),
        Q!("    csel            " "x15, x15, x17, ne"),
        Q!("    add             " p0!() ", " p0!() ", #128"),

        Q!("    cmp             " bf!() ", #6"),
        Q!("    ldp             " "x16, x17, [" p0!() "]"),
        Q!("    csel            " "x0, x0, x16, ne"),
        Q!("    csel            " "x1, x1, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #16]"),
        Q!("    csel            " "x2, x2, x16, ne"),
        Q!("    csel            " "x3, x3, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #32]"),
        Q!("    csel            " "x4, x4, x16, ne"),
        Q!("    csel            " "x5, x5, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #48]"),
        Q!("    csel            " "x6, x6, x16, ne"),
        Q!("    csel            " "x7, x7, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #64]"),
        Q!("    csel            " "x8, x8, x16, ne"),
        Q!("    csel            " "x9, x9, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #80]"),
        Q!("    csel            " "x10, x10, x16, ne"),
        Q!("    csel            " "x11, x11, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #96]"),
        Q!("    csel            " "x12, x12, x16, ne"),
        Q!("    csel            " "x13, x13, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #112]"),
        Q!("    csel            " "x14, x14, x16, ne"),
        Q!("    csel            " "x15, x15, x17, ne"),
        Q!("    add             " p0!() ", " p0!() ", #128"),

        Q!("    cmp             " bf!() ", #7"),
        Q!("    ldp             " "x16, x17, [" p0!() "]"),
        Q!("    csel            " "x0, x0, x16, ne"),
        Q!("    csel            " "x1, x1, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #16]"),
        Q!("    csel            " "x2, x2, x16, ne"),
        Q!("    csel            " "x3, x3, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #32]"),
        Q!("    csel            " "x4, x4, x16, ne"),
        Q!("    csel            " "x5, x5, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #48]"),
        Q!("    csel            " "x6, x6, x16, ne"),
        Q!("    csel            " "x7, x7, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #64]"),
        Q!("    csel            " "x8, x8, x16, ne"),
        Q!("    csel            " "x9, x9, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #80]"),
        Q!("    csel            " "x10, x10, x16, ne"),
        Q!("    csel            " "x11, x11, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #96]"),
        Q!("    csel            " "x12, x12, x16, ne"),
        Q!("    csel            " "x13, x13, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #112]"),
        Q!("    csel            " "x14, x14, x16, ne"),
        Q!("    csel            " "x15, x15, x17, ne"),
        Q!("    add             " p0!() ", " p0!() ", #128"),

        Q!("    cmp             " bf!() ", #8"),
        Q!("    ldp             " "x16, x17, [" p0!() "]"),
        Q!("    csel            " "x0, x0, x16, ne"),
        Q!("    csel            " "x1, x1, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #16]"),
        Q!("    csel            " "x2, x2, x16, ne"),
        Q!("    csel            " "x3, x3, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #32]"),
        Q!("    csel            " "x4, x4, x16, ne"),
        Q!("    csel            " "x5, x5, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #48]"),
        Q!("    csel            " "x6, x6, x16, ne"),
        Q!("    csel            " "x7, x7, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #64]"),
        Q!("    csel            " "x8, x8, x16, ne"),
        Q!("    csel            " "x9, x9, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #80]"),
        Q!("    csel            " "x10, x10, x16, ne"),
        Q!("    csel            " "x11, x11, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #96]"),
        Q!("    csel            " "x12, x12, x16, ne"),
        Q!("    csel            " "x13, x13, x17, ne"),
        Q!("    ldp             " "x16, x17, [" p0!() ", #112]"),
        Q!("    csel            " "x14, x14, x16, ne"),
        Q!("    csel            " "x15, x15, x17, ne"),

        // ... then optionally negating before storing. This time the table
        // entry is extended-projective, and is in registers thus:
        //
        //      [x3;x2;x1;x0] = X
        //      [x7;x6;x5;x4] = Y
        //      [x11;x10;x9;x8] = Z
        //      [x15;x14;x13;x12] = W
        //
        // This time we just need to negate the X and the W fields.
        // The crude way negation is done can result in values of X or W
        // (when initially zero before negation) being exactly equal to
        // 2^256-38, but the "pepadd" function handles that correctly.

        Q!("    eor             " "x0, x0, " cf!()),
        Q!("    eor             " "x1, x1, " cf!()),
        Q!("    eor             " "x2, x2, " cf!()),
        Q!("    eor             " "x3, x3, " cf!()),
        Q!("    mov             " "x16, #37"),
        Q!("    and             " "x16, x16, " cf!()),
        Q!("    subs            " "x0, x0, x16"),
        Q!("    sbcs            " "x1, x1, xzr"),
        Q!("    sbcs            " "x2, x2, xzr"),
        Q!("    sbc             " "x3, x3, xzr"),

        Q!("    eor             " "x12, x12, " cf!()),
        Q!("    eor             " "x13, x13, " cf!()),
        Q!("    eor             " "x14, x14, " cf!()),
        Q!("    eor             " "x15, x15, " cf!()),
        Q!("    subs            " "x12, x12, x16"),
        Q!("    sbcs            " "x13, x13, xzr"),
        Q!("    sbcs            " "x14, x14, xzr"),
        Q!("    sbc             " "x15, x15, xzr"),

        Q!("    stp             " "x0, x1, [" tabent!() "]"),
        Q!("    stp             " "x2, x3, [" tabent!() "+ 16]"),
        Q!("    stp             " "x4, x5, [" tabent!() "+ 32]"),
        Q!("    stp             " "x6, x7, [" tabent!() "+ 48]"),
        Q!("    stp             " "x8, x9, [" tabent!() "+ 64]"),
        Q!("    stp             " "x10, x11, [" tabent!() "+ 80]"),
        Q!("    stp             " "x12, x13, [" tabent!() "+ 96]"),
        Q!("    stp             " "x14, x15, [" tabent!() "+ 112]"),

        // Double to acc' = 4 * acc

        Q!("    add             " p0!() ", " acc!()),
        Q!("    add             " p1!() ", " acc!()),
        Q!("    bl              " Label!("Ledwards25519_scalarmuldouble_alt_pdouble", 6, After)),

        // Add tabent := tabent + btabent

        Q!("    add             " p0!() ", " tabent!()),
        Q!("    add             " p1!() ", " tabent!()),
        Q!("    add             " p2!() ", " btabent!()),
        Q!("    bl              " Label!("Ledwards25519_scalarmuldouble_alt_pepadd", 4, After)),

        // Double to acc' = 8 * acc

        Q!("    add             " p0!() ", " acc!()),
        Q!("    add             " p1!() ", " acc!()),
        Q!("    bl              " Label!("Ledwards25519_scalarmuldouble_alt_pdouble", 6, After)),

        // Double to acc' = 16 * acc

        Q!("    add             " p0!() ", " acc!()),
        Q!("    add             " p1!() ", " acc!()),
        Q!("    bl              " Label!("Ledwards25519_scalarmuldouble_alt_epdouble", 2, After)),

        // Add table entry, acc := acc + tabent

        Q!("    add             " p0!() ", " acc!()),
        Q!("    add             " p1!() ", " acc!()),
        Q!("    add             " p2!() ", " tabent!()),
        Q!("    bl              " Label!("Ledwards25519_scalarmuldouble_alt_epadd", 3, After)),

        // Loop down

        Q!("    cbnz            " i!() ", " Label!("Ledwards25519_scalarmuldouble_alt_loop", 5, Before)),

        // Modular inverse setup

        Q!("    add             " "x0, " tabent!()),
        Q!("    add             " "x1, " acc!() "+ 64"),

        // Inline copy of bignum_inv_p25519, identical except for stripping out
        // the prologue and epilogue saving and restoring registers and making
        // and reclaiming room on the stack. For more details and explanations see
        // "arm/curve25519/bignum_inv_p25519.S". Note that the stack it uses for
        // its own temporaries is 128 bytes, so it has no effect on variables
        // that are needed in the rest of our computation here: res, acc, tabent.

        Q!("    mov             " "x20, x0"),
        Q!("    mov             " "x10, #0xffffffffffffffed"),
        Q!("    mov             " "x11, #0xffffffffffffffff"),
        Q!("    stp             " "x10, x11, [sp]"),
        Q!("    mov             " "x12, #0x7fffffffffffffff"),
        Q!("    stp             " "x11, x12, [sp, #16]"),
        Q!("    ldp             " "x2, x3, [x1]"),
        Q!("    ldp             " "x4, x5, [x1, #16]"),
        Q!("    mov             " "x7, #0x13"),
        Q!("    lsr             " "x6, x5, #63"),
        Q!("    madd            " "x6, x7, x6, x7"),
        Q!("    adds            " "x2, x2, x6"),
        Q!("    adcs            " "x3, x3, xzr"),
        Q!("    adcs            " "x4, x4, xzr"),
        Q!("    orr             " "x5, x5, #0x8000000000000000"),
        Q!("    adcs            " "x5, x5, xzr"),
        Q!("    csel            " "x6, x7, xzr, cc"),
        Q!("    subs            " "x2, x2, x6"),
        Q!("    sbcs            " "x3, x3, xzr"),
        Q!("    sbcs            " "x4, x4, xzr"),
        Q!("    sbc             " "x5, x5, xzr"),
        Q!("    and             " "x5, x5, #0x7fffffffffffffff"),
        Q!("    stp             " "x2, x3, [sp, #32]"),
        Q!("    stp             " "x4, x5, [sp, #48]"),
        Q!("    stp             " "xzr, xzr, [sp, #64]"),
        Q!("    stp             " "xzr, xzr, [sp, #80]"),
        Q!("    mov             " "x10, #0x2099"),
        Q!("    movk            " "x10, #0x7502, lsl #16"),
        Q!("    movk            " "x10, #0x9e23, lsl #32"),
        Q!("    movk            " "x10, #0xa0f9, lsl #48"),
        Q!("    mov             " "x11, #0x2595"),
        Q!("    movk            " "x11, #0x1d13, lsl #16"),
        Q!("    movk            " "x11, #0x8f3f, lsl #32"),
        Q!("    movk            " "x11, #0xa8c6, lsl #48"),
        Q!("    mov             " "x12, #0x5242"),
        Q!("    movk            " "x12, #0x5ac, lsl #16"),
        Q!("    movk            " "x12, #0x8938, lsl #32"),
        Q!("    movk            " "x12, #0x6c6c, lsl #48"),
        Q!("    mov             " "x13, #0x615"),
        Q!("    movk            " "x13, #0x4177, lsl #16"),
        Q!("    movk            " "x13, #0x8b2, lsl #32"),
        Q!("    movk            " "x13, #0x2765, lsl #48"),
        Q!("    stp             " "x10, x11, [sp, #96]"),
        Q!("    stp             " "x12, x13, [sp, #112]"),
        Q!("    mov             " "x21, #0xa"),
        Q!("    mov             " "x22, #0x1"),
        Q!("    b               " Label!("Ledwards25519_scalarmuldouble_alt_invmidloop", 7, After)),
        Q!(Label!("Ledwards25519_scalarmuldouble_alt_invloop", 8) ":"),
        Q!("    cmp             " "x10, xzr"),
        Q!("    csetm           " "x14, mi"),
        Q!("    cneg            " "x10, x10, mi"),
        Q!("    cmp             " "x11, xzr"),
        Q!("    csetm           " "x15, mi"),
        Q!("    cneg            " "x11, x11, mi"),
        Q!("    cmp             " "x12, xzr"),
        Q!("    csetm           " "x16, mi"),
        Q!("    cneg            " "x12, x12, mi"),
        Q!("    cmp             " "x13, xzr"),
        Q!("    csetm           " "x17, mi"),
        Q!("    cneg            " "x13, x13, mi"),
        Q!("    and             " "x0, x10, x14"),
        Q!("    and             " "x1, x11, x15"),
        Q!("    add             " "x9, x0, x1"),
        Q!("    and             " "x0, x12, x16"),
        Q!("    and             " "x1, x13, x17"),
        Q!("    add             " "x19, x0, x1"),
        Q!("    ldr             " "x7, [sp]"),
        Q!("    eor             " "x1, x7, x14"),
        Q!("    mul             " "x0, x1, x10"),
        Q!("    umulh           " "x1, x1, x10"),
        Q!("    adds            " "x4, x9, x0"),
        Q!("    adc             " "x2, xzr, x1"),
        Q!("    ldr             " "x8, [sp, #32]"),
        Q!("    eor             " "x1, x8, x15"),
        Q!("    mul             " "x0, x1, x11"),
        Q!("    umulh           " "x1, x1, x11"),
        Q!("    adds            " "x4, x4, x0"),
        Q!("    adc             " "x2, x2, x1"),
        Q!("    eor             " "x1, x7, x16"),
        Q!("    mul             " "x0, x1, x12"),
        Q!("    umulh           " "x1, x1, x12"),
        Q!("    adds            " "x5, x19, x0"),
        Q!("    adc             " "x3, xzr, x1"),
        Q!("    eor             " "x1, x8, x17"),
        Q!("    mul             " "x0, x1, x13"),
        Q!("    umulh           " "x1, x1, x13"),
        Q!("    adds            " "x5, x5, x0"),
        Q!("    adc             " "x3, x3, x1"),
        Q!("    ldr             " "x7, [sp, #8]"),
        Q!("    eor             " "x1, x7, x14"),
        Q!("    mul             " "x0, x1, x10"),
        Q!("    umulh           " "x1, x1, x10"),
        Q!("    adds            " "x2, x2, x0"),
        Q!("    adc             " "x6, xzr, x1"),
        Q!("    ldr             " "x8, [sp, #40]"),
        Q!("    eor             " "x1, x8, x15"),
        Q!("    mul             " "x0, x1, x11"),
        Q!("    umulh           " "x1, x1, x11"),
        Q!("    adds            " "x2, x2, x0"),
        Q!("    adc             " "x6, x6, x1"),
        Q!("    extr            " "x4, x2, x4, #59"),
        Q!("    str             " "x4, [sp]"),
        Q!("    eor             " "x1, x7, x16"),
        Q!("    mul             " "x0, x1, x12"),
        Q!("    umulh           " "x1, x1, x12"),
        Q!("    adds            " "x3, x3, x0"),
        Q!("    adc             " "x4, xzr, x1"),
        Q!("    eor             " "x1, x8, x17"),
        Q!("    mul             " "x0, x1, x13"),
        Q!("    umulh           " "x1, x1, x13"),
        Q!("    adds            " "x3, x3, x0"),
        Q!("    adc             " "x4, x4, x1"),
        Q!("    extr            " "x5, x3, x5, #59"),
        Q!("    str             " "x5, [sp, #32]"),
        Q!("    ldr             " "x7, [sp, #16]"),
        Q!("    eor             " "x1, x7, x14"),
        Q!("    mul             " "x0, x1, x10"),
        Q!("    umulh           " "x1, x1, x10"),
        Q!("    adds            " "x6, x6, x0"),
        Q!("    adc             " "x5, xzr, x1"),
        Q!("    ldr             " "x8, [sp, #48]"),
        Q!("    eor             " "x1, x8, x15"),
        Q!("    mul             " "x0, x1, x11"),
        Q!("    umulh           " "x1, x1, x11"),
        Q!("    adds            " "x6, x6, x0"),
        Q!("    adc             " "x5, x5, x1"),
        Q!("    extr            " "x2, x6, x2, #59"),
        Q!("    str             " "x2, [sp, #8]"),
        Q!("    eor             " "x1, x7, x16"),
        Q!("    mul             " "x0, x1, x12"),
        Q!("    umulh           " "x1, x1, x12"),
        Q!("    adds            " "x4, x4, x0"),
        Q!("    adc             " "x2, xzr, x1"),
        Q!("    eor             " "x1, x8, x17"),
        Q!("    mul             " "x0, x1, x13"),
        Q!("    umulh           " "x1, x1, x13"),
        Q!("    adds            " "x4, x4, x0"),
        Q!("    adc             " "x2, x2, x1"),
        Q!("    extr            " "x3, x4, x3, #59"),
        Q!("    str             " "x3, [sp, #40]"),
        Q!("    ldr             " "x7, [sp, #24]"),
        Q!("    eor             " "x1, x7, x14"),
        Q!("    asr             " "x3, x1, #63"),
        Q!("    and             " "x3, x3, x10"),
        Q!("    neg             " "x3, x3"),
        Q!("    mul             " "x0, x1, x10"),
        Q!("    umulh           " "x1, x1, x10"),
        Q!("    adds            " "x5, x5, x0"),
        Q!("    adc             " "x3, x3, x1"),
        Q!("    ldr             " "x8, [sp, #56]"),
        Q!("    eor             " "x1, x8, x15"),
        Q!("    asr             " "x0, x1, #63"),
        Q!("    and             " "x0, x0, x11"),
        Q!("    sub             " "x3, x3, x0"),
        Q!("    mul             " "x0, x1, x11"),
        Q!("    umulh           " "x1, x1, x11"),
        Q!("    adds            " "x5, x5, x0"),
        Q!("    adc             " "x3, x3, x1"),
        Q!("    extr            " "x6, x5, x6, #59"),
        Q!("    str             " "x6, [sp, #16]"),
        Q!("    extr            " "x5, x3, x5, #59"),
        Q!("    str             " "x5, [sp, #24]"),
        Q!("    eor             " "x1, x7, x16"),
        Q!("    asr             " "x5, x1, #63"),
        Q!("    and             " "x5, x5, x12"),
        Q!("    neg             " "x5, x5"),
        Q!("    mul             " "x0, x1, x12"),
        Q!("    umulh           " "x1, x1, x12"),
        Q!("    adds            " "x2, x2, x0"),
        Q!("    adc             " "x5, x5, x1"),
        Q!("    eor             " "x1, x8, x17"),
        Q!("    asr             " "x0, x1, #63"),
        Q!("    and             " "x0, x0, x13"),
        Q!("    sub             " "x5, x5, x0"),
        Q!("    mul             " "x0, x1, x13"),
        Q!("    umulh           " "x1, x1, x13"),
        Q!("    adds            " "x2, x2, x0"),
        Q!("    adc             " "x5, x5, x1"),
        Q!("    extr            " "x4, x2, x4, #59"),
        Q!("    str             " "x4, [sp, #48]"),
        Q!("    extr            " "x2, x5, x2, #59"),
        Q!("    str             " "x2, [sp, #56]"),
        Q!("    ldr             " "x7, [sp, #64]"),
        Q!("    eor             " "x1, x7, x14"),
        Q!("    mul             " "x0, x1, x10"),
        Q!("    umulh           " "x1, x1, x10"),
        Q!("    adds            " "x4, x9, x0"),
        Q!("    adc             " "x2, xzr, x1"),
        Q!("    ldr             " "x8, [sp, #96]"),
        Q!("    eor             " "x1, x8, x15"),
        Q!("    mul             " "x0, x1, x11"),
        Q!("    umulh           " "x1, x1, x11"),
        Q!("    adds            " "x4, x4, x0"),
        Q!("    str             " "x4, [sp, #64]"),
        Q!("    adc             " "x2, x2, x1"),
        Q!("    eor             " "x1, x7, x16"),
        Q!("    mul             " "x0, x1, x12"),
        Q!("    umulh           " "x1, x1, x12"),
        Q!("    adds            " "x5, x19, x0"),
        Q!("    adc             " "x3, xzr, x1"),
        Q!("    eor             " "x1, x8, x17"),
        Q!("    mul             " "x0, x1, x13"),
        Q!("    umulh           " "x1, x1, x13"),
        Q!("    adds            " "x5, x5, x0"),
        Q!("    str             " "x5, [sp, #96]"),
        Q!("    adc             " "x3, x3, x1"),
        Q!("    ldr             " "x7, [sp, #72]"),
        Q!("    eor             " "x1, x7, x14"),
        Q!("    mul             " "x0, x1, x10"),
        Q!("    umulh           " "x1, x1, x10"),
        Q!("    adds            " "x2, x2, x0"),
        Q!("    adc             " "x6, xzr, x1"),
        Q!("    ldr             " "x8, [sp, #104]"),
        Q!("    eor             " "x1, x8, x15"),
        Q!("    mul             " "x0, x1, x11"),
        Q!("    umulh           " "x1, x1, x11"),
        Q!("    adds            " "x2, x2, x0"),
        Q!("    str             " "x2, [sp, #72]"),
        Q!("    adc             " "x6, x6, x1"),
        Q!("    eor             " "x1, x7, x16"),
        Q!("    mul             " "x0, x1, x12"),
        Q!("    umulh           " "x1, x1, x12"),
        Q!("    adds            " "x3, x3, x0"),
        Q!("    adc             " "x4, xzr, x1"),
        Q!("    eor             " "x1, x8, x17"),
        Q!("    mul             " "x0, x1, x13"),
        Q!("    umulh           " "x1, x1, x13"),
        Q!("    adds            " "x3, x3, x0"),
        Q!("    str             " "x3, [sp, #104]"),
        Q!("    adc             " "x4, x4, x1"),
        Q!("    ldr             " "x7, [sp, #80]"),
        Q!("    eor             " "x1, x7, x14"),
        Q!("    mul             " "x0, x1, x10"),
        Q!("    umulh           " "x1, x1, x10"),
        Q!("    adds            " "x6, x6, x0"),
        Q!("    adc             " "x5, xzr, x1"),
        Q!("    ldr             " "x8, [sp, #112]"),
        Q!("    eor             " "x1, x8, x15"),
        Q!("    mul             " "x0, x1, x11"),
        Q!("    umulh           " "x1, x1, x11"),
        Q!("    adds            " "x6, x6, x0"),
        Q!("    str             " "x6, [sp, #80]"),
        Q!("    adc             " "x5, x5, x1"),
        Q!("    eor             " "x1, x7, x16"),
        Q!("    mul             " "x0, x1, x12"),
        Q!("    umulh           " "x1, x1, x12"),
        Q!("    adds            " "x4, x4, x0"),
        Q!("    adc             " "x2, xzr, x1"),
        Q!("    eor             " "x1, x8, x17"),
        Q!("    mul             " "x0, x1, x13"),
        Q!("    umulh           " "x1, x1, x13"),
        Q!("    adds            " "x4, x4, x0"),
        Q!("    str             " "x4, [sp, #112]"),
        Q!("    adc             " "x2, x2, x1"),
        Q!("    ldr             " "x7, [sp, #88]"),
        Q!("    eor             " "x1, x7, x14"),
        Q!("    and             " "x3, x14, x10"),
        Q!("    neg             " "x3, x3"),
        Q!("    mul             " "x0, x1, x10"),
        Q!("    umulh           " "x1, x1, x10"),
        Q!("    adds            " "x5, x5, x0"),
        Q!("    adc             " "x3, x3, x1"),
        Q!("    ldr             " "x8, [sp, #120]"),
        Q!("    eor             " "x1, x8, x15"),
        Q!("    and             " "x0, x15, x11"),
        Q!("    sub             " "x3, x3, x0"),
        Q!("    mul             " "x0, x1, x11"),
        Q!("    umulh           " "x1, x1, x11"),
        Q!("    adds            " "x5, x5, x0"),
        Q!("    adc             " "x3, x3, x1"),
        Q!("    extr            " "x6, x3, x5, #63"),
        Q!("    ldp             " "x0, x1, [sp, #64]"),
        Q!("    add             " "x6, x6, x3, asr #63"),
        Q!("    mov             " "x3, #0x13"),
        Q!("    mul             " "x4, x6, x3"),
        Q!("    add             " "x5, x5, x6, lsl #63"),
        Q!("    smulh           " "x3, x6, x3"),
        Q!("    ldr             " "x6, [sp, #80]"),
        Q!("    adds            " "x0, x0, x4"),
        Q!("    adcs            " "x1, x1, x3"),
        Q!("    asr             " "x3, x3, #63"),
        Q!("    adcs            " "x6, x6, x3"),
        Q!("    adc             " "x5, x5, x3"),
        Q!("    stp             " "x0, x1, [sp, #64]"),
        Q!("    stp             " "x6, x5, [sp, #80]"),
        Q!("    eor             " "x1, x7, x16"),
        Q!("    and             " "x5, x16, x12"),
        Q!("    neg             " "x5, x5"),
        Q!("    mul             " "x0, x1, x12"),
        Q!("    umulh           " "x1, x1, x12"),
        Q!("    adds            " "x2, x2, x0"),
        Q!("    adc             " "x5, x5, x1"),
        Q!("    eor             " "x1, x8, x17"),
        Q!("    and             " "x0, x17, x13"),
        Q!("    sub             " "x5, x5, x0"),
        Q!("    mul             " "x0, x1, x13"),
        Q!("    umulh           " "x1, x1, x13"),
        Q!("    adds            " "x2, x2, x0"),
        Q!("    adc             " "x5, x5, x1"),
        Q!("    extr            " "x6, x5, x2, #63"),
        Q!("    ldp             " "x0, x1, [sp, #96]"),
        Q!("    add             " "x6, x6, x5, asr #63"),
        Q!("    mov             " "x5, #0x13"),
        Q!("    mul             " "x4, x6, x5"),
        Q!("    add             " "x2, x2, x6, lsl #63"),
        Q!("    smulh           " "x5, x6, x5"),
        Q!("    ldr             " "x3, [sp, #112]"),
        Q!("    adds            " "x0, x0, x4"),
        Q!("    adcs            " "x1, x1, x5"),
        Q!("    asr             " "x5, x5, #63"),
        Q!("    adcs            " "x3, x3, x5"),
        Q!("    adc             " "x2, x2, x5"),
        Q!("    stp             " "x0, x1, [sp, #96]"),
        Q!("    stp             " "x3, x2, [sp, #112]"),
        Q!(Label!("Ledwards25519_scalarmuldouble_alt_invmidloop", 7) ":"),
        Q!("    mov             " "x1, x22"),
        Q!("    ldr             " "x2, [sp]"),
        Q!("    ldr             " "x3, [sp, #32]"),
        Q!("    and             " "x4, x2, #0xfffff"),
        Q!("    orr             " "x4, x4, #0xfffffe0000000000"),
        Q!("    and             " "x5, x3, #0xfffff"),
        Q!("    orr             " "x5, x5, #0xc000000000000000"),
        Q!("    tst             " "x5, #0x1"),
        Q!("    csel            " "x6, x4, xzr, ne"),
        Q!("    ccmp            " "x1, xzr, #0x8, ne"),
        Q!("    cneg            " "x1, x1, ge"),
        Q!("    cneg            " "x6, x6, ge"),
        Q!("    csel            " "x4, x5, x4, ge"),
        Q!("    add             " "x5, x5, x6"),
        Q!("    add             " "x1, x1, #0x2"),
        Q!("    tst             " "x5, #0x2"),
        Q!("    asr             " "x5, x5, #1"),
        Q!("    csel            " "x6, x4, xzr, ne"),
        Q!("    ccmp            " "x1, xzr, #0x8, ne"),
        Q!("    cneg            " "x1, x1, ge"),
        Q!("    cneg            " "x6, x6, ge"),
        Q!("    csel            " "x4, x5, x4, ge"),
        Q!("    add             " "x5, x5, x6"),
        Q!("    add             " "x1, x1, #0x2"),
        Q!("    tst             " "x5, #0x2"),
        Q!("    asr             " "x5, x5, #1"),
        Q!("    csel            " "x6, x4, xzr, ne"),
        Q!("    ccmp            " "x1, xzr, #0x8, ne"),
        Q!("    cneg            " "x1, x1, ge"),
        Q!("    cneg            " "x6, x6, ge"),
        Q!("    csel            " "x4, x5, x4, ge"),
        Q!("    add             " "x5, x5, x6"),
        Q!("    add             " "x1, x1, #0x2"),
        Q!("    tst             " "x5, #0x2"),
        Q!("    asr             " "x5, x5, #1"),
        Q!("    csel            " "x6, x4, xzr, ne"),
        Q!("    ccmp            " "x1, xzr, #0x8, ne"),
        Q!("    cneg            " "x1, x1, ge"),
        Q!("    cneg            " "x6, x6, ge"),
        Q!("    csel            " "x4, x5, x4, ge"),
        Q!("    add             " "x5, x5, x6"),
        Q!("    add             " "x1, x1, #0x2"),
        Q!("    tst             " "x5, #0x2"),
        Q!("    asr             " "x5, x5, #1"),
        Q!("    csel            " "x6, x4, xzr, ne"),
        Q!("    ccmp            " "x1, xzr, #0x8, ne"),
        Q!("    cneg            " "x1, x1, ge"),
        Q!("    cneg            " "x6, x6, ge"),
        Q!("    csel            " "x4, x5, x4, ge"),
        Q!("    add             " "x5, x5, x6"),
        Q!("    add             " "x1, x1, #0x2"),
        Q!("    tst             " "x5, #0x2"),
        Q!("    asr             " "x5, x5, #1"),
        Q!("    csel            " "x6, x4, xzr, ne"),
        Q!("    ccmp            " "x1, xzr, #0x8, ne"),
        Q!("    cneg            " "x1, x1, ge"),
        Q!("    cneg            " "x6, x6, ge"),
        Q!("    csel            " "x4, x5, x4, ge"),
        Q!("    add             " "x5, x5, x6"),
        Q!("    add             " "x1, x1, #0x2"),
        Q!("    tst             " "x5, #0x2"),
        Q!("    asr             " "x5, x5, #1"),
        Q!("    csel            " "x6, x4, xzr, ne"),
        Q!("    ccmp            " "x1, xzr, #0x8, ne"),
        Q!("    cneg            " "x1, x1, ge"),
        Q!("    cneg            " "x6, x6, ge"),
        Q!("    csel            " "x4, x5, x4, ge"),
        Q!("    add             " "x5, x5, x6"),
        Q!("    add             " "x1, x1, #0x2"),
        Q!("    tst             " "x5, #0x2"),
        Q!("    asr             " "x5, x5, #1"),
        Q!("    csel            " "x6, x4, xzr, ne"),
        Q!("    ccmp            " "x1, xzr, #0x8, ne"),
        Q!("    cneg            " "x1, x1, ge"),
        Q!("    cneg            " "x6, x6, ge"),
        Q!("    csel            " "x4, x5, x4, ge"),
        Q!("    add             " "x5, x5, x6"),
        Q!("    add             " "x1, x1, #0x2"),
        Q!("    tst             " "x5, #0x2"),
        Q!("    asr             " "x5, x5, #1"),
        Q!("    csel            " "x6, x4, xzr, ne"),
        Q!("    ccmp            " "x1, xzr, #0x8, ne"),
        Q!("    cneg            " "x1, x1, ge"),
        Q!("    cneg            " "x6, x6, ge"),
        Q!("    csel            " "x4, x5, x4, ge"),
        Q!("    add             " "x5, x5, x6"),
        Q!("    add             " "x1, x1, #0x2"),
        Q!("    tst             " "x5, #0x2"),
        Q!("    asr             " "x5, x5, #1"),
        Q!("    csel            " "x6, x4, xzr, ne"),
        Q!("    ccmp            " "x1, xzr, #0x8, ne"),
        Q!("    cneg            " "x1, x1, ge"),
        Q!("    cneg            " "x6, x6, ge"),
        Q!("    csel            " "x4, x5, x4, ge"),
        Q!("    add             " "x5, x5, x6"),
        Q!("    add             " "x1, x1, #0x2"),
        Q!("    tst             " "x5, #0x2"),
        Q!("    asr             " "x5, x5, #1"),
        Q!("    csel            " "x6, x4, xzr, ne"),
        Q!("    ccmp            " "x1, xzr, #0x8, ne"),
        Q!("    cneg            " "x1, x1, ge"),
        Q!("    cneg            " "x6, x6, ge"),
        Q!("    csel            " "x4, x5, x4, ge"),
        Q!("    add             " "x5, x5, x6"),
        Q!("    add             " "x1, x1, #0x2"),
        Q!("    tst             " "x5, #0x2"),
        Q!("    asr             " "x5, x5, #1"),
        Q!("    csel            " "x6, x4, xzr, ne"),
        Q!("    ccmp            " "x1, xzr, #0x8, ne"),
        Q!("    cneg            " "x1, x1, ge"),
        Q!("    cneg            " "x6, x6, ge"),
        Q!("    csel            " "x4, x5, x4, ge"),
        Q!("    add             " "x5, x5, x6"),
        Q!("    add             " "x1, x1, #0x2"),
        Q!("    tst             " "x5, #0x2"),
        Q!("    asr             " "x5, x5, #1"),
        Q!("    csel            " "x6, x4, xzr, ne"),
        Q!("    ccmp            " "x1, xzr, #0x8, ne"),
        Q!("    cneg            " "x1, x1, ge"),
        Q!("    cneg            " "x6, x6, ge"),
        Q!("    csel            " "x4, x5, x4, ge"),
        Q!("    add             " "x5, x5, x6"),
        Q!("    add             " "x1, x1, #0x2"),
        Q!("    tst             " "x5, #0x2"),
        Q!("    asr             " "x5, x5, #1"),
        Q!("    csel            " "x6, x4, xzr, ne"),
        Q!("    ccmp            " "x1, xzr, #0x8, ne"),
        Q!("    cneg            " "x1, x1, ge"),
        Q!("    cneg            " "x6, x6, ge"),
        Q!("    csel            " "x4, x5, x4, ge"),
        Q!("    add             " "x5, x5, x6"),
        Q!("    add             " "x1, x1, #0x2"),
        Q!("    tst             " "x5, #0x2"),
        Q!("    asr             " "x5, x5, #1"),
        Q!("    csel            " "x6, x4, xzr, ne"),
        Q!("    ccmp            " "x1, xzr, #0x8, ne"),
        Q!("    cneg            " "x1, x1, ge"),
        Q!("    cneg            " "x6, x6, ge"),
        Q!("    csel            " "x4, x5, x4, ge"),
        Q!("    add             " "x5, x5, x6"),
        Q!("    add             " "x1, x1, #0x2"),
        Q!("    tst             " "x5, #0x2"),
        Q!("    asr             " "x5, x5, #1"),
        Q!("    csel            " "x6, x4, xzr, ne"),
        Q!("    ccmp            " "x1, xzr, #0x8, ne"),
        Q!("    cneg            " "x1, x1, ge"),
        Q!("    cneg            " "x6, x6, ge"),
        Q!("    csel            " "x4, x5, x4, ge"),
        Q!("    add             " "x5, x5, x6"),
        Q!("    add             " "x1, x1, #0x2"),
        Q!("    tst             " "x5, #0x2"),
        Q!("    asr             " "x5, x5, #1"),
        Q!("    csel            " "x6, x4, xzr, ne"),
        Q!("    ccmp            " "x1, xzr, #0x8, ne"),
        Q!("    cneg            " "x1, x1, ge"),
        Q!("    cneg            " "x6, x6, ge"),
        Q!("    csel            " "x4, x5, x4, ge"),
        Q!("    add             " "x5, x5, x6"),
        Q!("    add             " "x1, x1, #0x2"),
        Q!("    tst             " "x5, #0x2"),
        Q!("    asr             " "x5, x5, #1"),
        Q!("    csel            " "x6, x4, xzr, ne"),
        Q!("    ccmp            " "x1, xzr, #0x8, ne"),
        Q!("    cneg            " "x1, x1, ge"),
        Q!("    cneg            " "x6, x6, ge"),
        Q!("    csel            " "x4, x5, x4, ge"),
        Q!("    add             " "x5, x5, x6"),
        Q!("    add             " "x1, x1, #0x2"),
        Q!("    tst             " "x5, #0x2"),
        Q!("    asr             " "x5, x5, #1"),
        Q!("    csel            " "x6, x4, xzr, ne"),
        Q!("    ccmp            " "x1, xzr, #0x8, ne"),
        Q!("    cneg            " "x1, x1, ge"),
        Q!("    cneg            " "x6, x6, ge"),
        Q!("    csel            " "x4, x5, x4, ge"),
        Q!("    add             " "x5, x5, x6"),
        Q!("    add             " "x1, x1, #0x2"),
        Q!("    tst             " "x5, #0x2"),
        Q!("    asr             " "x5, x5, #1"),
        Q!("    csel            " "x6, x4, xzr, ne"),
        Q!("    ccmp            " "x1, xzr, #0x8, ne"),
        Q!("    cneg            " "x1, x1, ge"),
        Q!("    cneg            " "x6, x6, ge"),
        Q!("    csel            " "x4, x5, x4, ge"),
        Q!("    add             " "x5, x5, x6"),
        Q!("    add             " "x1, x1, #0x2"),
        Q!("    asr             " "x5, x5, #1"),
        Q!("    add             " "x8, x4, #0x100, lsl #12"),
        Q!("    sbfx            " "x8, x8, #21, #21"),
        Q!("    mov             " "x11, #0x100000"),
        Q!("    add             " "x11, x11, x11, lsl #21"),
        Q!("    add             " "x9, x4, x11"),
        Q!("    asr             " "x9, x9, #42"),
        Q!("    add             " "x10, x5, #0x100, lsl #12"),
        Q!("    sbfx            " "x10, x10, #21, #21"),
        Q!("    add             " "x11, x5, x11"),
        Q!("    asr             " "x11, x11, #42"),
        Q!("    mul             " "x6, x8, x2"),
        Q!("    mul             " "x7, x9, x3"),
        Q!("    mul             " "x2, x10, x2"),
        Q!("    mul             " "x3, x11, x3"),
        Q!("    add             " "x4, x6, x7"),
        Q!("    add             " "x5, x2, x3"),
        Q!("    asr             " "x2, x4, #20"),
        Q!("    asr             " "x3, x5, #20"),
        Q!("    and             " "x4, x2, #0xfffff"),
        Q!("    orr             " "x4, x4, #0xfffffe0000000000"),
        Q!("    and             " "x5, x3, #0xfffff"),
        Q!("    orr             " "x5, x5, #0xc000000000000000"),
        Q!("    tst             " "x5, #0x1"),
        Q!("    csel            " "x6, x4, xzr, ne"),
        Q!("    ccmp            " "x1, xzr, #0x8, ne"),
        Q!("    cneg            " "x1, x1, ge"),
        Q!("    cneg            " "x6, x6, ge"),
        Q!("    csel            " "x4, x5, x4, ge"),
        Q!("    add             " "x5, x5, x6"),
        Q!("    add             " "x1, x1, #0x2"),
        Q!("    tst             " "x5, #0x2"),
        Q!("    asr             " "x5, x5, #1"),
        Q!("    csel            " "x6, x4, xzr, ne"),
        Q!("    ccmp            " "x1, xzr, #0x8, ne"),
        Q!("    cneg            " "x1, x1, ge"),
        Q!("    cneg            " "x6, x6, ge"),
        Q!("    csel            " "x4, x5, x4, ge"),
        Q!("    add             " "x5, x5, x6"),
        Q!("    add             " "x1, x1, #0x2"),
        Q!("    tst             " "x5, #0x2"),
        Q!("    asr             " "x5, x5, #1"),
        Q!("    csel            " "x6, x4, xzr, ne"),
        Q!("    ccmp            " "x1, xzr, #0x8, ne"),
        Q!("    cneg            " "x1, x1, ge"),
        Q!("    cneg            " "x6, x6, ge"),
        Q!("    csel            " "x4, x5, x4, ge"),
        Q!("    add             " "x5, x5, x6"),
        Q!("    add             " "x1, x1, #0x2"),
        Q!("    tst             " "x5, #0x2"),
        Q!("    asr             " "x5, x5, #1"),
        Q!("    csel            " "x6, x4, xzr, ne"),
        Q!("    ccmp            " "x1, xzr, #0x8, ne"),
        Q!("    cneg            " "x1, x1, ge"),
        Q!("    cneg            " "x6, x6, ge"),
        Q!("    csel            " "x4, x5, x4, ge"),
        Q!("    add             " "x5, x5, x6"),
        Q!("    add             " "x1, x1, #0x2"),
        Q!("    tst             " "x5, #0x2"),
        Q!("    asr             " "x5, x5, #1"),
        Q!("    csel            " "x6, x4, xzr, ne"),
        Q!("    ccmp            " "x1, xzr, #0x8, ne"),
        Q!("    cneg            " "x1, x1, ge"),
        Q!("    cneg            " "x6, x6, ge"),
        Q!("    csel            " "x4, x5, x4, ge"),
        Q!("    add             " "x5, x5, x6"),
        Q!("    add             " "x1, x1, #0x2"),
        Q!("    tst             " "x5, #0x2"),
        Q!("    asr             " "x5, x5, #1"),
        Q!("    csel            " "x6, x4, xzr, ne"),
        Q!("    ccmp            " "x1, xzr, #0x8, ne"),
        Q!("    cneg            " "x1, x1, ge"),
        Q!("    cneg            " "x6, x6, ge"),
        Q!("    csel            " "x4, x5, x4, ge"),
        Q!("    add             " "x5, x5, x6"),
        Q!("    add             " "x1, x1, #0x2"),
        Q!("    tst             " "x5, #0x2"),
        Q!("    asr             " "x5, x5, #1"),
        Q!("    csel            " "x6, x4, xzr, ne"),
        Q!("    ccmp            " "x1, xzr, #0x8, ne"),
        Q!("    cneg            " "x1, x1, ge"),
        Q!("    cneg            " "x6, x6, ge"),
        Q!("    csel            " "x4, x5, x4, ge"),
        Q!("    add             " "x5, x5, x6"),
        Q!("    add             " "x1, x1, #0x2"),
        Q!("    tst             " "x5, #0x2"),
        Q!("    asr             " "x5, x5, #1"),
        Q!("    csel            " "x6, x4, xzr, ne"),
        Q!("    ccmp            " "x1, xzr, #0x8, ne"),
        Q!("    cneg            " "x1, x1, ge"),
        Q!("    cneg            " "x6, x6, ge"),
        Q!("    csel            " "x4, x5, x4, ge"),
        Q!("    add             " "x5, x5, x6"),
        Q!("    add             " "x1, x1, #0x2"),
        Q!("    tst             " "x5, #0x2"),
        Q!("    asr             " "x5, x5, #1"),
        Q!("    csel            " "x6, x4, xzr, ne"),
        Q!("    ccmp            " "x1, xzr, #0x8, ne"),
        Q!("    cneg            " "x1, x1, ge"),
        Q!("    cneg            " "x6, x6, ge"),
        Q!("    csel            " "x4, x5, x4, ge"),
        Q!("    add             " "x5, x5, x6"),
        Q!("    add             " "x1, x1, #0x2"),
        Q!("    tst             " "x5, #0x2"),
        Q!("    asr             " "x5, x5, #1"),
        Q!("    csel            " "x6, x4, xzr, ne"),
        Q!("    ccmp            " "x1, xzr, #0x8, ne"),
        Q!("    cneg            " "x1, x1, ge"),
        Q!("    cneg            " "x6, x6, ge"),
        Q!("    csel            " "x4, x5, x4, ge"),
        Q!("    add             " "x5, x5, x6"),
        Q!("    add             " "x1, x1, #0x2"),
        Q!("    tst             " "x5, #0x2"),
        Q!("    asr             " "x5, x5, #1"),
        Q!("    csel            " "x6, x4, xzr, ne"),
        Q!("    ccmp            " "x1, xzr, #0x8, ne"),
        Q!("    cneg            " "x1, x1, ge"),
        Q!("    cneg            " "x6, x6, ge"),
        Q!("    csel            " "x4, x5, x4, ge"),
        Q!("    add             " "x5, x5, x6"),
        Q!("    add             " "x1, x1, #0x2"),
        Q!("    tst             " "x5, #0x2"),
        Q!("    asr             " "x5, x5, #1"),
        Q!("    csel            " "x6, x4, xzr, ne"),
        Q!("    ccmp            " "x1, xzr, #0x8, ne"),
        Q!("    cneg            " "x1, x1, ge"),
        Q!("    cneg            " "x6, x6, ge"),
        Q!("    csel            " "x4, x5, x4, ge"),
        Q!("    add             " "x5, x5, x6"),
        Q!("    add             " "x1, x1, #0x2"),
        Q!("    tst             " "x5, #0x2"),
        Q!("    asr             " "x5, x5, #1"),
        Q!("    csel            " "x6, x4, xzr, ne"),
        Q!("    ccmp            " "x1, xzr, #0x8, ne"),
        Q!("    cneg            " "x1, x1, ge"),
        Q!("    cneg            " "x6, x6, ge"),
        Q!("    csel            " "x4, x5, x4, ge"),
        Q!("    add             " "x5, x5, x6"),
        Q!("    add             " "x1, x1, #0x2"),
        Q!("    tst             " "x5, #0x2"),
        Q!("    asr             " "x5, x5, #1"),
        Q!("    csel            " "x6, x4, xzr, ne"),
        Q!("    ccmp            " "x1, xzr, #0x8, ne"),
        Q!("    cneg            " "x1, x1, ge"),
        Q!("    cneg            " "x6, x6, ge"),
        Q!("    csel            " "x4, x5, x4, ge"),
        Q!("    add             " "x5, x5, x6"),
        Q!("    add             " "x1, x1, #0x2"),
        Q!("    tst             " "x5, #0x2"),
        Q!("    asr             " "x5, x5, #1"),
        Q!("    csel            " "x6, x4, xzr, ne"),
        Q!("    ccmp            " "x1, xzr, #0x8, ne"),
        Q!("    cneg            " "x1, x1, ge"),
        Q!("    cneg            " "x6, x6, ge"),
        Q!("    csel            " "x4, x5, x4, ge"),
        Q!("    add             " "x5, x5, x6"),
        Q!("    add             " "x1, x1, #0x2"),
        Q!("    tst             " "x5, #0x2"),
        Q!("    asr             " "x5, x5, #1"),
        Q!("    csel            " "x6, x4, xzr, ne"),
        Q!("    ccmp            " "x1, xzr, #0x8, ne"),
        Q!("    cneg            " "x1, x1, ge"),
        Q!("    cneg            " "x6, x6, ge"),
        Q!("    csel            " "x4, x5, x4, ge"),
        Q!("    add             " "x5, x5, x6"),
        Q!("    add             " "x1, x1, #0x2"),
        Q!("    tst             " "x5, #0x2"),
        Q!("    asr             " "x5, x5, #1"),
        Q!("    csel            " "x6, x4, xzr, ne"),
        Q!("    ccmp            " "x1, xzr, #0x8, ne"),
        Q!("    cneg            " "x1, x1, ge"),
        Q!("    cneg            " "x6, x6, ge"),
        Q!("    csel            " "x4, x5, x4, ge"),
        Q!("    add             " "x5, x5, x6"),
        Q!("    add             " "x1, x1, #0x2"),
        Q!("    tst             " "x5, #0x2"),
        Q!("    asr             " "x5, x5, #1"),
        Q!("    csel            " "x6, x4, xzr, ne"),
        Q!("    ccmp            " "x1, xzr, #0x8, ne"),
        Q!("    cneg            " "x1, x1, ge"),
        Q!("    cneg            " "x6, x6, ge"),
        Q!("    csel            " "x4, x5, x4, ge"),
        Q!("    add             " "x5, x5, x6"),
        Q!("    add             " "x1, x1, #0x2"),
        Q!("    tst             " "x5, #0x2"),
        Q!("    asr             " "x5, x5, #1"),
        Q!("    csel            " "x6, x4, xzr, ne"),
        Q!("    ccmp            " "x1, xzr, #0x8, ne"),
        Q!("    cneg            " "x1, x1, ge"),
        Q!("    cneg            " "x6, x6, ge"),
        Q!("    csel            " "x4, x5, x4, ge"),
        Q!("    add             " "x5, x5, x6"),
        Q!("    add             " "x1, x1, #0x2"),
        Q!("    tst             " "x5, #0x2"),
        Q!("    asr             " "x5, x5, #1"),
        Q!("    csel            " "x6, x4, xzr, ne"),
        Q!("    ccmp            " "x1, xzr, #0x8, ne"),
        Q!("    cneg            " "x1, x1, ge"),
        Q!("    cneg            " "x6, x6, ge"),
        Q!("    csel            " "x4, x5, x4, ge"),
        Q!("    add             " "x5, x5, x6"),
        Q!("    add             " "x1, x1, #0x2"),
        Q!("    asr             " "x5, x5, #1"),
        Q!("    add             " "x12, x4, #0x100, lsl #12"),
        Q!("    sbfx            " "x12, x12, #21, #21"),
        Q!("    mov             " "x15, #0x100000"),
        Q!("    add             " "x15, x15, x15, lsl #21"),
        Q!("    add             " "x13, x4, x15"),
        Q!("    asr             " "x13, x13, #42"),
        Q!("    add             " "x14, x5, #0x100, lsl #12"),
        Q!("    sbfx            " "x14, x14, #21, #21"),
        Q!("    add             " "x15, x5, x15"),
        Q!("    asr             " "x15, x15, #42"),
        Q!("    mul             " "x6, x12, x2"),
        Q!("    mul             " "x7, x13, x3"),
        Q!("    mul             " "x2, x14, x2"),
        Q!("    mul             " "x3, x15, x3"),
        Q!("    add             " "x4, x6, x7"),
        Q!("    add             " "x5, x2, x3"),
        Q!("    asr             " "x2, x4, #20"),
        Q!("    asr             " "x3, x5, #20"),
        Q!("    and             " "x4, x2, #0xfffff"),
        Q!("    orr             " "x4, x4, #0xfffffe0000000000"),
        Q!("    and             " "x5, x3, #0xfffff"),
        Q!("    orr             " "x5, x5, #0xc000000000000000"),
        Q!("    tst             " "x5, #0x1"),
        Q!("    csel            " "x6, x4, xzr, ne"),
        Q!("    ccmp            " "x1, xzr, #0x8, ne"),
        Q!("    cneg            " "x1, x1, ge"),
        Q!("    cneg            " "x6, x6, ge"),
        Q!("    csel            " "x4, x5, x4, ge"),
        Q!("    add             " "x5, x5, x6"),
        Q!("    add             " "x1, x1, #0x2"),
        Q!("    tst             " "x5, #0x2"),
        Q!("    asr             " "x5, x5, #1"),
        Q!("    csel            " "x6, x4, xzr, ne"),
        Q!("    ccmp            " "x1, xzr, #0x8, ne"),
        Q!("    cneg            " "x1, x1, ge"),
        Q!("    cneg            " "x6, x6, ge"),
        Q!("    csel            " "x4, x5, x4, ge"),
        Q!("    add             " "x5, x5, x6"),
        Q!("    add             " "x1, x1, #0x2"),
        Q!("    tst             " "x5, #0x2"),
        Q!("    asr             " "x5, x5, #1"),
        Q!("    csel            " "x6, x4, xzr, ne"),
        Q!("    ccmp            " "x1, xzr, #0x8, ne"),
        Q!("    cneg            " "x1, x1, ge"),
        Q!("    cneg            " "x6, x6, ge"),
        Q!("    csel            " "x4, x5, x4, ge"),
        Q!("    add             " "x5, x5, x6"),
        Q!("    add             " "x1, x1, #0x2"),
        Q!("    tst             " "x5, #0x2"),
        Q!("    asr             " "x5, x5, #1"),
        Q!("    csel            " "x6, x4, xzr, ne"),
        Q!("    ccmp            " "x1, xzr, #0x8, ne"),
        Q!("    cneg            " "x1, x1, ge"),
        Q!("    cneg            " "x6, x6, ge"),
        Q!("    csel            " "x4, x5, x4, ge"),
        Q!("    add             " "x5, x5, x6"),
        Q!("    add             " "x1, x1, #0x2"),
        Q!("    tst             " "x5, #0x2"),
        Q!("    asr             " "x5, x5, #1"),
        Q!("    csel            " "x6, x4, xzr, ne"),
        Q!("    ccmp            " "x1, xzr, #0x8, ne"),
        Q!("    cneg            " "x1, x1, ge"),
        Q!("    cneg            " "x6, x6, ge"),
        Q!("    csel            " "x4, x5, x4, ge"),
        Q!("    add             " "x5, x5, x6"),
        Q!("    add             " "x1, x1, #0x2"),
        Q!("    tst             " "x5, #0x2"),
        Q!("    asr             " "x5, x5, #1"),
        Q!("    csel            " "x6, x4, xzr, ne"),
        Q!("    ccmp            " "x1, xzr, #0x8, ne"),
        Q!("    cneg            " "x1, x1, ge"),
        Q!("    cneg            " "x6, x6, ge"),
        Q!("    csel            " "x4, x5, x4, ge"),
        Q!("    add             " "x5, x5, x6"),
        Q!("    add             " "x1, x1, #0x2"),
        Q!("    tst             " "x5, #0x2"),
        Q!("    asr             " "x5, x5, #1"),
        Q!("    csel            " "x6, x4, xzr, ne"),
        Q!("    ccmp            " "x1, xzr, #0x8, ne"),
        Q!("    cneg            " "x1, x1, ge"),
        Q!("    cneg            " "x6, x6, ge"),
        Q!("    csel            " "x4, x5, x4, ge"),
        Q!("    add             " "x5, x5, x6"),
        Q!("    add             " "x1, x1, #0x2"),
        Q!("    tst             " "x5, #0x2"),
        Q!("    asr             " "x5, x5, #1"),
        Q!("    csel            " "x6, x4, xzr, ne"),
        Q!("    ccmp            " "x1, xzr, #0x8, ne"),
        Q!("    cneg            " "x1, x1, ge"),
        Q!("    cneg            " "x6, x6, ge"),
        Q!("    csel            " "x4, x5, x4, ge"),
        Q!("    add             " "x5, x5, x6"),
        Q!("    add             " "x1, x1, #0x2"),
        Q!("    tst             " "x5, #0x2"),
        Q!("    asr             " "x5, x5, #1"),
        Q!("    csel            " "x6, x4, xzr, ne"),
        Q!("    ccmp            " "x1, xzr, #0x8, ne"),
        Q!("    cneg            " "x1, x1, ge"),
        Q!("    cneg            " "x6, x6, ge"),
        Q!("    csel            " "x4, x5, x4, ge"),
        Q!("    add             " "x5, x5, x6"),
        Q!("    add             " "x1, x1, #0x2"),
        Q!("    tst             " "x5, #0x2"),
        Q!("    asr             " "x5, x5, #1"),
        Q!("    csel            " "x6, x4, xzr, ne"),
        Q!("    ccmp            " "x1, xzr, #0x8, ne"),
        Q!("    cneg            " "x1, x1, ge"),
        Q!("    cneg            " "x6, x6, ge"),
        Q!("    csel            " "x4, x5, x4, ge"),
        Q!("    add             " "x5, x5, x6"),
        Q!("    add             " "x1, x1, #0x2"),
        Q!("    tst             " "x5, #0x2"),
        Q!("    asr             " "x5, x5, #1"),
        Q!("    mul             " "x2, x12, x8"),
        Q!("    mul             " "x3, x12, x9"),
        Q!("    mul             " "x6, x14, x8"),
        Q!("    mul             " "x7, x14, x9"),
        Q!("    madd            " "x8, x13, x10, x2"),
        Q!("    madd            " "x9, x13, x11, x3"),
        Q!("    madd            " "x16, x15, x10, x6"),
        Q!("    madd            " "x17, x15, x11, x7"),
        Q!("    csel            " "x6, x4, xzr, ne"),
        Q!("    ccmp            " "x1, xzr, #0x8, ne"),
        Q!("    cneg            " "x1, x1, ge"),
        Q!("    cneg            " "x6, x6, ge"),
        Q!("    csel            " "x4, x5, x4, ge"),
        Q!("    add             " "x5, x5, x6"),
        Q!("    add             " "x1, x1, #0x2"),
        Q!("    tst             " "x5, #0x2"),
        Q!("    asr             " "x5, x5, #1"),
        Q!("    csel            " "x6, x4, xzr, ne"),
        Q!("    ccmp            " "x1, xzr, #0x8, ne"),
        Q!("    cneg            " "x1, x1, ge"),
        Q!("    cneg            " "x6, x6, ge"),
        Q!("    csel            " "x4, x5, x4, ge"),
        Q!("    add             " "x5, x5, x6"),
        Q!("    add             " "x1, x1, #0x2"),
        Q!("    tst             " "x5, #0x2"),
        Q!("    asr             " "x5, x5, #1"),
        Q!("    csel            " "x6, x4, xzr, ne"),
        Q!("    ccmp            " "x1, xzr, #0x8, ne"),
        Q!("    cneg            " "x1, x1, ge"),
        Q!("    cneg            " "x6, x6, ge"),
        Q!("    csel            " "x4, x5, x4, ge"),
        Q!("    add             " "x5, x5, x6"),
        Q!("    add             " "x1, x1, #0x2"),
        Q!("    tst             " "x5, #0x2"),
        Q!("    asr             " "x5, x5, #1"),
        Q!("    csel            " "x6, x4, xzr, ne"),
        Q!("    ccmp            " "x1, xzr, #0x8, ne"),
        Q!("    cneg            " "x1, x1, ge"),
        Q!("    cneg            " "x6, x6, ge"),
        Q!("    csel            " "x4, x5, x4, ge"),
        Q!("    add             " "x5, x5, x6"),
        Q!("    add             " "x1, x1, #0x2"),
        Q!("    tst             " "x5, #0x2"),
        Q!("    asr             " "x5, x5, #1"),
        Q!("    csel            " "x6, x4, xzr, ne"),
        Q!("    ccmp            " "x1, xzr, #0x8, ne"),
        Q!("    cneg            " "x1, x1, ge"),
        Q!("    cneg            " "x6, x6, ge"),
        Q!("    csel            " "x4, x5, x4, ge"),
        Q!("    add             " "x5, x5, x6"),
        Q!("    add             " "x1, x1, #0x2"),
        Q!("    tst             " "x5, #0x2"),
        Q!("    asr             " "x5, x5, #1"),
        Q!("    csel            " "x6, x4, xzr, ne"),
        Q!("    ccmp            " "x1, xzr, #0x8, ne"),
        Q!("    cneg            " "x1, x1, ge"),
        Q!("    cneg            " "x6, x6, ge"),
        Q!("    csel            " "x4, x5, x4, ge"),
        Q!("    add             " "x5, x5, x6"),
        Q!("    add             " "x1, x1, #0x2"),
        Q!("    tst             " "x5, #0x2"),
        Q!("    asr             " "x5, x5, #1"),
        Q!("    csel            " "x6, x4, xzr, ne"),
        Q!("    ccmp            " "x1, xzr, #0x8, ne"),
        Q!("    cneg            " "x1, x1, ge"),
        Q!("    cneg            " "x6, x6, ge"),
        Q!("    csel            " "x4, x5, x4, ge"),
        Q!("    add             " "x5, x5, x6"),
        Q!("    add             " "x1, x1, #0x2"),
        Q!("    tst             " "x5, #0x2"),
        Q!("    asr             " "x5, x5, #1"),
        Q!("    csel            " "x6, x4, xzr, ne"),
        Q!("    ccmp            " "x1, xzr, #0x8, ne"),
        Q!("    cneg            " "x1, x1, ge"),
        Q!("    cneg            " "x6, x6, ge"),
        Q!("    csel            " "x4, x5, x4, ge"),
        Q!("    add             " "x5, x5, x6"),
        Q!("    add             " "x1, x1, #0x2"),
        Q!("    tst             " "x5, #0x2"),
        Q!("    asr             " "x5, x5, #1"),
        Q!("    csel            " "x6, x4, xzr, ne"),
        Q!("    ccmp            " "x1, xzr, #0x8, ne"),
        Q!("    cneg            " "x1, x1, ge"),
        Q!("    cneg            " "x6, x6, ge"),
        Q!("    csel            " "x4, x5, x4, ge"),
        Q!("    add             " "x5, x5, x6"),
        Q!("    add             " "x1, x1, #0x2"),
        Q!("    asr             " "x5, x5, #1"),
        Q!("    add             " "x12, x4, #0x100, lsl #12"),
        Q!("    sbfx            " "x12, x12, #22, #21"),
        Q!("    mov             " "x15, #0x100000"),
        Q!("    add             " "x15, x15, x15, lsl #21"),
        Q!("    add             " "x13, x4, x15"),
        Q!("    asr             " "x13, x13, #43"),
        Q!("    add             " "x14, x5, #0x100, lsl #12"),
        Q!("    sbfx            " "x14, x14, #22, #21"),
        Q!("    add             " "x15, x5, x15"),
        Q!("    asr             " "x15, x15, #43"),
        Q!("    mneg            " "x2, x12, x8"),
        Q!("    mneg            " "x3, x12, x9"),
        Q!("    mneg            " "x4, x14, x8"),
        Q!("    mneg            " "x5, x14, x9"),
        Q!("    msub            " "x10, x13, x16, x2"),
        Q!("    msub            " "x11, x13, x17, x3"),
        Q!("    msub            " "x12, x15, x16, x4"),
        Q!("    msub            " "x13, x15, x17, x5"),
        Q!("    mov             " "x22, x1"),
        Q!("    subs            " "x21, x21, #0x1"),
        Q!("    b.ne            " Label!("Ledwards25519_scalarmuldouble_alt_invloop", 8, Before)),
        Q!("    ldr             " "x0, [sp]"),
        Q!("    ldr             " "x1, [sp, #32]"),
        Q!("    mul             " "x0, x0, x10"),
        Q!("    madd            " "x1, x1, x11, x0"),
        Q!("    asr             " "x0, x1, #63"),
        Q!("    cmp             " "x10, xzr"),
        Q!("    csetm           " "x14, mi"),
        Q!("    cneg            " "x10, x10, mi"),
        Q!("    eor             " "x14, x14, x0"),
        Q!("    cmp             " "x11, xzr"),
        Q!("    csetm           " "x15, mi"),
        Q!("    cneg            " "x11, x11, mi"),
        Q!("    eor             " "x15, x15, x0"),
        Q!("    cmp             " "x12, xzr"),
        Q!("    csetm           " "x16, mi"),
        Q!("    cneg            " "x12, x12, mi"),
        Q!("    eor             " "x16, x16, x0"),
        Q!("    cmp             " "x13, xzr"),
        Q!("    csetm           " "x17, mi"),
        Q!("    cneg            " "x13, x13, mi"),
        Q!("    eor             " "x17, x17, x0"),
        Q!("    and             " "x0, x10, x14"),
        Q!("    and             " "x1, x11, x15"),
        Q!("    add             " "x9, x0, x1"),
        Q!("    ldr             " "x7, [sp, #64]"),
        Q!("    eor             " "x1, x7, x14"),
        Q!("    mul             " "x0, x1, x10"),
        Q!("    umulh           " "x1, x1, x10"),
        Q!("    adds            " "x4, x9, x0"),
        Q!("    adc             " "x2, xzr, x1"),
        Q!("    ldr             " "x8, [sp, #96]"),
        Q!("    eor             " "x1, x8, x15"),
        Q!("    mul             " "x0, x1, x11"),
        Q!("    umulh           " "x1, x1, x11"),
        Q!("    adds            " "x4, x4, x0"),
        Q!("    str             " "x4, [sp, #64]"),
        Q!("    adc             " "x2, x2, x1"),
        Q!("    ldr             " "x7, [sp, #72]"),
        Q!("    eor             " "x1, x7, x14"),
        Q!("    mul             " "x0, x1, x10"),
        Q!("    umulh           " "x1, x1, x10"),
        Q!("    adds            " "x2, x2, x0"),
        Q!("    adc             " "x6, xzr, x1"),
        Q!("    ldr             " "x8, [sp, #104]"),
        Q!("    eor             " "x1, x8, x15"),
        Q!("    mul             " "x0, x1, x11"),
        Q!("    umulh           " "x1, x1, x11"),
        Q!("    adds            " "x2, x2, x0"),
        Q!("    str             " "x2, [sp, #72]"),
        Q!("    adc             " "x6, x6, x1"),
        Q!("    ldr             " "x7, [sp, #80]"),
        Q!("    eor             " "x1, x7, x14"),
        Q!("    mul             " "x0, x1, x10"),
        Q!("    umulh           " "x1, x1, x10"),
        Q!("    adds            " "x6, x6, x0"),
        Q!("    adc             " "x5, xzr, x1"),
        Q!("    ldr             " "x8, [sp, #112]"),
        Q!("    eor             " "x1, x8, x15"),
        Q!("    mul             " "x0, x1, x11"),
        Q!("    umulh           " "x1, x1, x11"),
        Q!("    adds            " "x6, x6, x0"),
        Q!("    str             " "x6, [sp, #80]"),
        Q!("    adc             " "x5, x5, x1"),
        Q!("    ldr             " "x7, [sp, #88]"),
        Q!("    eor             " "x1, x7, x14"),
        Q!("    and             " "x3, x14, x10"),
        Q!("    neg             " "x3, x3"),
        Q!("    mul             " "x0, x1, x10"),
        Q!("    umulh           " "x1, x1, x10"),
        Q!("    adds            " "x5, x5, x0"),
        Q!("    adc             " "x3, x3, x1"),
        Q!("    ldr             " "x8, [sp, #120]"),
        Q!("    eor             " "x1, x8, x15"),
        Q!("    and             " "x0, x15, x11"),
        Q!("    sub             " "x3, x3, x0"),
        Q!("    mul             " "x0, x1, x11"),
        Q!("    umulh           " "x1, x1, x11"),
        Q!("    adds            " "x5, x5, x0"),
        Q!("    adc             " "x3, x3, x1"),
        Q!("    extr            " "x6, x3, x5, #63"),
        Q!("    ldp             " "x0, x1, [sp, #64]"),
        Q!("    tst             " "x3, x3"),
        Q!("    cinc            " "x6, x6, pl"),
        Q!("    mov             " "x3, #0x13"),
        Q!("    mul             " "x4, x6, x3"),
        Q!("    add             " "x5, x5, x6, lsl #63"),
        Q!("    smulh           " "x6, x6, x3"),
        Q!("    ldr             " "x2, [sp, #80]"),
        Q!("    adds            " "x0, x0, x4"),
        Q!("    adcs            " "x1, x1, x6"),
        Q!("    asr             " "x6, x6, #63"),
        Q!("    adcs            " "x2, x2, x6"),
        Q!("    adcs            " "x5, x5, x6"),
        Q!("    csel            " "x3, x3, xzr, mi"),
        Q!("    subs            " "x0, x0, x3"),
        Q!("    sbcs            " "x1, x1, xzr"),
        Q!("    sbcs            " "x2, x2, xzr"),
        Q!("    sbc             " "x5, x5, xzr"),
        Q!("    and             " "x5, x5, #0x7fffffffffffffff"),
        Q!("    mov             " "x4, x20"),
        Q!("    stp             " "x0, x1, [x4]"),
        Q!("    stp             " "x2, x5, [x4, #16]"),

        // Store result. Note that these are the only reductions mod 2^255-19

        Q!("    mov             " p0!() ", " res!()),
        Q!("    add             " p1!() ", " acc!()),
        Q!("    add             " p2!() ", " tabent!()),
        mul_p25519!(x_0!(), x_1!(), x_2!()),

        Q!("    add             " p0!() ", " res!() ", #32"),
        Q!("    add             " p1!() ", " acc!() "+ 32"),
        Q!("    add             " p2!() ", " tabent!()),
        mul_p25519!(x_0!(), x_1!(), x_2!()),

        // Restore stack and registers

        Q!("    add             " "sp, sp, # (" NSPACE!() "+ 0)"),
        Q!("    ldp             " "x25, x30, [sp], #16"),
        Q!("    ldp             " "x23, x24, [sp], #16"),
        Q!("    ldp             " "x21, x22, [sp], #16"),
        Q!("    ldp             " "x19, x20, [sp], #16"),

        // proc hoisting in -> ret after Ledwards25519_scalarmuldouble_alt_pepadd
        Q!("    b               " Label!("hoist_finish", 9, After)),

        // ****************************************************************************
        // Localized versions of subroutines.
        // These are close to the standalone functions "edwards25519_epdouble" etc.,
        // but are only maintaining reduction modulo 2^256 - 38, not 2^255 - 19.
        // ****************************************************************************

        Q!(Label!("Ledwards25519_scalarmuldouble_alt_epdouble", 2) ":"),

        Q!("    sub             " "sp, sp, # (5 * " NUMSIZE!() "+ 0)"),
        add_twice4!(t0!(), x_1!(), y_1!()),
        sqr_4!(t1!(), z_1!()),
        sqr_4!(t2!(), x_1!()),
        sqr_4!(t3!(), y_1!()),
        double_twice4!(t1!(), t1!()),
        sqr_4!(t0!(), t0!()),
        add_twice4!(t4!(), t2!(), t3!()),
        sub_twice4!(t2!(), t2!(), t3!()),
        add_twice4!(t3!(), t1!(), t2!()),
        sub_twice4!(t1!(), t4!(), t0!()),
        mul_4!(y_0!(), t2!(), t4!()),
        mul_4!(z_0!(), t3!(), t2!()),
        mul_4!(w_0!(), t1!(), t4!()),
        mul_4!(x_0!(), t1!(), t3!()),
        Q!("    add             " "sp, sp, # (5 * " NUMSIZE!() "+ 0)"),
        Q!("    ret             " ),

        Q!(Label!("Ledwards25519_scalarmuldouble_alt_pdouble", 6) ":"),

        Q!("    sub             " "sp, sp, # (5 * " NUMSIZE!() "+ 0)"),
        add_twice4!(t0!(), x_1!(), y_1!()),
        sqr_4!(t1!(), z_1!()),
        sqr_4!(t2!(), x_1!()),
        sqr_4!(t3!(), y_1!()),
        double_twice4!(t1!(), t1!()),
        sqr_4!(t0!(), t0!()),
        add_twice4!(t4!(), t2!(), t3!()),
        sub_twice4!(t2!(), t2!(), t3!()),
        add_twice4!(t3!(), t1!(), t2!()),
        sub_twice4!(t1!(), t4!(), t0!()),
        mul_4!(y_0!(), t2!(), t4!()),
        mul_4!(z_0!(), t3!(), t2!()),
        mul_4!(x_0!(), t1!(), t3!()),
        Q!("    add             " "sp, sp, # (5 * " NUMSIZE!() "+ 0)"),
        Q!("    ret             " ),

        Q!(Label!("Ledwards25519_scalarmuldouble_alt_epadd", 3) ":"),

        Q!("    sub             " "sp, sp, # (6 * " NUMSIZE!() "+ 0)"),
        mul_4!(t0!(), w_1!(), w_2!()),
        sub_twice4!(t1!(), y_1!(), x_1!()),
        sub_twice4!(t2!(), y_2!(), x_2!()),
        add_twice4!(t3!(), y_1!(), x_1!()),
        add_twice4!(t4!(), y_2!(), x_2!()),
        double_twice4!(t5!(), z_2!()),
        mul_4!(t1!(), t1!(), t2!()),
        mul_4!(t3!(), t3!(), t4!()),
        load_k25519!(t2!()),
        mul_4!(t2!(), t2!(), t0!()),
        mul_4!(t4!(), z_1!(), t5!()),
        sub_twice4!(t0!(), t3!(), t1!()),
        add_twice4!(t5!(), t3!(), t1!()),
        sub_twice4!(t1!(), t4!(), t2!()),
        add_twice4!(t3!(), t4!(), t2!()),
        mul_4!(w_0!(), t0!(), t5!()),
        mul_4!(x_0!(), t0!(), t1!()),
        mul_4!(y_0!(), t3!(), t5!()),
        mul_4!(z_0!(), t1!(), t3!()),
        Q!("    add             " "sp, sp, # (6 * " NUMSIZE!() "+ 0)"),
        Q!("    ret             " ),

        Q!(Label!("Ledwards25519_scalarmuldouble_alt_pepadd", 4) ":"),

        Q!("    sub             " "sp, sp, # (6 * " NUMSIZE!() "+ 0)"),
        double_twice4!(t0!(), z_1!()),
        sub_twice4!(t1!(), y_1!(), x_1!()),
        add_twice4!(t2!(), y_1!(), x_1!()),
        mul_4!(t3!(), w_1!(), z_2!()),
        mul_4!(t1!(), t1!(), x_2!()),
        mul_4!(t2!(), t2!(), y_2!()),
        sub_twice4!(t4!(), t0!(), t3!()),
        add_twice4!(t0!(), t0!(), t3!()),
        sub_twice4!(t5!(), t2!(), t1!()),
        add_twice4!(t1!(), t2!(), t1!()),
        mul_4!(z_0!(), t4!(), t0!()),
        mul_4!(x_0!(), t5!(), t4!()),
        mul_4!(y_0!(), t0!(), t1!()),
        mul_4!(w_0!(), t5!(), t1!()),
        Q!("    add             " "sp, sp, # (6 * " NUMSIZE!() "+ 0)"),
        Q!("    ret             " ),
        Q!(Label!("hoist_finish", 9) ":"),
        inout("x0") res.as_mut_ptr() => _,
        inout("x1") scalar.as_ptr() => _,
        inout("x2") point.as_ptr() => _,
        inout("x3") bscalar.as_ptr() => _,
        edwards25519_scalarmuldouble_alt_constant = sym edwards25519_scalarmuldouble_alt_constant,
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
        out("x23") _,
        out("x24") _,
        out("x25") _,
        out("x30") _,
        out("x4") _,
        out("x5") _,
        out("x6") _,
        out("x7") _,
        out("x8") _,
        out("x9") _,
            )
    };
}

// ****************************************************************************
// The precomputed data (all read-only).
// ****************************************************************************

// Precomputed table of multiples of generator for edwards25519
// all in precomputed extended-projective (y-x,x+y,2*d*x*y) triples.

static edwards25519_scalarmuldouble_alt_constant: [u64; 96] = [
    // 1 * G
    0x9d103905d740913e,
    0xfd399f05d140beb3,
    0xa5c18434688f8a09,
    0x44fd2f9298f81267,
    0x2fbc93c6f58c3b85,
    0xcf932dc6fb8c0e19,
    0x270b4898643d42c2,
    0x07cf9d3a33d4ba65,
    0xabc91205877aaa68,
    0x26d9e823ccaac49e,
    0x5a1b7dcbdd43598c,
    0x6f117b689f0c65a8,
    // 2 * G
    0x8a99a56042b4d5a8,
    0x8f2b810c4e60acf6,
    0xe09e236bb16e37aa,
    0x6bb595a669c92555,
    0x9224e7fc933c71d7,
    0x9f469d967a0ff5b5,
    0x5aa69a65e1d60702,
    0x590c063fa87d2e2e,
    0x43faa8b3a59b7a5f,
    0x36c16bdd5d9acf78,
    0x500fa0840b3d6a31,
    0x701af5b13ea50b73,
    // 3 * G
    0x56611fe8a4fcd265,
    0x3bd353fde5c1ba7d,
    0x8131f31a214bd6bd,
    0x2ab91587555bda62,
    0xaf25b0a84cee9730,
    0x025a8430e8864b8a,
    0xc11b50029f016732,
    0x7a164e1b9a80f8f4,
    0x14ae933f0dd0d889,
    0x589423221c35da62,
    0xd170e5458cf2db4c,
    0x5a2826af12b9b4c6,
    // 4 * G
    0x95fe050a056818bf,
    0x327e89715660faa9,
    0xc3e8e3cd06a05073,
    0x27933f4c7445a49a,
    0x287351b98efc099f,
    0x6765c6f47dfd2538,
    0xca348d3dfb0a9265,
    0x680e910321e58727,
    0x5a13fbe9c476ff09,
    0x6e9e39457b5cc172,
    0x5ddbdcf9102b4494,
    0x7f9d0cbf63553e2b,
    // 5 * G
    0x7f9182c3a447d6ba,
    0xd50014d14b2729b7,
    0xe33cf11cb864a087,
    0x154a7e73eb1b55f3,
    0xa212bc4408a5bb33,
    0x8d5048c3c75eed02,
    0xdd1beb0c5abfec44,
    0x2945ccf146e206eb,
    0xbcbbdbf1812a8285,
    0x270e0807d0bdd1fc,
    0xb41b670b1bbda72d,
    0x43aabe696b3bb69a,
    // 6 * G
    0x499806b67b7d8ca4,
    0x575be28427d22739,
    0xbb085ce7204553b9,
    0x38b64c41ae417884,
    0x3a0ceeeb77157131,
    0x9b27158900c8af88,
    0x8065b668da59a736,
    0x51e57bb6a2cc38bd,
    0x85ac326702ea4b71,
    0xbe70e00341a1bb01,
    0x53e4a24b083bc144,
    0x10b8e91a9f0d61e3,
    // 7 * G
    0xba6f2c9aaa3221b1,
    0x6ca021533bba23a7,
    0x9dea764f92192c3a,
    0x1d6edd5d2e5317e0,
    0x6b1a5cd0944ea3bf,
    0x7470353ab39dc0d2,
    0x71b2528228542e49,
    0x461bea69283c927e,
    0xf1836dc801b8b3a2,
    0xb3035f47053ea49a,
    0x529c41ba5877adf3,
    0x7a9fbb1c6a0f90a7,
    // 8 * G
    0xe2a75dedf39234d9,
    0x963d7680e1b558f9,
    0x2c2741ac6e3c23fb,
    0x3a9024a1320e01c3,
    0x59b7596604dd3e8f,
    0x6cb30377e288702c,
    0xb1339c665ed9c323,
    0x0915e76061bce52f,
    0xe7c1f5d9c9a2911a,
    0xb8a371788bcca7d7,
    0x636412190eb62a32,
    0x26907c5c2ecc4e95,
];
