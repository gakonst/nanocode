// generated source. do not edit.
#![allow(non_upper_case_globals, unused_macros, unused_imports)]
use crate::low::macros::*;

// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

// ----------------------------------------------------------------------------
// Modular inverse modulo p_384 = 2^384 - 2^128 - 2^96 + 2^32 - 1
// Input x[6]; output z[6]
//
// extern void bignum_inv_p384(uint64_t z[static 6],const uint64_t x[static 6]);
//
// If the 6-digit input x is coprime to p_384, i.e. is not divisible
// by it, returns z < p_384 such that x * z == 1 (mod p_384). Note that
// x does not need to be reduced modulo p_384, but the output always is.
// If the input is divisible (i.e. is 0 or p_384), then there can be no
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
// The u and v variables are 6 words each as expected, but the f and g
// variables are 8 words each -- they need to have at least one extra
// word for a sign word, and to preserve alignment we "round up" to 8.
// In fact, we currently keep an extra word in u and v as well.

macro_rules! f {
    () => {
        "sp, #0"
    };
}
macro_rules! g { () => { Q!("sp, # (8 * " N!() ")") } }
macro_rules! u { () => { Q!("sp, # (16 * " N!() ")") } }
macro_rules! v { () => { Q!("sp, # (24 * " N!() ")") } }

// Total size to reserve on the stack

macro_rules! NSPACE { () => { Q!("32 * " N!()) } }

// ---------------------------------------------------------------------------
// Core signed almost-Montgomery reduction macro. Takes input in
// [d6;d5;d4;d3;d2;d1;d0] and returns result in [d6;d5d4;d3;d2;d1], adding
// to the existing [d6;d5;d4;d3;d2;d1], and re-using d0 as a temporary
// internally as well as t0, t1, t2. This is almost-Montgomery, i.e. the
// result fits in 6 digits but is not necessarily strictly reduced mod p_384.
// ---------------------------------------------------------------------------

macro_rules! amontred {
    ($d6:expr, $d5:expr, $d4:expr, $d3:expr, $d2:expr, $d1:expr, $d0:expr, $t3:expr, $t2:expr, $t1:expr) => { Q!(
        /* We only know the input is -2^444 < x < 2^444. To do traditional  */
        /* unsigned Montgomery reduction, start by adding 2^61 * p_384.     */
        "mov " $t1 ", #0xe000000000000000;\n"
        "adds " $d0 ", " $d0 ", " $t1 ";\n"
        "mov " $t2 ", #0x000000001fffffff;\n"
        "adcs " $d1 ", " $d1 ", " $t2 ";\n"
        "mov " $t3 ", #0xffffffffe0000000;\n"
        "bic " $t3 ", " $t3 ", #0x2000000000000000;\n"
        "adcs " $d2 ", " $d2 ", " $t3 ";\n"
        "sbcs " $d3 ", " $d3 ", xzr;\n"
        "sbcs " $d4 ", " $d4 ", xzr;\n"
        "sbcs " $d5 ", " $d5 ", xzr;\n"
        "mov " $t1 ", #0x1fffffffffffffff;\n"
        "adc " $d6 ", " $d6 ", " $t1 ";\n"
        /* Our correction multiplier is w = [d0 + (d0<<32)] mod 2^64  */
        /* Store it back into d0 since we no longer need that digit.  */
        "add " $d0 ", " $d0 ", " $d0 ", lsl #32;\n"
        /* Now let [t3;t2;t1;-] = (2^384 - p_384) * w                 */
        /* We know the lowest word will cancel d0 so we don't need it */
        "mov " $t1 ", #0xffffffff00000001;\n"
        "umulh " $t1 ", " $t1 ", " $d0 ";\n"
        "mov " $t2 ", #0x00000000ffffffff;\n"
        "mul " $t3 ", " $t2 ", " $d0 ";\n"
        "umulh " $t2 ", " $t2 ", " $d0 ";\n"
        "adds " $t1 ", " $t1 ", " $t3 ";\n"
        "adcs " $t2 ", " $t2 ", " $d0 ";\n"
        "cset " $t3 ", cs;\n"
        /* Now x + p_384 * w = (x + 2^384 * w) - (2^384 - p_384) * w */
        /* We catch the net top carry from add-subtract in the digit d0 */
        "adds " $d6 ", " $d6 ", " $d0 ";\n"
        "cset " $d0 ", cs;\n"
        "subs " $d1 ", " $d1 ", " $t1 ";\n"
        "sbcs " $d2 ", " $d2 ", " $t2 ";\n"
        "sbcs " $d3 ", " $d3 ", " $t3 ";\n"
        "sbcs " $d4 ", " $d4 ", xzr;\n"
        "sbcs " $d5 ", " $d5 ", xzr;\n"
        "sbcs " $d6 ", " $d6 ", xzr;\n"
        "sbcs " $d0 ", " $d0 ", xzr;\n"
        /* Now if d0 is nonzero we subtract p_384 (almost-Montgomery) */
        "neg " $d0 ", " $d0 ";\n"
        "and " $t1 ", " $d0 ", #0x00000000ffffffff;\n"
        "and " $t2 ", " $d0 ", #0xffffffff00000000;\n"
        "and " $t3 ", " $d0 ", #0xfffffffffffffffe;\n"
        "subs " $d1 ", " $d1 ", " $t1 ";\n"
        "sbcs " $d2 ", " $d2 ", " $t2 ";\n"
        "sbcs " $d3 ", " $d3 ", " $t3 ";\n"
        "sbcs " $d4 ", " $d4 ", " $d0 ";\n"
        "sbcs " $d5 ", " $d5 ", " $d0 ";\n"
        "sbc " $d6 ", " $d6 ", " $d0
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

/// Modular inverse modulo p_384 = 2^384 - 2^128 - 2^96 + 2^32 - 1
///
/// Input x[6]; output z[6]
///
/// If the 6-digit input x is coprime to p_384, i.e. is not divisible
/// by it, returns z < p_384 such that x * z == 1 (mod p_384). Note that
/// x does not need to be reduced modulo p_384, but the output always is.
/// If the input is divisible (i.e. is 0 or p_384), then there can be no
/// modular inverse and z = 0 is returned.
pub(crate) fn bignum_inv_p384(z: &mut [u64; 6], x: &[u64; 6]) {
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

        Q!("    mov             " "x10, #0x00000000ffffffff"),
        Q!("    mov             " "x11, #0xffffffff00000000"),
        Q!("    mov             " "x12, #0xfffffffffffffffe"),
        Q!("    mov             " "x15, #0xffffffffffffffff"),
        Q!("    stp             " "x10, x11, [" f!() "]"),
        Q!("    stp             " "x12, x15, [" f!() "+ 2 * " N!() "]"),
        Q!("    stp             " "x15, x15, [" f!() "+ 4 * " N!() "]"),
        Q!("    str             " "xzr, [" f!() "+ 6 * " N!() "]"),

        Q!("    ldp             " "x2, x3, [x1]"),
        Q!("    subs            " "x10, x2, x10"),
        Q!("    sbcs            " "x11, x3, x11"),
        Q!("    ldp             " "x4, x5, [x1, # (2 * " N!() ")]"),
        Q!("    sbcs            " "x12, x4, x12"),
        Q!("    sbcs            " "x13, x5, x15"),
        Q!("    ldp             " "x6, x7, [x1, # (4 * " N!() ")]"),
        Q!("    sbcs            " "x14, x6, x15"),
        Q!("    sbcs            " "x15, x7, x15"),

        Q!("    csel            " "x2, x2, x10, cc"),
        Q!("    csel            " "x3, x3, x11, cc"),
        Q!("    csel            " "x4, x4, x12, cc"),
        Q!("    csel            " "x5, x5, x13, cc"),
        Q!("    csel            " "x6, x6, x14, cc"),
        Q!("    csel            " "x7, x7, x15, cc"),

        Q!("    stp             " "x2, x3, [" g!() "]"),
        Q!("    stp             " "x4, x5, [" g!() "+ 2 * " N!() "]"),
        Q!("    stp             " "x6, x7, [" g!() "+ 4 * " N!() "]"),
        Q!("    str             " "xzr, [" g!() "+ 6 * " N!() "]"),

        // Also maintain reduced < 2^384 vector [u,v] such that
        // [f,g] == x * 2^{5*i-75} * [u,v] (mod p_384)
        // starting with [p_384,x] == x * 2^{5*0-75} * [0,2^75] (mod p_384)
        // The weird-looking 5*i modifications come in because we are doing
        // 64-bit word-sized Montgomery reductions at each stage, which is
        // 5 bits more than the 59-bit requirement to keep things stable.

        Q!("    stp             " "xzr, xzr, [" u!() "]"),
        Q!("    stp             " "xzr, xzr, [" u!() "+ 2 * " N!() "]"),
        Q!("    stp             " "xzr, xzr, [" u!() "+ 4 * " N!() "]"),

        Q!("    mov             " "x10, #2048"),
        Q!("    stp             " "xzr, x10, [" v!() "]"),
        Q!("    stp             " "xzr, xzr, [" v!() "+ 2 * " N!() "]"),
        Q!("    stp             " "xzr, xzr, [" v!() "+ 4 * " N!() "]"),

        // Start of main loop. We jump into the middle so that the divstep
        // portion is common to the special fifteenth iteration after a uniform
        // first 14.

        Q!("    mov             " i!() ", #15"),
        Q!("    mov             " d!() ", #1"),
        Q!("    b               " Label!("Lbignum_inv_p384_midloop", 2, After)),

        Q!(Label!("Lbignum_inv_p384_loop", 3) ":"),

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

        // Digit 3 of [f,g]

        Q!("    ldr             " "x7, [" f!() "+ 3 * " N!() "]"),
        Q!("    eor             " "x1, x7, " s00!()),
        Q!("    mul             " "x0, x1, " m00!()),
        Q!("    umulh           " "x1, x1, " m00!()),
        Q!("    adds            " "x5, x5, x0"),
        Q!("    adc             " "x3, xzr, x1"),
        Q!("    ldr             " "x8, [" g!() "+ 3 * " N!() "]"),
        Q!("    eor             " "x1, x8, " s01!()),
        Q!("    mul             " "x0, x1, " m01!()),
        Q!("    umulh           " "x1, x1, " m01!()),
        Q!("    adds            " "x5, x5, x0"),
        Q!("    adc             " "x3, x3, x1"),
        Q!("    extr            " "x6, x5, x6, #59"),
        Q!("    str             " "x6, [" f!() "+ 2 * " N!() "]"),

        Q!("    eor             " "x1, x7, " s10!()),
        Q!("    mul             " "x0, x1, " m10!()),
        Q!("    umulh           " "x1, x1, " m10!()),
        Q!("    adds            " "x2, x2, x0"),
        Q!("    adc             " "x6, xzr, x1"),
        Q!("    eor             " "x1, x8, " s11!()),
        Q!("    mul             " "x0, x1, " m11!()),
        Q!("    umulh           " "x1, x1, " m11!()),
        Q!("    adds            " "x2, x2, x0"),
        Q!("    adc             " "x6, x6, x1"),
        Q!("    extr            " "x4, x2, x4, #59"),
        Q!("    str             " "x4, [" g!() "+ 2 * " N!() "]"),

        // Digit 4 of [f,g]

        Q!("    ldr             " "x7, [" f!() "+ 4 * " N!() "]"),
        Q!("    eor             " "x1, x7, " s00!()),
        Q!("    mul             " "x0, x1, " m00!()),
        Q!("    umulh           " "x1, x1, " m00!()),
        Q!("    adds            " "x3, x3, x0"),
        Q!("    adc             " "x4, xzr, x1"),
        Q!("    ldr             " "x8, [" g!() "+ 4 * " N!() "]"),
        Q!("    eor             " "x1, x8, " s01!()),
        Q!("    mul             " "x0, x1, " m01!()),
        Q!("    umulh           " "x1, x1, " m01!()),
        Q!("    adds            " "x3, x3, x0"),
        Q!("    adc             " "x4, x4, x1"),
        Q!("    extr            " "x5, x3, x5, #59"),
        Q!("    str             " "x5, [" f!() "+ 3 * " N!() "]"),

        Q!("    eor             " "x1, x7, " s10!()),
        Q!("    mul             " "x0, x1, " m10!()),
        Q!("    umulh           " "x1, x1, " m10!()),
        Q!("    adds            " "x6, x6, x0"),
        Q!("    adc             " "x5, xzr, x1"),
        Q!("    eor             " "x1, x8, " s11!()),
        Q!("    mul             " "x0, x1, " m11!()),
        Q!("    umulh           " "x1, x1, " m11!()),
        Q!("    adds            " "x6, x6, x0"),
        Q!("    adc             " "x5, x5, x1"),
        Q!("    extr            " "x2, x6, x2, #59"),
        Q!("    str             " "x2, [" g!() "+ 3 * " N!() "]"),

        // Digits 5 and 6 of [f,g]

        Q!("    ldr             " "x7, [" f!() "+ 5 * " N!() "]"),
        Q!("    eor             " "x1, x7, " s00!()),
        Q!("    ldr             " "x23, [" f!() "+ 6 * " N!() "]"),
        Q!("    eor             " "x2, x23, " s00!()),
        Q!("    and             " "x2, x2, " m00!()),
        Q!("    neg             " "x2, x2"),
        Q!("    mul             " "x0, x1, " m00!()),
        Q!("    umulh           " "x1, x1, " m00!()),
        Q!("    adds            " "x4, x4, x0"),
        Q!("    adc             " "x2, x2, x1"),
        Q!("    ldr             " "x8, [" g!() "+ 5 * " N!() "]"),
        Q!("    eor             " "x1, x8, " s01!()),
        Q!("    ldr             " "x24, [" g!() "+ 6 * " N!() "]"),
        Q!("    eor             " "x0, x24, " s01!()),
        Q!("    and             " "x0, x0, " m01!()),
        Q!("    sub             " "x2, x2, x0"),
        Q!("    mul             " "x0, x1, " m01!()),
        Q!("    umulh           " "x1, x1, " m01!()),
        Q!("    adds            " "x4, x4, x0"),
        Q!("    adc             " "x2, x2, x1"),
        Q!("    extr            " "x3, x4, x3, #59"),
        Q!("    str             " "x3, [" f!() "+ 4 * " N!() "]"),
        Q!("    extr            " "x4, x2, x4, #59"),
        Q!("    str             " "x4, [" f!() "+ 5 * " N!() "]"),
        Q!("    asr             " "x2, x2, #59"),
        Q!("    str             " "x2, [" f!() "+ 6 * " N!() "]"),

        Q!("    eor             " "x1, x7, " s10!()),
        Q!("    eor             " "x4, x23, " s10!()),
        Q!("    and             " "x4, x4, " m10!()),
        Q!("    neg             " "x4, x4"),
        Q!("    mul             " "x0, x1, " m10!()),
        Q!("    umulh           " "x1, x1, " m10!()),
        Q!("    adds            " "x5, x5, x0"),
        Q!("    adc             " "x4, x4, x1"),
        Q!("    eor             " "x1, x8, " s11!()),
        Q!("    eor             " "x0, x24, " s11!()),
        Q!("    and             " "x0, x0, " m11!()),
        Q!("    sub             " "x4, x4, x0"),
        Q!("    mul             " "x0, x1, " m11!()),
        Q!("    umulh           " "x1, x1, " m11!()),
        Q!("    adds            " "x5, x5, x0"),
        Q!("    adc             " "x4, x4, x1"),
        Q!("    extr            " "x6, x5, x6, #59"),
        Q!("    str             " "x6, [" g!() "+ 4 * " N!() "]"),
        Q!("    extr            " "x5, x4, x5, #59"),
        Q!("    str             " "x5, [" g!() "+ 5 * " N!() "]"),
        Q!("    asr             " "x4, x4, #59"),
        Q!("    str             " "x4, [" g!() "+ 6 * " N!() "]"),

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

        // Digit 3 of [u,v]

        Q!("    ldr             " "x7, [" u!() "+ 3 * " N!() "]"),
        Q!("    eor             " "x1, x7, " s00!()),
        Q!("    mul             " "x0, x1, " m00!()),
        Q!("    umulh           " "x1, x1, " m00!()),
        Q!("    adds            " "x5, x5, x0"),
        Q!("    adc             " "x3, xzr, x1"),
        Q!("    ldr             " "x8, [" v!() "+ 3 * " N!() "]"),
        Q!("    eor             " "x1, x8, " s01!()),
        Q!("    mul             " "x0, x1, " m01!()),
        Q!("    umulh           " "x1, x1, " m01!()),
        Q!("    adds            " "x5, x5, x0"),
        Q!("    str             " "x5, [" u!() "+ 3 * " N!() "]"),
        Q!("    adc             " "x3, x3, x1"),

        Q!("    eor             " "x1, x7, " s10!()),
        Q!("    mul             " "x0, x1, " m10!()),
        Q!("    umulh           " "x1, x1, " m10!()),
        Q!("    adds            " "x2, x2, x0"),
        Q!("    adc             " "x6, xzr, x1"),
        Q!("    eor             " "x1, x8, " s11!()),
        Q!("    mul             " "x0, x1, " m11!()),
        Q!("    umulh           " "x1, x1, " m11!()),
        Q!("    adds            " "x2, x2, x0"),
        Q!("    str             " "x2, [" v!() "+ 3 * " N!() "]"),
        Q!("    adc             " "x6, x6, x1"),

        // Digit 4 of [u,v]

        Q!("    ldr             " "x7, [" u!() "+ 4 * " N!() "]"),
        Q!("    eor             " "x1, x7, " s00!()),
        Q!("    mul             " "x0, x1, " m00!()),
        Q!("    umulh           " "x1, x1, " m00!()),
        Q!("    adds            " "x3, x3, x0"),
        Q!("    adc             " "x4, xzr, x1"),
        Q!("    ldr             " "x8, [" v!() "+ 4 * " N!() "]"),
        Q!("    eor             " "x1, x8, " s01!()),
        Q!("    mul             " "x0, x1, " m01!()),
        Q!("    umulh           " "x1, x1, " m01!()),
        Q!("    adds            " "x3, x3, x0"),
        Q!("    str             " "x3, [" u!() "+ 4 * " N!() "]"),
        Q!("    adc             " "x4, x4, x1"),

        Q!("    eor             " "x1, x7, " s10!()),
        Q!("    mul             " "x0, x1, " m10!()),
        Q!("    umulh           " "x1, x1, " m10!()),
        Q!("    adds            " "x6, x6, x0"),
        Q!("    adc             " "x5, xzr, x1"),
        Q!("    eor             " "x1, x8, " s11!()),
        Q!("    mul             " "x0, x1, " m11!()),
        Q!("    umulh           " "x1, x1, " m11!()),
        Q!("    adds            " "x6, x6, x0"),
        Q!("    str             " "x6, [" v!() "+ 4 * " N!() "]"),
        Q!("    adc             " "x5, x5, x1"),

        // Digits 5 and 6 of [u,v] (top is unsigned)

        Q!("    ldr             " "x7, [" u!() "+ 5 * " N!() "]"),
        Q!("    eor             " "x1, x7, " s00!()),
        Q!("    and             " "x2, " s00!() ", " m00!()),
        Q!("    neg             " "x2, x2"),
        Q!("    mul             " "x0, x1, " m00!()),
        Q!("    umulh           " "x1, x1, " m00!()),
        Q!("    adds            " "x4, x4, x0"),
        Q!("    adc             " "x2, x2, x1"),
        Q!("    ldr             " "x8, [" v!() "+ 5 * " N!() "]"),
        Q!("    eor             " "x1, x8, " s01!()),
        Q!("    and             " "x0, " s01!() ", " m01!()),
        Q!("    sub             " "x2, x2, x0"),
        Q!("    mul             " "x0, x1, " m01!()),
        Q!("    umulh           " "x1, x1, " m01!()),
        Q!("    adds            " "x4, x4, x0"),
        Q!("    str             " "x4, [" u!() "+ 5 * " N!() "]"),
        Q!("    adc             " "x2, x2, x1"),
        Q!("    str             " "x2, [" u!() "+ 6 * " N!() "]"),

        Q!("    eor             " "x1, x7, " s10!()),
        Q!("    and             " "x4, " s10!() ", " m10!()),
        Q!("    neg             " "x4, x4"),
        Q!("    mul             " "x0, x1, " m10!()),
        Q!("    umulh           " "x1, x1, " m10!()),
        Q!("    adds            " "x5, x5, x0"),
        Q!("    adc             " "x4, x4, x1"),
        Q!("    eor             " "x1, x8, " s11!()),
        Q!("    and             " "x0, " s11!() ", " m11!()),
        Q!("    sub             " "x4, x4, x0"),
        Q!("    mul             " "x0, x1, " m11!()),
        Q!("    umulh           " "x1, x1, " m11!()),
        Q!("    adds            " "x5, x5, x0"),
        Q!("    str             " "x5, [" v!() "+ 5 * " N!() "]"),
        Q!("    adc             " "x4, x4, x1"),
        Q!("    str             " "x4, [" v!() "+ 6 * " N!() "]"),

        // Montgomery reduction of u

        Q!("    ldp             " "x0, x1, [" u!() "]"),
        Q!("    ldp             " "x2, x3, [" u!() "+ 16]"),
        Q!("    ldp             " "x4, x5, [" u!() "+ 32]"),
        Q!("    ldr             " "x6, [" u!() "+ 48]"),
        amontred!("x6", "x5", "x4", "x3", "x2", "x1", "x0", "x9", "x8", "x7"),
        Q!("    stp             " "x1, x2, [" u!() "]"),
        Q!("    stp             " "x3, x4, [" u!() "+ 16]"),
        Q!("    stp             " "x5, x6, [" u!() "+ 32]"),

        // Montgomery reduction of v

        Q!("    ldp             " "x0, x1, [" v!() "]"),
        Q!("    ldp             " "x2, x3, [" v!() "+ 16]"),
        Q!("    ldp             " "x4, x5, [" v!() "+ 32]"),
        Q!("    ldr             " "x6, [" v!() "+ 48]"),
        amontred!("x6", "x5", "x4", "x3", "x2", "x1", "x0", "x9", "x8", "x7"),
        Q!("    stp             " "x1, x2, [" v!() "]"),
        Q!("    stp             " "x3, x4, [" v!() "+ 16]"),
        Q!("    stp             " "x5, x6, [" v!() "+ 32]"),

        Q!(Label!("Lbignum_inv_p384_midloop", 2) ":"),

        Q!("    mov             " "x1, " d!()),
        Q!("    ldr             " "x2, [" f!() "]"),
        Q!("    ldr             " "x3, [" g!() "]"),
        divstep59!(),
        Q!("    mov             " d!() ", x1"),

        // Next iteration

        Q!("    subs            " i!() ", " i!() ", #1"),
        Q!("    bne             " Label!("Lbignum_inv_p384_loop", 3, Before)),

        // The 15th and last iteration does not need anything except the
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
        // sign above, so then since [f,0] == x * [u,v] (mod p_384)
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

        // Digit 3 of [u]

        Q!("    ldr             " "x7, [" u!() "+ 3 * " N!() "]"),
        Q!("    eor             " "x1, x7, " s00!()),
        Q!("    mul             " "x0, x1, " m00!()),
        Q!("    umulh           " "x1, x1, " m00!()),
        Q!("    adds            " "x5, x5, x0"),
        Q!("    adc             " "x3, xzr, x1"),
        Q!("    ldr             " "x8, [" v!() "+ 3 * " N!() "]"),
        Q!("    eor             " "x1, x8, " s01!()),
        Q!("    mul             " "x0, x1, " m01!()),
        Q!("    umulh           " "x1, x1, " m01!()),
        Q!("    adds            " "x5, x5, x0"),
        Q!("    str             " "x5, [" u!() "+ 3 * " N!() "]"),
        Q!("    adc             " "x3, x3, x1"),

        // Digit 4 of [u]

        Q!("    ldr             " "x7, [" u!() "+ 4 * " N!() "]"),
        Q!("    eor             " "x1, x7, " s00!()),
        Q!("    mul             " "x0, x1, " m00!()),
        Q!("    umulh           " "x1, x1, " m00!()),
        Q!("    adds            " "x3, x3, x0"),
        Q!("    adc             " "x4, xzr, x1"),
        Q!("    ldr             " "x8, [" v!() "+ 4 * " N!() "]"),
        Q!("    eor             " "x1, x8, " s01!()),
        Q!("    mul             " "x0, x1, " m01!()),
        Q!("    umulh           " "x1, x1, " m01!()),
        Q!("    adds            " "x3, x3, x0"),
        Q!("    str             " "x3, [" u!() "+ 4 * " N!() "]"),
        Q!("    adc             " "x4, x4, x1"),

        // Digits 5 and 6 of [u] (top is unsigned)

        Q!("    ldr             " "x7, [" u!() "+ 5 * " N!() "]"),
        Q!("    eor             " "x1, x7, " s00!()),
        Q!("    and             " "x2, " s00!() ", " m00!()),
        Q!("    neg             " "x2, x2"),
        Q!("    mul             " "x0, x1, " m00!()),
        Q!("    umulh           " "x1, x1, " m00!()),
        Q!("    adds            " "x4, x4, x0"),
        Q!("    adc             " "x2, x2, x1"),
        Q!("    ldr             " "x8, [" v!() "+ 5 * " N!() "]"),
        Q!("    eor             " "x1, x8, " s01!()),
        Q!("    and             " "x0, " s01!() ", " m01!()),
        Q!("    sub             " "x2, x2, x0"),
        Q!("    mul             " "x0, x1, " m01!()),
        Q!("    umulh           " "x1, x1, " m01!()),
        Q!("    adds            " "x4, x4, x0"),
        Q!("    str             " "x4, [" u!() "+ 5 * " N!() "]"),
        Q!("    adc             " "x2, x2, x1"),
        Q!("    str             " "x2, [" u!() "+ 6 * " N!() "]"),

        // Montgomery reduction of u. This needs to be strict not "almost"
        // so it is followed by an optional subtraction of p_384

        Q!("    ldp             " "x10, x0, [" u!() "]"),
        Q!("    ldp             " "x1, x2, [" u!() "+ 16]"),
        Q!("    ldp             " "x3, x4, [" u!() "+ 32]"),
        Q!("    ldr             " "x5, [" u!() "+ 48]"),
        amontred!("x5", "x4", "x3", "x2", "x1", "x0", "x10", "x9", "x8", "x7"),

        Q!("    mov             " "x10, #0x00000000ffffffff"),
        Q!("    subs            " "x10, x0, x10"),
        Q!("    mov             " "x11, #0xffffffff00000000"),
        Q!("    sbcs            " "x11, x1, x11"),
        Q!("    mov             " "x12, #0xfffffffffffffffe"),
        Q!("    sbcs            " "x12, x2, x12"),
        Q!("    mov             " "x15, #0xffffffffffffffff"),
        Q!("    sbcs            " "x13, x3, x15"),
        Q!("    sbcs            " "x14, x4, x15"),
        Q!("    sbcs            " "x15, x5, x15"),

        Q!("    csel            " "x0, x0, x10, cc"),
        Q!("    csel            " "x1, x1, x11, cc"),
        Q!("    csel            " "x2, x2, x12, cc"),
        Q!("    csel            " "x3, x3, x13, cc"),
        Q!("    csel            " "x4, x4, x14, cc"),
        Q!("    csel            " "x5, x5, x15, cc"),

        // Store it back to the final output

        Q!("    stp             " "x0, x1, [" res!() "]"),
        Q!("    stp             " "x2, x3, [" res!() ", #16]"),
        Q!("    stp             " "x4, x5, [" res!() ", #32]"),

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
        out("v5") _,
        out("v6") _,
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
