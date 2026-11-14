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
// buflen bytes, assumed to be a multiple of 12. Fills the output array
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
// Standard x86-64 ABI: RDI = r, RSI = buf, RDX = buflen, RCX = table
// Microsoft x64 ABI:   RCX = r, RDX = buf, R8 = buflen, R9 = table
// ----------------------------------------------------------------------------

// This code is effectively the same as the original here:
// https://github.com/pq-code-package/mlkem-native/blob/main/mlkem/src/native/x86_64/src/rej_uniform_asm.S
//
// There are a few changes:
//
// * Use xmm3 in place of xmm6, so we don't need to save/restore it for Windows
// * Directly test for null input (buflen = 0), and skip body and return 0

/// Uniform rejection sampling for ML-KEM
///
/// Inputs *buf (unsigned bytes), buflen, table (unsigned bytes); output r[256] (signed 16-bit words), return
///
/// Interprets the input buffer as packed 12-bit numbers with a length of
/// buflen bytes, assumed to be a multiple of 12. Fills the output array
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

        Q!("    endbr64         " ),

        Q!("    sub             " "rsp, 0x210"),

        Q!("    xor             " "eax, eax"),
        Q!("    test            " "rdx, rdx"),
        Q!("    jz              " Label!("Lmlkem_rej_uniform_VARIABLE_TIME_end", 2, After)),

        Q!("    movabs          " "rax, 0xd010d010d010d01"),
        Q!("    movq            " "xmm0, rax"),
        Q!("    pinsrq          " "xmm0, rax, 0x1"),
        Q!("    movabs          " "rax, 0xfff0fff0fff0fff"),
        Q!("    movq            " "xmm5, rax"),
        Q!("    pinsrq          " "xmm5, rax, 0x1"),
        Q!("    movabs          " "rax, 0x504040302010100"),
        Q!("    movq            " "xmm4, rax"),
        Q!("    movabs          " "rax, 0xb0a0a0908070706"),
        Q!("    pinsrq          " "xmm4, rax, 0x1"),
        Q!("    mov             " "rax, 0x0"),
        Q!("    mov             " "r8, 0x0"),
        Q!("    mov             " "r9, 0x5555"),

        Q!(Label!("Lmlkem_rej_uniform_VARIABLE_TIME_loop", 3) ":"),
        Q!("    movq            " "xmm2, [rsi + r8]"),
        Q!("    pinsrd          " "xmm2, [rsi + r8 + 8], 0x2"),
        Q!("    pshufb          " "xmm2, xmm4"),
        Q!("    movdqa          " "xmm3, xmm2"),
        Q!("    psrlw           " "xmm3, 0x4"),
        Q!("    pblendw         " "xmm2, xmm3, 0xaa"),
        Q!("    pand            " "xmm2, xmm5"),
        Q!("    movdqa          " "xmm1, xmm0"),
        Q!("    pcmpgtw         " "xmm1, xmm2"),
        Q!("    pmovmskb        " "r11d, xmm1"),
        Q!("    pext            " "r11, r11, r9"),
        Q!("    mov             " "r10, r11"),
        Q!("    shl             " "r10, 0x4"),
        Q!("    movdqu          " "xmm3, [rcx + r10]"),
        Q!("    pshufb          " "xmm2, xmm3"),
        Q!("    movdqu          " "[rsp + 2 * rax], xmm2"),
        Q!("    popcnt          " "r11, r11"),
        Q!("    add             " "rax, r11"),
        Q!("    cmp             " "rax, 0x100"),
        Q!("    jae             " Label!("Lmlkem_rej_uniform_VARIABLE_TIME_final_copy", 4, After)),
        Q!("    add             " "r8, 0xc"),
        Q!("    cmp             " "rdx, r8"),
        Q!("    ja              " Label!("Lmlkem_rej_uniform_VARIABLE_TIME_loop", 3, Before)),

        Q!(Label!("Lmlkem_rej_uniform_VARIABLE_TIME_final_copy", 4) ":"),
        Q!("    mov             " "rcx, 0x100"),
        Q!("    cmp             " "rax, 0x100"),
        Q!("    cmova           " "rax, rcx"),
        Q!("    mov             " "rsi, rsp"),
        Q!("    mov             " "rcx, rax"),
        Q!("    shl             " "rcx, 1"),
        Q!("    repz            " "movsb"),

        Q!(Label!("Lmlkem_rej_uniform_VARIABLE_TIME_end", 2) ":"),
        Q!("    add             " "rsp, 0x210"),

        inout("rdi") r.as_mut_ptr() => _,
        inout("rsi") input.as_ptr() => _,
        inout("rdx") input.len() => _,
        inout("rcx") table.as_ptr() => _,
        out("rax") ret,
        // clobbers
        out("r10") _,
        out("r11") _,
        out("r8") _,
        out("r9") _,
        out("zmm0") _,
        out("zmm1") _,
        out("zmm2") _,
        out("zmm3") _,
        out("zmm4") _,
        out("zmm5") _,
            )
    };
    ret
}
