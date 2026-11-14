// generated source. do not edit.
#![allow(non_upper_case_globals, unused_macros, unused_imports)]
use crate::low::macros::*;

// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

// ----------------------------------------------------------------------------
// Point doubling on NIST curve P-256 in Montgomery-Jacobian coordinates
//
//    extern void p256_montjdouble_alt(uint64_t p3[static 12],
//                                     const uint64_t p1[static 12]);
//
// Does p3 := 2 * p1 where all points are regarded as Jacobian triples with
// each coordinate in the Montgomery domain, i.e. x' = (2^256 * x) mod p_256.
// A Jacobian triple (x',y',z') represents affine point (x/z^2,y/z^3).
//
// Standard ARM ABI: X0 = p3, X1 = p1
// ----------------------------------------------------------------------------

// Size of individual field elements

macro_rules! NUMSIZE {
    () => {
        "32"
    };
}

// Stable homes for input arguments during main code sequence

macro_rules! input_z {
    () => {
        "x15"
    };
}
macro_rules! input_x {
    () => {
        "x16"
    };
}

// Pointer-offset pairs for inputs and outputs

macro_rules! x_1 { () => { Q!(input_x!() ", #0") } }
macro_rules! y_1 { () => { Q!(input_x!() ", # " NUMSIZE!()) } }
macro_rules! z_1 { () => { Q!(input_x!() ", # (2 * " NUMSIZE!() ")") } }

macro_rules! x_3 { () => { Q!(input_z!() ", #0") } }
macro_rules! y_3 { () => { Q!(input_z!() ", # " NUMSIZE!()) } }
macro_rules! z_3 { () => { Q!(input_z!() ", # (2 * " NUMSIZE!() ")") } }

// Pointer-offset pairs for temporaries, with some aliasing
// #NSPACE is the total stack needed for these temporaries

macro_rules! z2 { () => { Q!("sp, # (" NUMSIZE!() "* 0)") } }
macro_rules! y4 { () => { Q!("sp, # (" NUMSIZE!() "* 0)") } }

macro_rules! y2 { () => { Q!("sp, # (" NUMSIZE!() "* 1)") } }

macro_rules! t1 { () => { Q!("sp, # (" NUMSIZE!() "* 2)") } }

macro_rules! t2 { () => { Q!("sp, # (" NUMSIZE!() "* 3)") } }
macro_rules! x2p { () => { Q!("sp, # (" NUMSIZE!() "* 3)") } }
macro_rules! dx2 { () => { Q!("sp, # (" NUMSIZE!() "* 3)") } }

macro_rules! xy2 { () => { Q!("sp, # (" NUMSIZE!() "* 4)") } }

macro_rules! x4p { () => { Q!("sp, # (" NUMSIZE!() "* 5)") } }
macro_rules! d { () => { Q!("sp, # (" NUMSIZE!() "* 5)") } }

macro_rules! NSPACE { () => { Q!(NUMSIZE!() "* 6") } }

// Corresponds exactly to bignum_montmul_p256_alt except registers

macro_rules! montmul_p256 {
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
        "umulh x0, x3, x9;\n"
        "adcs x14, x14, x11;\n"
        "mul x11, x3, x10;\n"
        "umulh x1, x3, x10;\n"
        "adcs x0, x0, x11;\n"
        "adc x1, x1, xzr;\n"
        "ldp x5, x6, [" $P1 "+ 16];\n"
        "mul x11, x4, x7;\n"
        "adds x13, x13, x11;\n"
        "mul x11, x4, x8;\n"
        "adcs x14, x14, x11;\n"
        "mul x11, x4, x9;\n"
        "adcs x0, x0, x11;\n"
        "mul x11, x4, x10;\n"
        "adcs x1, x1, x11;\n"
        "umulh x3, x4, x10;\n"
        "adc x3, x3, xzr;\n"
        "umulh x11, x4, x7;\n"
        "adds x14, x14, x11;\n"
        "umulh x11, x4, x8;\n"
        "adcs x0, x0, x11;\n"
        "umulh x11, x4, x9;\n"
        "adcs x1, x1, x11;\n"
        "adc x3, x3, xzr;\n"
        "mul x11, x5, x7;\n"
        "adds x14, x14, x11;\n"
        "mul x11, x5, x8;\n"
        "adcs x0, x0, x11;\n"
        "mul x11, x5, x9;\n"
        "adcs x1, x1, x11;\n"
        "mul x11, x5, x10;\n"
        "adcs x3, x3, x11;\n"
        "umulh x4, x5, x10;\n"
        "adc x4, x4, xzr;\n"
        "umulh x11, x5, x7;\n"
        "adds x0, x0, x11;\n"
        "umulh x11, x5, x8;\n"
        "adcs x1, x1, x11;\n"
        "umulh x11, x5, x9;\n"
        "adcs x3, x3, x11;\n"
        "adc x4, x4, xzr;\n"
        "mul x11, x6, x7;\n"
        "adds x0, x0, x11;\n"
        "mul x11, x6, x8;\n"
        "adcs x1, x1, x11;\n"
        "mul x11, x6, x9;\n"
        "adcs x3, x3, x11;\n"
        "mul x11, x6, x10;\n"
        "adcs x4, x4, x11;\n"
        "umulh x5, x6, x10;\n"
        "adc x5, x5, xzr;\n"
        "mov x10, #0xffffffff00000001;\n"
        "adds x13, x13, x12, lsl #32;\n"
        "lsr x11, x12, #32;\n"
        "adcs x14, x14, x11;\n"
        "mul x11, x12, x10;\n"
        "umulh x12, x12, x10;\n"
        "adcs x0, x0, x11;\n"
        "adc x12, x12, xzr;\n"
        "umulh x11, x6, x7;\n"
        "adds x1, x1, x11;\n"
        "umulh x11, x6, x8;\n"
        "adcs x3, x3, x11;\n"
        "umulh x11, x6, x9;\n"
        "adcs x4, x4, x11;\n"
        "adc x5, x5, xzr;\n"
        "adds x14, x14, x13, lsl #32;\n"
        "lsr x11, x13, #32;\n"
        "adcs x0, x0, x11;\n"
        "mul x11, x13, x10;\n"
        "umulh x13, x13, x10;\n"
        "adcs x12, x12, x11;\n"
        "adc x13, x13, xzr;\n"
        "adds x0, x0, x14, lsl #32;\n"
        "lsr x11, x14, #32;\n"
        "adcs x12, x12, x11;\n"
        "mul x11, x14, x10;\n"
        "umulh x14, x14, x10;\n"
        "adcs x13, x13, x11;\n"
        "adc x14, x14, xzr;\n"
        "adds x12, x12, x0, lsl #32;\n"
        "lsr x11, x0, #32;\n"
        "adcs x13, x13, x11;\n"
        "mul x11, x0, x10;\n"
        "umulh x0, x0, x10;\n"
        "adcs x14, x14, x11;\n"
        "adc x0, x0, xzr;\n"
        "adds x12, x12, x1;\n"
        "adcs x13, x13, x3;\n"
        "adcs x14, x14, x4;\n"
        "adcs x0, x0, x5;\n"
        "cset x8, cs;\n"
        "mov x11, #0xffffffff;\n"
        "adds x1, x12, #0x1;\n"
        "sbcs x3, x13, x11;\n"
        "sbcs x4, x14, xzr;\n"
        "sbcs x5, x0, x10;\n"
        "sbcs xzr, x8, xzr;\n"
        "csel x12, x12, x1, cc;\n"
        "csel x13, x13, x3, cc;\n"
        "csel x14, x14, x4, cc;\n"
        "csel x0, x0, x5, cc;\n"
        "stp x12, x13, [" $P0 "];\n"
        "stp x14, x0, [" $P0 "+ 16]"
    )}
}

// Corresponds exactly to bignum_montsqr_p256_alt

macro_rules! montsqr_p256 {
    ($P0:expr, $P1:expr) => { Q!(
        "ldp x2, x3, [" $P1 "];\n"
        "mul x9, x2, x3;\n"
        "umulh x10, x2, x3;\n"
        "ldp x4, x5, [" $P1 "+ 16];\n"
        "mul x11, x2, x5;\n"
        "umulh x12, x2, x5;\n"
        "mul x6, x2, x4;\n"
        "umulh x7, x2, x4;\n"
        "adds x10, x10, x6;\n"
        "adcs x11, x11, x7;\n"
        "mul x6, x3, x4;\n"
        "umulh x7, x3, x4;\n"
        "adc x7, x7, xzr;\n"
        "adds x11, x11, x6;\n"
        "mul x13, x4, x5;\n"
        "umulh x14, x4, x5;\n"
        "adcs x12, x12, x7;\n"
        "mul x6, x3, x5;\n"
        "umulh x7, x3, x5;\n"
        "adc x7, x7, xzr;\n"
        "adds x12, x12, x6;\n"
        "adcs x13, x13, x7;\n"
        "adc x14, x14, xzr;\n"
        "adds x9, x9, x9;\n"
        "adcs x10, x10, x10;\n"
        "adcs x11, x11, x11;\n"
        "adcs x12, x12, x12;\n"
        "adcs x13, x13, x13;\n"
        "adcs x14, x14, x14;\n"
        "cset x7, hs;\n"
        "umulh x6, x2, x2;\n"
        "mul x8, x2, x2;\n"
        "adds x9, x9, x6;\n"
        "mul x6, x3, x3;\n"
        "adcs x10, x10, x6;\n"
        "umulh x6, x3, x3;\n"
        "adcs x11, x11, x6;\n"
        "mul x6, x4, x4;\n"
        "adcs x12, x12, x6;\n"
        "umulh x6, x4, x4;\n"
        "adcs x13, x13, x6;\n"
        "mul x6, x5, x5;\n"
        "adcs x14, x14, x6;\n"
        "umulh x6, x5, x5;\n"
        "adc x7, x7, x6;\n"
        "mov x5, #-4294967295;\n"
        "adds x9, x9, x8, lsl #32;\n"
        "lsr x3, x8, #32;\n"
        "adcs x10, x10, x3;\n"
        "mul x2, x8, x5;\n"
        "umulh x8, x8, x5;\n"
        "adcs x11, x11, x2;\n"
        "adc x8, x8, xzr;\n"
        "adds x10, x10, x9, lsl #32;\n"
        "lsr x3, x9, #32;\n"
        "adcs x11, x11, x3;\n"
        "mul x2, x9, x5;\n"
        "umulh x9, x9, x5;\n"
        "adcs x8, x8, x2;\n"
        "adc x9, x9, xzr;\n"
        "adds x11, x11, x10, lsl #32;\n"
        "lsr x3, x10, #32;\n"
        "adcs x8, x8, x3;\n"
        "mul x2, x10, x5;\n"
        "umulh x10, x10, x5;\n"
        "adcs x9, x9, x2;\n"
        "adc x10, x10, xzr;\n"
        "adds x8, x8, x11, lsl #32;\n"
        "lsr x3, x11, #32;\n"
        "adcs x9, x9, x3;\n"
        "mul x2, x11, x5;\n"
        "umulh x11, x11, x5;\n"
        "adcs x10, x10, x2;\n"
        "adc x11, x11, xzr;\n"
        "adds x8, x8, x12;\n"
        "adcs x9, x9, x13;\n"
        "adcs x10, x10, x14;\n"
        "adcs x11, x11, x7;\n"
        "cset x2, hs;\n"
        "mov x3, #4294967295;\n"
        "adds x12, x8, #1;\n"
        "sbcs x13, x9, x3;\n"
        "sbcs x14, x10, xzr;\n"
        "sbcs x7, x11, x5;\n"
        "sbcs xzr, x2, xzr;\n"
        "csel x8, x8, x12, lo;\n"
        "csel x9, x9, x13, lo;\n"
        "csel x10, x10, x14, lo;\n"
        "csel x11, x11, x7, lo;\n"
        "stp x8, x9, [" $P0 "];\n"
        "stp x10, x11, [" $P0 "+ 16]"
    )}
}

// Corresponds exactly to bignum_sub_p256

macro_rules! sub_p256 {
    ($P0:expr, $P1:expr, $P2:expr) => { Q!(
        "ldp x5, x6, [" $P1 "];\n"
        "ldp x4, x3, [" $P2 "];\n"
        "subs x5, x5, x4;\n"
        "sbcs x6, x6, x3;\n"
        "ldp x7, x8, [" $P1 "+ 16];\n"
        "ldp x4, x3, [" $P2 "+ 16];\n"
        "sbcs x7, x7, x4;\n"
        "sbcs x8, x8, x3;\n"
        "csetm x3, lo;\n"
        "adds x5, x5, x3;\n"
        "and x4, x3, #0xffffffff;\n"
        "adcs x6, x6, x4;\n"
        "adcs x7, x7, xzr;\n"
        "and x4, x3, #0xffffffff00000001;\n"
        "adc x8, x8, x4;\n"
        "stp x5, x6, [" $P0 "];\n"
        "stp x7, x8, [" $P0 "+ 16]"
    )}
}

// Corresponds exactly to bignum_add_p256

macro_rules! add_p256 {
    ($P0:expr, $P1:expr, $P2:expr) => { Q!(
        "ldp x5, x6, [" $P1 "];\n"
        "ldp x4, x3, [" $P2 "];\n"
        "adds x5, x5, x4;\n"
        "adcs x6, x6, x3;\n"
        "ldp x7, x8, [" $P1 "+ 16];\n"
        "ldp x4, x3, [" $P2 "+ 16];\n"
        "adcs x7, x7, x4;\n"
        "adcs x8, x8, x3;\n"
        "adc x3, xzr, xzr;\n"
        "cmn x5, #1;\n"
        "mov x4, #4294967295;\n"
        "sbcs xzr, x6, x4;\n"
        "sbcs xzr, x7, xzr;\n"
        "mov x4, #-4294967295;\n"
        "sbcs xzr, x8, x4;\n"
        "adcs x3, x3, xzr;\n"
        "csetm x3, ne;\n"
        "subs x5, x5, x3;\n"
        "and x4, x3, #0xffffffff;\n"
        "sbcs x6, x6, x4;\n"
        "sbcs x7, x7, xzr;\n"
        "and x4, x3, #0xffffffff00000001;\n"
        "sbc x8, x8, x4;\n"
        "stp x5, x6, [" $P0 "];\n"
        "stp x7, x8, [" $P0 "+ 16]"
    )}
}

// A weak version of add that only guarantees sum in 4 digits

macro_rules! weakadd_p256 {
    ($P0:expr, $P1:expr, $P2:expr) => { Q!(
        "ldp x5, x6, [" $P1 "];\n"
        "ldp x4, x3, [" $P2 "];\n"
        "adds x5, x5, x4;\n"
        "adcs x6, x6, x3;\n"
        "ldp x7, x8, [" $P1 "+ 16];\n"
        "ldp x4, x3, [" $P2 "+ 16];\n"
        "adcs x7, x7, x4;\n"
        "adcs x8, x8, x3;\n"
        "csetm x3, cs;\n"
        "subs x5, x5, x3;\n"
        "and x1, x3, #4294967295;\n"
        "sbcs x6, x6, x1;\n"
        "sbcs x7, x7, xzr;\n"
        "and x2, x3, #-4294967295;\n"
        "sbc x8, x8, x2;\n"
        "stp x5, x6, [" $P0 "];\n"
        "stp x7, x8, [" $P0 "+ 16]"
    )}
}

// P0 = C * P1 - D * P2 computed as D * (p_256 - P2) + C * P1
// Quotient estimation is done just as q = h + 1 as in bignum_triple_p256
// This also applies to the other functions following.

macro_rules! cmsub_p256 {
    ($P0:expr, $C:expr, $P1:expr, $D:expr, $P2:expr) => { Q!(
        "mov x1, " $D ";\n"
        "mov x2, #-1;\n"
        "ldp x9, x10, [" $P2 "];\n"
        "subs x9, x2, x9;\n"
        "mov x2, #4294967295;\n"
        "sbcs x10, x2, x10;\n"
        "ldp x11, x12, [" $P2 "+ 16];\n"
        "sbcs x11, xzr, x11;\n"
        "mov x2, #-4294967295;\n"
        "sbc x12, x2, x12;\n"
        "mul x3, x1, x9;\n"
        "mul x4, x1, x10;\n"
        "mul x5, x1, x11;\n"
        "mul x6, x1, x12;\n"
        "umulh x9, x1, x9;\n"
        "umulh x10, x1, x10;\n"
        "umulh x11, x1, x11;\n"
        "umulh x7, x1, x12;\n"
        "adds x4, x4, x9;\n"
        "adcs x5, x5, x10;\n"
        "adcs x6, x6, x11;\n"
        "adc x7, x7, xzr;\n"
        "mov x1, " $C ";\n"
        "ldp x9, x10, [" $P1 "];\n"
        "mul x8, x9, x1;\n"
        "umulh x9, x9, x1;\n"
        "adds x3, x3, x8;\n"
        "mul x8, x10, x1;\n"
        "umulh x10, x10, x1;\n"
        "adcs x4, x4, x8;\n"
        "ldp x11, x12, [" $P1 "+ 16];\n"
        "mul x8, x11, x1;\n"
        "umulh x11, x11, x1;\n"
        "adcs x5, x5, x8;\n"
        "mul x8, x12, x1;\n"
        "umulh x12, x12, x1;\n"
        "adcs x6, x6, x8;\n"
        "adc x7, x7, xzr;\n"
        "adds x4, x4, x9;\n"
        "adcs x5, x5, x10;\n"
        "adcs x6, x6, x11;\n"
        "adc x7, x7, x12;\n"
        "add x8, x7, #1;\n"
        "lsl x10, x8, #32;\n"
        "adds x6, x6, x10;\n"
        "adc x7, x7, xzr;\n"
        "neg x9, x8;\n"
        "sub x10, x10, #1;\n"
        "subs x3, x3, x9;\n"
        "sbcs x4, x4, x10;\n"
        "sbcs x5, x5, xzr;\n"
        "sbcs x6, x6, x8;\n"
        "sbc x8, x7, x8;\n"
        "adds x3, x3, x8;\n"
        "and x9, x8, #4294967295;\n"
        "adcs x4, x4, x9;\n"
        "adcs x5, x5, xzr;\n"
        "neg x10, x9;\n"
        "adc x6, x6, x10;\n"
        "stp x3, x4, [" $P0 "];\n"
        "stp x5, x6, [" $P0 "+ 16]"
    )}
}

// P0 = 4 * P1 - P2, by direct subtraction of P2; the method
// in bignum_cmul_p256 etc. for quotient estimation still
// works when the value to be reduced is negative, as
// long as it is  > -p_256, which is the case here. The
// actual accumulation of q * p_256 is done a bit differently
// so it works for the q = 0 case.

macro_rules! cmsub41_p256 {
    ($P0:expr, $P1:expr, $P2:expr) => { Q!(
        "ldp x1, x2, [" $P1 "];\n"
        "lsl x0, x1, #2;\n"
        "ldp x6, x7, [" $P2 "];\n"
        "subs x0, x0, x6;\n"
        "extr x1, x2, x1, #62;\n"
        "sbcs x1, x1, x7;\n"
        "ldp x3, x4, [" $P1 "+ 16];\n"
        "extr x2, x3, x2, #62;\n"
        "ldp x6, x7, [" $P2 "+ 16];\n"
        "sbcs x2, x2, x6;\n"
        "extr x3, x4, x3, #62;\n"
        "sbcs x3, x3, x7;\n"
        "lsr x4, x4, #62;\n"
        "sbc x4, x4, xzr;\n"
        "add x5, x4, #1;\n"
        "lsl x8, x5, #32;\n"
        "subs x6, xzr, x8;\n"
        "sbcs x7, xzr, xzr;\n"
        "sbc x8, x8, x5;\n"
        "adds x0, x0, x5;\n"
        "adcs x1, x1, x6;\n"
        "adcs x2, x2, x7;\n"
        "adcs x3, x3, x8;\n"
        "csetm x5, cc;\n"
        "adds x0, x0, x5;\n"
        "and x6, x5, #4294967295;\n"
        "adcs x1, x1, x6;\n"
        "adcs x2, x2, xzr;\n"
        "neg x7, x6;\n"
        "adc x3, x3, x7;\n"
        "stp x0, x1, [" $P0 "];\n"
        "stp x2, x3, [" $P0 "+ 16]"
    )}
}

// P0 = 3 * P1 - 8 * P2, computed as (p_256 - P2) << 3 + 3 * P1

macro_rules! cmsub38_p256 {
    ($P0:expr, $P1:expr, $P2:expr) => { Q!(
        "mov x1, 8;\n"
        "mov x2, #-1;\n"
        "ldp x9, x10, [" $P2 "];\n"
        "subs x9, x2, x9;\n"
        "mov x2, #4294967295;\n"
        "sbcs x10, x2, x10;\n"
        "ldp x11, x12, [" $P2 "+ 16];\n"
        "sbcs x11, xzr, x11;\n"
        "mov x2, #-4294967295;\n"
        "sbc x12, x2, x12;\n"
        "lsl x3, x9, #3;\n"
        "extr x4, x10, x9, #61;\n"
        "extr x5, x11, x10, #61;\n"
        "extr x6, x12, x11, #61;\n"
        "lsr x7, x12, #61;\n"
        "mov x1, 3;\n"
        "ldp x9, x10, [" $P1 "];\n"
        "mul x8, x9, x1;\n"
        "umulh x9, x9, x1;\n"
        "adds x3, x3, x8;\n"
        "mul x8, x10, x1;\n"
        "umulh x10, x10, x1;\n"
        "adcs x4, x4, x8;\n"
        "ldp x11, x12, [" $P1 "+ 16];\n"
        "mul x8, x11, x1;\n"
        "umulh x11, x11, x1;\n"
        "adcs x5, x5, x8;\n"
        "mul x8, x12, x1;\n"
        "umulh x12, x12, x1;\n"
        "adcs x6, x6, x8;\n"
        "adc x7, x7, xzr;\n"
        "adds x4, x4, x9;\n"
        "adcs x5, x5, x10;\n"
        "adcs x6, x6, x11;\n"
        "adc x7, x7, x12;\n"
        "add x8, x7, #1;\n"
        "lsl x10, x8, #32;\n"
        "adds x6, x6, x10;\n"
        "adc x7, x7, xzr;\n"
        "neg x9, x8;\n"
        "sub x10, x10, #1;\n"
        "subs x3, x3, x9;\n"
        "sbcs x4, x4, x10;\n"
        "sbcs x5, x5, xzr;\n"
        "sbcs x6, x6, x8;\n"
        "sbc x8, x7, x8;\n"
        "adds x3, x3, x8;\n"
        "and x9, x8, #4294967295;\n"
        "adcs x4, x4, x9;\n"
        "adcs x5, x5, xzr;\n"
        "neg x10, x9;\n"
        "adc x6, x6, x10;\n"
        "stp x3, x4, [" $P0 "];\n"
        "stp x5, x6, [" $P0 "+ 16]"
    )}
}

/// Point doubling on NIST curve P-256 in Montgomery-Jacobian coordinates
///
///
/// Does p3 := 2 * p1 where all points are regarded as Jacobian triples with
/// each coordinate in the Montgomery domain, i.e. x' = (2^256 * x) mod p_256.
/// A Jacobian triple (x',y',z') represents affine point (x/z^2,y/z^3).
pub(crate) fn p256_montjdouble(p3: &mut [u64; 12], p1: &[u64; 12]) {
    // SAFETY: inline assembly. see [crate::low::inline_assembly_safety] for safety info.
    unsafe {
        core::arch::asm!(

        // Make room on stack for temporary variables

        Q!("    sub             " "sp, sp, # (" NSPACE!() "+ 0)"),

        // Move the input arguments to stable places

        Q!("    mov             " input_z!() ", x0"),
        Q!("    mov             " input_x!() ", x1"),

        // Main code, just a sequence of basic field operations

        // z2 = z^2
        // y2 = y^2

        montsqr_p256!(z2!(), z_1!()),
        montsqr_p256!(y2!(), y_1!()),

        // x2p = x^2 - z^4 = (x + z^2) * (x - z^2)

        sub_p256!(t2!(), x_1!(), z2!()),
        weakadd_p256!(t1!(), x_1!(), z2!()),
        montmul_p256!(x2p!(), t1!(), t2!()),

        // t1 = y + z
        // xy2 = x * y^2
        // x4p = x2p^2

        add_p256!(t1!(), y_1!(), z_1!()),
        montmul_p256!(xy2!(), x_1!(), y2!()),
        montsqr_p256!(x4p!(), x2p!()),

        // t1 = (y + z)^2

        montsqr_p256!(t1!(), t1!()),

        // d = 12 * xy2 - 9 * x4p
        // t1 = y^2 + 2 * y * z

        cmsub_p256!(d!(), "12", xy2!(), "9", x4p!()),
        sub_p256!(t1!(), t1!(), z2!()),

        // y4 = y^4

        montsqr_p256!(y4!(), y2!()),

        // dx2 = d * x2p

        montmul_p256!(dx2!(), d!(), x2p!()),

        // z_3' = 2 * y * z

        sub_p256!(z_3!(), t1!(), y2!()),

        // x' = 4 * xy2 - d

        cmsub41_p256!(x_3!(), xy2!(), d!()),

        // y' = 3 * dx2 - 8 * y4

        cmsub38_p256!(y_3!(), dx2!(), y4!()),

        // Restore stack and return

        Q!("    add             " "sp, sp, # (" NSPACE!() "+ 0)"),
        inout("x0") p3.as_mut_ptr() => _,
        inout("x1") p1.as_ptr() => _,
        // clobbers
        out("x10") _,
        out("x11") _,
        out("x12") _,
        out("x13") _,
        out("x14") _,
        out("x15") _,
        out("x16") _,
        out("x2") _,
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
