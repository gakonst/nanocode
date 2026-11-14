// generated source. do not edit.
#![allow(non_upper_case_globals, unused_macros, unused_imports)]
use crate::low::macros::*;

// Copyright (c) 2024 The mlkem-native project authors
// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT

// ----------------------------------------------------------------------------
// Uniform rejection sampling for ML-KEM
// Inputs *buf (unsigned bytes), buflen, table (unsigned bytes); output r[256] (signed 16-bit words), return
//
// extern uint64_t mlkem_rej_uniform_VARIABLE_TIME
//                     (int16_t r[S2N_BIGNUM_STATIC 256],
//                      const uint8_t *buf,uint64_t buflen,
//                      const uint8_t *table);
//
// Interprets the input buffer as packed 12-bit numbers with a length of
// buflen bytes, assumed to be a multiple of 24. Fills the output array
// with those numbers from the packed buffer that are < 3329, in the order
// of appearance, returning the total number of entries written, with a
// maximum of 256. The table argument is a specific precomputed table of
// constants that is defined in this file (see also our test code):
//
//   https://github.com/pq-code-package/mlkem-native/blob/main/mlkem/native/aarch64/src/rej_uniform_table.c
//
// Unique (at the moment) among s2n-bignum functions this is *not* a
// constant-time function. The time taken depends not only on the
// buffer size "buflen", but also how many elements of the buffer are
// needed to provide the 256 entries for the output.
//
// Standard ARM ABI: X0 = buf, X1 = r, X2 = buflen, X3 = table
// ----------------------------------------------------------------------------

/// Uniform rejection sampling for ML-KEM
///
/// Inputs *buf (unsigned bytes), buflen, table (unsigned bytes); output r[256] (signed 16-bit words), return
///
/// Interprets the input buffer as packed 12-bit numbers with a length of
/// buflen bytes, assumed to be a multiple of 24. Fills the output array
/// with those numbers from the packed buffer that are < 3329, in the order
/// of appearance, returning the total number of entries written, with a
/// maximum of 256. The table argument is a specific precomputed table of
/// constants that is defined in this file (see also our test code):
///
///   https://github.com/pq-code-package/mlkem-native/blob/main/mlkem/native/aarch64/src/rej_uniform_table.c
///
/// Unique (at the moment) among s2n-bignum functions this is *not* a
/// constant-time function. The time taken depends not only on the
/// buffer size "buflen", but also how many elements of the buffer are
/// needed to provide the 256 entries for the output.
pub(crate) fn mlkem_rej_uniform_vartime(r: &mut [i16; 256], input: &[u8], table: &[i8]) -> u64 {
    let ret: u64;
    // SAFETY: inline assembly. see [crate::low::inline_assembly_safety] for safety info.
    unsafe {
        core::arch::asm!(

        // This is almost identical to the code from mlkem-native:
        //
        //   https://github.com/pq-code-package/mlkem-native/blob/main/mlkem/native/aarch64/src/rej_uniform_asm.S
        //
        // The only difference is systematic use of full-length scalar registers
        // Xnn instead of the mixed use of 32-bit counterparts Wnn in most
        // settings where that is applicable.

        Q!("    sub             " "sp, sp, # (576 + 0)"),
        Q!("    mov             " "x7, #0x1"),
        Q!("    movk            " "x7, #0x2, lsl #16"),
        Q!("    movk            " "x7, #0x4, lsl #32"),
        Q!("    movk            " "x7, #0x8, lsl #48"),
        Q!("    mov             " "v31.d[0], x7"),
        Q!("    mov             " "x7, #0x10"),
        Q!("    movk            " "x7, #0x20, lsl #16"),
        Q!("    movk            " "x7, #0x40, lsl #32"),
        Q!("    movk            " "x7, #0x80, lsl #48"),
        Q!("    mov             " "v31.d[1], x7"),
        Q!("    mov             " "w11, #0xd01"),
        Q!("    dup             " "v30.8h, w11"),
        Q!("    mov             " "x8, sp"),
        Q!("    mov             " "x7, x8"),
        Q!("    mov             " "w11, #0x0"),
        Q!("    eor             " "v16.16b, v16.16b, v16.16b"),
        Q!(Label!("Lmlkem_rej_uniform_initial_zero", 2) ":"),
        Q!("    str             " "q16, [x7], #0x40"),
        Q!("    stur            " "q16, [x7, #-0x30]"),
        Q!("    stur            " "q16, [x7, #-0x20]"),
        Q!("    stur            " "q16, [x7, #-0x10]"),
        Q!("    add             " "x11, x11, #0x20"),
        Q!("    cmp             " "x11, #0x100"),
        Q!("    b.lt            " Label!("Lmlkem_rej_uniform_initial_zero", 2, Before)),
        Q!("    mov             " "x7, x8"),
        Q!("    mov             " "w9, #0x0"),
        Q!("    mov             " "w4, #0x100"),
        Q!("    cmp             " "x2, #0x30"),
        Q!("    b.lo            " Label!("Lmlkem_rej_uniform_loop48_end", 3, After)),

        Q!(Label!("Lmlkem_rej_uniform_loop48", 4) ":"),
        Q!("    cmp             " "x9, x4"),
        Q!("    b.hs            " Label!("Lmlkem_rej_uniform_memory_copy", 5, After)),
        Q!("    sub             " "x2, x2, #0x30"),
        Q!("    ld3             " "{{ v0.16b, v1.16b, v2.16b }}, [x1], #48"),
        Q!("    zip1            " "v4.16b, v0.16b, v1.16b"),
        Q!("    zip2            " "v5.16b, v0.16b, v1.16b"),
        Q!("    zip1            " "v6.16b, v1.16b, v2.16b"),
        Q!("    zip2            " "v7.16b, v1.16b, v2.16b"),
        Q!("    bic             " "v4.8h, #0xf0, lsl #8"),
        Q!("    bic             " "v5.8h, #0xf0, lsl #8"),
        Q!("    ushr            " "v6.8h, v6.8h, #0x4"),
        Q!("    ushr            " "v7.8h, v7.8h, #0x4"),
        Q!("    zip1            " "v16.8h, v4.8h, v6.8h"),
        Q!("    zip2            " "v17.8h, v4.8h, v6.8h"),
        Q!("    zip1            " "v18.8h, v5.8h, v7.8h"),
        Q!("    zip2            " "v19.8h, v5.8h, v7.8h"),
        Q!("    cmhi            " "v4.8h, v30.8h, v16.8h"),
        Q!("    cmhi            " "v5.8h, v30.8h, v17.8h"),
        Q!("    cmhi            " "v6.8h, v30.8h, v18.8h"),
        Q!("    cmhi            " "v7.8h, v30.8h, v19.8h"),
        Q!("    and             " "v4.16b, v4.16b, v31.16b"),
        Q!("    and             " "v5.16b, v5.16b, v31.16b"),
        Q!("    and             " "v6.16b, v6.16b, v31.16b"),
        Q!("    and             " "v7.16b, v7.16b, v31.16b"),
        Q!("    uaddlv          " "s20, v4.8h"),
        Q!("    uaddlv          " "s21, v5.8h"),
        Q!("    uaddlv          " "s22, v6.8h"),
        Q!("    uaddlv          " "s23, v7.8h"),
        Q!("    fmov            " "w12, s20"),
        Q!("    fmov            " "w13, s21"),
        Q!("    fmov            " "w14, s22"),
        Q!("    fmov            " "w15, s23"),
        Q!("    ldr             " "q24, [x3, x12, lsl #4]"),
        Q!("    ldr             " "q25, [x3, x13, lsl #4]"),
        Q!("    ldr             " "q26, [x3, x14, lsl #4]"),
        Q!("    ldr             " "q27, [x3, x15, lsl #4]"),
        Q!("    cnt             " "v4.16b, v4.16b"),
        Q!("    cnt             " "v5.16b, v5.16b"),
        Q!("    cnt             " "v6.16b, v6.16b"),
        Q!("    cnt             " "v7.16b, v7.16b"),
        Q!("    uaddlv          " "s20, v4.8h"),
        Q!("    uaddlv          " "s21, v5.8h"),
        Q!("    uaddlv          " "s22, v6.8h"),
        Q!("    uaddlv          " "s23, v7.8h"),
        Q!("    fmov            " "w12, s20"),
        Q!("    fmov            " "w13, s21"),
        Q!("    fmov            " "w14, s22"),
        Q!("    fmov            " "w15, s23"),
        Q!("    tbl             " "v16.16b, {{ v16.16b }}, v24.16b"),
        Q!("    tbl             " "v17.16b, {{ v17.16b }}, v25.16b"),
        Q!("    tbl             " "v18.16b, {{ v18.16b }}, v26.16b"),
        Q!("    tbl             " "v19.16b, {{ v19.16b }}, v27.16b"),
        Q!("    str             " "q16, [x7]"),
        Q!("    add             " "x7, x7, x12, lsl #1"),
        Q!("    str             " "q17, [x7]"),
        Q!("    add             " "x7, x7, x13, lsl #1"),
        Q!("    str             " "q18, [x7]"),
        Q!("    add             " "x7, x7, x14, lsl #1"),
        Q!("    str             " "q19, [x7]"),
        Q!("    add             " "x7, x7, x15, lsl #1"),
        Q!("    add             " "x12, x12, x13"),
        Q!("    add             " "x14, x14, x15"),
        Q!("    add             " "x9, x9, x12"),
        Q!("    add             " "x9, x9, x14"),
        Q!("    cmp             " "x2, #0x30"),
        Q!("    b.hs            " Label!("Lmlkem_rej_uniform_loop48", 4, Before)),

        Q!(Label!("Lmlkem_rej_uniform_loop48_end", 3) ":"),
        Q!("    cmp             " "x9, x4"),
        Q!("    b.hs            " Label!("Lmlkem_rej_uniform_memory_copy", 5, After)),
        Q!("    cmp             " "x2, #0x18"),
        Q!("    b.lo            " Label!("Lmlkem_rej_uniform_memory_copy", 5, After)),
        Q!("    sub             " "x2, x2, #0x18"),
        Q!("    ld3             " "{{ v0.8b, v1.8b, v2.8b }}, [x1], #24"),
        Q!("    zip1            " "v4.16b, v0.16b, v1.16b"),
        Q!("    zip1            " "v5.16b, v1.16b, v2.16b"),
        Q!("    bic             " "v4.8h, #0xf0, lsl #8"),
        Q!("    ushr            " "v5.8h, v5.8h, #0x4"),
        Q!("    zip1            " "v16.8h, v4.8h, v5.8h"),
        Q!("    zip2            " "v17.8h, v4.8h, v5.8h"),
        Q!("    cmhi            " "v4.8h, v30.8h, v16.8h"),
        Q!("    cmhi            " "v5.8h, v30.8h, v17.8h"),
        Q!("    and             " "v4.16b, v4.16b, v31.16b"),
        Q!("    and             " "v5.16b, v5.16b, v31.16b"),
        Q!("    uaddlv          " "s20, v4.8h"),
        Q!("    uaddlv          " "s21, v5.8h"),
        Q!("    fmov            " "w12, s20"),
        Q!("    fmov            " "w13, s21"),
        Q!("    ldr             " "q24, [x3, x12, lsl #4]"),
        Q!("    ldr             " "q25, [x3, x13, lsl #4]"),
        Q!("    cnt             " "v4.16b, v4.16b"),
        Q!("    cnt             " "v5.16b, v5.16b"),
        Q!("    uaddlv          " "s20, v4.8h"),
        Q!("    uaddlv          " "s21, v5.8h"),
        Q!("    fmov            " "w12, s20"),
        Q!("    fmov            " "w13, s21"),
        Q!("    tbl             " "v16.16b, {{ v16.16b }}, v24.16b"),
        Q!("    tbl             " "v17.16b, {{ v17.16b }}, v25.16b"),
        Q!("    str             " "q16, [x7]"),
        Q!("    add             " "x7, x7, x12, lsl #1"),
        Q!("    str             " "q17, [x7]"),
        Q!("    add             " "x7, x7, x13, lsl #1"),
        Q!("    add             " "x9, x9, x12"),
        Q!("    add             " "x9, x9, x13"),

        Q!(Label!("Lmlkem_rej_uniform_memory_copy", 5) ":"),
        Q!("    cmp             " "x9, x4"),
        Q!("    csel            " "x9, x9, x4, lo"),
        Q!("    mov             " "x11, #0x0"),
        Q!("    mov             " "x7, x8"),

        Q!(Label!("Lmlkem_rej_uniform_final_copy", 6) ":"),
        Q!("    ldr             " "q16, [x7], #0x40"),
        Q!("    ldur            " "q17, [x7, #-0x30]"),
        Q!("    ldur            " "q18, [x7, #-0x20]"),
        Q!("    ldur            " "q19, [x7, #-0x10]"),
        Q!("    str             " "q16, [x0], #0x40"),
        Q!("    stur            " "q17, [x0, #-0x30]"),
        Q!("    stur            " "q18, [x0, #-0x20]"),
        Q!("    stur            " "q19, [x0, #-0x10]"),
        Q!("    add             " "x11, x11, #0x20"),
        Q!("    cmp             " "x11, #0x100"),
        Q!("    b.lt            " Label!("Lmlkem_rej_uniform_final_copy", 6, Before)),
        Q!("    mov             " "x0, x9"),
        Q!("    b               " Label!("Lmlkem_rej_uniform_return", 7, After)),

        Q!(Label!("Lmlkem_rej_uniform_return", 7) ":"),
        Q!("    add             " "sp, sp, # (576 + 0)"),
        inout("x0") r.as_mut_ptr() => ret,
        inout("x1") input.as_ptr() => _,
        inout("x2") input.len() => _,
        inout("x3") table.as_ptr() => _,
        // clobbers
        out("v0") _,
        out("v1") _,
        out("v16") _,
        out("v17") _,
        out("v18") _,
        out("v19") _,
        out("v2") _,
        out("v20") _,
        out("v21") _,
        out("v22") _,
        out("v23") _,
        out("v24") _,
        out("v25") _,
        out("v26") _,
        out("v27") _,
        out("v30") _,
        out("v31") _,
        out("v4") _,
        out("v5") _,
        out("v6") _,
        out("v7") _,
        out("x11") _,
        out("x12") _,
        out("x13") _,
        out("x14") _,
        out("x15") _,
        out("x4") _,
        out("x7") _,
        out("x8") _,
        out("x9") _,
            )
    };
    ret
}
