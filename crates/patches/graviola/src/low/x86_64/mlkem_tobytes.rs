// generated source. do not edit.
#![allow(non_upper_case_globals, unused_macros, unused_imports)]
use crate::low::macros::*;

// Copyright (c) 2024 The mlkem-native project authors
// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT

// ----------------------------------------------------------------------------
// Pack ML-KEM polynomial coefficients as 12-bit numbers
// Input a[256] (signed 16-bit words); output r[384] (bytes)
//
// This accepts an array of 256 16-bit numbers assumed to be in the range
// 0 <= a[i] < 2^12 (typically they will be < 3329, the ML-KEM prime).
// It packs them into the output array as 12-bit unsigned numbers.
//
// extern void mlkem_tobytes(uint8_t r[static 384],const int16_t a[static 256]);

// Standard x86-64 ABI: RDI = r, RSI = a
// Microsoft x64 ABI:   RCX = r, RDX = a
// ----------------------------------------------------------------------------

/// Pack ML-KEM polynomial coefficients as 12-bit numbers
///
/// Input a[256] (signed 16-bit words); output r[384] (bytes)
///
/// This accepts an array of 256 16-bit numbers assumed to be in the range
/// 0 <= a[i] < 2^12 (typically they will be < 3329, the ML-KEM prime).
/// It packs them into the output array as 12-bit unsigned numbers.
pub(crate) fn mlkem_tobytes(r: &mut [u8; 384], a: &[i16; 256]) {
    debug_assert!((a.as_ptr() as usize).is_multiple_of(32));
    // SAFETY: inline assembly. see [crate::low::inline_assembly_safety] for safety info.
    unsafe {
        core::arch::asm!(

        Q!("    endbr64         " ),

        Q!("    mov             " "eax, 0xd010d01"),
        Q!("    vmovd           " "xmm0, eax"),
        Q!("    vpbroadcastd    " "ymm0, xmm0"),
        Q!("    vmovdqa         " "ymm5, [rsi]"),
        Q!("    vmovdqa         " "ymm6, [32 + rsi]"),
        Q!("    vmovdqa         " "ymm7, [64 + rsi]"),
        Q!("    vmovdqa         " "ymm8, [96 + rsi]"),
        Q!("    vmovdqa         " "ymm9, [128 + rsi]"),
        Q!("    vmovdqa         " "ymm10, [160 + rsi]"),
        Q!("    vmovdqa         " "ymm11, [192 + rsi]"),
        Q!("    vmovdqa         " "ymm12, [224 + rsi]"),
        Q!("    vpsllw          " "ymm4, ymm6, 0xc"),
        Q!("    vpor            " "ymm4, ymm5, ymm4"),
        Q!("    vpsrlw          " "ymm5, ymm6, 0x4"),
        Q!("    vpsllw          " "ymm6, ymm7, 0x8"),
        Q!("    vpor            " "ymm5, ymm6, ymm5"),
        Q!("    vpsrlw          " "ymm6, ymm7, 0x8"),
        Q!("    vpsllw          " "ymm7, ymm8, 0x4"),
        Q!("    vpor            " "ymm6, ymm7, ymm6"),
        Q!("    vpsllw          " "ymm7, ymm10, 0xc"),
        Q!("    vpor            " "ymm7, ymm9, ymm7"),
        Q!("    vpsrlw          " "ymm8, ymm10, 0x4"),
        Q!("    vpsllw          " "ymm9, ymm11, 0x8"),
        Q!("    vpor            " "ymm8, ymm9, ymm8"),
        Q!("    vpsrlw          " "ymm9, ymm11, 0x8"),
        Q!("    vpsllw          " "ymm10, ymm12, 0x4"),
        Q!("    vpor            " "ymm9, ymm10, ymm9"),
        Q!("    vpslld          " "ymm3, ymm5, 0x10"),
        Q!("    vpblendw        " "ymm3, ymm4, ymm3, 0xaa"),
        Q!("    vpsrld          " "ymm4, ymm4, 0x10"),
        Q!("    vpblendw        " "ymm5, ymm4, ymm5, 0xaa"),
        Q!("    vpslld          " "ymm4, ymm7, 0x10"),
        Q!("    vpblendw        " "ymm4, ymm6, ymm4, 0xaa"),
        Q!("    vpsrld          " "ymm6, ymm6, 0x10"),
        Q!("    vpblendw        " "ymm7, ymm6, ymm7, 0xaa"),
        Q!("    vpslld          " "ymm6, ymm9, 0x10"),
        Q!("    vpblendw        " "ymm6, ymm8, ymm6, 0xaa"),
        Q!("    vpsrld          " "ymm8, ymm8, 0x10"),
        Q!("    vpblendw        " "ymm9, ymm8, ymm9, 0xaa"),
        Q!("    vmovsldup       " "ymm8, ymm4"),
        Q!("    vpblendd        " "ymm8, ymm3, ymm8, 0xaa"),
        Q!("    vpsrlq          " "ymm3, ymm3, 0x20"),
        Q!("    vpblendd        " "ymm4, ymm3, ymm4, 0xaa"),
        Q!("    vmovsldup       " "ymm3, ymm5"),
        Q!("    vpblendd        " "ymm3, ymm6, ymm3, 0xaa"),
        Q!("    vpsrlq          " "ymm6, ymm6, 0x20"),
        Q!("    vpblendd        " "ymm5, ymm6, ymm5, 0xaa"),
        Q!("    vmovsldup       " "ymm6, ymm9"),
        Q!("    vpblendd        " "ymm6, ymm7, ymm6, 0xaa"),
        Q!("    vpsrlq          " "ymm7, ymm7, 0x20"),
        Q!("    vpblendd        " "ymm9, ymm7, ymm9, 0xaa"),
        Q!("    vpunpcklqdq     " "ymm7, ymm8, ymm3"),
        Q!("    vpunpckhqdq     " "ymm3, ymm8, ymm3"),
        Q!("    vpunpcklqdq     " "ymm8, ymm6, ymm4"),
        Q!("    vpunpckhqdq     " "ymm4, ymm6, ymm4"),
        Q!("    vpunpcklqdq     " "ymm6, ymm5, ymm9"),
        Q!("    vpunpckhqdq     " "ymm9, ymm5, ymm9"),
        Q!("    vperm2i128      " "ymm5, ymm7, ymm8, 0x20"),
        Q!("    vperm2i128      " "ymm8, ymm7, ymm8, 0x31"),
        Q!("    vperm2i128      " "ymm7, ymm6, ymm3, 0x20"),
        Q!("    vperm2i128      " "ymm3, ymm6, ymm3, 0x31"),
        Q!("    vperm2i128      " "ymm6, ymm4, ymm9, 0x20"),
        Q!("    vperm2i128      " "ymm9, ymm4, ymm9, 0x31"),
        Q!("    vmovdqu         " "[rdi], ymm5"),
        Q!("    vmovdqu         " "[32 + rdi], ymm7"),
        Q!("    vmovdqu         " "[64 + rdi], ymm6"),
        Q!("    vmovdqu         " "[96 + rdi], ymm8"),
        Q!("    vmovdqu         " "[128 + rdi], ymm3"),
        Q!("    vmovdqu         " "[160 + rdi], ymm9"),
        Q!("    vmovdqa         " "ymm5, [256 + rsi]"),
        Q!("    vmovdqa         " "ymm6, [288 + rsi]"),
        Q!("    vmovdqa         " "ymm7, [320 + rsi]"),
        Q!("    vmovdqa         " "ymm8, [352 + rsi]"),
        Q!("    vmovdqa         " "ymm9, [384 + rsi]"),
        Q!("    vmovdqa         " "ymm10, [416 + rsi]"),
        Q!("    vmovdqa         " "ymm11, [448 + rsi]"),
        Q!("    vmovdqa         " "ymm12, [480 + rsi]"),
        Q!("    vpsllw          " "ymm4, ymm6, 0xc"),
        Q!("    vpor            " "ymm4, ymm5, ymm4"),
        Q!("    vpsrlw          " "ymm5, ymm6, 0x4"),
        Q!("    vpsllw          " "ymm6, ymm7, 0x8"),
        Q!("    vpor            " "ymm5, ymm6, ymm5"),
        Q!("    vpsrlw          " "ymm6, ymm7, 0x8"),
        Q!("    vpsllw          " "ymm7, ymm8, 0x4"),
        Q!("    vpor            " "ymm6, ymm7, ymm6"),
        Q!("    vpsllw          " "ymm7, ymm10, 0xc"),
        Q!("    vpor            " "ymm7, ymm9, ymm7"),
        Q!("    vpsrlw          " "ymm8, ymm10, 0x4"),
        Q!("    vpsllw          " "ymm9, ymm11, 0x8"),
        Q!("    vpor            " "ymm8, ymm9, ymm8"),
        Q!("    vpsrlw          " "ymm9, ymm11, 0x8"),
        Q!("    vpsllw          " "ymm10, ymm12, 0x4"),
        Q!("    vpor            " "ymm9, ymm10, ymm9"),
        Q!("    vpslld          " "ymm3, ymm5, 0x10"),
        Q!("    vpblendw        " "ymm3, ymm4, ymm3, 0xaa"),
        Q!("    vpsrld          " "ymm4, ymm4, 0x10"),
        Q!("    vpblendw        " "ymm5, ymm4, ymm5, 0xaa"),
        Q!("    vpslld          " "ymm4, ymm7, 0x10"),
        Q!("    vpblendw        " "ymm4, ymm6, ymm4, 0xaa"),
        Q!("    vpsrld          " "ymm6, ymm6, 0x10"),
        Q!("    vpblendw        " "ymm7, ymm6, ymm7, 0xaa"),
        Q!("    vpslld          " "ymm6, ymm9, 0x10"),
        Q!("    vpblendw        " "ymm6, ymm8, ymm6, 0xaa"),
        Q!("    vpsrld          " "ymm8, ymm8, 0x10"),
        Q!("    vpblendw        " "ymm9, ymm8, ymm9, 0xaa"),
        Q!("    vmovsldup       " "ymm8, ymm4"),
        Q!("    vpblendd        " "ymm8, ymm3, ymm8, 0xaa"),
        Q!("    vpsrlq          " "ymm3, ymm3, 0x20"),
        Q!("    vpblendd        " "ymm4, ymm3, ymm4, 0xaa"),
        Q!("    vmovsldup       " "ymm3, ymm5"),
        Q!("    vpblendd        " "ymm3, ymm6, ymm3, 0xaa"),
        Q!("    vpsrlq          " "ymm6, ymm6, 0x20"),
        Q!("    vpblendd        " "ymm5, ymm6, ymm5, 0xaa"),
        Q!("    vmovsldup       " "ymm6, ymm9"),
        Q!("    vpblendd        " "ymm6, ymm7, ymm6, 0xaa"),
        Q!("    vpsrlq          " "ymm7, ymm7, 0x20"),
        Q!("    vpblendd        " "ymm9, ymm7, ymm9, 0xaa"),
        Q!("    vpunpcklqdq     " "ymm7, ymm8, ymm3"),
        Q!("    vpunpckhqdq     " "ymm3, ymm8, ymm3"),
        Q!("    vpunpcklqdq     " "ymm8, ymm6, ymm4"),
        Q!("    vpunpckhqdq     " "ymm4, ymm6, ymm4"),
        Q!("    vpunpcklqdq     " "ymm6, ymm5, ymm9"),
        Q!("    vpunpckhqdq     " "ymm9, ymm5, ymm9"),
        Q!("    vperm2i128      " "ymm5, ymm7, ymm8, 0x20"),
        Q!("    vperm2i128      " "ymm8, ymm7, ymm8, 0x31"),
        Q!("    vperm2i128      " "ymm7, ymm6, ymm3, 0x20"),
        Q!("    vperm2i128      " "ymm3, ymm6, ymm3, 0x31"),
        Q!("    vperm2i128      " "ymm6, ymm4, ymm9, 0x20"),
        Q!("    vperm2i128      " "ymm9, ymm4, ymm9, 0x31"),
        Q!("    vmovdqu         " "[192 + rdi], ymm5"),
        Q!("    vmovdqu         " "[224 + rdi], ymm7"),
        Q!("    vmovdqu         " "[256 + rdi], ymm6"),
        Q!("    vmovdqu         " "[288 + rdi], ymm8"),
        Q!("    vmovdqu         " "[320 + rdi], ymm3"),
        Q!("    vmovdqu         " "[352 + rdi], ymm9"),

        inout("rdi") r.as_mut_ptr() => _,
        inout("rsi") a.as_ptr() => _,
        // clobbers
        out("rax") _,
        out("zmm0") _,
        out("zmm10") _,
        out("zmm11") _,
        out("zmm12") _,
        out("zmm3") _,
        out("zmm4") _,
        out("zmm5") _,
        out("zmm6") _,
        out("zmm7") _,
        out("zmm8") _,
        out("zmm9") _,
            )
    };
}
