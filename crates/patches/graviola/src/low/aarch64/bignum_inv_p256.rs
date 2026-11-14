// generated source. do not edit.
#![allow(non_upper_case_globals, unused_macros, unused_imports)]
use crate::low::macros::*;

// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

// ----------------------------------------------------------------------------
// Modular inverse modulo p_256 = 2^256 - 2^224 + 2^192 + 2^96 - 1
// Input x[4]; output z[4]
//
// extern void bignum_inv_p256(uint64_t z[static 4],const uint64_t x[static 4]);
//
// If the 4-digit input x is coprime to p_256, i.e. is not divisible
// by it, returns z < p_256 such that x * z == 1 (mod p_256). Note that
// x does not need to be reduced modulo p_256, but the output always is.
// If the input is divisible (i.e. is 0 or p_256), then there can be no
// modular inverse and z = 0 is returned.
//
// Standard ARM ABI: X0 = z, X1 = x
// ----------------------------------------------------------------------------

// Size in bytes of a 64-bit word

macro_rules! N {
    () => {
        "8"
    };
}

// Used for the return pointer

macro_rules! res {
    () => {
        "x20"
    };
}

// Loop counter and d = 2 * delta value for divstep

macro_rules! i {
    () => {
        "x21"
    };
}
macro_rules! d {
    () => {
        "x22"
    };
}

// Registers used for matrix element magnitudes and signs

macro_rules! m00 {
    () => {
        "x10"
    };
}
macro_rules! m01 {
    () => {
        "x11"
    };
}
macro_rules! m10 {
    () => {
        "x12"
    };
}
macro_rules! m11 {
    () => {
        "x13"
    };
}
macro_rules! s00 {
    () => {
        "x14"
    };
}
macro_rules! s01 {
    () => {
        "x15"
    };
}
macro_rules! s10 {
    () => {
        "x16"
    };
}
macro_rules! s11 {
    () => {
        "x17"
    };
}

// Initial carries for combinations

macro_rules! car0 {
    () => {
        "x9"
    };
}
macro_rules! car1 {
    () => {
        "x19"
    };
}

// Input and output, plain registers treated according to pattern

macro_rules! reg0 {
    () => {
        "x0, #0"
    };
}
macro_rules! reg1 {
    () => {
        "x1, #0"
    };
}
macro_rules! reg2 {
    () => {
        "x2, #0"
    };
}
macro_rules! reg3 {
    () => {
        "x3, #0"
    };
}
macro_rules! reg4 {
    () => {
        "x4, #0"
    };
}

macro_rules! x {
    () => {
        "x1, #0"
    };
}
macro_rules! z {
    () => {
        "x0, #0"
    };
}

// Pointer-offset pairs for temporaries on stack

macro_rules! f {
    () => {
        "sp, #0"
    };
}
macro_rules! g { () => { Q!("sp, # (6 * " N!() ")") } }
macro_rules! u { () => { Q!("sp, # (12 * " N!() ")") } }
macro_rules! v { () => { Q!("sp, # (16 * " N!() ")") } }

// Total size to reserve on the stack

macro_rules! NSPACE { () => { Q!("20 * " N!()) } }

// ---------------------------------------------------------------------------
// Core signed almost-Montgomery reduction macro. Takes input in
// [d4;d3;d2;d1;d0] and returns result in [d4;d3;d2;d1], adding to
// the existing [d4;d3;d2;d1], and re-using d0 as a temporary internally
// as well as t0, t1, t2. This is almost-Montgomery, i.e. the result fits
// in 4 digits but is not necessarily strictly reduced mod p_256.
// ---------------------------------------------------------------------------

macro_rules! amontred {
    ($d4:expr, $d3:expr, $d2:expr, $d1:expr, $d0:expr, $t2:expr, $t1:expr, $t0:expr) => { Q!(
        /* We only know the input is -2^316 < x < 2^316. To do traditional  */
        /* unsigned Montgomery reduction, start by adding 2^61 * p_256.     */
        "mov " $t0 ", #0xe000000000000000;\n"
        "adds " $d0 ", " $d0 ", " $t0 ";\n"
        "sbcs " $d1 ", " $d1 ", xzr;\n"
        "mov " $t1 ", #0x000000001fffffff;\n"
        "adcs " $d2 ", " $d2 ", " $t1 ";\n"
        "mov " $t2 ", #0x2000000000000000;\n"
        "adcs " $d3 ", " $d3 ", " $t2 ";\n"
        "mov " $t0 ", #0x1fffffffe0000000;\n"
        "adc " $d4 ", " $d4 ", " $t0 ";\n"
        /* Let w = d0, the original word we use as offset; d0 gets recycled */
        /* First let [t2;t1] = 2^32 * w                                     */
        /* then let [d0;t0] = (2^64 - 2^32 + 1) * w (overwrite old d0)      */
        "lsl " $t1 ", " $d0 ", #32;\n"
        "subs " $t0 ", " $d0 ", " $t1 ";\n"
        "lsr " $t2 ", " $d0 ", #32;\n"
        "sbc " $d0 ", " $d0 ", " $t2 ";\n"
        /* Hence basic [d4;d3;d2;d1] += (2^256 - 2^224 + 2^192 + 2^96) * w  */
        "adds " $d1 ", " $d1 ", " $t1 ";\n"
        "adcs " $d2 ", " $d2 ", " $t2 ";\n"
        "adcs " $d3 ", " $d3 ", " $t0 ";\n"
        "adcs " $d4 ", " $d4 ", " $d0 ";\n"
        /* Now capture top carry and subtract p_256 if set (almost-Montgomery) */
        "mov " $t0 ", #0xffffffffffffffff;\n"
        "mov " $t1 ", #0x00000000ffffffff;\n"
        "mov " $t2 ", #0xffffffff00000001;\n"
        "csel " $t0 ", " $t0 ", xzr, cs;\n"
        "csel " $t1 ", " $t1 ", xzr, cs;\n"
        "csel " $t2 ", " $t2 ", xzr, cs;\n"
        "subs " $d1 ", " $d1 ", " $t0 ";\n"
        "sbcs " $d2 ", " $d2 ", " $t1 ";\n"
        "sbcs " $d3 ", " $d3 ", xzr;\n"
        "sbc " $d4 ", " $d4 ", " $t2
    )}
}

// Very similar to a subroutine call to the s2n-bignum word_divstep59.
// But different in register usage and returning the final matrix in
// registers as follows
//
// [ m00  m01]
// [ m10  m11]

macro_rules! divstep59 {
    () => { Q!(
        "and x4, x2, #0xfffff;\n"
        "orr x4, x4, #0xfffffe0000000000;\n"
        "and x5, x3, #0xfffff;\n"
        "orr x5, x5, #0xc000000000000000;\n"
        "tst x5, #0x1;\n"
        "csel x6, x4, xzr, ne;\n"
        "ccmp x1, xzr, #0x8, ne;\n"
        "cneg x1, x1, ge;\n"
        "cneg x6, x6, ge;\n"
        "csel x4, x5, x4, ge;\n"
        "add x5, x5, x6;\n"
        "add x1, x1, #0x2;\n"
        "tst x5, #0x2;\n"
        "asr x5, x5, #1;\n"
        "csel x6, x4, xzr, ne;\n"
        "ccmp x1, xzr, #0x8, ne;\n"
        "cneg x1, x1, ge;\n"
        "cneg x6, x6, ge;\n"
        "csel x4, x5, x4, ge;\n"
        "add x5, x5, x6;\n"
        "add x1, x1, #0x2;\n"
        "tst x5, #0x2;\n"
        "asr x5, x5, #1;\n"
        "csel x6, x4, xzr, ne;\n"
        "ccmp x1, xzr, #0x8, ne;\n"
        "cneg x1, x1, ge;\n"
        "cneg x6, x6, ge;\n"
        "csel x4, x5, x4, ge;\n"
        "add x5, x5, x6;\n"
        "add x1, x1, #0x2;\n"
        "tst x5, #0x2;\n"
        "asr x5, x5, #1;\n"
        "csel x6, x4, xzr, ne;\n"
        "ccmp x1, xzr, #0x8, ne;\n"
        "cneg x1, x1, ge;\n"
        "cneg x6, x6, ge;\n"
        "csel x4, x5, x4, ge;\n"
        "add x5, x5, x6;\n"
        "add x1, x1, #0x2;\n"
        "tst x5, #0x2;\n"
        "asr x5, x5, #1;\n"
        "csel x6, x4, xzr, ne;\n"
        "ccmp x1, xzr, #0x8, ne;\n"
        "cneg x1, x1, ge;\n"
        "cneg x6, x6, ge;\n"
        "csel x4, x5, x4, ge;\n"
        "add x5, x5, x6;\n"
        "add x1, x1, #0x2;\n"
        "tst x5, #0x2;\n"
        "asr x5, x5, #1;\n"
        "csel x6, x4, xzr, ne;\n"
        "ccmp x1, xzr, #0x8, ne;\n"
        "cneg x1, x1, ge;\n"
        "cneg x6, x6, ge;\n"
        "csel x4, x5, x4, ge;\n"
        "add x5, x5, x6;\n"
        "add x1, x1, #0x2;\n"
        "tst x5, #0x2;\n"
        "asr x5, x5, #1;\n"
        "csel x6, x4, xzr, ne;\n"
        "ccmp x1, xzr, #0x8, ne;\n"
        "cneg x1, x1, ge;\n"
        "cneg x6, x6, ge;\n"
        "csel x4, x5, x4, ge;\n"
        "add x5, x5, x6;\n"
        "add x1, x1, #0x2;\n"
        "tst x5, #0x2;\n"
        "asr x5, x5, #1;\n"
        "csel x6, x4, xzr, ne;\n"
        "ccmp x1, xzr, #0x8, ne;\n"
        "cneg x1, x1, ge;\n"
        "cneg x6, x6, ge;\n"
        "csel x4, x5, x4, ge;\n"
        "add x5, x5, x6;\n"
        "add x1, x1, #0x2;\n"
        "tst x5, #0x2;\n"
        "asr x5, x5, #1;\n"
        "csel x6, x4, xzr, ne;\n"
        "ccmp x1, xzr, #0x8, ne;\n"
        "cneg x1, x1, ge;\n"
        "cneg x6, x6, ge;\n"
        "csel x4, x5, x4, ge;\n"
        "add x5, x5, x6;\n"
        "add x1, x1, #0x2;\n"
        "tst x5, #0x2;\n"
        "asr x5, x5, #1;\n"
        "csel x6, x4, xzr, ne;\n"
        "ccmp x1, xzr, #0x8, ne;\n"
        "cneg x1, x1, ge;\n"
        "cneg x6, x6, ge;\n"
        "csel x4, x5, x4, ge;\n"
        "add x5, x5, x6;\n"
        "add x1, x1, #0x2;\n"
        "tst x5, #0x2;\n"
        "asr x5, x5, #1;\n"
        "csel x6, x4, xzr, ne;\n"
        "ccmp x1, xzr, #0x8, ne;\n"
        "cneg x1, x1, ge;\n"
        "cneg x6, x6, ge;\n"
        "csel x4, x5, x4, ge;\n"
        "add x5, x5, x6;\n"
        "add x1, x1, #0x2;\n"
        "tst x5, #0x2;\n"
        "asr x5, x5, #1;\n"
        "csel x6, x4, xzr, ne;\n"
        "ccmp x1, xzr, #0x8, ne;\n"
        "cneg x1, x1, ge;\n"
        "cneg x6, x6, ge;\n"
        "csel x4, x5, x4, ge;\n"
        "add x5, x5, x6;\n"
        "add x1, x1, #0x2;\n"
        "tst x5, #0x2;\n"
        "asr x5, x5, #1;\n"
        "csel x6, x4, xzr, ne;\n"
        "ccmp x1, xzr, #0x8, ne;\n"
        "cneg x1, x1, ge;\n"
        "cneg x6, x6, ge;\n"
        "csel x4, x5, x4, ge;\n"
        "add x5, x5, x6;\n"
        "add x1, x1, #0x2;\n"
        "tst x5, #0x2;\n"
        "asr x5, x5, #1;\n"
        "csel x6, x4, xzr, ne;\n"
        "ccmp x1, xzr, #0x8, ne;\n"
        "cneg x1, x1, ge;\n"
        "cneg x6, x6, ge;\n"
        "csel x4, x5, x4, ge;\n"
        "add x5, x5, x6;\n"
        "add x1, x1, #0x2;\n"
        "tst x5, #0x2;\n"
        "asr x5, x5, #1;\n"
        "csel x6, x4, xzr, ne;\n"
        "ccmp x1, xzr, #0x8, ne;\n"
        "cneg x1, x1, ge;\n"
        "cneg x6, x6, ge;\n"
        "csel x4, x5, x4, ge;\n"
        "add x5, x5, x6;\n"
        "add x1, x1, #0x2;\n"
        "tst x5, #0x2;\n"
        "asr x5, x5, #1;\n"
        "csel x6, x4, xzr, ne;\n"
        "ccmp x1, xzr, #0x8, ne;\n"
        "cneg x1, x1, ge;\n"
        "cneg x6, x6, ge;\n"
        "csel x4, x5, x4, ge;\n"
        "add x5, x5, x6;\n"
        "add x1, x1, #0x2;\n"
        "tst x5, #0x2;\n"
        "asr x5, x5, #1;\n"
        "csel x6, x4, xzr, ne;\n"
        "ccmp x1, xzr, #0x8, ne;\n"
        "cneg x1, x1, ge;\n"
        "cneg x6, x6, ge;\n"
        "csel x4, x5, x4, ge;\n"
        "add x5, x5, x6;\n"
        "add x1, x1, #0x2;\n"
        "tst x5, #0x2;\n"
        "asr x5, x5, #1;\n"
        "csel x6, x4, xzr, ne;\n"
        "ccmp x1, xzr, #0x8, ne;\n"
        "cneg x1, x1, ge;\n"
        "cneg x6, x6, ge;\n"
        "csel x4, x5, x4, ge;\n"
        "add x5, x5, x6;\n"
        "add x1, x1, #0x2;\n"
        "tst x5, #0x2;\n"
        "asr x5, x5, #1;\n"
        "csel x6, x4, xzr, ne;\n"
        "ccmp x1, xzr, #0x8, ne;\n"
        "cneg x1, x1, ge;\n"
        "cneg x6, x6, ge;\n"
        "csel x4, x5, x4, ge;\n"
        "add x5, x5, x6;\n"
        "add x1, x1, #0x2;\n"
        "tst x5, #0x2;\n"
        "asr x5, x5, #1;\n"
        "csel x6, x4, xzr, ne;\n"
        "ccmp x1, xzr, #0x8, ne;\n"
        "cneg x1, x1, ge;\n"
        "cneg x6, x6, ge;\n"
        "csel x4, x5, x4, ge;\n"
        "add x5, x5, x6;\n"
        "add x1, x1, #0x2;\n"
        "asr x5, x5, #1;\n"
        "add x8, x4, #0x100, lsl #12;\n"
        "sbfx x8, x8, #21, #21;\n"
        "mov x11, #0x100000;\n"
        "add x11, x11, x11, lsl #21;\n"
        "add x9, x4, x11;\n"
        "asr x9, x9, #42;\n"
        "add x10, x5, #0x100, lsl #12;\n"
        "sbfx x10, x10, #21, #21;\n"
        "add x11, x5, x11;\n"
        "asr x11, x11, #42;\n"
        "mul x6, x8, x2;\n"
        "mul x7, x9, x3;\n"
        "mul x2, x10, x2;\n"
        "mul x3, x11, x3;\n"
        "add x4, x6, x7;\n"
        "add x5, x2, x3;\n"
        "asr x2, x4, #20;\n"
        "asr x3, x5, #20;\n"
        "and x4, x2, #0xfffff;\n"
        "orr x4, x4, #0xfffffe0000000000;\n"
        "and x5, x3, #0xfffff;\n"
        "orr x5, x5, #0xc000000000000000;\n"
        "tst x5, #0x1;\n"
        "csel x6, x4, xzr, ne;\n"
        "ccmp x1, xzr, #0x8, ne;\n"
        "cneg x1, x1, ge;\n"
        "cneg x6, x6, ge;\n"
        "csel x4, x5, x4, ge;\n"
        "add x5, x5, x6;\n"
        "add x1, x1, #0x2;\n"
        "tst x5, #0x2;\n"
        "asr x5, x5, #1;\n"
        "csel x6, x4, xzr, ne;\n"
        "ccmp x1, xzr, #0x8, ne;\n"
        "cneg x1, x1, ge;\n"
        "cneg x6, x6, ge;\n"
        "csel x4, x5, x4, ge;\n"
        "add x5, x5, x6;\n"
        "add x1, x1, #0x2;\n"
        "tst x5, #0x2;\n"
        "asr x5, x5, #1;\n"
        "csel x6, x4, xzr, ne;\n"
        "ccmp x1, xzr, #0x8, ne;\n"
        "cneg x1, x1, ge;\n"
        "cneg x6, x6, ge;\n"
        "csel x4, x5, x4, ge;\n"
        "add x5, x5, x6;\n"
        "add x1, x1, #0x2;\n"
        "tst x5, #0x2;\n"
        "asr x5, x5, #1;\n"
        "csel x6, x4, xzr, ne;\n"
        "ccmp x1, xzr, #0x8, ne;\n"
        "cneg x1, x1, ge;\n"
        "cneg x6, x6, ge;\n"
        "csel x4, x5, x4, ge;\n"
        "add x5, x5, x6;\n"
        "add x1, x1, #0x2;\n"
        "tst x5, #0x2;\n"
        "asr x5, x5, #1;\n"
        "csel x6, x4, xzr, ne;\n"
        "ccmp x1, xzr, #0x8, ne;\n"
        "cneg x1, x1, ge;\n"
        "cneg x6, x6, ge;\n"
        "csel x4, x5, x4, ge;\n"
        "add x5, x5, x6;\n"
        "add x1, x1, #0x2;\n"
        "tst x5, #0x2;\n"
        "asr x5, x5, #1;\n"
        "csel x6, x4, xzr, ne;\n"
        "ccmp x1, xzr, #0x8, ne;\n"
        "cneg x1, x1, ge;\n"
        "cneg x6, x6, ge;\n"
        "csel x4, x5, x4, ge;\n"
        "add x5, x5, x6;\n"
        "add x1, x1, #0x2;\n"
        "tst x5, #0x2;\n"
        "asr x5, x5, #1;\n"
        "csel x6, x4, xzr, ne;\n"
        "ccmp x1, xzr, #0x8, ne;\n"
        "cneg x1, x1, ge;\n"
        "cneg x6, x6, ge;\n"
        "csel x4, x5, x4, ge;\n"
        "add x5, x5, x6;\n"
        "add x1, x1, #0x2;\n"
        "tst x5, #0x2;\n"
        "asr x5, x5, #1;\n"
        "csel x6, x4, xzr, ne;\n"
        "ccmp x1, xzr, #0x8, ne;\n"
        "cneg x1, x1, ge;\n"
        "cneg x6, x6, ge;\n"
        "csel x4, x5, x4, ge;\n"
        "add x5, x5, x6;\n"
        "add x1, x1, #0x2;\n"
        "tst x5, #0x2;\n"
        "asr x5, x5, #1;\n"
        "csel x6, x4, xzr, ne;\n"
        "ccmp x1, xzr, #0x8, ne;\n"
        "cneg x1, x1, ge;\n"
        "cneg x6, x6, ge;\n"
        "csel x4, x5, x4, ge;\n"
        "add x5, x5, x6;\n"
        "add x1, x1, #0x2;\n"
        "tst x5, #0x2;\n"
        "asr x5, x5, #1;\n"
        "csel x6, x4, xzr, ne;\n"
        "ccmp x1, xzr, #0x8, ne;\n"
        "cneg x1, x1, ge;\n"
        "cneg x6, x6, ge;\n"
        "csel x4, x5, x4, ge;\n"
        "add x5, x5, x6;\n"
        "add x1, x1, #0x2;\n"
        "tst x5, #0x2;\n"
        "asr x5, x5, #1;\n"
        "csel x6, x4, xzr, ne;\n"
        "ccmp x1, xzr, #0x8, ne;\n"
        "cneg x1, x1, ge;\n"
        "cneg x6, x6, ge;\n"
        "csel x4, x5, x4, ge;\n"
        "add x5, x5, x6;\n"
        "add x1, x1, #0x2;\n"
        "tst x5, #0x2;\n"
        "asr x5, x5, #1;\n"
        "csel x6, x4, xzr, ne;\n"
        "ccmp x1, xzr, #0x8, ne;\n"
        "cneg x1, x1, ge;\n"
        "cneg x6, x6, ge;\n"
        "csel x4, x5, x4, ge;\n"
        "add x5, x5, x6;\n"
        "add x1, x1, #0x2;\n"
        "tst x5, #0x2;\n"
        "asr x5, x5, #1;\n"
        "csel x6, x4, xzr, ne;\n"
        "ccmp x1, xzr, #0x8, ne;\n"
        "cneg x1, x1, ge;\n"
        "cneg x6, x6, ge;\n"
        "csel x4, x5, x4, ge;\n"
        "add x5, x5, x6;\n"
        "add x1, x1, #0x2;\n"
        "tst x5, #0x2;\n"
        "asr x5, x5, #1;\n"
        "csel x6, x4, xzr, ne;\n"
        "ccmp x1, xzr, #0x8, ne;\n"
        "cneg x1, x1, ge;\n"
        "cneg x6, x6, ge;\n"
        "csel x4, x5, x4, ge;\n"
        "add x5, x5, x6;\n"
        "add x1, x1, #0x2;\n"
        "tst x5, #0x2;\n"
        "asr x5, x5, #1;\n"
        "csel x6, x4, xzr, ne;\n"
        "ccmp x1, xzr, #0x8, ne;\n"
        "cneg x1, x1, ge;\n"
        "cneg x6, x6, ge;\n"
        "csel x4, x5, x4, ge;\n"
        "add x5, x5, x6;\n"
        "add x1, x1, #0x2;\n"
        "tst x5, #0x2;\n"
        "asr x5, x5, #1;\n"
        "csel x6, x4, xzr, ne;\n"
        "ccmp x1, xzr, #0x8, ne;\n"
        "cneg x1, x1, ge;\n"
        "cneg x6, x6, ge;\n"
        "csel x4, x5, x4, ge;\n"
        "add x5, x5, x6;\n"
        "add x1, x1, #0x2;\n"
        "tst x5, #0x2;\n"
        "asr x5, x5, #1;\n"
        "csel x6, x4, xzr, ne;\n"
        "ccmp x1, xzr, #0x8, ne;\n"
        "cneg x1, x1, ge;\n"
        "cneg x6, x6, ge;\n"
        "csel x4, x5, x4, ge;\n"
        "add x5, x5, x6;\n"
        "add x1, x1, #0x2;\n"
        "tst x5, #0x2;\n"
        "asr x5, x5, #1;\n"
        "csel x6, x4, xzr, ne;\n"
        "ccmp x1, xzr, #0x8, ne;\n"
        "cneg x1, x1, ge;\n"
        "cneg x6, x6, ge;\n"
        "csel x4, x5, x4, ge;\n"
        "add x5, x5, x6;\n"
        "add x1, x1, #0x2;\n"
        "tst x5, #0x2;\n"
        "asr x5, x5, #1;\n"
        "csel x6, x4, xzr, ne;\n"
        "ccmp x1, xzr, #0x8, ne;\n"
        "cneg x1, x1, ge;\n"
        "cneg x6, x6, ge;\n"
        "csel x4, x5, x4, ge;\n"
        "add x5, x5, x6;\n"
        "add x1, x1, #0x2;\n"
        "tst x5, #0x2;\n"
        "asr x5, x5, #1;\n"
        "csel x6, x4, xzr, ne;\n"
        "ccmp x1, xzr, #0x8, ne;\n"
        "cneg x1, x1, ge;\n"
        "cneg x6, x6, ge;\n"
        "csel x4, x5, x4, ge;\n"
        "add x5, x5, x6;\n"
        "add x1, x1, #0x2;\n"
        "asr x5, x5, #1;\n"
        "add x12, x4, #0x100, lsl #12;\n"
        "sbfx x12, x12, #21, #21;\n"
        "mov x15, #0x100000;\n"
        "add x15, x15, x15, lsl #21;\n"
        "add x13, x4, x15;\n"
        "asr x13, x13, #42;\n"
        "add x14, x5, #0x100, lsl #12;\n"
        "sbfx x14, x14, #21, #21;\n"
        "add x15, x5, x15;\n"
        "asr x15, x15, #42;\n"
        "mul x6, x12, x2;\n"
        "mul x7, x13, x3;\n"
        "mul x2, x14, x2;\n"
        "mul x3, x15, x3;\n"
        "add x4, x6, x7;\n"
        "add x5, x2, x3;\n"
        "asr x2, x4, #20;\n"
        "asr x3, x5, #20;\n"
        "and x4, x2, #0xfffff;\n"
        "orr x4, x4, #0xfffffe0000000000;\n"
        "and x5, x3, #0xfffff;\n"
        "orr x5, x5, #0xc000000000000000;\n"
        "tst x5, #0x1;\n"
        "csel x6, x4, xzr, ne;\n"
        "ccmp x1, xzr, #0x8, ne;\n"
        "cneg x1, x1, ge;\n"
        "cneg x6, x6, ge;\n"
        "csel x4, x5, x4, ge;\n"
        "add x5, x5, x6;\n"
        "add x1, x1, #0x2;\n"
        "tst x5, #0x2;\n"
        "asr x5, x5, #1;\n"
        "csel x6, x4, xzr, ne;\n"
        "ccmp x1, xzr, #0x8, ne;\n"
        "cneg x1, x1, ge;\n"
        "cneg x6, x6, ge;\n"
        "csel x4, x5, x4, ge;\n"
        "add x5, x5, x6;\n"
        "add x1, x1, #0x2;\n"
        "tst x5, #0x2;\n"
        "asr x5, x5, #1;\n"
        "csel x6, x4, xzr, ne;\n"
        "ccmp x1, xzr, #0x8, ne;\n"
        "cneg x1, x1, ge;\n"
        "cneg x6, x6, ge;\n"
        "csel x4, x5, x4, ge;\n"
        "add x5, x5, x6;\n"
        "add x1, x1, #0x2;\n"
        "tst x5, #0x2;\n"
        "asr x5, x5, #1;\n"
        "csel x6, x4, xzr, ne;\n"
        "ccmp x1, xzr, #0x8, ne;\n"
        "cneg x1, x1, ge;\n"
        "cneg x6, x6, ge;\n"
        "csel x4, x5, x4, ge;\n"
        "add x5, x5, x6;\n"
        "add x1, x1, #0x2;\n"
        "tst x5, #0x2;\n"
        "asr x5, x5, #1;\n"
        "csel x6, x4, xzr, ne;\n"
        "ccmp x1, xzr, #0x8, ne;\n"
        "cneg x1, x1, ge;\n"
        "cneg x6, x6, ge;\n"
        "csel x4, x5, x4, ge;\n"
        "add x5, x5, x6;\n"
        "add x1, x1, #0x2;\n"
        "tst x5, #0x2;\n"
        "asr x5, x5, #1;\n"
        "csel x6, x4, xzr, ne;\n"
        "ccmp x1, xzr, #0x8, ne;\n"
        "cneg x1, x1, ge;\n"
        "cneg x6, x6, ge;\n"
        "csel x4, x5, x4, ge;\n"
        "add x5, x5, x6;\n"
        "add x1, x1, #0x2;\n"
        "tst x5, #0x2;\n"
        "asr x5, x5, #1;\n"
        "csel x6, x4, xzr, ne;\n"
        "ccmp x1, xzr, #0x8, ne;\n"
        "cneg x1, x1, ge;\n"
        "cneg x6, x6, ge;\n"
        "csel x4, x5, x4, ge;\n"
        "add x5, x5, x6;\n"
        "add x1, x1, #0x2;\n"
        "tst x5, #0x2;\n"
        "asr x5, x5, #1;\n"
        "csel x6, x4, xzr, ne;\n"
        "ccmp x1, xzr, #0x8, ne;\n"
        "cneg x1, x1, ge;\n"
        "cneg x6, x6, ge;\n"
        "csel x4, x5, x4, ge;\n"
        "add x5, x5, x6;\n"
        "add x1, x1, #0x2;\n"
        "tst x5, #0x2;\n"
        "asr x5, x5, #1;\n"
        "csel x6, x4, xzr, ne;\n"
        "ccmp x1, xzr, #0x8, ne;\n"
        "cneg x1, x1, ge;\n"
        "cneg x6, x6, ge;\n"
        "csel x4, x5, x4, ge;\n"
        "add x5, x5, x6;\n"
        "add x1, x1, #0x2;\n"
        "tst x5, #0x2;\n"
        "asr x5, x5, #1;\n"
        "csel x6, x4, xzr, ne;\n"
        "ccmp x1, xzr, #0x8, ne;\n"
        "cneg x1, x1, ge;\n"
        "cneg x6, x6, ge;\n"
        "csel x4, x5, x4, ge;\n"
        "add x5, x5, x6;\n"
        "add x1, x1, #0x2;\n"
        "tst x5, #0x2;\n"
        "asr x5, x5, #1;\n"
        "mul x2, x12, x8;\n"
        "mul x3, x12, x9;\n"
        "mul x6, x14, x8;\n"
        "mul x7, x14, x9;\n"
        "madd x8, x13, x10, x2;\n"
        "madd x9, x13, x11, x3;\n"
        "madd x16, x15, x10, x6;\n"
        "madd x17, x15, x11, x7;\n"
        "csel x6, x4, xzr, ne;\n"
        "ccmp x1, xzr, #0x8, ne;\n"
        "cneg x1, x1, ge;\n"
        "cneg x6, x6, ge;\n"
        "csel x4, x5, x4, ge;\n"
        "add x5, x5, x6;\n"
        "add x1, x1, #0x2;\n"
        "tst x5, #0x2;\n"
        "asr x5, x5, #1;\n"
        "csel x6, x4, xzr, ne;\n"
        "ccmp x1, xzr, #0x8, ne;\n"
        "cneg x1, x1, ge;\n"
        "cneg x6, x6, ge;\n"
        "csel x4, x5, x4, ge;\n"
        "add x5, x5, x6;\n"
        "add x1, x1, #0x2;\n"
        "tst x5, #0x2;\n"
        "asr x5, x5, #1;\n"
        "csel x6, x4, xzr, ne;\n"
        "ccmp x1, xzr, #0x8, ne;\n"
        "cneg x1, x1, ge;\n"
        "cneg x6, x6, ge;\n"
        "csel x4, x5, x4, ge;\n"
        "add x5, x5, x6;\n"
        "add x1, x1, #0x2;\n"
        "tst x5, #0x2;\n"
        "asr x5, x5, #1;\n"
        "csel x6, x4, xzr, ne;\n"
        "ccmp x1, xzr, #0x8, ne;\n"
        "cneg x1, x1, ge;\n"
        "cneg x6, x6, ge;\n"
        "csel x4, x5, x4, ge;\n"
        "add x5, x5, x6;\n"
        "add x1, x1, #0x2;\n"
        "tst x5, #0x2;\n"
        "asr x5, x5, #1;\n"
        "csel x6, x4, xzr, ne;\n"
        "ccmp x1, xzr, #0x8, ne;\n"
        "cneg x1, x1, ge;\n"
        "cneg x6, x6, ge;\n"
        "csel x4, x5, x4, ge;\n"
        "add x5, x5, x6;\n"
        "add x1, x1, #0x2;\n"
        "tst x5, #0x2;\n"
        "asr x5, x5, #1;\n"
        "csel x6, x4, xzr, ne;\n"
        "ccmp x1, xzr, #0x8, ne;\n"
        "cneg x1, x1, ge;\n"
        "cneg x6, x6, ge;\n"
        "csel x4, x5, x4, ge;\n"
        "add x5, x5, x6;\n"
        "add x1, x1, #0x2;\n"
        "tst x5, #0x2;\n"
        "asr x5, x5, #1;\n"
        "csel x6, x4, xzr, ne;\n"
        "ccmp x1, xzr, #0x8, ne;\n"
        "cneg x1, x1, ge;\n"
        "cneg x6, x6, ge;\n"
        "csel x4, x5, x4, ge;\n"
        "add x5, x5, x6;\n"
        "add x1, x1, #0x2;\n"
        "tst x5, #0x2;\n"
        "asr x5, x5, #1;\n"
        "csel x6, x4, xzr, ne;\n"
        "ccmp x1, xzr, #0x8, ne;\n"
        "cneg x1, x1, ge;\n"
        "cneg x6, x6, ge;\n"
        "csel x4, x5, x4, ge;\n"
        "add x5, x5, x6;\n"
        "add x1, x1, #0x2;\n"
        "tst x5, #0x2;\n"
        "asr x5, x5, #1;\n"
        "csel x6, x4, xzr, ne;\n"
        "ccmp x1, xzr, #0x8, ne;\n"
        "cneg x1, x1, ge;\n"
        "cneg x6, x6, ge;\n"
        "csel x4, x5, x4, ge;\n"
        "add x5, x5, x6;\n"
        "add x1, x1, #0x2;\n"
        "asr x5, x5, #1;\n"
        "add x12, x4, #0x100, lsl #12;\n"
        "sbfx x12, x12, #22, #21;\n"
        "mov x15, #0x100000;\n"
        "add x15, x15, x15, lsl #21;\n"
        "add x13, x4, x15;\n"
        "asr x13, x13, #43;\n"
        "add x14, x5, #0x100, lsl #12;\n"
        "sbfx x14, x14, #22, #21;\n"
        "add x15, x5, x15;\n"
        "asr x15, x15, #43;\n"
        "mneg x2, x12, x8;\n"
        "mneg x3, x12, x9;\n"
        "mneg x4, x14, x8;\n"
        "mneg x5, x14, x9;\n"
        "msub " m00!() ", x13, x16, x2;\n"
        "msub " m01!() ", x13, x17, x3;\n"
        "msub " m10!() ", x15, x16, x4;\n"
        "msub " m11!() ", x15, x17, x5"
    )}
}

/// Modular inverse modulo p_256 = 2^256 - 2^224 + 2^192 + 2^96 - 1
///
/// Input x[4]; output z[4]
///
/// If the 4-digit input x is coprime to p_256, i.e. is not divisible
/// by it, returns z < p_256 such that x * z == 1 (mod p_256). Note that
/// x does not need to be reduced modulo p_256, but the output always is.
/// If the input is divisible (i.e. is 0 or p_256), then there can be no
/// modular inverse and z = 0 is returned.
pub(crate) fn bignum_inv_p256(z: &mut [u64; 4], x: &[u64; 4]) {
    // SAFETY: inline assembly. see [crate::low::inline_assembly_safety] for safety info.
    unsafe {
        core::arch::asm!(

        // Save registers and make room for temporaries

        Q!("    stp             " "x19, x20, [sp, #-16] !"),
        Q!("    stp             " "x21, x22, [sp, #-16] !"),
        Q!("    stp             " "x23, x24, [sp, #-16] !"),
        Q!("    sub             " "sp, sp, # (" NSPACE!() "+ 0)"),

        // Save the return pointer for the end so we can overwrite x0 later

        Q!("    mov             " res!() ", x0"),

        // Copy the prime and input into the main f and g variables respectively.
        // Make sure x is reduced so that g <= f as assumed in the bound proof.

        Q!("    mov             " "x10, #0xffffffffffffffff"),
        Q!("    mov             " "x11, #0x00000000ffffffff"),
        Q!("    mov             " "x13, #0xffffffff00000001"),
        Q!("    stp             " "x10, x11, [" f!() "]"),
        Q!("    stp             " "xzr, x13, [" f!() "+ 2 * " N!() "]"),
        Q!("    str             " "xzr, [" f!() "+ 4 * " N!() "]"),

        Q!("    ldp             " "x2, x3, [x1]"),
        Q!("    subs            " "x10, x2, x10"),
        Q!("    sbcs            " "x11, x3, x11"),
        Q!("    ldp             " "x4, x5, [x1, # (2 * " N!() ")]"),
        Q!("    sbcs            " "x12, x4, xzr"),
        Q!("    sbcs            " "x13, x5, x13"),

        Q!("    csel            " "x2, x2, x10, cc"),
        Q!("    csel            " "x3, x3, x11, cc"),
        Q!("    csel            " "x4, x4, x12, cc"),
        Q!("    csel            " "x5, x5, x13, cc"),

        Q!("    stp             " "x2, x3, [" g!() "]"),
        Q!("    stp             " "x4, x5, [" g!() "+ 2 * " N!() "]"),
        Q!("    str             " "xzr, [" g!() "+ 4 * " N!() "]"),

        // Also maintain reduced < 2^256 vector [u,v] such that
        // [f,g] == x * 2^{5*i-50} * [u,v] (mod p_256)
        // starting with [p_256,x] == x * 2^{5*0-50} * [0,2^50] (mod p_256)
        // The weird-looking 5*i modifications come in because we are doing
        // 64-bit word-sized Montgomery reductions at each stage, which is
        // 5 bits more than the 59-bit requirement to keep things stable.

        Q!("    stp             " "xzr, xzr, [" u!() "]"),
        Q!("    stp             " "xzr, xzr, [" u!() "+ 2 * " N!() "]"),

        Q!("    mov             " "x10, #0x0004000000000000"),
        Q!("    stp             " "x10, xzr, [" v!() "]"),
        Q!("    stp             " "xzr, xzr, [" v!() "+ 2 * " N!() "]"),

        // Start of main loop. We jump into the middle so that the divstep
        // portion is common to the special tenth iteration after a uniform
        // first 9.

        Q!("    mov             " i!() ", #10"),
        Q!("    mov             " d!() ", #1"),
        Q!("    b               " Label!("Lbignum_inv_p256_midloop", 2, After)),

        Q!(Label!("Lbignum_inv_p256_loop", 3) ":"),

        // Separate the matrix elements into sign-magnitude pairs

        Q!("    cmp             " m00!() ", xzr"),
        Q!("    csetm           " s00!() ", mi"),
        Q!("    cneg            " m00!() ", " m00!() ", mi"),

        Q!("    cmp             " m01!() ", xzr"),
        Q!("    csetm           " s01!() ", mi"),
        Q!("    cneg            " m01!() ", " m01!() ", mi"),

        Q!("    cmp             " m10!() ", xzr"),
        Q!("    csetm           " s10!() ", mi"),
        Q!("    cneg            " m10!() ", " m10!() ", mi"),

        Q!("    cmp             " m11!() ", xzr"),
        Q!("    csetm           " s11!() ", mi"),
        Q!("    cneg            " m11!() ", " m11!() ", mi"),

        // Adjust the initial values to allow for complement instead of negation
        // This initial offset is the same for [f,g] and [u,v] compositions.
        // Save it in stable registers for the [u,v] part and do [f,g] first.

        Q!("    and             " "x0, " m00!() ", " s00!()),
        Q!("    and             " "x1, " m01!() ", " s01!()),
        Q!("    add             " car0!() ", x0, x1"),

        Q!("    and             " "x0, " m10!() ", " s10!()),
        Q!("    and             " "x1, " m11!() ", " s11!()),
        Q!("    add             " car1!() ", x0, x1"),

        // Now the computation of the updated f and g values. This maintains a
        // 2-word carry between stages so we can conveniently insert the shift
        // right by 59 before storing back, and not overwrite digits we need
        // again of the old f and g values.
        //
        // Digit 0 of [f,g]

        Q!("    ldr             " "x7, [" f!() "]"),
        Q!("    eor             " "x1, x7, " s00!()),
        Q!("    mul             " "x0, x1, " m00!()),
        Q!("    umulh           " "x1, x1, " m00!()),
        Q!("    adds            " "x4, " car0!() ", x0"),
        Q!("    adc             " "x2, xzr, x1"),
        Q!("    ldr             " "x8, [" g!() "]"),
        Q!("    eor             " "x1, x8, " s01!()),
        Q!("    mul             " "x0, x1, " m01!()),
        Q!("    umulh           " "x1, x1, " m01!()),
        Q!("    adds            " "x4, x4, x0"),
        Q!("    adc             " "x2, x2, x1"),

        Q!("    eor             " "x1, x7, " s10!()),
        Q!("    mul             " "x0, x1, " m10!()),
        Q!("    umulh           " "x1, x1, " m10!()),
        Q!("    adds            " "x5, " car1!() ", x0"),
        Q!("    adc             " "x3, xzr, x1"),
        Q!("    eor             " "x1, x8, " s11!()),
        Q!("    mul             " "x0, x1, " m11!()),
        Q!("    umulh           " "x1, x1, " m11!()),
        Q!("    adds            " "x5, x5, x0"),
        Q!("    adc             " "x3, x3, x1"),

        // Digit 1 of [f,g]

        Q!("    ldr             " "x7, [" f!() "+ " N!() "]"),
        Q!("    eor             " "x1, x7, " s00!()),
        Q!("    mul             " "x0, x1, " m00!()),
        Q!("    umulh           " "x1, x1, " m00!()),
        Q!("    adds            " "x2, x2, x0"),
        Q!("    adc             " "x6, xzr, x1"),
        Q!("    ldr             " "x8, [" g!() "+ " N!() "]"),
        Q!("    eor             " "x1, x8, " s01!()),
        Q!("    mul             " "x0, x1, " m01!()),
        Q!("    umulh           " "x1, x1, " m01!()),
        Q!("    adds            " "x2, x2, x0"),
        Q!("    adc             " "x6, x6, x1"),
        Q!("    extr            " "x4, x2, x4, #59"),
        Q!("    str             " "x4, [" f!() "]"),

        Q!("    eor             " "x1, x7, " s10!()),
        Q!("    mul             " "x0, x1, " m10!()),
        Q!("    umulh           " "x1, x1, " m10!()),
        Q!("    adds            " "x3, x3, x0"),
        Q!("    adc             " "x4, xzr, x1"),
        Q!("    eor             " "x1, x8, " s11!()),
        Q!("    mul             " "x0, x1, " m11!()),
        Q!("    umulh           " "x1, x1, " m11!()),
        Q!("    adds            " "x3, x3, x0"),
        Q!("    adc             " "x4, x4, x1"),
        Q!("    extr            " "x5, x3, x5, #59"),
        Q!("    str             " "x5, [" g!() "]"),

        // Digit 2 of [f,g]

        Q!("    ldr             " "x7, [" f!() "+ 2 * " N!() "]"),
        Q!("    eor             " "x1, x7, " s00!()),
        Q!("    mul             " "x0, x1, " m00!()),
        Q!("    umulh           " "x1, x1, " m00!()),
        Q!("    adds            " "x6, x6, x0"),
        Q!("    adc             " "x5, xzr, x1"),
        Q!("    ldr             " "x8, [" g!() "+ 2 * " N!() "]"),
        Q!("    eor             " "x1, x8, " s01!()),
        Q!("    mul             " "x0, x1, " m01!()),
        Q!("    umulh           " "x1, x1, " m01!()),
        Q!("    adds            " "x6, x6, x0"),
        Q!("    adc             " "x5, x5, x1"),
        Q!("    extr            " "x2, x6, x2, #59"),
        Q!("    str             " "x2, [" f!() "+ " N!() "]"),

        Q!("    eor             " "x1, x7, " s10!()),
        Q!("    mul             " "x0, x1, " m10!()),
        Q!("    umulh           " "x1, x1, " m10!()),
        Q!("    adds            " "x4, x4, x0"),
        Q!("    adc             " "x2, xzr, x1"),
        Q!("    eor             " "x1, x8, " s11!()),
        Q!("    mul             " "x0, x1, " m11!()),
        Q!("    umulh           " "x1, x1, " m11!()),
        Q!("    adds            " "x4, x4, x0"),
        Q!("    adc             " "x2, x2, x1"),
        Q!("    extr            " "x3, x4, x3, #59"),
        Q!("    str             " "x3, [" g!() "+ " N!() "]"),

        // Digits 3 and 4 of [f,g]

        Q!("    ldr             " "x7, [" f!() "+ 3 * " N!() "]"),
        Q!("    eor             " "x1, x7, " s00!()),
        Q!("    ldr             " "x23, [" f!() "+ 4 * " N!() "]"),
        Q!("    eor             " "x3, x23, " s00!()),
        Q!("    and             " "x3, x3, " m00!()),
        Q!("    neg             " "x3, x3"),
        Q!("    mul             " "x0, x1, " m00!()),
        Q!("    umulh           " "x1, x1, " m00!()),
        Q!("    adds            " "x5, x5, x0"),
        Q!("    adc             " "x3, x3, x1"),
        Q!("    ldr             " "x8, [" g!() "+ 3 * " N!() "]"),
        Q!("    eor             " "x1, x8, " s01!()),
        Q!("    ldr             " "x24, [" g!() "+ 4 * " N!() "]"),
        Q!("    eor             " "x0, x24, " s01!()),
        Q!("    and             " "x0, x0, " m01!()),
        Q!("    sub             " "x3, x3, x0"),
        Q!("    mul             " "x0, x1, " m01!()),
        Q!("    umulh           " "x1, x1, " m01!()),
        Q!("    adds            " "x5, x5, x0"),
        Q!("    adc             " "x3, x3, x1"),
        Q!("    extr            " "x6, x5, x6, #59"),
        Q!("    str             " "x6, [" f!() "+ 2 * " N!() "]"),
        Q!("    extr            " "x5, x3, x5, #59"),
        Q!("    str             " "x5, [" f!() "+ 3 * " N!() "]"),
        Q!("    asr             " "x3, x3, #59"),
        Q!("    str             " "x3, [" f!() "+ 4 * " N!() "]"),

        Q!("    eor             " "x1, x7, " s10!()),
        Q!("    eor             " "x5, x23, " s10!()),
        Q!("    and             " "x5, x5, " m10!()),
        Q!("    neg             " "x5, x5"),
        Q!("    mul             " "x0, x1, " m10!()),
        Q!("    umulh           " "x1, x1, " m10!()),
        Q!("    adds            " "x2, x2, x0"),
        Q!("    adc             " "x5, x5, x1"),
        Q!("    eor             " "x1, x8, " s11!()),
        Q!("    eor             " "x0, x24, " s11!()),
        Q!("    and             " "x0, x0, " m11!()),
        Q!("    sub             " "x5, x5, x0"),
        Q!("    mul             " "x0, x1, " m11!()),
        Q!("    umulh           " "x1, x1, " m11!()),
        Q!("    adds            " "x2, x2, x0"),
        Q!("    adc             " "x5, x5, x1"),
        Q!("    extr            " "x4, x2, x4, #59"),
        Q!("    str             " "x4, [" g!() "+ 2 * " N!() "]"),
        Q!("    extr            " "x2, x5, x2, #59"),
        Q!("    str             " "x2, [" g!() "+ 3 * " N!() "]"),
        Q!("    asr             " "x5, x5, #59"),
        Q!("    str             " "x5, [" g!() "+ 4 * " N!() "]"),

        // Now the computation of the updated u and v values and their
        // Montgomery reductions. A very similar accumulation except that
        // the top words of u and v are unsigned and we don't shift.
        //
        // Digit 0 of [u,v]

        Q!("    ldr             " "x7, [" u!() "]"),
        Q!("    eor             " "x1, x7, " s00!()),
        Q!("    mul             " "x0, x1, " m00!()),
        Q!("    umulh           " "x1, x1, " m00!()),
        Q!("    adds            " "x4, " car0!() ", x0"),
        Q!("    adc             " "x2, xzr, x1"),
        Q!("    ldr             " "x8, [" v!() "]"),
        Q!("    eor             " "x1, x8, " s01!()),
        Q!("    mul             " "x0, x1, " m01!()),
        Q!("    umulh           " "x1, x1, " m01!()),
        Q!("    adds            " "x4, x4, x0"),
        Q!("    str             " "x4, [" u!() "]"),
        Q!("    adc             " "x2, x2, x1"),

        Q!("    eor             " "x1, x7, " s10!()),
        Q!("    mul             " "x0, x1, " m10!()),
        Q!("    umulh           " "x1, x1, " m10!()),
        Q!("    adds            " "x5, " car1!() ", x0"),
        Q!("    adc             " "x3, xzr, x1"),
        Q!("    eor             " "x1, x8, " s11!()),
        Q!("    mul             " "x0, x1, " m11!()),
        Q!("    umulh           " "x1, x1, " m11!()),
        Q!("    adds            " "x5, x5, x0"),
        Q!("    str             " "x5, [" v!() "]"),
        Q!("    adc             " "x3, x3, x1"),

        // Digit 1 of [u,v]

        Q!("    ldr             " "x7, [" u!() "+ " N!() "]"),
        Q!("    eor             " "x1, x7, " s00!()),
        Q!("    mul             " "x0, x1, " m00!()),
        Q!("    umulh           " "x1, x1, " m00!()),
        Q!("    adds            " "x2, x2, x0"),
        Q!("    adc             " "x6, xzr, x1"),
        Q!("    ldr             " "x8, [" v!() "+ " N!() "]"),
        Q!("    eor             " "x1, x8, " s01!()),
        Q!("    mul             " "x0, x1, " m01!()),
        Q!("    umulh           " "x1, x1, " m01!()),
        Q!("    adds            " "x2, x2, x0"),
        Q!("    str             " "x2, [" u!() "+ " N!() "]"),
        Q!("    adc             " "x6, x6, x1"),

        Q!("    eor             " "x1, x7, " s10!()),
        Q!("    mul             " "x0, x1, " m10!()),
        Q!("    umulh           " "x1, x1, " m10!()),
        Q!("    adds            " "x3, x3, x0"),
        Q!("    adc             " "x4, xzr, x1"),
        Q!("    eor             " "x1, x8, " s11!()),
        Q!("    mul             " "x0, x1, " m11!()),
        Q!("    umulh           " "x1, x1, " m11!()),
        Q!("    adds            " "x3, x3, x0"),
        Q!("    str             " "x3, [" v!() "+ " N!() "]"),
        Q!("    adc             " "x4, x4, x1"),

        // Digit 2 of [u,v]

        Q!("    ldr             " "x7, [" u!() "+ 2 * " N!() "]"),
        Q!("    eor             " "x1, x7, " s00!()),
        Q!("    mul             " "x0, x1, " m00!()),
        Q!("    umulh           " "x1, x1, " m00!()),
        Q!("    adds            " "x6, x6, x0"),
        Q!("    adc             " "x5, xzr, x1"),
        Q!("    ldr             " "x8, [" v!() "+ 2 * " N!() "]"),
        Q!("    eor             " "x1, x8, " s01!()),
        Q!("    mul             " "x0, x1, " m01!()),
        Q!("    umulh           " "x1, x1, " m01!()),
        Q!("    adds            " "x6, x6, x0"),
        Q!("    str             " "x6, [" u!() "+ 2 * " N!() "]"),
        Q!("    adc             " "x5, x5, x1"),

        Q!("    eor             " "x1, x7, " s10!()),
        Q!("    mul             " "x0, x1, " m10!()),
        Q!("    umulh           " "x1, x1, " m10!()),
        Q!("    adds            " "x4, x4, x0"),
        Q!("    adc             " "x2, xzr, x1"),
        Q!("    eor             " "x1, x8, " s11!()),
        Q!("    mul             " "x0, x1, " m11!()),
        Q!("    umulh           " "x1, x1, " m11!()),
        Q!("    adds            " "x4, x4, x0"),
        Q!("    str             " "x4, [" v!() "+ 2 * " N!() "]"),
        Q!("    adc             " "x2, x2, x1"),

        // Digits 3 and 4 of u (top is unsigned)

        Q!("    ldr             " "x7, [" u!() "+ 3 * " N!() "]"),
        Q!("    eor             " "x1, x7, " s00!()),
        Q!("    and             " "x3, " s00!() ", " m00!()),
        Q!("    neg             " "x3, x3"),
        Q!("    mul             " "x0, x1, " m00!()),
        Q!("    umulh           " "x1, x1, " m00!()),
        Q!("    adds            " "x5, x5, x0"),
        Q!("    adc             " "x3, x3, x1"),
        Q!("    ldr             " "x8, [" v!() "+ 3 * " N!() "]"),
        Q!("    eor             " "x1, x8, " s01!()),
        Q!("    and             " "x0, " s01!() ", " m01!()),
        Q!("    sub             " "x3, x3, x0"),
        Q!("    mul             " "x0, x1, " m01!()),
        Q!("    umulh           " "x1, x1, " m01!()),
        Q!("    adds            " "x5, x5, x0"),
        Q!("    adc             " "x3, x3, x1"),

        // Montgomery reduction of u

        Q!("    ldp             " "x0, x1, [" u!() "]"),
        Q!("    ldr             " "x6, [" u!() "+ 2 * " N!() "]"),
        amontred!("x3", "x5", "x6", "x1", "x0", "x10", "x11", "x14"),
        Q!("    stp             " "x1, x6, [" u!() "]"),
        Q!("    stp             " "x5, x3, [" u!() "+ 16]"),

        // Digits 3 and 4 of v (top is unsigned)

        Q!("    eor             " "x1, x7, " s10!()),
        Q!("    and             " "x5, " s10!() ", " m10!()),
        Q!("    neg             " "x5, x5"),
        Q!("    mul             " "x0, x1, " m10!()),
        Q!("    umulh           " "x1, x1, " m10!()),
        Q!("    adds            " "x2, x2, x0"),
        Q!("    adc             " "x5, x5, x1"),
        Q!("    eor             " "x1, x8, " s11!()),
        Q!("    and             " "x0, " s11!() ", " m11!()),
        Q!("    sub             " "x5, x5, x0"),
        Q!("    mul             " "x0, x1, " m11!()),
        Q!("    umulh           " "x1, x1, " m11!()),
        Q!("    adds            " "x2, x2, x0"),
        Q!("    adc             " "x5, x5, x1"),

        // Montgomery reduction of v

        Q!("    ldp             " "x0, x1, [" v!() "]"),
        Q!("    ldr             " "x3, [" v!() "+ 2 * " N!() "]"),
        amontred!("x5", "x2", "x3", "x1", "x0", "x10", "x11", "x14"),
        Q!("    stp             " "x1, x3, [" v!() "]"),
        Q!("    stp             " "x2, x5, [" v!() "+ 16]"),

        Q!(Label!("Lbignum_inv_p256_midloop", 2) ":"),

        Q!("    mov             " "x1, " d!()),
        Q!("    ldr             " "x2, [" f!() "]"),
        Q!("    ldr             " "x3, [" g!() "]"),
        divstep59!(),
        Q!("    mov             " d!() ", x1"),

        // Next iteration

        Q!("    subs            " i!() ", " i!() ", #1"),
        Q!("    bne             " Label!("Lbignum_inv_p256_loop", 3, Before)),

        // The 10th and last iteration does not need anything except the
        // u value and the sign of f; the latter can be obtained from the
        // lowest word of f. So it's done differently from the main loop.
        // Find the sign of the new f. For this we just need one digit
        // since we know (for in-scope cases) that f is either +1 or -1.
        // We don't explicitly shift right by 59 either, but looking at
        // bit 63 (or any bit >= 60) of the unshifted result is enough
        // to distinguish -1 from +1; this is then made into a mask.

        Q!("    ldr             " "x0, [" f!() "]"),
        Q!("    ldr             " "x1, [" g!() "]"),
        Q!("    mul             " "x0, x0, " m00!()),
        Q!("    madd            " "x1, x1, " m01!() ", x0"),
        Q!("    asr             " "x0, x1, #63"),

        // Now separate out the matrix into sign-magnitude pairs
        // and adjust each one based on the sign of f.
        //
        // Note that at this point we expect |f|=1 and we got its
        // sign above, so then since [f,0] == x * [u,v] (mod p_256)
        // we want to flip the sign of u according to that of f.

        Q!("    cmp             " m00!() ", xzr"),
        Q!("    csetm           " s00!() ", mi"),
        Q!("    cneg            " m00!() ", " m00!() ", mi"),
        Q!("    eor             " s00!() ", " s00!() ", x0"),

        Q!("    cmp             " m01!() ", xzr"),
        Q!("    csetm           " s01!() ", mi"),
        Q!("    cneg            " m01!() ", " m01!() ", mi"),
        Q!("    eor             " s01!() ", " s01!() ", x0"),

        Q!("    cmp             " m10!() ", xzr"),
        Q!("    csetm           " s10!() ", mi"),
        Q!("    cneg            " m10!() ", " m10!() ", mi"),
        Q!("    eor             " s10!() ", " s10!() ", x0"),

        Q!("    cmp             " m11!() ", xzr"),
        Q!("    csetm           " s11!() ", mi"),
        Q!("    cneg            " m11!() ", " m11!() ", mi"),
        Q!("    eor             " s11!() ", " s11!() ", x0"),

        // Adjust the initial value to allow for complement instead of negation

        Q!("    and             " "x0, " m00!() ", " s00!()),
        Q!("    and             " "x1, " m01!() ", " s01!()),
        Q!("    add             " car0!() ", x0, x1"),

        // Digit 0 of [u]

        Q!("    ldr             " "x7, [" u!() "]"),
        Q!("    eor             " "x1, x7, " s00!()),
        Q!("    mul             " "x0, x1, " m00!()),
        Q!("    umulh           " "x1, x1, " m00!()),
        Q!("    adds            " "x4, " car0!() ", x0"),
        Q!("    adc             " "x2, xzr, x1"),
        Q!("    ldr             " "x8, [" v!() "]"),
        Q!("    eor             " "x1, x8, " s01!()),
        Q!("    mul             " "x0, x1, " m01!()),
        Q!("    umulh           " "x1, x1, " m01!()),
        Q!("    adds            " "x4, x4, x0"),
        Q!("    str             " "x4, [" u!() "]"),
        Q!("    adc             " "x2, x2, x1"),

        // Digit 1 of [u]

        Q!("    ldr             " "x7, [" u!() "+ " N!() "]"),
        Q!("    eor             " "x1, x7, " s00!()),
        Q!("    mul             " "x0, x1, " m00!()),
        Q!("    umulh           " "x1, x1, " m00!()),
        Q!("    adds            " "x2, x2, x0"),
        Q!("    adc             " "x6, xzr, x1"),
        Q!("    ldr             " "x8, [" v!() "+ " N!() "]"),
        Q!("    eor             " "x1, x8, " s01!()),
        Q!("    mul             " "x0, x1, " m01!()),
        Q!("    umulh           " "x1, x1, " m01!()),
        Q!("    adds            " "x2, x2, x0"),
        Q!("    str             " "x2, [" u!() "+ " N!() "]"),
        Q!("    adc             " "x6, x6, x1"),

        // Digit 2 of [u]

        Q!("    ldr             " "x7, [" u!() "+ 2 * " N!() "]"),
        Q!("    eor             " "x1, x7, " s00!()),
        Q!("    mul             " "x0, x1, " m00!()),
        Q!("    umulh           " "x1, x1, " m00!()),
        Q!("    adds            " "x6, x6, x0"),
        Q!("    adc             " "x5, xzr, x1"),
        Q!("    ldr             " "x8, [" v!() "+ 2 * " N!() "]"),
        Q!("    eor             " "x1, x8, " s01!()),
        Q!("    mul             " "x0, x1, " m01!()),
        Q!("    umulh           " "x1, x1, " m01!()),
        Q!("    adds            " "x6, x6, x0"),
        Q!("    str             " "x6, [" u!() "+ 2 * " N!() "]"),
        Q!("    adc             " "x5, x5, x1"),

        // Digits 3 and 4 of u (top is unsigned)

        Q!("    ldr             " "x7, [" u!() "+ 3 * " N!() "]"),
        Q!("    eor             " "x1, x7, " s00!()),
        Q!("    and             " "x3, " s00!() ", " m00!()),
        Q!("    neg             " "x3, x3"),
        Q!("    mul             " "x0, x1, " m00!()),
        Q!("    umulh           " "x1, x1, " m00!()),
        Q!("    adds            " "x5, x5, x0"),
        Q!("    adc             " "x3, x3, x1"),
        Q!("    ldr             " "x8, [" v!() "+ 3 * " N!() "]"),
        Q!("    eor             " "x1, x8, " s01!()),
        Q!("    and             " "x0, " s01!() ", " m01!()),
        Q!("    sub             " "x3, x3, x0"),
        Q!("    mul             " "x0, x1, " m01!()),
        Q!("    umulh           " "x1, x1, " m01!()),
        Q!("    adds            " "x5, x5, x0"),
        Q!("    adc             " "x3, x3, x1"),

        // Montgomery reduction of u. This needs to be strict not "almost"
        // so it is followed by an optional subtraction of p_256

        Q!("    ldp             " "x0, x1, [" u!() "]"),
        Q!("    ldr             " "x2, [" u!() "+ 2 * " N!() "]"),
        amontred!("x3", "x5", "x2", "x1", "x0", "x10", "x11", "x14"),

        Q!("    mov             " "x10, #0xffffffffffffffff"),
        Q!("    subs            " "x10, x1, x10"),
        Q!("    mov             " "x11, #0x00000000ffffffff"),
        Q!("    sbcs            " "x11, x2, x11"),
        Q!("    mov             " "x13, #0xffffffff00000001"),
        Q!("    sbcs            " "x12, x5, xzr"),
        Q!("    sbcs            " "x13, x3, x13"),

        Q!("    csel            " "x10, x1, x10, cc"),
        Q!("    csel            " "x11, x2, x11, cc"),
        Q!("    csel            " "x12, x5, x12, cc"),
        Q!("    csel            " "x13, x3, x13, cc"),

        // Store it back to the final output

        Q!("    stp             " "x10, x11, [" res!() "]"),
        Q!("    stp             " "x12, x13, [" res!() ", #16]"),

        // Restore stack and registers

        Q!("    add             " "sp, sp, # (" NSPACE!() "+ 0)"),
        Q!("    ldp             " "x23, x24, [sp], #16"),
        Q!("    ldp             " "x21, x22, [sp], #16"),
        Q!("    ldp             " "x19, x20, [sp], #16"),
        inout("x0") z.as_mut_ptr() => _,
        inout("x1") x.as_ptr() => _,
        // clobbers
        out("v0") _,
        out("v1") _,
        out("v2") _,
        out("v3") _,
        out("v4") _,
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
