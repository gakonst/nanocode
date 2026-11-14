// generated source. do not edit.
#![allow(non_upper_case_globals, unused_macros, unused_imports)]
use crate::low::macros::*;

// Copyright (c) 2024 The mlkem-native project authors
// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT

// ----------------------------------------------------------------------------
// Reorder ML-KEM polynomial coefficients for x86 implementation
// Input a[256] (signed 16-bit words); output a[256] (signed 16-bit words)
//
// This accepts an array of 256 16-bit numbers and reorders them.
//
// extern void mlkem_unpack(int16_t a[static 256]);

// Standard x86-64 ABI: RDI = a
// Microsoft x64 ABI:   RCX = a
// ----------------------------------------------------------------------------

/// Reorder ML-KEM polynomial coefficients for x86 implementation
///
/// Input a[256] (signed 16-bit words); output a[256] (signed 16-bit words)
///
/// This accepts an array of 256 16-bit numbers and reorders them.
pub(crate) fn mlkem_unpack(a: &mut [i16; 256]) {
    // SAFETY: inline assembly. see [crate::low::inline_assembly_safety] for safety info.
    unsafe {
        core::arch::asm!(

        Q!("    endbr64         " ),

        Q!("    vmovdqa         " "ymm4, [rdi]"),
        Q!("    vmovdqa         " "ymm5, [32 + rdi]"),
        Q!("    vmovdqa         " "ymm6, [64 + rdi]"),
        Q!("    vmovdqa         " "ymm7, [96 + rdi]"),
        Q!("    vmovdqa         " "ymm8, [128 + rdi]"),
        Q!("    vmovdqa         " "ymm9, [160 + rdi]"),
        Q!("    vmovdqa         " "ymm10, [192 + rdi]"),
        Q!("    vmovdqa         " "ymm11, [224 + rdi]"),
        Q!("    vperm2i128      " "ymm3, ymm4, ymm8, 0x20"),
        Q!("    vperm2i128      " "ymm8, ymm4, ymm8, 0x31"),
        Q!("    vperm2i128      " "ymm4, ymm5, ymm9, 0x20"),
        Q!("    vperm2i128      " "ymm9, ymm5, ymm9, 0x31"),
        Q!("    vperm2i128      " "ymm5, ymm6, ymm10, 0x20"),
        Q!("    vperm2i128      " "ymm10, ymm6, ymm10, 0x31"),
        Q!("    vperm2i128      " "ymm6, ymm7, ymm11, 0x20"),
        Q!("    vperm2i128      " "ymm11, ymm7, ymm11, 0x31"),
        Q!("    vpunpcklqdq     " "ymm7, ymm3, ymm5"),
        Q!("    vpunpckhqdq     " "ymm5, ymm3, ymm5"),
        Q!("    vpunpcklqdq     " "ymm3, ymm8, ymm10"),
        Q!("    vpunpckhqdq     " "ymm10, ymm8, ymm10"),
        Q!("    vpunpcklqdq     " "ymm8, ymm4, ymm6"),
        Q!("    vpunpckhqdq     " "ymm6, ymm4, ymm6"),
        Q!("    vpunpcklqdq     " "ymm4, ymm9, ymm11"),
        Q!("    vpunpckhqdq     " "ymm11, ymm9, ymm11"),
        Q!("    vmovsldup       " "ymm9, ymm8"),
        Q!("    vpblendd        " "ymm9, ymm7, ymm9, 0xaa"),
        Q!("    vpsrlq          " "ymm7, ymm7, 0x20"),
        Q!("    vpblendd        " "ymm8, ymm7, ymm8, 0xaa"),
        Q!("    vmovsldup       " "ymm7, ymm6"),
        Q!("    vpblendd        " "ymm7, ymm5, ymm7, 0xaa"),
        Q!("    vpsrlq          " "ymm5, ymm5, 0x20"),
        Q!("    vpblendd        " "ymm6, ymm5, ymm6, 0xaa"),
        Q!("    vmovsldup       " "ymm5, ymm4"),
        Q!("    vpblendd        " "ymm5, ymm3, ymm5, 0xaa"),
        Q!("    vpsrlq          " "ymm3, ymm3, 0x20"),
        Q!("    vpblendd        " "ymm4, ymm3, ymm4, 0xaa"),
        Q!("    vmovsldup       " "ymm3, ymm11"),
        Q!("    vpblendd        " "ymm3, ymm10, ymm3, 0xaa"),
        Q!("    vpsrlq          " "ymm10, ymm10, 0x20"),
        Q!("    vpblendd        " "ymm11, ymm10, ymm11, 0xaa"),
        Q!("    vpslld          " "ymm10, ymm5, 0x10"),
        Q!("    vpblendw        " "ymm10, ymm9, ymm10, 0xaa"),
        Q!("    vpsrld          " "ymm9, ymm9, 0x10"),
        Q!("    vpblendw        " "ymm5, ymm9, ymm5, 0xaa"),
        Q!("    vpslld          " "ymm9, ymm4, 0x10"),
        Q!("    vpblendw        " "ymm9, ymm8, ymm9, 0xaa"),
        Q!("    vpsrld          " "ymm8, ymm8, 0x10"),
        Q!("    vpblendw        " "ymm4, ymm8, ymm4, 0xaa"),
        Q!("    vpslld          " "ymm8, ymm3, 0x10"),
        Q!("    vpblendw        " "ymm8, ymm7, ymm8, 0xaa"),
        Q!("    vpsrld          " "ymm7, ymm7, 0x10"),
        Q!("    vpblendw        " "ymm3, ymm7, ymm3, 0xaa"),
        Q!("    vpslld          " "ymm7, ymm11, 0x10"),
        Q!("    vpblendw        " "ymm7, ymm6, ymm7, 0xaa"),
        Q!("    vpsrld          " "ymm6, ymm6, 0x10"),
        Q!("    vpblendw        " "ymm11, ymm6, ymm11, 0xaa"),
        Q!("    vmovdqa         " "[rdi], ymm10"),
        Q!("    vmovdqa         " "[32 + rdi], ymm5"),
        Q!("    vmovdqa         " "[64 + rdi], ymm9"),
        Q!("    vmovdqa         " "[96 + rdi], ymm4"),
        Q!("    vmovdqa         " "[128 + rdi], ymm8"),
        Q!("    vmovdqa         " "[160 + rdi], ymm3"),
        Q!("    vmovdqa         " "[192 + rdi], ymm7"),
        Q!("    vmovdqa         " "[224 + rdi], ymm11"),
        Q!("    vmovdqa         " "ymm4, [256 + rdi]"),
        Q!("    vmovdqa         " "ymm5, [288 + rdi]"),
        Q!("    vmovdqa         " "ymm6, [320 + rdi]"),
        Q!("    vmovdqa         " "ymm7, [352 + rdi]"),
        Q!("    vmovdqa         " "ymm8, [384 + rdi]"),
        Q!("    vmovdqa         " "ymm9, [416 + rdi]"),
        Q!("    vmovdqa         " "ymm10, [448 + rdi]"),
        Q!("    vmovdqa         " "ymm11, [480 + rdi]"),
        Q!("    vperm2i128      " "ymm3, ymm4, ymm8, 0x20"),
        Q!("    vperm2i128      " "ymm8, ymm4, ymm8, 0x31"),
        Q!("    vperm2i128      " "ymm4, ymm5, ymm9, 0x20"),
        Q!("    vperm2i128      " "ymm9, ymm5, ymm9, 0x31"),
        Q!("    vperm2i128      " "ymm5, ymm6, ymm10, 0x20"),
        Q!("    vperm2i128      " "ymm10, ymm6, ymm10, 0x31"),
        Q!("    vperm2i128      " "ymm6, ymm7, ymm11, 0x20"),
        Q!("    vperm2i128      " "ymm11, ymm7, ymm11, 0x31"),
        Q!("    vpunpcklqdq     " "ymm7, ymm3, ymm5"),
        Q!("    vpunpckhqdq     " "ymm5, ymm3, ymm5"),
        Q!("    vpunpcklqdq     " "ymm3, ymm8, ymm10"),
        Q!("    vpunpckhqdq     " "ymm10, ymm8, ymm10"),
        Q!("    vpunpcklqdq     " "ymm8, ymm4, ymm6"),
        Q!("    vpunpckhqdq     " "ymm6, ymm4, ymm6"),
        Q!("    vpunpcklqdq     " "ymm4, ymm9, ymm11"),
        Q!("    vpunpckhqdq     " "ymm11, ymm9, ymm11"),
        Q!("    vmovsldup       " "ymm9, ymm8"),
        Q!("    vpblendd        " "ymm9, ymm7, ymm9, 0xaa"),
        Q!("    vpsrlq          " "ymm7, ymm7, 0x20"),
        Q!("    vpblendd        " "ymm8, ymm7, ymm8, 0xaa"),
        Q!("    vmovsldup       " "ymm7, ymm6"),
        Q!("    vpblendd        " "ymm7, ymm5, ymm7, 0xaa"),
        Q!("    vpsrlq          " "ymm5, ymm5, 0x20"),
        Q!("    vpblendd        " "ymm6, ymm5, ymm6, 0xaa"),
        Q!("    vmovsldup       " "ymm5, ymm4"),
        Q!("    vpblendd        " "ymm5, ymm3, ymm5, 0xaa"),
        Q!("    vpsrlq          " "ymm3, ymm3, 0x20"),
        Q!("    vpblendd        " "ymm4, ymm3, ymm4, 0xaa"),
        Q!("    vmovsldup       " "ymm3, ymm11"),
        Q!("    vpblendd        " "ymm3, ymm10, ymm3, 0xaa"),
        Q!("    vpsrlq          " "ymm10, ymm10, 0x20"),
        Q!("    vpblendd        " "ymm11, ymm10, ymm11, 0xaa"),
        Q!("    vpslld          " "ymm10, ymm5, 0x10"),
        Q!("    vpblendw        " "ymm10, ymm9, ymm10, 0xaa"),
        Q!("    vpsrld          " "ymm9, ymm9, 0x10"),
        Q!("    vpblendw        " "ymm5, ymm9, ymm5, 0xaa"),
        Q!("    vpslld          " "ymm9, ymm4, 0x10"),
        Q!("    vpblendw        " "ymm9, ymm8, ymm9, 0xaa"),
        Q!("    vpsrld          " "ymm8, ymm8, 0x10"),
        Q!("    vpblendw        " "ymm4, ymm8, ymm4, 0xaa"),
        Q!("    vpslld          " "ymm8, ymm3, 0x10"),
        Q!("    vpblendw        " "ymm8, ymm7, ymm8, 0xaa"),
        Q!("    vpsrld          " "ymm7, ymm7, 0x10"),
        Q!("    vpblendw        " "ymm3, ymm7, ymm3, 0xaa"),
        Q!("    vpslld          " "ymm7, ymm11, 0x10"),
        Q!("    vpblendw        " "ymm7, ymm6, ymm7, 0xaa"),
        Q!("    vpsrld          " "ymm6, ymm6, 0x10"),
        Q!("    vpblendw        " "ymm11, ymm6, ymm11, 0xaa"),
        Q!("    vmovdqa         " "[256 + rdi], ymm10"),
        Q!("    vmovdqa         " "[288 + rdi], ymm5"),
        Q!("    vmovdqa         " "[320 + rdi], ymm9"),
        Q!("    vmovdqa         " "[352 + rdi], ymm4"),
        Q!("    vmovdqa         " "[384 + rdi], ymm8"),
        Q!("    vmovdqa         " "[416 + rdi], ymm3"),
        Q!("    vmovdqa         " "[448 + rdi], ymm7"),
        Q!("    vmovdqa         " "[480 + rdi], ymm11"),

        inout("rdi") a.as_mut_ptr() => _,
        // clobbers
        out("zmm10") _,
        out("zmm11") _,
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
