// generated source. do not edit.
#![allow(non_upper_case_globals, unused_macros, unused_imports)]
use crate::low::macros::*;

// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

// ----------------------------------------------------------------------------
// Point addition on NIST curve P-256 in Montgomery-Jacobian coordinates
//
//    extern void p256_montjadd_alt(uint64_t p3[static 12],
//                                  const uint64_t p1[static 12],
//                                  const uint64_t p2[static 12]);
//
// Does p3 := p1 + p2 where all points are regarded as Jacobian triples with
// each coordinate in the Montgomery domain, i.e. x' = (2^256 * x) mod p_256.
// A Jacobian triple (x',y',z') represents affine point (x/z^2,y/z^3).
//
// Standard ARM ABI: X0 = p3, X1 = p1, X2 = p2
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
macro_rules! input_y {
    () => {
        "x17"
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

// Corresponds to bignum_montmul_p256_alt except registers

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
        "cset x7, cs;\n"
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
        "adds x9, x9, x8, lsl #32;\n"
        "lsr x3, x8, #32;\n"
        "adcs x10, x10, x3;\n"
        "mov x3, #0xffffffff00000001;\n"
        "mul x2, x8, x3;\n"
        "umulh x8, x8, x3;\n"
        "adcs x11, x11, x2;\n"
        "adc x8, x8, xzr;\n"
        "adds x10, x10, x9, lsl #32;\n"
        "lsr x3, x9, #32;\n"
        "adcs x11, x11, x3;\n"
        "mov x3, #0xffffffff00000001;\n"
        "mul x2, x9, x3;\n"
        "umulh x9, x9, x3;\n"
        "adcs x8, x8, x2;\n"
        "adc x9, x9, xzr;\n"
        "adds x11, x11, x10, lsl #32;\n"
        "lsr x3, x10, #32;\n"
        "adcs x8, x8, x3;\n"
        "mov x3, #0xffffffff00000001;\n"
        "mul x2, x10, x3;\n"
        "umulh x10, x10, x3;\n"
        "adcs x9, x9, x2;\n"
        "adc x10, x10, xzr;\n"
        "adds x8, x8, x11, lsl #32;\n"
        "lsr x3, x11, #32;\n"
        "adcs x9, x9, x3;\n"
        "mov x3, #0xffffffff00000001;\n"
        "mul x2, x11, x3;\n"
        "umulh x11, x11, x3;\n"
        "adcs x10, x10, x2;\n"
        "adc x11, x11, xzr;\n"
        "adds x8, x8, x12;\n"
        "adcs x9, x9, x13;\n"
        "adcs x10, x10, x14;\n"
        "adcs x11, x11, x7;\n"
        "cset x2, cs;\n"
        "mov x3, #0xffffffff;\n"
        "mov x5, #0xffffffff00000001;\n"
        "adds x12, x8, #0x1;\n"
        "sbcs x13, x9, x3;\n"
        "sbcs x14, x10, xzr;\n"
        "sbcs x7, x11, x5;\n"
        "sbcs xzr, x2, xzr;\n"
        "csel x8, x8, x12, cc;\n"
        "csel x9, x9, x13, cc;\n"
        "csel x10, x10, x14, cc;\n"
        "csel x11, x11, x7, cc;\n"
        "stp x8, x9, [" $P0 "];\n"
        "stp x10, x11, [" $P0 "+ 16]"
    )}
}

// Almost-Montgomery variant which we use when an input to other muls
// with the other argument fully reduced (which is always safe).

macro_rules! amontsqr_p256 {
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
        "cset x7, cs;\n"
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
        "adds x9, x9, x8, lsl #32;\n"
        "lsr x3, x8, #32;\n"
        "adcs x10, x10, x3;\n"
        "mov x3, #0xffffffff00000001;\n"
        "mul x2, x8, x3;\n"
        "umulh x8, x8, x3;\n"
        "adcs x11, x11, x2;\n"
        "adc x8, x8, xzr;\n"
        "adds x10, x10, x9, lsl #32;\n"
        "lsr x3, x9, #32;\n"
        "adcs x11, x11, x3;\n"
        "mov x3, #0xffffffff00000001;\n"
        "mul x2, x9, x3;\n"
        "umulh x9, x9, x3;\n"
        "adcs x8, x8, x2;\n"
        "adc x9, x9, xzr;\n"
        "adds x11, x11, x10, lsl #32;\n"
        "lsr x3, x10, #32;\n"
        "adcs x8, x8, x3;\n"
        "mov x3, #0xffffffff00000001;\n"
        "mul x2, x10, x3;\n"
        "umulh x10, x10, x3;\n"
        "adcs x9, x9, x2;\n"
        "adc x10, x10, xzr;\n"
        "adds x8, x8, x11, lsl #32;\n"
        "lsr x3, x11, #32;\n"
        "adcs x9, x9, x3;\n"
        "mov x3, #0xffffffff00000001;\n"
        "mul x2, x11, x3;\n"
        "umulh x11, x11, x3;\n"
        "adcs x10, x10, x2;\n"
        "adc x11, x11, xzr;\n"
        "adds x8, x8, x12;\n"
        "adcs x9, x9, x13;\n"
        "adcs x10, x10, x14;\n"
        "adcs x11, x11, x7;\n"
        "mov x2, #0xffffffffffffffff;\n"
        "csel x2, xzr, x2, cc;\n"
        "mov x3, #0xffffffff;\n"
        "csel x3, xzr, x3, cc;\n"
        "mov x5, #0xffffffff00000001;\n"
        "csel x5, xzr, x5, cc;\n"
        "subs x8, x8, x2;\n"
        "sbcs x9, x9, x3;\n"
        "sbcs x10, x10, xzr;\n"
        "sbc x11, x11, x5;\n"
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
        "csetm x3, cc;\n"
        "adds x5, x5, x3;\n"
        "mov x4, #0xffffffff;\n"
        "and x4, x4, x3;\n"
        "adcs x6, x6, x4;\n"
        "adcs x7, x7, xzr;\n"
        "mov x4, #0xffffffff00000001;\n"
        "and x4, x4, x3;\n"
        "adc x8, x8, x4;\n"
        "stp x5, x6, [" $P0 "];\n"
        "stp x7, x8, [" $P0 "+ 16]"
    )}
}

/// Point addition on NIST curve P-256 in Montgomery-Jacobian coordinates
///
///
/// Does p3 := p1 + p2 where all points are regarded as Jacobian triples with
/// each coordinate in the Montgomery domain, i.e. x' = (2^256 * x) mod p_256.
/// A Jacobian triple (x',y',z') represents affine point (x/z^2,y/z^3).
pub(crate) fn p256_montjadd(p3: &mut [u64; 12], p1: &[u64; 12], p2: &[u64; 12]) {
    // SAFETY: inline assembly. see [crate::low::inline_assembly_safety] for safety info.
    unsafe {
        core::arch::asm!(

        // Make room on stack for temporary variables
        // Move the input arguments to stable places

        Q!("    sub             " "sp, sp, # (" NSPACE!() "+ 0)"),

        Q!("    mov             " input_z!() ", x0"),
        Q!("    mov             " input_x!() ", x1"),
        Q!("    mov             " input_y!() ", x2"),

        // Main code, just a sequence of basic field operations
        // 12 * multiply + 4 * square + 7 * subtract

        amontsqr_p256!(z1sq!(), z_1!()),
        amontsqr_p256!(z2sq!(), z_2!()),

        montmul_p256!(y1a!(), z_2!(), y_1!()),
        montmul_p256!(y2a!(), z_1!(), y_2!()),

        montmul_p256!(x2a!(), z1sq!(), x_2!()),
        montmul_p256!(x1a!(), z2sq!(), x_1!()),
        montmul_p256!(y2a!(), z1sq!(), y2a!()),
        montmul_p256!(y1a!(), z2sq!(), y1a!()),

        sub_p256!(xd!(), x2a!(), x1a!()),
        sub_p256!(yd!(), y2a!(), y1a!()),

        amontsqr_p256!(zz!(), xd!()),
        montsqr_p256!(ww!(), yd!()),

        montmul_p256!(zzx1!(), zz!(), x1a!()),
        montmul_p256!(zzx2!(), zz!(), x2a!()),

        sub_p256!(resx!(), ww!(), zzx1!()),
        sub_p256!(t1!(), zzx2!(), zzx1!()),

        montmul_p256!(xd!(), xd!(), z_1!()),

        sub_p256!(resx!(), resx!(), zzx2!()),

        sub_p256!(t2!(), zzx1!(), resx!()),

        montmul_p256!(t1!(), t1!(), y1a!()),
        montmul_p256!(resz!(), xd!(), z_2!()),
        montmul_p256!(t2!(), yd!(), t2!()),

        sub_p256!(resy!(), t2!(), t1!()),

        // Load in the z coordinates of the inputs to check for P1 = 0 and P2 = 0
        // The condition codes get set by a comparison (P2 != 0) - (P1 != 0)
        // So  "HI" <=> CF /\ ~ZF <=> P1 = 0 /\ ~(P2 = 0)
        // and "LO" <=> ~CF       <=> ~(P1 = 0) /\ P2 = 0

        Q!("    ldp             " "x0, x1, [" z_1!() "]"),
        Q!("    ldp             " "x2, x3, [" z_1!() "+ 16]"),

        Q!("    orr             " "x12, x0, x1"),
        Q!("    orr             " "x13, x2, x3"),
        Q!("    orr             " "x12, x12, x13"),
        Q!("    cmp             " "x12, xzr"),
        Q!("    cset            " "x12, ne"),

        Q!("    ldp             " "x4, x5, [" z_2!() "]"),
        Q!("    ldp             " "x6, x7, [" z_2!() "+ 16]"),

        Q!("    orr             " "x13, x4, x5"),
        Q!("    orr             " "x14, x6, x7"),
        Q!("    orr             " "x13, x13, x14"),
        Q!("    cmp             " "x13, xzr"),
        Q!("    cset            " "x13, ne"),

        Q!("    cmp             " "x13, x12"),

        // Multiplex the outputs accordingly, re-using the z's in registers

        Q!("    ldp             " "x8, x9, [" resz!() "]"),
        Q!("    csel            " "x8, x0, x8, lo"),
        Q!("    csel            " "x9, x1, x9, lo"),
        Q!("    csel            " "x8, x4, x8, hi"),
        Q!("    csel            " "x9, x5, x9, hi"),
        Q!("    ldp             " "x10, x11, [" resz!() "+ 16]"),
        Q!("    csel            " "x10, x2, x10, lo"),
        Q!("    csel            " "x11, x3, x11, lo"),
        Q!("    csel            " "x10, x6, x10, hi"),
        Q!("    csel            " "x11, x7, x11, hi"),

        Q!("    ldp             " "x12, x13, [" x_1!() "]"),
        Q!("    ldp             " "x0, x1, [" resx!() "]"),
        Q!("    csel            " "x0, x12, x0, lo"),
        Q!("    csel            " "x1, x13, x1, lo"),
        Q!("    ldp             " "x12, x13, [" x_2!() "]"),
        Q!("    csel            " "x0, x12, x0, hi"),
        Q!("    csel            " "x1, x13, x1, hi"),

        Q!("    ldp             " "x12, x13, [" x_1!() "+ 16]"),
        Q!("    ldp             " "x2, x3, [" resx!() "+ 16]"),
        Q!("    csel            " "x2, x12, x2, lo"),
        Q!("    csel            " "x3, x13, x3, lo"),
        Q!("    ldp             " "x12, x13, [" x_2!() "+ 16]"),
        Q!("    csel            " "x2, x12, x2, hi"),
        Q!("    csel            " "x3, x13, x3, hi"),

        Q!("    ldp             " "x12, x13, [" y_1!() "]"),
        Q!("    ldp             " "x4, x5, [" resy!() "]"),
        Q!("    csel            " "x4, x12, x4, lo"),
        Q!("    csel            " "x5, x13, x5, lo"),
        Q!("    ldp             " "x12, x13, [" y_2!() "]"),
        Q!("    csel            " "x4, x12, x4, hi"),
        Q!("    csel            " "x5, x13, x5, hi"),

        Q!("    ldp             " "x12, x13, [" y_1!() "+ 16]"),
        Q!("    ldp             " "x6, x7, [" resy!() "+ 16]"),
        Q!("    csel            " "x6, x12, x6, lo"),
        Q!("    csel            " "x7, x13, x7, lo"),
        Q!("    ldp             " "x12, x13, [" y_2!() "+ 16]"),
        Q!("    csel            " "x6, x12, x6, hi"),
        Q!("    csel            " "x7, x13, x7, hi"),

        // Finally store back the multiplexed values

        Q!("    stp             " "x0, x1, [" x_3!() "]"),
        Q!("    stp             " "x2, x3, [" x_3!() "+ 16]"),
        Q!("    stp             " "x4, x5, [" y_3!() "]"),
        Q!("    stp             " "x6, x7, [" y_3!() "+ 16]"),
        Q!("    stp             " "x8, x9, [" z_3!() "]"),
        Q!("    stp             " "x10, x11, [" z_3!() "+ 16]"),

        // Restore registers and return

        Q!("    add             " "sp, sp, # (" NSPACE!() "+ 0)"),
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
