// generated source. do not edit.
#![allow(non_upper_case_globals, unused_macros, unused_imports)]
use crate::low::macros::*;

// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

// ----------------------------------------------------------------------------
// Point addition on NIST curve P-384 in Montgomery-Jacobian coordinates
//
//    extern void p384_montjadd_alt(uint64_t p3[static 18],
//                                  const uint64_t p1[static 18],
//                                  const uint64_t p2[static 18]);
//
// Does p3 := p1 + p2 where all points are regarded as Jacobian triples with
// each coordinate in the Montgomery domain, i.e. x' = (2^384 * x) mod p_384.
// A Jacobian triple (x',y',z') represents affine point (x/z^2,y/z^3).
//
// Standard ARM ABI: X0 = p3, X1 = p1, X2 = p2
// ----------------------------------------------------------------------------

// Size of individual field elements

macro_rules! NUMSIZE {
    () => {
        "48"
    };
}

// Stable homes for input arguments during main code sequence

macro_rules! input_z {
    () => {
        "x24"
    };
}
macro_rules! input_x {
    () => {
        "x25"
    };
}
macro_rules! input_y {
    () => {
        "x26"
    };
}

// Pointer-offset pairs for inputs and outputs

macro_rules! x_1 { () => { Q!(input_x!() ", #0") } }
macro_rules! y_1 { () => { Q!(input_x!() ", # " NUMSIZE!()) } }
macro_rules! z_1 { () => { Q!(input_x!() ", # (2 * " NUMSIZE!() ")") } }

macro_rules! x_2 { () => { Q!(input_y!() ", #0") } }
macro_rules! y_2 { () => { Q!(input_y!() ", # " NUMSIZE!()) } }
macro_rules! z_2 { () => { Q!(input_y!() ", # (2 * " NUMSIZE!() ")") } }

macro_rules! x_3 { () => { Q!(input_z!() ", #0") } }
macro_rules! y_3 { () => { Q!(input_z!() ", # " NUMSIZE!()) } }
macro_rules! z_3 { () => { Q!(input_z!() ", # (2 * " NUMSIZE!() ")") } }

// Pointer-offset pairs for temporaries, with some aliasing
// #NSPACE is the total stack needed for these temporaries

macro_rules! z1sq { () => { Q!("sp, # (" NUMSIZE!() "* 0)") } }
macro_rules! ww { () => { Q!("sp, # (" NUMSIZE!() "* 0)") } }
macro_rules! resx { () => { Q!("sp, # (" NUMSIZE!() "* 0)") } }

macro_rules! yd { () => { Q!("sp, # (" NUMSIZE!() "* 1)") } }
macro_rules! y2a { () => { Q!("sp, # (" NUMSIZE!() "* 1)") } }

macro_rules! x2a { () => { Q!("sp, # (" NUMSIZE!() "* 2)") } }
macro_rules! zzx2 { () => { Q!("sp, # (" NUMSIZE!() "* 2)") } }

macro_rules! zz { () => { Q!("sp, # (" NUMSIZE!() "* 3)") } }
macro_rules! t1 { () => { Q!("sp, # (" NUMSIZE!() "* 3)") } }

macro_rules! t2 { () => { Q!("sp, # (" NUMSIZE!() "* 4)") } }
macro_rules! x1a { () => { Q!("sp, # (" NUMSIZE!() "* 4)") } }
macro_rules! zzx1 { () => { Q!("sp, # (" NUMSIZE!() "* 4)") } }
macro_rules! resy { () => { Q!("sp, # (" NUMSIZE!() "* 4)") } }

macro_rules! xd { () => { Q!("sp, # (" NUMSIZE!() "* 5)") } }
macro_rules! z2sq { () => { Q!("sp, # (" NUMSIZE!() "* 5)") } }
macro_rules! resz { () => { Q!("sp, # (" NUMSIZE!() "* 5)") } }

macro_rules! y1a { () => { Q!("sp, # (" NUMSIZE!() "* 6)") } }

macro_rules! NSPACE { () => { Q!(NUMSIZE!() "* 7") } }

// Corresponds exactly to bignum_montmul_p384_alt

macro_rules! montmul_p384 {
    ($P0:expr, $P1:expr, $P2:expr) => { Q!(
        "ldp x3, x4, [" $P1 "];\n"
        "ldp x5, x6, [" $P2 "];\n"
        "mul x12, x3, x5;\n"
        "umulh x13, x3, x5;\n"
        "mul x11, x3, x6;\n"
        "umulh x14, x3, x6;\n"
        "adds x13, x13, x11;\n"
        "ldp x7, x8, [" $P2 "+ 16];\n"
        "mul x11, x3, x7;\n"
        "umulh x15, x3, x7;\n"
        "adcs x14, x14, x11;\n"
        "mul x11, x3, x8;\n"
        "umulh x16, x3, x8;\n"
        "adcs x15, x15, x11;\n"
        "ldp x9, x10, [" $P2 "+ 32];\n"
        "mul x11, x3, x9;\n"
        "umulh x17, x3, x9;\n"
        "adcs x16, x16, x11;\n"
        "mul x11, x3, x10;\n"
        "umulh x19, x3, x10;\n"
        "adcs x17, x17, x11;\n"
        "adc x19, x19, xzr;\n"
        "mul x11, x4, x5;\n"
        "adds x13, x13, x11;\n"
        "mul x11, x4, x6;\n"
        "adcs x14, x14, x11;\n"
        "mul x11, x4, x7;\n"
        "adcs x15, x15, x11;\n"
        "mul x11, x4, x8;\n"
        "adcs x16, x16, x11;\n"
        "mul x11, x4, x9;\n"
        "adcs x17, x17, x11;\n"
        "mul x11, x4, x10;\n"
        "adcs x19, x19, x11;\n"
        "cset x20, cs;\n"
        "umulh x11, x4, x5;\n"
        "adds x14, x14, x11;\n"
        "umulh x11, x4, x6;\n"
        "adcs x15, x15, x11;\n"
        "umulh x11, x4, x7;\n"
        "adcs x16, x16, x11;\n"
        "umulh x11, x4, x8;\n"
        "adcs x17, x17, x11;\n"
        "umulh x11, x4, x9;\n"
        "adcs x19, x19, x11;\n"
        "umulh x11, x4, x10;\n"
        "adc x20, x20, x11;\n"
        "ldp x3, x4, [" $P1 "+ 16];\n"
        "mul x11, x3, x5;\n"
        "adds x14, x14, x11;\n"
        "mul x11, x3, x6;\n"
        "adcs x15, x15, x11;\n"
        "mul x11, x3, x7;\n"
        "adcs x16, x16, x11;\n"
        "mul x11, x3, x8;\n"
        "adcs x17, x17, x11;\n"
        "mul x11, x3, x9;\n"
        "adcs x19, x19, x11;\n"
        "mul x11, x3, x10;\n"
        "adcs x20, x20, x11;\n"
        "cset x21, cs;\n"
        "umulh x11, x3, x5;\n"
        "adds x15, x15, x11;\n"
        "umulh x11, x3, x6;\n"
        "adcs x16, x16, x11;\n"
        "umulh x11, x3, x7;\n"
        "adcs x17, x17, x11;\n"
        "umulh x11, x3, x8;\n"
        "adcs x19, x19, x11;\n"
        "umulh x11, x3, x9;\n"
        "adcs x20, x20, x11;\n"
        "umulh x11, x3, x10;\n"
        "adc x21, x21, x11;\n"
        "mul x11, x4, x5;\n"
        "adds x15, x15, x11;\n"
        "mul x11, x4, x6;\n"
        "adcs x16, x16, x11;\n"
        "mul x11, x4, x7;\n"
        "adcs x17, x17, x11;\n"
        "mul x11, x4, x8;\n"
        "adcs x19, x19, x11;\n"
        "mul x11, x4, x9;\n"
        "adcs x20, x20, x11;\n"
        "mul x11, x4, x10;\n"
        "adcs x21, x21, x11;\n"
        "cset x22, cs;\n"
        "umulh x11, x4, x5;\n"
        "adds x16, x16, x11;\n"
        "umulh x11, x4, x6;\n"
        "adcs x17, x17, x11;\n"
        "umulh x11, x4, x7;\n"
        "adcs x19, x19, x11;\n"
        "umulh x11, x4, x8;\n"
        "adcs x20, x20, x11;\n"
        "umulh x11, x4, x9;\n"
        "adcs x21, x21, x11;\n"
        "umulh x11, x4, x10;\n"
        "adc x22, x22, x11;\n"
        "ldp x3, x4, [" $P1 "+ 32];\n"
        "mul x11, x3, x5;\n"
        "adds x16, x16, x11;\n"
        "mul x11, x3, x6;\n"
        "adcs x17, x17, x11;\n"
        "mul x11, x3, x7;\n"
        "adcs x19, x19, x11;\n"
        "mul x11, x3, x8;\n"
        "adcs x20, x20, x11;\n"
        "mul x11, x3, x9;\n"
        "adcs x21, x21, x11;\n"
        "mul x11, x3, x10;\n"
        "adcs x22, x22, x11;\n"
        "cset x2, cs;\n"
        "umulh x11, x3, x5;\n"
        "adds x17, x17, x11;\n"
        "umulh x11, x3, x6;\n"
        "adcs x19, x19, x11;\n"
        "umulh x11, x3, x7;\n"
        "adcs x20, x20, x11;\n"
        "umulh x11, x3, x8;\n"
        "adcs x21, x21, x11;\n"
        "umulh x11, x3, x9;\n"
        "adcs x22, x22, x11;\n"
        "umulh x11, x3, x10;\n"
        "adc x2, x2, x11;\n"
        "mul x11, x4, x5;\n"
        "adds x17, x17, x11;\n"
        "mul x11, x4, x6;\n"
        "adcs x19, x19, x11;\n"
        "mul x11, x4, x7;\n"
        "adcs x20, x20, x11;\n"
        "mul x11, x4, x8;\n"
        "adcs x21, x21, x11;\n"
        "mul x11, x4, x9;\n"
        "adcs x22, x22, x11;\n"
        "mul x11, x4, x10;\n"
        "adcs x2, x2, x11;\n"
        "cset x1, cs;\n"
        "umulh x11, x4, x5;\n"
        "adds x19, x19, x11;\n"
        "umulh x11, x4, x6;\n"
        "adcs x20, x20, x11;\n"
        "umulh x11, x4, x7;\n"
        "adcs x21, x21, x11;\n"
        "umulh x11, x4, x8;\n"
        "adcs x22, x22, x11;\n"
        "umulh x11, x4, x9;\n"
        "adcs x2, x2, x11;\n"
        "umulh x11, x4, x10;\n"
        "adc x1, x1, x11;\n"
        "lsl x7, x12, #32;\n"
        "add x12, x7, x12;\n"
        "mov x7, #0xffffffff00000001;\n"
        "umulh x7, x7, x12;\n"
        "mov x6, #0xffffffff;\n"
        "mul x5, x6, x12;\n"
        "umulh x6, x6, x12;\n"
        "adds x7, x7, x5;\n"
        "adcs x6, x6, x12;\n"
        "adc x5, xzr, xzr;\n"
        "subs x13, x13, x7;\n"
        "sbcs x14, x14, x6;\n"
        "sbcs x15, x15, x5;\n"
        "sbcs x16, x16, xzr;\n"
        "sbcs x17, x17, xzr;\n"
        "sbc x12, x12, xzr;\n"
        "lsl x7, x13, #32;\n"
        "add x13, x7, x13;\n"
        "mov x7, #0xffffffff00000001;\n"
        "umulh x7, x7, x13;\n"
        "mov x6, #0xffffffff;\n"
        "mul x5, x6, x13;\n"
        "umulh x6, x6, x13;\n"
        "adds x7, x7, x5;\n"
        "adcs x6, x6, x13;\n"
        "adc x5, xzr, xzr;\n"
        "subs x14, x14, x7;\n"
        "sbcs x15, x15, x6;\n"
        "sbcs x16, x16, x5;\n"
        "sbcs x17, x17, xzr;\n"
        "sbcs x12, x12, xzr;\n"
        "sbc x13, x13, xzr;\n"
        "lsl x7, x14, #32;\n"
        "add x14, x7, x14;\n"
        "mov x7, #0xffffffff00000001;\n"
        "umulh x7, x7, x14;\n"
        "mov x6, #0xffffffff;\n"
        "mul x5, x6, x14;\n"
        "umulh x6, x6, x14;\n"
        "adds x7, x7, x5;\n"
        "adcs x6, x6, x14;\n"
        "adc x5, xzr, xzr;\n"
        "subs x15, x15, x7;\n"
        "sbcs x16, x16, x6;\n"
        "sbcs x17, x17, x5;\n"
        "sbcs x12, x12, xzr;\n"
        "sbcs x13, x13, xzr;\n"
        "sbc x14, x14, xzr;\n"
        "lsl x7, x15, #32;\n"
        "add x15, x7, x15;\n"
        "mov x7, #0xffffffff00000001;\n"
        "umulh x7, x7, x15;\n"
        "mov x6, #0xffffffff;\n"
        "mul x5, x6, x15;\n"
        "umulh x6, x6, x15;\n"
        "adds x7, x7, x5;\n"
        "adcs x6, x6, x15;\n"
        "adc x5, xzr, xzr;\n"
        "subs x16, x16, x7;\n"
        "sbcs x17, x17, x6;\n"
        "sbcs x12, x12, x5;\n"
        "sbcs x13, x13, xzr;\n"
        "sbcs x14, x14, xzr;\n"
        "sbc x15, x15, xzr;\n"
        "lsl x7, x16, #32;\n"
        "add x16, x7, x16;\n"
        "mov x7, #0xffffffff00000001;\n"
        "umulh x7, x7, x16;\n"
        "mov x6, #0xffffffff;\n"
        "mul x5, x6, x16;\n"
        "umulh x6, x6, x16;\n"
        "adds x7, x7, x5;\n"
        "adcs x6, x6, x16;\n"
        "adc x5, xzr, xzr;\n"
        "subs x17, x17, x7;\n"
        "sbcs x12, x12, x6;\n"
        "sbcs x13, x13, x5;\n"
        "sbcs x14, x14, xzr;\n"
        "sbcs x15, x15, xzr;\n"
        "sbc x16, x16, xzr;\n"
        "lsl x7, x17, #32;\n"
        "add x17, x7, x17;\n"
        "mov x7, #0xffffffff00000001;\n"
        "umulh x7, x7, x17;\n"
        "mov x6, #0xffffffff;\n"
        "mul x5, x6, x17;\n"
        "umulh x6, x6, x17;\n"
        "adds x7, x7, x5;\n"
        "adcs x6, x6, x17;\n"
        "adc x5, xzr, xzr;\n"
        "subs x12, x12, x7;\n"
        "sbcs x13, x13, x6;\n"
        "sbcs x14, x14, x5;\n"
        "sbcs x15, x15, xzr;\n"
        "sbcs x16, x16, xzr;\n"
        "sbc x17, x17, xzr;\n"
        "adds x12, x12, x19;\n"
        "adcs x13, x13, x20;\n"
        "adcs x14, x14, x21;\n"
        "adcs x15, x15, x22;\n"
        "adcs x16, x16, x2;\n"
        "adcs x17, x17, x1;\n"
        "adc x10, xzr, xzr;\n"
        "mov x11, #0xffffffff00000001;\n"
        "adds x19, x12, x11;\n"
        "mov x11, #0xffffffff;\n"
        "adcs x20, x13, x11;\n"
        "mov x11, #0x1;\n"
        "adcs x21, x14, x11;\n"
        "adcs x22, x15, xzr;\n"
        "adcs x2, x16, xzr;\n"
        "adcs x1, x17, xzr;\n"
        "adcs x10, x10, xzr;\n"
        "csel x12, x12, x19, eq;\n"
        "csel x13, x13, x20, eq;\n"
        "csel x14, x14, x21, eq;\n"
        "csel x15, x15, x22, eq;\n"
        "csel x16, x16, x2, eq;\n"
        "csel x17, x17, x1, eq;\n"
        "stp x12, x13, [" $P0 "];\n"
        "stp x14, x15, [" $P0 "+ 16];\n"
        "stp x16, x17, [" $P0 "+ 32]"
    )}
}

// Corresponds exactly to bignum_montsqr_p384_alt

macro_rules! montsqr_p384 {
    ($P0:expr, $P1:expr) => { Q!(
        "ldp x2, x3, [" $P1 "];\n"
        "mul x9, x2, x3;\n"
        "umulh x10, x2, x3;\n"
        "ldp x4, x5, [" $P1 "+ 16];\n"
        "mul x8, x2, x4;\n"
        "adds x10, x10, x8;\n"
        "mul x11, x2, x5;\n"
        "mul x8, x3, x4;\n"
        "adcs x11, x11, x8;\n"
        "umulh x12, x2, x5;\n"
        "mul x8, x3, x5;\n"
        "adcs x12, x12, x8;\n"
        "ldp x6, x7, [" $P1 "+ 32];\n"
        "mul x13, x2, x7;\n"
        "mul x8, x3, x6;\n"
        "adcs x13, x13, x8;\n"
        "umulh x14, x2, x7;\n"
        "mul x8, x3, x7;\n"
        "adcs x14, x14, x8;\n"
        "mul x15, x5, x6;\n"
        "adcs x15, x15, xzr;\n"
        "umulh x16, x5, x6;\n"
        "adc x16, x16, xzr;\n"
        "umulh x8, x2, x4;\n"
        "adds x11, x11, x8;\n"
        "umulh x8, x3, x4;\n"
        "adcs x12, x12, x8;\n"
        "umulh x8, x3, x5;\n"
        "adcs x13, x13, x8;\n"
        "umulh x8, x3, x6;\n"
        "adcs x14, x14, x8;\n"
        "umulh x8, x3, x7;\n"
        "adcs x15, x15, x8;\n"
        "adc x16, x16, xzr;\n"
        "mul x8, x2, x6;\n"
        "adds x12, x12, x8;\n"
        "mul x8, x4, x5;\n"
        "adcs x13, x13, x8;\n"
        "mul x8, x4, x6;\n"
        "adcs x14, x14, x8;\n"
        "mul x8, x4, x7;\n"
        "adcs x15, x15, x8;\n"
        "mul x8, x5, x7;\n"
        "adcs x16, x16, x8;\n"
        "mul x17, x6, x7;\n"
        "adcs x17, x17, xzr;\n"
        "umulh x19, x6, x7;\n"
        "adc x19, x19, xzr;\n"
        "umulh x8, x2, x6;\n"
        "adds x13, x13, x8;\n"
        "umulh x8, x4, x5;\n"
        "adcs x14, x14, x8;\n"
        "umulh x8, x4, x6;\n"
        "adcs x15, x15, x8;\n"
        "umulh x8, x4, x7;\n"
        "adcs x16, x16, x8;\n"
        "umulh x8, x5, x7;\n"
        "adcs x17, x17, x8;\n"
        "adc x19, x19, xzr;\n"
        "adds x9, x9, x9;\n"
        "adcs x10, x10, x10;\n"
        "adcs x11, x11, x11;\n"
        "adcs x12, x12, x12;\n"
        "adcs x13, x13, x13;\n"
        "adcs x14, x14, x14;\n"
        "adcs x15, x15, x15;\n"
        "adcs x16, x16, x16;\n"
        "adcs x17, x17, x17;\n"
        "adcs x19, x19, x19;\n"
        "cset x20, hs;\n"
        "umulh x8, x2, x2;\n"
        "mul x2, x2, x2;\n"
        "adds x9, x9, x8;\n"
        "mul x8, x3, x3;\n"
        "adcs x10, x10, x8;\n"
        "umulh x8, x3, x3;\n"
        "adcs x11, x11, x8;\n"
        "mul x8, x4, x4;\n"
        "adcs x12, x12, x8;\n"
        "umulh x8, x4, x4;\n"
        "adcs x13, x13, x8;\n"
        "mul x8, x5, x5;\n"
        "adcs x14, x14, x8;\n"
        "umulh x8, x5, x5;\n"
        "adcs x15, x15, x8;\n"
        "mul x8, x6, x6;\n"
        "adcs x16, x16, x8;\n"
        "umulh x8, x6, x6;\n"
        "adcs x17, x17, x8;\n"
        "mul x8, x7, x7;\n"
        "adcs x19, x19, x8;\n"
        "umulh x8, x7, x7;\n"
        "adc x20, x20, x8;\n"
        "lsl x5, x2, #32;\n"
        "add x2, x5, x2;\n"
        "mov x5, #-4294967295;\n"
        "umulh x5, x5, x2;\n"
        "mov x4, #4294967295;\n"
        "mul x3, x4, x2;\n"
        "umulh x4, x4, x2;\n"
        "adds x5, x5, x3;\n"
        "adcs x4, x4, x2;\n"
        "adc x3, xzr, xzr;\n"
        "subs x9, x9, x5;\n"
        "sbcs x10, x10, x4;\n"
        "sbcs x11, x11, x3;\n"
        "sbcs x12, x12, xzr;\n"
        "sbcs x13, x13, xzr;\n"
        "sbc x2, x2, xzr;\n"
        "lsl x5, x9, #32;\n"
        "add x9, x5, x9;\n"
        "mov x5, #-4294967295;\n"
        "umulh x5, x5, x9;\n"
        "mov x4, #4294967295;\n"
        "mul x3, x4, x9;\n"
        "umulh x4, x4, x9;\n"
        "adds x5, x5, x3;\n"
        "adcs x4, x4, x9;\n"
        "adc x3, xzr, xzr;\n"
        "subs x10, x10, x5;\n"
        "sbcs x11, x11, x4;\n"
        "sbcs x12, x12, x3;\n"
        "sbcs x13, x13, xzr;\n"
        "sbcs x2, x2, xzr;\n"
        "sbc x9, x9, xzr;\n"
        "lsl x5, x10, #32;\n"
        "add x10, x5, x10;\n"
        "mov x5, #-4294967295;\n"
        "umulh x5, x5, x10;\n"
        "mov x4, #4294967295;\n"
        "mul x3, x4, x10;\n"
        "umulh x4, x4, x10;\n"
        "adds x5, x5, x3;\n"
        "adcs x4, x4, x10;\n"
        "adc x3, xzr, xzr;\n"
        "subs x11, x11, x5;\n"
        "sbcs x12, x12, x4;\n"
        "sbcs x13, x13, x3;\n"
        "sbcs x2, x2, xzr;\n"
        "sbcs x9, x9, xzr;\n"
        "sbc x10, x10, xzr;\n"
        "lsl x5, x11, #32;\n"
        "add x11, x5, x11;\n"
        "mov x5, #-4294967295;\n"
        "umulh x5, x5, x11;\n"
        "mov x4, #4294967295;\n"
        "mul x3, x4, x11;\n"
        "umulh x4, x4, x11;\n"
        "adds x5, x5, x3;\n"
        "adcs x4, x4, x11;\n"
        "adc x3, xzr, xzr;\n"
        "subs x12, x12, x5;\n"
        "sbcs x13, x13, x4;\n"
        "sbcs x2, x2, x3;\n"
        "sbcs x9, x9, xzr;\n"
        "sbcs x10, x10, xzr;\n"
        "sbc x11, x11, xzr;\n"
        "lsl x5, x12, #32;\n"
        "add x12, x5, x12;\n"
        "mov x5, #-4294967295;\n"
        "umulh x5, x5, x12;\n"
        "mov x4, #4294967295;\n"
        "mul x3, x4, x12;\n"
        "umulh x4, x4, x12;\n"
        "adds x5, x5, x3;\n"
        "adcs x4, x4, x12;\n"
        "adc x3, xzr, xzr;\n"
        "subs x13, x13, x5;\n"
        "sbcs x2, x2, x4;\n"
        "sbcs x9, x9, x3;\n"
        "sbcs x10, x10, xzr;\n"
        "sbcs x11, x11, xzr;\n"
        "sbc x12, x12, xzr;\n"
        "lsl x5, x13, #32;\n"
        "add x13, x5, x13;\n"
        "mov x5, #-4294967295;\n"
        "umulh x5, x5, x13;\n"
        "mov x4, #4294967295;\n"
        "mul x3, x4, x13;\n"
        "umulh x4, x4, x13;\n"
        "adds x5, x5, x3;\n"
        "adcs x4, x4, x13;\n"
        "adc x3, xzr, xzr;\n"
        "subs x2, x2, x5;\n"
        "sbcs x9, x9, x4;\n"
        "sbcs x10, x10, x3;\n"
        "sbcs x11, x11, xzr;\n"
        "sbcs x12, x12, xzr;\n"
        "sbc x13, x13, xzr;\n"
        "adds x2, x2, x14;\n"
        "adcs x9, x9, x15;\n"
        "adcs x10, x10, x16;\n"
        "adcs x11, x11, x17;\n"
        "adcs x12, x12, x19;\n"
        "adcs x13, x13, x20;\n"
        "adc x6, xzr, xzr;\n"
        "mov x8, #-4294967295;\n"
        "adds x14, x2, x8;\n"
        "mov x8, #4294967295;\n"
        "adcs x15, x9, x8;\n"
        "mov x8, #1;\n"
        "adcs x16, x10, x8;\n"
        "adcs x17, x11, xzr;\n"
        "adcs x19, x12, xzr;\n"
        "adcs x20, x13, xzr;\n"
        "adcs x6, x6, xzr;\n"
        "csel x2, x2, x14, eq;\n"
        "csel x9, x9, x15, eq;\n"
        "csel x10, x10, x16, eq;\n"
        "csel x11, x11, x17, eq;\n"
        "csel x12, x12, x19, eq;\n"
        "csel x13, x13, x20, eq;\n"
        "stp x2, x9, [" $P0 "];\n"
        "stp x10, x11, [" $P0 "+ 16];\n"
        "stp x12, x13, [" $P0 "+ 32]"
    )}
}

// Almost-Montgomery variant which we use when an input to other muls
// with the other argument fully reduced (which is always safe). In
// fact, with the Karatsuba-based Montgomery mul here, we don't even
// *need* the restriction that the other argument is reduced.

macro_rules! amontsqr_p384 {
    ($P0:expr, $P1:expr) => { Q!(
        "ldp x2, x3, [" $P1 "];\n"
        "mul x9, x2, x3;\n"
        "umulh x10, x2, x3;\n"
        "ldp x4, x5, [" $P1 "+ 16];\n"
        "mul x8, x2, x4;\n"
        "adds x10, x10, x8;\n"
        "mul x11, x2, x5;\n"
        "mul x8, x3, x4;\n"
        "adcs x11, x11, x8;\n"
        "umulh x12, x2, x5;\n"
        "mul x8, x3, x5;\n"
        "adcs x12, x12, x8;\n"
        "ldp x6, x7, [" $P1 "+ 32];\n"
        "mul x13, x2, x7;\n"
        "mul x8, x3, x6;\n"
        "adcs x13, x13, x8;\n"
        "umulh x14, x2, x7;\n"
        "mul x8, x3, x7;\n"
        "adcs x14, x14, x8;\n"
        "mul x15, x5, x6;\n"
        "adcs x15, x15, xzr;\n"
        "umulh x16, x5, x6;\n"
        "adc x16, x16, xzr;\n"
        "umulh x8, x2, x4;\n"
        "adds x11, x11, x8;\n"
        "umulh x8, x3, x4;\n"
        "adcs x12, x12, x8;\n"
        "umulh x8, x3, x5;\n"
        "adcs x13, x13, x8;\n"
        "umulh x8, x3, x6;\n"
        "adcs x14, x14, x8;\n"
        "umulh x8, x3, x7;\n"
        "adcs x15, x15, x8;\n"
        "adc x16, x16, xzr;\n"
        "mul x8, x2, x6;\n"
        "adds x12, x12, x8;\n"
        "mul x8, x4, x5;\n"
        "adcs x13, x13, x8;\n"
        "mul x8, x4, x6;\n"
        "adcs x14, x14, x8;\n"
        "mul x8, x4, x7;\n"
        "adcs x15, x15, x8;\n"
        "mul x8, x5, x7;\n"
        "adcs x16, x16, x8;\n"
        "mul x17, x6, x7;\n"
        "adcs x17, x17, xzr;\n"
        "umulh x19, x6, x7;\n"
        "adc x19, x19, xzr;\n"
        "umulh x8, x2, x6;\n"
        "adds x13, x13, x8;\n"
        "umulh x8, x4, x5;\n"
        "adcs x14, x14, x8;\n"
        "umulh x8, x4, x6;\n"
        "adcs x15, x15, x8;\n"
        "umulh x8, x4, x7;\n"
        "adcs x16, x16, x8;\n"
        "umulh x8, x5, x7;\n"
        "adcs x17, x17, x8;\n"
        "adc x19, x19, xzr;\n"
        "adds x9, x9, x9;\n"
        "adcs x10, x10, x10;\n"
        "adcs x11, x11, x11;\n"
        "adcs x12, x12, x12;\n"
        "adcs x13, x13, x13;\n"
        "adcs x14, x14, x14;\n"
        "adcs x15, x15, x15;\n"
        "adcs x16, x16, x16;\n"
        "adcs x17, x17, x17;\n"
        "adcs x19, x19, x19;\n"
        "cset x20, hs;\n"
        "umulh x8, x2, x2;\n"
        "mul x2, x2, x2;\n"
        "adds x9, x9, x8;\n"
        "mul x8, x3, x3;\n"
        "adcs x10, x10, x8;\n"
        "umulh x8, x3, x3;\n"
        "adcs x11, x11, x8;\n"
        "mul x8, x4, x4;\n"
        "adcs x12, x12, x8;\n"
        "umulh x8, x4, x4;\n"
        "adcs x13, x13, x8;\n"
        "mul x8, x5, x5;\n"
        "adcs x14, x14, x8;\n"
        "umulh x8, x5, x5;\n"
        "adcs x15, x15, x8;\n"
        "mul x8, x6, x6;\n"
        "adcs x16, x16, x8;\n"
        "umulh x8, x6, x6;\n"
        "adcs x17, x17, x8;\n"
        "mul x8, x7, x7;\n"
        "adcs x19, x19, x8;\n"
        "umulh x8, x7, x7;\n"
        "adc x20, x20, x8;\n"
        "lsl x5, x2, #32;\n"
        "add x2, x5, x2;\n"
        "mov x5, #-4294967295;\n"
        "umulh x5, x5, x2;\n"
        "mov x4, #4294967295;\n"
        "mul x3, x4, x2;\n"
        "umulh x4, x4, x2;\n"
        "adds x5, x5, x3;\n"
        "adcs x4, x4, x2;\n"
        "adc x3, xzr, xzr;\n"
        "subs x9, x9, x5;\n"
        "sbcs x10, x10, x4;\n"
        "sbcs x11, x11, x3;\n"
        "sbcs x12, x12, xzr;\n"
        "sbcs x13, x13, xzr;\n"
        "sbc x2, x2, xzr;\n"
        "lsl x5, x9, #32;\n"
        "add x9, x5, x9;\n"
        "mov x5, #-4294967295;\n"
        "umulh x5, x5, x9;\n"
        "mov x4, #4294967295;\n"
        "mul x3, x4, x9;\n"
        "umulh x4, x4, x9;\n"
        "adds x5, x5, x3;\n"
        "adcs x4, x4, x9;\n"
        "adc x3, xzr, xzr;\n"
        "subs x10, x10, x5;\n"
        "sbcs x11, x11, x4;\n"
        "sbcs x12, x12, x3;\n"
        "sbcs x13, x13, xzr;\n"
        "sbcs x2, x2, xzr;\n"
        "sbc x9, x9, xzr;\n"
        "lsl x5, x10, #32;\n"
        "add x10, x5, x10;\n"
        "mov x5, #-4294967295;\n"
        "umulh x5, x5, x10;\n"
        "mov x4, #4294967295;\n"
        "mul x3, x4, x10;\n"
        "umulh x4, x4, x10;\n"
        "adds x5, x5, x3;\n"
        "adcs x4, x4, x10;\n"
        "adc x3, xzr, xzr;\n"
        "subs x11, x11, x5;\n"
        "sbcs x12, x12, x4;\n"
        "sbcs x13, x13, x3;\n"
        "sbcs x2, x2, xzr;\n"
        "sbcs x9, x9, xzr;\n"
        "sbc x10, x10, xzr;\n"
        "lsl x5, x11, #32;\n"
        "add x11, x5, x11;\n"
        "mov x5, #-4294967295;\n"
        "umulh x5, x5, x11;\n"
        "mov x4, #4294967295;\n"
        "mul x3, x4, x11;\n"
        "umulh x4, x4, x11;\n"
        "adds x5, x5, x3;\n"
        "adcs x4, x4, x11;\n"
        "adc x3, xzr, xzr;\n"
        "subs x12, x12, x5;\n"
        "sbcs x13, x13, x4;\n"
        "sbcs x2, x2, x3;\n"
        "sbcs x9, x9, xzr;\n"
        "sbcs x10, x10, xzr;\n"
        "sbc x11, x11, xzr;\n"
        "lsl x5, x12, #32;\n"
        "add x12, x5, x12;\n"
        "mov x5, #-4294967295;\n"
        "umulh x5, x5, x12;\n"
        "mov x4, #4294967295;\n"
        "mul x3, x4, x12;\n"
        "umulh x4, x4, x12;\n"
        "adds x5, x5, x3;\n"
        "adcs x4, x4, x12;\n"
        "adc x3, xzr, xzr;\n"
        "subs x13, x13, x5;\n"
        "sbcs x2, x2, x4;\n"
        "sbcs x9, x9, x3;\n"
        "sbcs x10, x10, xzr;\n"
        "sbcs x11, x11, xzr;\n"
        "sbc x12, x12, xzr;\n"
        "lsl x5, x13, #32;\n"
        "add x13, x5, x13;\n"
        "mov x5, #-4294967295;\n"
        "umulh x5, x5, x13;\n"
        "mov x4, #4294967295;\n"
        "mul x3, x4, x13;\n"
        "umulh x4, x4, x13;\n"
        "adds x5, x5, x3;\n"
        "adcs x4, x4, x13;\n"
        "adc x3, xzr, xzr;\n"
        "subs x2, x2, x5;\n"
        "sbcs x9, x9, x4;\n"
        "sbcs x10, x10, x3;\n"
        "sbcs x11, x11, xzr;\n"
        "sbcs x12, x12, xzr;\n"
        "sbc x13, x13, xzr;\n"
        "adds x2, x2, x14;\n"
        "adcs x9, x9, x15;\n"
        "adcs x10, x10, x16;\n"
        "adcs x11, x11, x17;\n"
        "adcs x12, x12, x19;\n"
        "adcs x13, x13, x20;\n"
        "mov x14, #-4294967295;\n"
        "mov x15, #4294967295;\n"
        "csel x14, x14, xzr, cs;\n"
        "csel x15, x15, xzr, cs;\n"
        "cset x16, cs;\n"
        "adds x2, x2, x14;\n"
        "adcs x9, x9, x15;\n"
        "adcs x10, x10, x16;\n"
        "adcs x11, x11, xzr;\n"
        "adcs x12, x12, xzr;\n"
        "adc x13, x13, xzr;\n"
        "stp x2, x9, [" $P0 "];\n"
        "stp x10, x11, [" $P0 "+ 16];\n"
        "stp x12, x13, [" $P0 "+ 32]"
    )}
}

// Corresponds exactly to bignum_sub_p384

macro_rules! sub_p384 {
    ($P0:expr, $P1:expr, $P2:expr) => { Q!(
        "ldp x5, x6, [" $P1 "];\n"
        "ldp x4, x3, [" $P2 "];\n"
        "subs x5, x5, x4;\n"
        "sbcs x6, x6, x3;\n"
        "ldp x7, x8, [" $P1 "+ 16];\n"
        "ldp x4, x3, [" $P2 "+ 16];\n"
        "sbcs x7, x7, x4;\n"
        "sbcs x8, x8, x3;\n"
        "ldp x9, x10, [" $P1 "+ 32];\n"
        "ldp x4, x3, [" $P2 "+ 32];\n"
        "sbcs x9, x9, x4;\n"
        "sbcs x10, x10, x3;\n"
        "csetm x3, lo;\n"
        "mov x4, #4294967295;\n"
        "and x4, x4, x3;\n"
        "adds x5, x5, x4;\n"
        "eor x4, x4, x3;\n"
        "adcs x6, x6, x4;\n"
        "mov x4, #-2;\n"
        "and x4, x4, x3;\n"
        "adcs x7, x7, x4;\n"
        "adcs x8, x8, x3;\n"
        "adcs x9, x9, x3;\n"
        "adc x10, x10, x3;\n"
        "stp x5, x6, [" $P0 "];\n"
        "stp x7, x8, [" $P0 "+ 16];\n"
        "stp x9, x10, [" $P0 "+ 32]"
    )}
}

/// Point addition on NIST curve P-384 in Montgomery-Jacobian coordinates
///
///
/// Does p3 := p1 + p2 where all points are regarded as Jacobian triples with
/// each coordinate in the Montgomery domain, i.e. x' = (2^384 * x) mod p_384.
/// A Jacobian triple (x',y',z') represents affine point (x/z^2,y/z^3).
pub(crate) fn p384_montjadd(p3: &mut [u64; 18], p1: &[u64; 18], p2: &[u64; 18]) {
    // SAFETY: inline assembly. see [crate::low::inline_assembly_safety] for safety info.
    unsafe {
        core::arch::asm!(

        // Save regs and make room on stack for temporary variables

        Q!("    stp             " "x19, x20, [sp, #-16] !"),
        Q!("    stp             " "x21, x22, [sp, #-16] !"),
        Q!("    stp             " "x23, x24, [sp, #-16] !"),
        Q!("    stp             " "x25, x26, [sp, #-16] !"),
        Q!("    sub             " "sp, sp, # (" NSPACE!() "+ 0)"),

        // Move the input arguments to stable places

        Q!("    mov             " input_z!() ", x0"),
        Q!("    mov             " input_x!() ", x1"),
        Q!("    mov             " input_y!() ", x2"),

        // Main code, just a sequence of basic field operations
        // 8 * multiply + 3 * square + 7 * subtract

        amontsqr_p384!(z1sq!(), z_1!()),
        amontsqr_p384!(z2sq!(), z_2!()),

        montmul_p384!(y1a!(), z_2!(), y_1!()),
        montmul_p384!(y2a!(), z_1!(), y_2!()),

        montmul_p384!(x2a!(), z1sq!(), x_2!()),
        montmul_p384!(x1a!(), z2sq!(), x_1!()),
        montmul_p384!(y2a!(), z1sq!(), y2a!()),
        montmul_p384!(y1a!(), z2sq!(), y1a!()),

        sub_p384!(xd!(), x2a!(), x1a!()),
        sub_p384!(yd!(), y2a!(), y1a!()),

        amontsqr_p384!(zz!(), xd!()),
        montsqr_p384!(ww!(), yd!()),

        montmul_p384!(zzx1!(), zz!(), x1a!()),
        montmul_p384!(zzx2!(), zz!(), x2a!()),

        sub_p384!(resx!(), ww!(), zzx1!()),
        sub_p384!(t1!(), zzx2!(), zzx1!()),

        montmul_p384!(xd!(), xd!(), z_1!()),

        sub_p384!(resx!(), resx!(), zzx2!()),

        sub_p384!(t2!(), zzx1!(), resx!()),

        montmul_p384!(t1!(), t1!(), y1a!()),
        montmul_p384!(resz!(), xd!(), z_2!()),
        montmul_p384!(t2!(), yd!(), t2!()),

        sub_p384!(resy!(), t2!(), t1!()),

        // Load in the z coordinates of the inputs to check for P1 = 0 and P2 = 0
        // The condition codes get set by a comparison (P2 != 0) - (P1 != 0)
        // So  "HI" <=> CF /\ ~ZF <=> P1 = 0 /\ ~(P2 = 0)
        // and "LO" <=> ~CF       <=> ~(P1 = 0) /\ P2 = 0

        Q!("    ldp             " "x0, x1, [" z_1!() "]"),
        Q!("    ldp             " "x2, x3, [" z_1!() "+ 16]"),
        Q!("    ldp             " "x4, x5, [" z_1!() "+ 32]"),

        Q!("    orr             " "x20, x0, x1"),
        Q!("    orr             " "x21, x2, x3"),
        Q!("    orr             " "x22, x4, x5"),
        Q!("    orr             " "x20, x20, x21"),
        Q!("    orr             " "x20, x20, x22"),
        Q!("    cmp             " "x20, xzr"),
        Q!("    cset            " "x20, ne"),

        Q!("    ldp             " "x6, x7, [" z_2!() "]"),
        Q!("    ldp             " "x8, x9, [" z_2!() "+ 16]"),
        Q!("    ldp             " "x10, x11, [" z_2!() "+ 32]"),

        Q!("    orr             " "x21, x6, x7"),
        Q!("    orr             " "x22, x8, x9"),
        Q!("    orr             " "x23, x10, x11"),
        Q!("    orr             " "x21, x21, x22"),
        Q!("    orr             " "x21, x21, x23"),
        Q!("    cmp             " "x21, xzr"),
        Q!("    cset            " "x21, ne"),

        Q!("    cmp             " "x21, x20"),

        // Multiplex the outputs accordingly, re-using the z's in registers

        Q!("    ldp             " "x12, x13, [" resz!() "]"),
        Q!("    csel            " "x12, x0, x12, lo"),
        Q!("    csel            " "x13, x1, x13, lo"),
        Q!("    csel            " "x12, x6, x12, hi"),
        Q!("    csel            " "x13, x7, x13, hi"),
        Q!("    ldp             " "x14, x15, [" resz!() "+ 16]"),
        Q!("    csel            " "x14, x2, x14, lo"),
        Q!("    csel            " "x15, x3, x15, lo"),
        Q!("    csel            " "x14, x8, x14, hi"),
        Q!("    csel            " "x15, x9, x15, hi"),
        Q!("    ldp             " "x16, x17, [" resz!() "+ 32]"),
        Q!("    csel            " "x16, x4, x16, lo"),
        Q!("    csel            " "x17, x5, x17, lo"),
        Q!("    csel            " "x16, x10, x16, hi"),
        Q!("    csel            " "x17, x11, x17, hi"),

        Q!("    ldp             " "x20, x21, [" x_1!() "]"),
        Q!("    ldp             " "x0, x1, [" resx!() "]"),
        Q!("    csel            " "x0, x20, x0, lo"),
        Q!("    csel            " "x1, x21, x1, lo"),
        Q!("    ldp             " "x20, x21, [" x_2!() "]"),
        Q!("    csel            " "x0, x20, x0, hi"),
        Q!("    csel            " "x1, x21, x1, hi"),

        Q!("    ldp             " "x20, x21, [" x_1!() "+ 16]"),
        Q!("    ldp             " "x2, x3, [" resx!() "+ 16]"),
        Q!("    csel            " "x2, x20, x2, lo"),
        Q!("    csel            " "x3, x21, x3, lo"),
        Q!("    ldp             " "x20, x21, [" x_2!() "+ 16]"),
        Q!("    csel            " "x2, x20, x2, hi"),
        Q!("    csel            " "x3, x21, x3, hi"),

        Q!("    ldp             " "x20, x21, [" x_1!() "+ 32]"),
        Q!("    ldp             " "x4, x5, [" resx!() "+ 32]"),
        Q!("    csel            " "x4, x20, x4, lo"),
        Q!("    csel            " "x5, x21, x5, lo"),
        Q!("    ldp             " "x20, x21, [" x_2!() "+ 32]"),
        Q!("    csel            " "x4, x20, x4, hi"),
        Q!("    csel            " "x5, x21, x5, hi"),

        Q!("    ldp             " "x20, x21, [" y_1!() "]"),
        Q!("    ldp             " "x6, x7, [" resy!() "]"),
        Q!("    csel            " "x6, x20, x6, lo"),
        Q!("    csel            " "x7, x21, x7, lo"),
        Q!("    ldp             " "x20, x21, [" y_2!() "]"),
        Q!("    csel            " "x6, x20, x6, hi"),
        Q!("    csel            " "x7, x21, x7, hi"),

        Q!("    ldp             " "x20, x21, [" y_1!() "+ 16]"),
        Q!("    ldp             " "x8, x9, [" resy!() "+ 16]"),
        Q!("    csel            " "x8, x20, x8, lo"),
        Q!("    csel            " "x9, x21, x9, lo"),
        Q!("    ldp             " "x20, x21, [" y_2!() "+ 16]"),
        Q!("    csel            " "x8, x20, x8, hi"),
        Q!("    csel            " "x9, x21, x9, hi"),

        Q!("    ldp             " "x20, x21, [" y_1!() "+ 32]"),
        Q!("    ldp             " "x10, x11, [" resy!() "+ 32]"),
        Q!("    csel            " "x10, x20, x10, lo"),
        Q!("    csel            " "x11, x21, x11, lo"),
        Q!("    ldp             " "x20, x21, [" y_2!() "+ 32]"),
        Q!("    csel            " "x10, x20, x10, hi"),
        Q!("    csel            " "x11, x21, x11, hi"),

        // Finally store back the multiplexed values

        Q!("    stp             " "x0, x1, [" x_3!() "]"),
        Q!("    stp             " "x2, x3, [" x_3!() "+ 16]"),
        Q!("    stp             " "x4, x5, [" x_3!() "+ 32]"),
        Q!("    stp             " "x6, x7, [" y_3!() "]"),
        Q!("    stp             " "x8, x9, [" y_3!() "+ 16]"),
        Q!("    stp             " "x10, x11, [" y_3!() "+ 32]"),
        Q!("    stp             " "x12, x13, [" z_3!() "]"),
        Q!("    stp             " "x14, x15, [" z_3!() "+ 16]"),
        Q!("    stp             " "x16, x17, [" z_3!() "+ 32]"),

        // Restore stack and registers

        Q!("    add             " "sp, sp, # (" NSPACE!() "+ 0)"),

        Q!("    ldp             " "x25, x26, [sp], #16"),
        Q!("    ldp             " "x23, x24, [sp], #16"),
        Q!("    ldp             " "x21, x22, [sp], #16"),
        Q!("    ldp             " "x19, x20, [sp], #16"),

        inout("x0") p3.as_mut_ptr() => _,
        inout("x1") p1.as_ptr() => _,
        inout("x2") p2.as_ptr() => _,
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
        out("x26") _,
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
