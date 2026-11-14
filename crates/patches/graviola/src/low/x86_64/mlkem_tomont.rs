// generated source. do not edit.
#![allow(non_upper_case_globals, unused_macros, unused_imports)]
use crate::low::macros::*;

// Copyright (c) 2024 The mlkem-native project authors
// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT

// ----------------------------------------------------------------------------
// Conversion of ML-KEM polynomial coefficients to Montgomery form
// Input a[256] (signed 16-bit words); output a[256] (signed 16-bit words)
//
// This converts each element of the 256-element array of 16-bit signed
// integers modulo 3329 into Montgomery form, giving a signed result
// satisfying (output[i] == 2^16 * input[i]) (mod 3329), without full
// modular reduction but with |output[i]| < 3329 guaranteed.
//
// extern void mlkem_tomont(int16_t a[static 256]);
//
// Standard x86 ABI: RDI = a
// ----------------------------------------------------------------------------

/// Conversion of ML-KEM polynomial coefficients to Montgomery form
///
/// Input a[256] (signed 16-bit words); output a[256] (signed 16-bit words)
///
/// This converts each element of the 256-element array of 16-bit signed
/// integers modulo 3329 into Montgomery form, giving a signed result
/// satisfying (output[i] == 2^16 * input[i]) (mod 3329), without full
/// modular reduction but with |output[i]| < 3329 guaranteed.
pub(crate) fn mlkem_tomont(a: &mut [i16; 256]) {
    // SAFETY: inline assembly. see [crate::low::inline_assembly_safety] for safety info.
    unsafe {
        core::arch::asm!(

        Q!("    endbr64         " ),

        Q!("    mov             " "eax, 0xd010d01"),
        Q!("    vmovd           " "xmm0, eax"),
        Q!("    vpbroadcastd    " "ymm0, xmm0"),
        Q!("    mov             " "eax, 0x50495049"),
        Q!("    vmovd           " "xmm1, eax"),
        Q!("    vpbroadcastd    " "ymm1, xmm1"),
        Q!("    mov             " "eax, 0x5490549"),
        Q!("    vmovd           " "xmm2, eax"),
        Q!("    vpbroadcastd    " "ymm2, xmm2"),
        Q!("    vmovdqa         " "ymm3, [rdi]"),
        Q!("    vmovdqa         " "ymm4, [32 + rdi]"),
        Q!("    vmovdqa         " "ymm5, [64 + rdi]"),
        Q!("    vmovdqa         " "ymm6, [96 + rdi]"),
        Q!("    vmovdqa         " "ymm7, [128 + rdi]"),
        Q!("    vmovdqa         " "ymm8, [160 + rdi]"),
        Q!("    vmovdqa         " "ymm9, [192 + rdi]"),
        Q!("    vmovdqa         " "ymm10, [224 + rdi]"),
        Q!("    vpmullw         " "ymm11, ymm3, ymm1"),
        Q!("    vpmulhw         " "ymm3, ymm3, ymm2"),
        Q!("    vpmulhw         " "ymm11, ymm11, ymm0"),
        Q!("    vpsubw          " "ymm3, ymm3, ymm11"),
        Q!("    vpmullw         " "ymm12, ymm4, ymm1"),
        Q!("    vpmulhw         " "ymm4, ymm4, ymm2"),
        Q!("    vpmulhw         " "ymm12, ymm12, ymm0"),
        Q!("    vpsubw          " "ymm4, ymm4, ymm12"),
        Q!("    vpmullw         " "ymm13, ymm5, ymm1"),
        Q!("    vpmulhw         " "ymm5, ymm5, ymm2"),
        Q!("    vpmulhw         " "ymm13, ymm13, ymm0"),
        Q!("    vpsubw          " "ymm5, ymm5, ymm13"),
        Q!("    vpmullw         " "ymm14, ymm6, ymm1"),
        Q!("    vpmulhw         " "ymm6, ymm6, ymm2"),
        Q!("    vpmulhw         " "ymm14, ymm14, ymm0"),
        Q!("    vpsubw          " "ymm6, ymm6, ymm14"),
        Q!("    vpmullw         " "ymm15, ymm7, ymm1"),
        Q!("    vpmulhw         " "ymm7, ymm7, ymm2"),
        Q!("    vpmulhw         " "ymm15, ymm15, ymm0"),
        Q!("    vpsubw          " "ymm7, ymm7, ymm15"),
        Q!("    vpmullw         " "ymm11, ymm8, ymm1"),
        Q!("    vpmulhw         " "ymm8, ymm8, ymm2"),
        Q!("    vpmulhw         " "ymm11, ymm11, ymm0"),
        Q!("    vpsubw          " "ymm8, ymm8, ymm11"),
        Q!("    vpmullw         " "ymm12, ymm9, ymm1"),
        Q!("    vpmulhw         " "ymm9, ymm9, ymm2"),
        Q!("    vpmulhw         " "ymm12, ymm12, ymm0"),
        Q!("    vpsubw          " "ymm9, ymm9, ymm12"),
        Q!("    vpmullw         " "ymm13, ymm10, ymm1"),
        Q!("    vpmulhw         " "ymm10, ymm10, ymm2"),
        Q!("    vpmulhw         " "ymm13, ymm13, ymm0"),
        Q!("    vpsubw          " "ymm10, ymm10, ymm13"),
        Q!("    vmovdqa         " "[rdi], ymm3"),
        Q!("    vmovdqa         " "[32 + rdi], ymm4"),
        Q!("    vmovdqa         " "[64 + rdi], ymm5"),
        Q!("    vmovdqa         " "[96 + rdi], ymm6"),
        Q!("    vmovdqa         " "[128 + rdi], ymm7"),
        Q!("    vmovdqa         " "[160 + rdi], ymm8"),
        Q!("    vmovdqa         " "[192 + rdi], ymm9"),
        Q!("    vmovdqa         " "[224 + rdi], ymm10"),
        Q!("    vmovdqa         " "ymm3, [256 + rdi]"),
        Q!("    vmovdqa         " "ymm4, [288 + rdi]"),
        Q!("    vmovdqa         " "ymm5, [320 + rdi]"),
        Q!("    vmovdqa         " "ymm6, [352 + rdi]"),
        Q!("    vmovdqa         " "ymm7, [384 + rdi]"),
        Q!("    vmovdqa         " "ymm8, [416 + rdi]"),
        Q!("    vmovdqa         " "ymm9, [448 + rdi]"),
        Q!("    vmovdqa         " "ymm10, [480 + rdi]"),
        Q!("    vpmullw         " "ymm11, ymm3, ymm1"),
        Q!("    vpmulhw         " "ymm3, ymm3, ymm2"),
        Q!("    vpmulhw         " "ymm11, ymm11, ymm0"),
        Q!("    vpsubw          " "ymm3, ymm3, ymm11"),
        Q!("    vpmullw         " "ymm12, ymm4, ymm1"),
        Q!("    vpmulhw         " "ymm4, ymm4, ymm2"),
        Q!("    vpmulhw         " "ymm12, ymm12, ymm0"),
        Q!("    vpsubw          " "ymm4, ymm4, ymm12"),
        Q!("    vpmullw         " "ymm13, ymm5, ymm1"),
        Q!("    vpmulhw         " "ymm5, ymm5, ymm2"),
        Q!("    vpmulhw         " "ymm13, ymm13, ymm0"),
        Q!("    vpsubw          " "ymm5, ymm5, ymm13"),
        Q!("    vpmullw         " "ymm14, ymm6, ymm1"),
        Q!("    vpmulhw         " "ymm6, ymm6, ymm2"),
        Q!("    vpmulhw         " "ymm14, ymm14, ymm0"),
        Q!("    vpsubw          " "ymm6, ymm6, ymm14"),
        Q!("    vpmullw         " "ymm15, ymm7, ymm1"),
        Q!("    vpmulhw         " "ymm7, ymm7, ymm2"),
        Q!("    vpmulhw         " "ymm15, ymm15, ymm0"),
        Q!("    vpsubw          " "ymm7, ymm7, ymm15"),
        Q!("    vpmullw         " "ymm11, ymm8, ymm1"),
        Q!("    vpmulhw         " "ymm8, ymm8, ymm2"),
        Q!("    vpmulhw         " "ymm11, ymm11, ymm0"),
        Q!("    vpsubw          " "ymm8, ymm8, ymm11"),
        Q!("    vpmullw         " "ymm12, ymm9, ymm1"),
        Q!("    vpmulhw         " "ymm9, ymm9, ymm2"),
        Q!("    vpmulhw         " "ymm12, ymm12, ymm0"),
        Q!("    vpsubw          " "ymm9, ymm9, ymm12"),
        Q!("    vpmullw         " "ymm13, ymm10, ymm1"),
        Q!("    vpmulhw         " "ymm10, ymm10, ymm2"),
        Q!("    vpmulhw         " "ymm13, ymm13, ymm0"),
        Q!("    vpsubw          " "ymm10, ymm10, ymm13"),
        Q!("    vmovdqa         " "[256 + rdi], ymm3"),
        Q!("    vmovdqa         " "[288 + rdi], ymm4"),
        Q!("    vmovdqa         " "[320 + rdi], ymm5"),
        Q!("    vmovdqa         " "[352 + rdi], ymm6"),
        Q!("    vmovdqa         " "[384 + rdi], ymm7"),
        Q!("    vmovdqa         " "[416 + rdi], ymm8"),
        Q!("    vmovdqa         " "[448 + rdi], ymm9"),
        Q!("    vmovdqa         " "[480 + rdi], ymm10"),

        inout("rdi") a.as_mut_ptr() => _,
        // clobbers
        out("rax") _,
        out("zmm0") _,
        out("zmm1") _,
        out("zmm10") _,
        out("zmm11") _,
        out("zmm12") _,
        out("zmm13") _,
        out("zmm14") _,
        out("zmm15") _,
        out("zmm2") _,
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
