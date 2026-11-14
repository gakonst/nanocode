// generated source. do not edit.
#![allow(non_upper_case_globals, unused_macros, unused_imports)]
use crate::low::macros::*;

// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

// ----------------------------------------------------------------------------
// The x25519 function for curve25519
// Inputs scalar[4], point[4]; output res[4]
//
// extern void curve25519_x25519_alt
//   (uint64_t res[static 4],const uint64_t scalar[static 4],
//    const uint64_t point[static 4]);
//
// Given a scalar n and the X coordinate of an input point P = (X,Y) on
// curve25519 (Y can live in any extension field of characteristic 2^255-19),
// this returns the X coordinate of n * P = (X, Y), or 0 when n * P is the
// point at infinity. Both n and X inputs are first slightly modified/mangled
// as specified in the relevant RFC (https://www.rfc-editor.org/rfc/rfc7748);
// in particular the lower three bits of n are set to zero. Does not implement
// the zero-check specified in Section 6.1.
//
// Standard ARM ABI: X0 = res, X1 = scalar, X2 = point
// ----------------------------------------------------------------------------

// Size of individual field elements

macro_rules! NUMSIZE {
    () => {
        "32"
    };
}

// Stable homes for the input result argument during the whole body
// and other variables that are only needed prior to the modular inverse.

macro_rules! res {
    () => {
        "x23"
    };
}
macro_rules! i {
    () => {
        "x20"
    };
}
macro_rules! swap {
    () => {
        "x21"
    };
}

// Pointers to result x coord to be written

macro_rules! resx { () => { Q!(res!() ", #0") } }

// Pointer-offset pairs for temporaries on stack with some aliasing.

macro_rules! scalar { () => { Q!("sp, # (0 * " NUMSIZE!() ")") } }

macro_rules! pointx { () => { Q!("sp, # (1 * " NUMSIZE!() ")") } }

macro_rules! zm { () => { Q!("sp, # (2 * " NUMSIZE!() ")") } }
macro_rules! sm { () => { Q!("sp, # (2 * " NUMSIZE!() ")") } }
macro_rules! dpro { () => { Q!("sp, # (2 * " NUMSIZE!() ")") } }

macro_rules! sn { () => { Q!("sp, # (3 * " NUMSIZE!() ")") } }

macro_rules! dm { () => { Q!("sp, # (4 * " NUMSIZE!() ")") } }

macro_rules! zn { () => { Q!("sp, # (5 * " NUMSIZE!() ")") } }
macro_rules! dn { () => { Q!("sp, # (5 * " NUMSIZE!() ")") } }
macro_rules! e { () => { Q!("sp, # (5 * " NUMSIZE!() ")") } }

macro_rules! dmsn { () => { Q!("sp, # (6 * " NUMSIZE!() ")") } }
macro_rules! p { () => { Q!("sp, # (6 * " NUMSIZE!() ")") } }

macro_rules! xm { () => { Q!("sp, # (7 * " NUMSIZE!() ")") } }
macro_rules! dnsm { () => { Q!("sp, # (7 * " NUMSIZE!() ")") } }
macro_rules! spro { () => { Q!("sp, # (7 * " NUMSIZE!() ")") } }

macro_rules! d { () => { Q!("sp, # (8 * " NUMSIZE!() ")") } }

macro_rules! xn { () => { Q!("sp, # (9 * " NUMSIZE!() ")") } }
macro_rules! s { () => { Q!("sp, # (9 * " NUMSIZE!() ")") } }

// Total size to reserve on the stack

macro_rules! NSPACE { () => { Q!("10 * " NUMSIZE!()) } }

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

// Modular addition with double modulus 2 * p_25519 = 2^256 - 38.
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

// Modular subtraction with double modulus 2 * p_25519 = 2^256 - 38

macro_rules! sub_twice4 {
    ($p0:expr, $p1:expr, $p2:expr) => { Q!(
        "ldp x5, x6, [" $p1 "];\n"
        "ldp x4, x3, [" $p2 "];\n"
        "subs x5, x5, x4;\n"
        "sbcs x6, x6, x3;\n"
        "ldp x7, x8, [" $p1 "+ 16];\n"
        "ldp x4, x3, [" $p2 "+ 16];\n"
        "sbcs x7, x7, x4;\n"
        "sbcs x8, x8, x3;\n"
        "mov x4, #38;\n"
        "csel x3, x4, xzr, lo;\n"
        "subs x5, x5, x3;\n"
        "sbcs x6, x6, xzr;\n"
        "sbcs x7, x7, xzr;\n"
        "sbc x8, x8, xzr;\n"
        "stp x5, x6, [" $p0 "];\n"
        "stp x7, x8, [" $p0 "+ 16]"
    )}
}

// Combined z = c * x + y with reduction only < 2 * p_25519
// where c is initially in the X1 register. It is assumed
// that 19 * (c * x + y) < 2^60 * 2^256 so we don't need a
// high mul in the final part.

macro_rules! cmadd_4 {
    ($p0:expr, $p2:expr, $p3:expr) => { Q!(
        "ldp x7, x8, [" $p2 "];\n"
        "ldp x9, x10, [" $p2 "+ 16];\n"
        "mul x3, x1, x7;\n"
        "mul x4, x1, x8;\n"
        "mul x5, x1, x9;\n"
        "mul x6, x1, x10;\n"
        "umulh x7, x1, x7;\n"
        "umulh x8, x1, x8;\n"
        "umulh x9, x1, x9;\n"
        "umulh x10, x1, x10;\n"
        "adds x4, x4, x7;\n"
        "adcs x5, x5, x8;\n"
        "adcs x6, x6, x9;\n"
        "adc x10, x10, xzr;\n"
        "ldp x7, x8, [" $p3 "];\n"
        "adds x3, x3, x7;\n"
        "adcs x4, x4, x8;\n"
        "ldp x7, x8, [" $p3 "+ 16];\n"
        "adcs x5, x5, x7;\n"
        "adcs x6, x6, x8;\n"
        "adc x10, x10, xzr;\n"
        "cmn x6, x6;\n"
        "bic x6, x6, #0x8000000000000000;\n"
        "adc x8, x10, x10;\n"
        "mov x9, #19;\n"
        "mul x7, x8, x9;\n"
        "adds x3, x3, x7;\n"
        "adcs x4, x4, xzr;\n"
        "adcs x5, x5, xzr;\n"
        "adc x6, x6, xzr;\n"
        "stp x3, x4, [" $p0 "];\n"
        "stp x5, x6, [" $p0 "+ 16]"
    )}
}

// Multiplex: z := if NZ then x else y

macro_rules! mux_4 {
    ($p0:expr, $p1:expr, $p2:expr) => { Q!(
        "ldp x0, x1, [" $p1 "];\n"
        "ldp x2, x3, [" $p2 "];\n"
        "csel x0, x0, x2, ne;\n"
        "csel x1, x1, x3, ne;\n"
        "stp x0, x1, [" $p0 "];\n"
        "ldp x0, x1, [" $p1 "+ 16];\n"
        "ldp x2, x3, [" $p2 "+ 16];\n"
        "csel x0, x0, x2, ne;\n"
        "csel x1, x1, x3, ne;\n"
        "stp x0, x1, [" $p0 "+ 16]"
    )}
}

/// The x25519 function for curve25519
///
/// Inputs scalar[4], point[4]; output res[4]
///
/// Given a scalar n and the X coordinate of an input point P = (X,Y) on
/// curve25519 (Y can live in any extension field of characteristic 2^255-19),
/// this returns the X coordinate of n * P = (X, Y), or 0 when n * P is the
/// point at infinity. Both n and X inputs are first slightly modified/mangled
/// as specified in the relevant RFC (https://www.rfc-editor.org/rfc/rfc7748);
/// in particular the lower three bits of n are set to zero. Does not implement
/// the zero-check specified in Section 6.1.
pub(crate) fn curve25519_x25519(res: &mut [u64; 4], scalar: &[u64; 4], point: &[u64; 4]) {
    // SAFETY: inline assembly. see [crate::low::inline_assembly_safety] for safety info.
    unsafe {
        core::arch::asm!(

        // Save regs and make room for temporaries

        Q!("    stp             " "x19, x20, [sp, #-16] !"),
        Q!("    stp             " "x21, x22, [sp, #-16] !"),
        Q!("    stp             " "x23, x24, [sp, #-16] !"),
        Q!("    sub             " "sp, sp, # (" NSPACE!() "+ 0)"),

        // Move the output pointer to a stable place

        Q!("    mov             " res!() ", x0"),

        // Copy the inputs to the local variables with minimal mangling:
        //
        //  - The scalar is in principle turned into 01xxx...xxx000 but
        //    in the structure below the special handling of these bits is
        //    explicit in the main computation; the scalar is just copied.
        //
        //  - The point x coord is reduced mod 2^255 by masking off the
        //    top bit. In the main loop we only need reduction < 2 * p_25519.

        Q!("    ldp             " "x10, x11, [x1]"),
        Q!("    stp             " "x10, x11, [" scalar!() "]"),
        Q!("    ldp             " "x12, x13, [x1, #16]"),
        Q!("    stp             " "x12, x13, [" scalar!() "+ 16]"),

        Q!("    ldp             " "x10, x11, [x2]"),
        Q!("    stp             " "x10, x11, [" pointx!() "]"),
        Q!("    ldp             " "x12, x13, [x2, #16]"),
        Q!("    and             " "x13, x13, #0x7fffffffffffffff"),
        Q!("    stp             " "x12, x13, [" pointx!() "+ 16]"),

        // Initialize with explicit doubling in order to handle set bit 254.
        // Set swap = 1 and (xm,zm) = (x,1) then double as (xn,zn) = 2 * (x,1).
        // We use the fact that the point x coordinate is still in registers.
        // Since zm = 1 we could do the doubling with an operation count of
        // 2 * S + M instead of 2 * S + 2 * M, but it doesn't seem worth
        // the slight complication arising from a different linear combination.

        Q!("    mov             " swap!() ", #1"),
        Q!("    stp             " "x10, x11, [" xm!() "]"),
        Q!("    stp             " "x12, x13, [" xm!() "+ 16]"),
        Q!("    stp             " swap!() ", xzr, [" zm!() "]"),
        Q!("    stp             " "xzr, xzr, [" zm!() "+ 16]"),

        sub_twice4!(d!(), xm!(), zm!()),
        add_twice4!(s!(), xm!(), zm!()),
        sqr_4!(d!(), d!()),
        sqr_4!(s!(), s!()),
        sub_twice4!(p!(), s!(), d!()),
        Q!("    mov             " "x1, 0xdb42"),
        Q!("    orr             " "x1, x1, 0x10000"),
        cmadd_4!(e!(), p!(), d!()),
        mul_4!(xn!(), s!(), d!()),
        mul_4!(zn!(), p!(), e!()),

        // The main loop over unmodified bits from i = 253, ..., i = 3 (inclusive).
        // This is a classic Montgomery ladder, with the main coordinates only
        // reduced mod 2 * p_25519, some intermediate results even more loosely.

        Q!("    mov             " i!() ", #253"),

        Q!(Label!("Lcurve25519_x25519_alt_scalarloop", 2) ":"),

        // sm = xm + zm; sn = xn + zn; dm = xm - zm; dn = xn - zn

        sub_twice4!(dm!(), xm!(), zm!()),
        add_twice4!(sn!(), xn!(), zn!()),
        sub_twice4!(dn!(), xn!(), zn!()),
        add_twice4!(sm!(), xm!(), zm!()),

        // ADDING: dmsn = dm * sn
        // DOUBLING: mux d = xt - zt and s = xt + zt for appropriate choice of (xt,zt)

        mul_4!(dmsn!(), sn!(), dm!()),

        Q!("    lsr             " "x0, " i!() ", #6"),
        Q!("    ldr             " "x2, [sp, x0, lsl #3]"),
        Q!("    lsr             " "x2, x2, " i!()),
        Q!("    and             " "x2, x2, #1"),

        Q!("    cmp             " swap!() ", x2"),
        Q!("    mov             " swap!() ", x2"),

        mux_4!(d!(), dm!(), dn!()),
        mux_4!(s!(), sm!(), sn!()),

        // ADDING: dnsm = sm * dn

        mul_4!(dnsm!(), sm!(), dn!()),

        // DOUBLING: d = (xt - zt)^2

        sqr_4!(d!(), d!()),

        // ADDING: dpro = (dmsn - dnsm)^2, spro = (dmsn + dnsm)^2
        // DOUBLING: s = (xt + zt)^2

        sub_twice4!(dpro!(), dmsn!(), dnsm!()),
        sqr_4!(s!(), s!()),
        add_twice4!(spro!(), dmsn!(), dnsm!()),
        sqr_4!(dpro!(), dpro!()),

        // DOUBLING: p = 4 * xt * zt = s - d

        sub_twice4!(p!(), s!(), d!()),

        // ADDING: xm' = (dmsn + dnsm)^2

        sqr_4!(xm!(), spro!()),

        // DOUBLING: e = 121666 * p + d

        Q!("    mov             " "x1, 0xdb42"),
        Q!("    orr             " "x1, x1, 0x10000"),
        cmadd_4!(e!(), p!(), d!()),

        // DOUBLING: xn' = (xt + zt)^2 * (xt - zt)^2 = s * d

        mul_4!(xn!(), s!(), d!()),

        // ADDING: zm' = x * (dmsn - dnsm)^2

        mul_4!(zm!(), dpro!(), pointx!()),

        // DOUBLING: zn' = (4 * xt * zt) * ((xt - zt)^2 + 121666 * (4 * xt * zt))
        //               = p * (d + 121666 * p)

        mul_4!(zn!(), p!(), e!()),

        // Loop down as far as 3 (inclusive)

        Q!("    sub             " i!() ", " i!() ", #1"),
        Q!("    cmp             " i!() ", #3"),
        Q!("    bcs             " Label!("Lcurve25519_x25519_alt_scalarloop", 2, Before)),

        // Multiplex directly into (xn,zn) then do three pure doubling steps;
        // this accounts for the implicit zeroing of the three lowest bits
        // of the scalar.

        Q!("    cmp             " swap!() ", xzr"),
        mux_4!(xn!(), xm!(), xn!()),
        mux_4!(zn!(), zm!(), zn!()),

        sub_twice4!(d!(), xn!(), zn!()),
        add_twice4!(s!(), xn!(), zn!()),
        sqr_4!(d!(), d!()),
        sqr_4!(s!(), s!()),
        sub_twice4!(p!(), s!(), d!()),
        Q!("    mov             " "x1, 0xdb42"),
        Q!("    orr             " "x1, x1, 0x10000"),
        cmadd_4!(e!(), p!(), d!()),
        mul_4!(xn!(), s!(), d!()),
        mul_4!(zn!(), p!(), e!()),

        sub_twice4!(d!(), xn!(), zn!()),
        add_twice4!(s!(), xn!(), zn!()),
        sqr_4!(d!(), d!()),
        sqr_4!(s!(), s!()),
        sub_twice4!(p!(), s!(), d!()),
        Q!("    mov             " "x1, 0xdb42"),
        Q!("    orr             " "x1, x1, 0x10000"),
        cmadd_4!(e!(), p!(), d!()),
        mul_4!(xn!(), s!(), d!()),
        mul_4!(zn!(), p!(), e!()),

        sub_twice4!(d!(), xn!(), zn!()),
        add_twice4!(s!(), xn!(), zn!()),
        sqr_4!(d!(), d!()),
        sqr_4!(s!(), s!()),
        sub_twice4!(p!(), s!(), d!()),
        Q!("    mov             " "x1, 0xdb42"),
        Q!("    orr             " "x1, x1, 0x10000"),
        cmadd_4!(e!(), p!(), d!()),
        mul_4!(xn!(), s!(), d!()),
        mul_4!(zn!(), p!(), e!()),

        // The projective result of the scalar multiplication is now (xn,zn).
        // Prepare to call the modular inverse function to get zn' = 1/zn

        Q!("    add             " "x0, " zn!()),
        Q!("    add             " "x1, " zn!()),

        // Inline copy of bignum_inv_p25519, identical except for stripping out
        // the prologue and epilogue saving and restoring registers and making
        // and reclaiming room on the stack. For more details and explanations see
        // "arm/curve25519/bignum_inv_p25519.S". Note that the stack it uses for
        // its own temporaries is 128 bytes, so it has no effect on variables
        // that are needed in the rest of our computation here: res, xn and zn.

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
        Q!("    b               " Label!("Lcurve25519_x25519_alt_invmidloop", 3, After)),
        Q!(Label!("Lcurve25519_x25519_alt_invloop", 4) ":"),
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
        Q!(Label!("Lcurve25519_x25519_alt_invmidloop", 3) ":"),
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
        Q!("    b.ne            " Label!("Lcurve25519_x25519_alt_invloop", 4, Before)),
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

        // Now the result is xn * (1/zn), fully reduced modulo p.
        // Note that in the degenerate case zn = 0 (mod p_25519), the
        // modular inverse code above will produce 1/zn = 0, giving
        // the correct overall X25519 result of zero for the point at
        // infinity.

        mul_p25519!(resx!(), xn!(), zn!()),

        // Restore stack and registers

        Q!("    add             " "sp, sp, # (" NSPACE!() "+ 0)"),
        Q!("    ldp             " "x23, x24, [sp], #16"),
        Q!("    ldp             " "x21, x22, [sp], #16"),
        Q!("    ldp             " "x19, x20, [sp], #16"),

        inout("x0") res.as_mut_ptr() => _,
        inout("x1") scalar.as_ptr() => _,
        inout("x2") point.as_ptr() => _,
        // clobbers
        out("p0") _,
        out("p1") _,
        out("p2") _,
        out("p3") _,
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
        out("x3") _,
        out("x4") _,
        out("x5") _,
        out("x6") _,
        out("x7") _,
        out("x8") _,
        out("x9") _,
            )
    };
}
