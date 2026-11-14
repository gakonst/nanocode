// generated source. do not edit.
#![allow(non_upper_case_globals, unused_macros, unused_imports)]
use crate::low::macros::*;

// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

// ----------------------------------------------------------------------------
// Decode compressed 256-bit form of edwards25519 point
// Input c[32] (bytes); output function return and z[8]
//
// extern uint64_t edwards25519_decode_alt(uint64_t z[static 8], const uint8_t c[static 32]);
//
// This interprets the input byte string as a little-endian number
// representing a point (x,y) on the edwards25519 curve, encoded as
// 2^255 * x_0 + y where x_0 is the least significant bit of x. It
// returns the full pair of coordinates x (at z) and y (at z+4). The
// return code is 0 for success and 1 for failure, which means that
// the input does not correspond to the encoding of any edwards25519
// point. This can happen for three reasons, where y = the lowest
// 255 bits of the input:
//
//  * y >= p_25519
//    Input y coordinate is not reduced
//  * (y^2 - 1) * (1 + d_25519 * y^2) has no modular square root
//    There is no x such that (x,y) is on the curve
//  * y^2 = 1 and top bit of input is set
//    Cannot be the canonical encoding of (0,1) or (0,-1)
//
// Standard ARM ABI: X0 = z, X1 = c
// ----------------------------------------------------------------------------

// Size in bytes of a 64-bit word

macro_rules! N {
    () => {
        "8"
    };
}

// Pointer-offset pairs for temporaries on stack

macro_rules! y {
    () => {
        "sp, #0"
    };
}
macro_rules! s { () => { Q!("sp, # (4 * " N!() ")") } }
macro_rules! t { () => { Q!("sp, # (8 * " N!() ")") } }
macro_rules! u { () => { Q!("sp, # (12 * " N!() ")") } }
macro_rules! v { () => { Q!("sp, # (16 * " N!() ")") } }
macro_rules! w { () => { Q!("sp, # (20 * " N!() ")") } }

// Other temporary variables in register

macro_rules! res {
    () => {
        "x19"
    };
}
macro_rules! sgnbit {
    () => {
        "x20"
    };
}
macro_rules! badun {
    () => {
        "x21"
    };
}

// Total size to reserve on the stack

macro_rules! NSPACE { () => { Q!("24 * " N!()) } }

// Loading large constants

macro_rules! movbig {
    ($nn:expr, $n3:expr, $n2:expr, $n1:expr, $n0:expr) => { Q!(
        "movz " $nn ", " $n0 ";\n"
        "movk " $nn ", " $n1 ", lsl #16;\n"
        "movk " $nn ", " $n2 ", lsl #32;\n"
        "movk " $nn ", " $n3 ", lsl #48"
    )}
}

// Macros wrapping up calls to the local subroutines

macro_rules! mulp {
    ($dest:expr, $src1:expr, $src2:expr) => { Q!(
        "add x0, " $dest ";\n"
        "add x1, " $src1 ";\n"
        "add x2, " $src2 ";\n"
        "bl " Label!("Ledwards25519_decode_alt_mul_p25519", 3, After)
    )}
}

macro_rules! nsqr {
    ($dest:expr, $n:expr, $src:expr) => { Q!(
        "add x0, " $dest ";\n"
        "mov x1, " $n ";\n"
        "add x2, " $src ";\n"
        "bl " Label!("Ledwards25519_decode_alt_nsqr_p25519", 4, After)
    )}
}

/// Decode compressed 256-bit form of edwards25519 point
///
/// Input c[32] (bytes); output function return and z[8]
///
/// This interprets the input byte string as a little-endian number
/// representing a point (x,y) on the edwards25519 curve, encoded as
/// 2^255 * x_0 + y where x_0 is the least significant bit of x. It
/// returns the full pair of coordinates x (at z) and y (at z+4). The
/// return code is 0 for success and 1 for failure, which means that
/// the input does not correspond to the encoding of any edwards25519
/// point. This can happen for three reasons, where y = the lowest
/// 255 bits of the input:
///
///  * y >= p_25519
///    Input y coordinate is not reduced
///  * (y^2 - 1) * (1 + d_25519 * y^2) has no modular square root
///    There is no x such that (x,y) is on the curve
///  * y^2 = 1 and top bit of input is set
///    Cannot be the canonical encoding of (0,1) or (0,-1)
pub(crate) fn edwards25519_decode(z: &mut [u64; 8], c: &[u8; 32]) -> bool {
    let ret: u64;
    // SAFETY: inline assembly. see [crate::low::inline_assembly_safety] for safety info.
    unsafe {
        core::arch::asm!(

        // Save registers and make room for temporaries

        Q!("    stp             " "x19, x20, [sp, #-16] !"),
        Q!("    stp             " "x21, x30, [sp, #-16] !"),
        Q!("    sub             " "sp, sp, # (" NSPACE!() "+ 0)"),

        // Save the return pointer for the end so we can overwrite x0 later

        Q!("    mov             " res!() ", x0"),

        // Load the inputs, using byte operations in case of big-endian setting.
        // Let y be the lowest 255 bits of the input and sgnbit the desired parity.
        // If y >= p_25519 then already flag the input as invalid (badun = 1).

        Q!("    ldrb            " "w0, [x1]"),
        Q!("    lsl             " "x4, x0, #56"),
        Q!("    ldrb            " "w0, [x1, #1]"),
        Q!("    extr            " "x4, x0, x4, #8"),
        Q!("    ldrb            " "w0, [x1, #2]"),
        Q!("    extr            " "x4, x0, x4, #8"),
        Q!("    ldrb            " "w0, [x1, #3]"),
        Q!("    extr            " "x4, x0, x4, #8"),
        Q!("    ldrb            " "w0, [x1, #4]"),
        Q!("    extr            " "x4, x0, x4, #8"),
        Q!("    ldrb            " "w0, [x1, #5]"),
        Q!("    extr            " "x4, x0, x4, #8"),
        Q!("    ldrb            " "w0, [x1, #6]"),
        Q!("    extr            " "x4, x0, x4, #8"),
        Q!("    ldrb            " "w0, [x1, #7]"),
        Q!("    extr            " "x4, x0, x4, #8"),

        Q!("    ldrb            " "w0, [x1, #8]"),
        Q!("    lsl             " "x5, x0, #56"),
        Q!("    ldrb            " "w0, [x1, #9]"),
        Q!("    extr            " "x5, x0, x5, #8"),
        Q!("    ldrb            " "w0, [x1, #10]"),
        Q!("    extr            " "x5, x0, x5, #8"),
        Q!("    ldrb            " "w0, [x1, #11]"),
        Q!("    extr            " "x5, x0, x5, #8"),
        Q!("    ldrb            " "w0, [x1, #12]"),
        Q!("    extr            " "x5, x0, x5, #8"),
        Q!("    ldrb            " "w0, [x1, #13]"),
        Q!("    extr            " "x5, x0, x5, #8"),
        Q!("    ldrb            " "w0, [x1, #14]"),
        Q!("    extr            " "x5, x0, x5, #8"),
        Q!("    ldrb            " "w0, [x1, #15]"),
        Q!("    extr            " "x5, x0, x5, #8"),

        Q!("    ldrb            " "w0, [x1, #16]"),
        Q!("    lsl             " "x6, x0, #56"),
        Q!("    ldrb            " "w0, [x1, #17]"),
        Q!("    extr            " "x6, x0, x6, #8"),
        Q!("    ldrb            " "w0, [x1, #18]"),
        Q!("    extr            " "x6, x0, x6, #8"),
        Q!("    ldrb            " "w0, [x1, #19]"),
        Q!("    extr            " "x6, x0, x6, #8"),
        Q!("    ldrb            " "w0, [x1, #20]"),
        Q!("    extr            " "x6, x0, x6, #8"),
        Q!("    ldrb            " "w0, [x1, #21]"),
        Q!("    extr            " "x6, x0, x6, #8"),
        Q!("    ldrb            " "w0, [x1, #22]"),
        Q!("    extr            " "x6, x0, x6, #8"),
        Q!("    ldrb            " "w0, [x1, #23]"),
        Q!("    extr            " "x6, x0, x6, #8"),

        Q!("    ldrb            " "w0, [x1, #24]"),
        Q!("    lsl             " "x7, x0, #56"),
        Q!("    ldrb            " "w0, [x1, #25]"),
        Q!("    extr            " "x7, x0, x7, #8"),
        Q!("    ldrb            " "w0, [x1, #26]"),
        Q!("    extr            " "x7, x0, x7, #8"),
        Q!("    ldrb            " "w0, [x1, #27]"),
        Q!("    extr            " "x7, x0, x7, #8"),
        Q!("    ldrb            " "w0, [x1, #28]"),
        Q!("    extr            " "x7, x0, x7, #8"),
        Q!("    ldrb            " "w0, [x1, #29]"),
        Q!("    extr            " "x7, x0, x7, #8"),
        Q!("    ldrb            " "w0, [x1, #30]"),
        Q!("    extr            " "x7, x0, x7, #8"),
        Q!("    ldrb            " "w0, [x1, #31]"),
        Q!("    extr            " "x7, x0, x7, #8"),

        Q!("    stp             " "x4, x5, [" y!() "]"),
        Q!("    lsr             " sgnbit!() ", x7, #63"),
        Q!("    and             " "x7, x7, #0x7FFFFFFFFFFFFFFF"),
        Q!("    stp             " "x6, x7, [" y!() "+ 16]"),

        Q!("    adds            " "xzr, x4, #19"),
        Q!("    adcs            " "xzr, x5, xzr"),
        Q!("    adcs            " "xzr, x6, xzr"),
        Q!("    adcs            " "xzr, x7, xzr"),
        Q!("    cset            " badun!() ", mi"),

        // u = y^2 - 1 (actually y + 2^255-20, not reduced modulo)
        // v = 1 + d * y^2 (not reduced modulo from the +1)
        // w = u * v

        nsqr!(v!(), "1", y!()),
        Q!("    ldp             " "x0, x1, [" v!() "]"),
        Q!("    ldp             " "x2, x3, [" v!() "+ 16]"),
        Q!("    mov             " "x4, #0x8000000000000000"),
        Q!("    subs            " "x0, x0, #20"),
        Q!("    sbcs            " "x1, x1, xzr"),
        Q!("    sbcs            " "x2, x2, xzr"),
        Q!("    sbc             " "x3, x3, x4"),
        Q!("    stp             " "x0, x1, [" u!() "]"),
        Q!("    stp             " "x2, x3, [" u!() "+ 16]"),

        movbig!("x0", "#0x75eb", "#0x4dca", "#0x1359", "#0x78a3"),
        movbig!("x1", "#0x0070", "#0x0a4d", "#0x4141", "#0xd8ab"),
        movbig!("x2", "#0x8cc7", "#0x4079", "#0x7779", "#0xe898"),
        movbig!("x3", "#0x5203", "#0x6cee", "#0x2b6f", "#0xfe73"),
        Q!("    stp             " "x0, x1, [" w!() "]"),
        Q!("    stp             " "x2, x3, [" w!() "+ 16]"),
        mulp!(v!(), w!(), v!()),
        Q!("    ldp             " "x0, x1, [" v!() "]"),
        Q!("    ldp             " "x2, x3, [" v!() "+ 16]"),
        Q!("    adds            " "x0, x0, #1"),
        Q!("    adcs            " "x1, x1, xzr"),
        Q!("    adcs            " "x2, x2, xzr"),
        Q!("    adcs            " "x3, x3, xzr"),
        Q!("    stp             " "x0, x1, [" v!() "]"),
        Q!("    stp             " "x2, x3, [" v!() "+ 16]"),

        mulp!(w!(), u!(), v!()),

        // Get s = w^{252-3} as a candidate inverse square root 1/sqrt(w).
        // This power tower computation is the same as bignum_invsqrt_p25519

        nsqr!(t!(), "1", w!()),
        mulp!(t!(), t!(), w!()),
        nsqr!(s!(), "2", t!()),
        mulp!(t!(), s!(), t!()),
        nsqr!(s!(), "1", t!()),
        mulp!(v!(), s!(), w!()),
        nsqr!(s!(), "5", v!()),
        mulp!(t!(), s!(), v!()),
        nsqr!(s!(), "10", t!()),
        mulp!(t!(), s!(), t!()),
        nsqr!(s!(), "5", t!()),
        mulp!(v!(), s!(), v!()),
        nsqr!(s!(), "25", v!()),
        mulp!(t!(), s!(), v!()),
        nsqr!(s!(), "50", t!()),
        mulp!(t!(), s!(), t!()),
        nsqr!(s!(), "25", t!()),
        mulp!(v!(), s!(), v!()),
        nsqr!(s!(), "125", v!()),
        mulp!(v!(), s!(), v!()),
        nsqr!(s!(), "2", v!()),
        mulp!(s!(), s!(), w!()),

        // Compute v' = s^2 * w to discriminate whether the square root sqrt(u/v)
        // exists, in which case we should get 0, 1 or -1.

        nsqr!(v!(), "1", s!()),
        mulp!(v!(), v!(), w!()),

        // Get the two candidates for sqrt(u / v), one being s = u * w^{252-3}
        // and the other being t = s * j_25519 where j_25519 = sqrt(-1).

        mulp!(s!(), u!(), s!()),
        movbig!("x0", "#0xc4ee", "#0x1b27", "#0x4a0e", "#0xa0b0"),
        movbig!("x1", "#0x2f43", "#0x1806", "#0xad2f", "#0xe478"),
        movbig!("x2", "#0x2b4d", "#0x0099", "#0x3dfb", "#0xd7a7"),
        movbig!("x3", "#0x2b83", "#0x2480", "#0x4fc1", "#0xdf0b"),
        Q!("    stp             " "x0, x1, [" t!() "]"),
        Q!("    stp             " "x2, x3, [" t!() "+ 16]"),
        mulp!(t!(), s!(), t!()),

        // x4 = 0 <=> s^2 * w = 0 or 1

        Q!("    ldp             " "x0, x1, [" v!() "]"),
        Q!("    ldp             " "x2, x3, [" v!() "+ 16]"),
        Q!("    bic             " "x4, x0, #1"),
        Q!("    orr             " "x4, x4, x1"),
        Q!("    orr             " "x5, x2, x3"),
        Q!("    orr             " "x4, x4, x5"),

        // x0 = 0 <=> s^2 * w = -1 (mod p_25519, i.e. s^2 * w = 2^255 - 20)

        Q!("    add             " "x0, x0, #20"),
        Q!("    add             " "x1, x1, #1"),
        Q!("    orr             " "x0, x0, x1"),
        Q!("    add             " "x2, x2, #1"),
        Q!("    eor             " "x3, x3, #0x7FFFFFFFFFFFFFFF"),
        Q!("    orr             " "x2, x2, x3"),
        Q!("    orr             " "x0, x0, x2"),

        // If s^2 * w is not 0 or 1 then replace s by t

        Q!("    cmp             " "x4, xzr"),
        Q!("    ldp             " "x10, x11, [" s!() "]"),
        Q!("    ldp             " "x14, x15, [" t!() "]"),
        Q!("    csel            " "x10, x10, x14, eq"),
        Q!("    csel            " "x11, x11, x15, eq"),
        Q!("    ldp             " "x12, x13, [" s!() "+ 16]"),
        Q!("    ldp             " "x16, x17, [" t!() "+ 16]"),
        Q!("    csel            " "x12, x12, x16, eq"),
        Q!("    csel            " "x13, x13, x17, eq"),
        Q!("    stp             " "x10, x11, [" s!() "]"),
        Q!("    stp             " "x12, x13, [" s!() "+ 16]"),

        // Check invalidity, occurring if s^2 * w is not in {0,1,-1}

        Q!("    ccmp            " "x0, xzr, 4, ne"),
        Q!("    cset            " "x0, ne"),
        Q!("    orr             " badun!() ", " badun!() ", x0"),

        // Let [x3;x2;x1;x0] = s and [x7;x6;x5;x4] = p_25519 - s

        Q!("    ldp             " "x0, x1, [" s!() "]"),
        Q!("    ldp             " "x2, x3, [" s!() "+ 16]"),
        Q!("    mov             " "x4, #-19"),
        Q!("    subs            " "x4, x4, x0"),
        Q!("    mov             " "x6, #-1"),
        Q!("    sbcs            " "x5, x6, x1"),
        Q!("    sbcs            " "x6, x6, x2"),
        Q!("    mov             " "x7, #0x7FFFFFFFFFFFFFFF"),
        Q!("    sbc             " "x7, x7, x3"),

        // Decide whether a flip is apparently indicated, s_0 <=> sgnbit
        // Decide also if s = 0 by OR-ing its digits. Now if a flip is indicated:
        //  - if s = 0 then mark as invalid
        //  - if s <> 0 then indeed flip

        Q!("    and             " "x9, x0, #1"),
        Q!("    eor             " sgnbit!() ", x9, " sgnbit!()),
        Q!("    orr             " "x8, x0, x1"),
        Q!("    orr             " "x9, x2, x3"),
        Q!("    orr             " "x8, x8, x9"),
        Q!("    orr             " "x10, " badun!() ", " sgnbit!()),
        Q!("    cmp             " "x8, xzr"),
        Q!("    csel            " badun!() ", x10, " badun!() ", eq"),
        Q!("    ccmp            " sgnbit!() ", xzr, #4, ne"),

        // Actual selection of x as s or -s, copying of y and return of validity

        Q!("    csel            " "x0, x0, x4, eq"),
        Q!("    csel            " "x1, x1, x5, eq"),
        Q!("    csel            " "x2, x2, x6, eq"),
        Q!("    csel            " "x3, x3, x7, eq"),
        Q!("    ldp             " "x8, x9, [" y!() "]"),
        Q!("    ldp             " "x10, x11, [" y!() "+ 16]"),

        Q!("    stp             " "x0, x1, [" res!() "]"),
        Q!("    stp             " "x2, x3, [" res!() ", #16]"),
        Q!("    stp             " "x8, x9, [" res!() ", #32]"),
        Q!("    stp             " "x10, x11, [" res!() ", #48]"),

        Q!("    mov             " "x0, " badun!()),

        // Restore stack and registers

        Q!("    add             " "sp, sp, # (" NSPACE!() "+ 0)"),

        Q!("    ldp             " "x21, x30, [sp], #16"),
        Q!("    ldp             " "x19, x20, [sp], #16"),
        // proc hoisting in -> ret after Ledwards25519_decode_alt_loop
        Q!("    b               " Label!("hoist_finish", 2, After)),

        // *************************************************************
        // Local z = x * y
        // *************************************************************

        Q!(Label!("Ledwards25519_decode_alt_mul_p25519", 3) ":"),

        Q!("    ldp             " "x3, x4, [x1]"),
        Q!("    ldp             " "x7, x8, [x2]"),
        Q!("    mul             " "x12, x3, x7"),
        Q!("    umulh           " "x13, x3, x7"),
        Q!("    mul             " "x11, x3, x8"),
        Q!("    umulh           " "x14, x3, x8"),
        Q!("    adds            " "x13, x13, x11"),
        Q!("    ldp             " "x9, x10, [x2, #16]"),
        Q!("    mul             " "x11, x3, x9"),
        Q!("    umulh           " "x15, x3, x9"),
        Q!("    adcs            " "x14, x14, x11"),
        Q!("    mul             " "x11, x3, x10"),
        Q!("    umulh           " "x16, x3, x10"),
        Q!("    adcs            " "x15, x15, x11"),
        Q!("    adc             " "x16, x16, xzr"),
        Q!("    ldp             " "x5, x6, [x1, #16]"),
        Q!("    mul             " "x11, x4, x7"),
        Q!("    adds            " "x13, x13, x11"),
        Q!("    mul             " "x11, x4, x8"),
        Q!("    adcs            " "x14, x14, x11"),
        Q!("    mul             " "x11, x4, x9"),
        Q!("    adcs            " "x15, x15, x11"),
        Q!("    mul             " "x11, x4, x10"),
        Q!("    adcs            " "x16, x16, x11"),
        Q!("    umulh           " "x3, x4, x10"),
        Q!("    adc             " "x3, x3, xzr"),
        Q!("    umulh           " "x11, x4, x7"),
        Q!("    adds            " "x14, x14, x11"),
        Q!("    umulh           " "x11, x4, x8"),
        Q!("    adcs            " "x15, x15, x11"),
        Q!("    umulh           " "x11, x4, x9"),
        Q!("    adcs            " "x16, x16, x11"),
        Q!("    adc             " "x3, x3, xzr"),
        Q!("    mul             " "x11, x5, x7"),
        Q!("    adds            " "x14, x14, x11"),
        Q!("    mul             " "x11, x5, x8"),
        Q!("    adcs            " "x15, x15, x11"),
        Q!("    mul             " "x11, x5, x9"),
        Q!("    adcs            " "x16, x16, x11"),
        Q!("    mul             " "x11, x5, x10"),
        Q!("    adcs            " "x3, x3, x11"),
        Q!("    umulh           " "x4, x5, x10"),
        Q!("    adc             " "x4, x4, xzr"),
        Q!("    umulh           " "x11, x5, x7"),
        Q!("    adds            " "x15, x15, x11"),
        Q!("    umulh           " "x11, x5, x8"),
        Q!("    adcs            " "x16, x16, x11"),
        Q!("    umulh           " "x11, x5, x9"),
        Q!("    adcs            " "x3, x3, x11"),
        Q!("    adc             " "x4, x4, xzr"),
        Q!("    mul             " "x11, x6, x7"),
        Q!("    adds            " "x15, x15, x11"),
        Q!("    mul             " "x11, x6, x8"),
        Q!("    adcs            " "x16, x16, x11"),
        Q!("    mul             " "x11, x6, x9"),
        Q!("    adcs            " "x3, x3, x11"),
        Q!("    mul             " "x11, x6, x10"),
        Q!("    adcs            " "x4, x4, x11"),
        Q!("    umulh           " "x5, x6, x10"),
        Q!("    adc             " "x5, x5, xzr"),
        Q!("    umulh           " "x11, x6, x7"),
        Q!("    adds            " "x16, x16, x11"),
        Q!("    umulh           " "x11, x6, x8"),
        Q!("    adcs            " "x3, x3, x11"),
        Q!("    umulh           " "x11, x6, x9"),
        Q!("    adcs            " "x4, x4, x11"),
        Q!("    adc             " "x5, x5, xzr"),
        Q!("    mov             " "x7, #38"),
        Q!("    mul             " "x11, x7, x16"),
        Q!("    umulh           " "x9, x7, x16"),
        Q!("    adds            " "x12, x12, x11"),
        Q!("    mul             " "x11, x7, x3"),
        Q!("    umulh           " "x3, x7, x3"),
        Q!("    adcs            " "x13, x13, x11"),
        Q!("    mul             " "x11, x7, x4"),
        Q!("    umulh           " "x4, x7, x4"),
        Q!("    adcs            " "x14, x14, x11"),
        Q!("    mul             " "x11, x7, x5"),
        Q!("    umulh           " "x5, x7, x5"),
        Q!("    adcs            " "x15, x15, x11"),
        Q!("    cset            " "x16, hs"),
        Q!("    adds            " "x15, x15, x4"),
        Q!("    adc             " "x16, x16, x5"),
        Q!("    cmn             " "x15, x15"),
        Q!("    orr             " "x15, x15, #0x8000000000000000"),
        Q!("    adc             " "x8, x16, x16"),
        Q!("    mov             " "x7, #19"),
        Q!("    madd            " "x11, x7, x8, x7"),
        Q!("    adds            " "x12, x12, x11"),
        Q!("    adcs            " "x13, x13, x9"),
        Q!("    adcs            " "x14, x14, x3"),
        Q!("    adcs            " "x15, x15, xzr"),
        Q!("    csel            " "x7, x7, xzr, lo"),
        Q!("    subs            " "x12, x12, x7"),
        Q!("    sbcs            " "x13, x13, xzr"),
        Q!("    sbcs            " "x14, x14, xzr"),
        Q!("    sbc             " "x15, x15, xzr"),
        Q!("    and             " "x15, x15, #0x7fffffffffffffff"),
        Q!("    stp             " "x12, x13, [x0]"),
        Q!("    stp             " "x14, x15, [x0, #16]"),
        Q!("    ret             " ),

        // *************************************************************
        // Local z = 2^n * x
        // *************************************************************

        Q!(Label!("Ledwards25519_decode_alt_nsqr_p25519", 4) ":"),

        // Copy input argument into [x5;x4;x3;x2] (overwriting input pointer x20

        Q!("    ldp             " "x6, x3, [x2]"),
        Q!("    ldp             " "x4, x5, [x2, #16]"),
        Q!("    mov             " "x2, x6"),

        // Main squaring loop, accumulating in [x5;x4;x3;x2] consistently and
        // only ensuring the intermediates are < 2 * p_25519 = 2^256 - 38

        Q!(Label!("Ledwards25519_decode_alt_loop", 5) ":"),
        Q!("    mul             " "x9, x2, x3"),
        Q!("    umulh           " "x10, x2, x3"),
        Q!("    mul             " "x11, x2, x5"),
        Q!("    umulh           " "x12, x2, x5"),
        Q!("    mul             " "x7, x2, x4"),
        Q!("    umulh           " "x6, x2, x4"),
        Q!("    adds            " "x10, x10, x7"),
        Q!("    adcs            " "x11, x11, x6"),
        Q!("    mul             " "x7, x3, x4"),
        Q!("    umulh           " "x6, x3, x4"),
        Q!("    adc             " "x6, x6, xzr"),
        Q!("    adds            " "x11, x11, x7"),
        Q!("    mul             " "x13, x4, x5"),
        Q!("    umulh           " "x14, x4, x5"),
        Q!("    adcs            " "x12, x12, x6"),
        Q!("    mul             " "x7, x3, x5"),
        Q!("    umulh           " "x6, x3, x5"),
        Q!("    adc             " "x6, x6, xzr"),
        Q!("    adds            " "x12, x12, x7"),
        Q!("    adcs            " "x13, x13, x6"),
        Q!("    adc             " "x14, x14, xzr"),
        Q!("    adds            " "x9, x9, x9"),
        Q!("    adcs            " "x10, x10, x10"),
        Q!("    adcs            " "x11, x11, x11"),
        Q!("    adcs            " "x12, x12, x12"),
        Q!("    adcs            " "x13, x13, x13"),
        Q!("    adcs            " "x14, x14, x14"),
        Q!("    cset            " "x6, hs"),
        Q!("    umulh           " "x7, x2, x2"),
        Q!("    mul             " "x8, x2, x2"),
        Q!("    adds            " "x9, x9, x7"),
        Q!("    mul             " "x7, x3, x3"),
        Q!("    adcs            " "x10, x10, x7"),
        Q!("    umulh           " "x7, x3, x3"),
        Q!("    adcs            " "x11, x11, x7"),
        Q!("    mul             " "x7, x4, x4"),
        Q!("    adcs            " "x12, x12, x7"),
        Q!("    umulh           " "x7, x4, x4"),
        Q!("    adcs            " "x13, x13, x7"),
        Q!("    mul             " "x7, x5, x5"),
        Q!("    adcs            " "x14, x14, x7"),
        Q!("    umulh           " "x7, x5, x5"),
        Q!("    adc             " "x6, x6, x7"),
        Q!("    mov             " "x3, #38"),
        Q!("    mul             " "x7, x3, x12"),
        Q!("    umulh           " "x4, x3, x12"),
        Q!("    adds            " "x8, x8, x7"),
        Q!("    mul             " "x7, x3, x13"),
        Q!("    umulh           " "x13, x3, x13"),
        Q!("    adcs            " "x9, x9, x7"),
        Q!("    mul             " "x7, x3, x14"),
        Q!("    umulh           " "x14, x3, x14"),
        Q!("    adcs            " "x10, x10, x7"),
        Q!("    mul             " "x7, x3, x6"),
        Q!("    umulh           " "x6, x3, x6"),
        Q!("    adcs            " "x11, x11, x7"),
        Q!("    cset            " "x12, hs"),
        Q!("    adds            " "x11, x11, x14"),
        Q!("    adc             " "x12, x12, x6"),
        Q!("    cmn             " "x11, x11"),
        Q!("    bic             " "x11, x11, #0x8000000000000000"),
        Q!("    adc             " "x2, x12, x12"),
        Q!("    mov             " "x3, #0x13"),
        Q!("    mul             " "x7, x3, x2"),
        Q!("    adds            " "x2, x8, x7"),
        Q!("    adcs            " "x3, x9, x4"),
        Q!("    adcs            " "x4, x10, x13"),
        Q!("    adc             " "x5, x11, xzr"),

        // Loop as applicable

        Q!("    subs            " "x1, x1, #1"),
        Q!("    bne             " Label!("Ledwards25519_decode_alt_loop", 5, Before)),

        // We know the intermediate result x < 2^256 - 38, and now we do strict
        // modular reduction mod 2^255 - 19. Note x < 2^255 - 19 <=> x + 19 < 2^255
        // which is equivalent to a "pl" condition.

        Q!("    adds            " "x6, x2, #19"),
        Q!("    adcs            " "x7, x3, xzr"),
        Q!("    adcs            " "x8, x4, xzr"),
        Q!("    adcs            " "x9, x5, xzr"),

        Q!("    csel            " "x2, x2, x6, pl"),
        Q!("    csel            " "x3, x3, x7, pl"),
        Q!("    csel            " "x4, x4, x8, pl"),
        Q!("    csel            " "x5, x5, x9, pl"),
        Q!("    bic             " "x5, x5, #0x8000000000000000"),

        // Copy result back into destination and return

        Q!("    stp             " "x2, x3, [x0]"),
        Q!("    stp             " "x4, x5, [x0, #16]"),
        Q!("    ret             " ),
        Q!(Label!("hoist_finish", 2) ":"),
        inout("x0") z.as_mut_ptr() => ret,
        inout("x1") c.as_ptr() => _,
        // clobbers
        out("x10") _,
        out("x11") _,
        out("x12") _,
        out("x13") _,
        out("x14") _,
        out("x15") _,
        out("x16") _,
        out("x17") _,
        out("x2") _,
        out("x20") _,
        out("x21") _,
        out("x3") _,
        out("x30") _,
        out("x4") _,
        out("x5") _,
        out("x6") _,
        out("x7") _,
        out("x8") _,
        out("x9") _,
            )
    };
    ret == 0
}
