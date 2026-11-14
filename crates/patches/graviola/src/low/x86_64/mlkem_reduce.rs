// generated source. do not edit.
#![allow(non_upper_case_globals, unused_macros, unused_imports)]
use crate::low::macros::*;

// Copyright (c) 2024 The mlkem-native project authors
// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT

// ----------------------------------------------------------------------------
// Canonical reduction of polynomial coefficients for ML-KEM
// Input a[256] (signed 16-bit words); output a[256] (signed 16-bit words)
//
// This reduces each element of the 256-element array of 16-bit signed
// integers modulo 3329 with the result being 0 <= r < 3329, in-place.
// This is intended for use when that array represents polynomial
// coefficients for ML-KEM, but that is not relevant to its operation.
//
// extern void mlkem_reduce(int16_t a[static 256]);
//
// Standard x86-64 ABI: RDI = a
// Microsoft x64 ABI:   RCX = a
// ----------------------------------------------------------------------------

/// Canonical reduction of polynomial coefficients for ML-KEM
///
/// Input a[256] (signed 16-bit words); output a[256] (signed 16-bit words)
///
/// This reduces each element of the 256-element array of 16-bit signed
/// integers modulo 3329 with the result being 0 <= r < 3329, in-place.
/// This is intended for use when that array represents polynomial
/// coefficients for ML-KEM, but that is not relevant to its operation.
pub(crate) fn mlkem_reduce(a: &mut [i16; 256]) {
    // SAFETY: inline assembly. see [crate::low::inline_assembly_safety] for safety info.
    unsafe {
        core::arch::asm!(

        Q!("    endbr64         " ),

        // Load 3329 (0x0D01) into all elements of ymm0:
        Q!("    mov             " "eax, 0x0D010D01"),
        Q!("    movd            " "xmm0, eax"),
        Q!("    vpbroadcastd    " "ymm0, xmm0"),

        // Load 20159 (0x4EBF) into all elements of ymm1:
        Q!("    mov             " "eax, 0x4EBF4EBF"),
        Q!("    movd            " "xmm1, eax"),
        Q!("    vpbroadcastd    " "ymm1, xmm1"),

        // We process 128 coefficients (8 ymm regs) at once.
        // Reduce the fist 128 coefficients:
        Q!("    vmovdqa         " "ymm2, [rdi + 0x00]"),
        Q!("    vmovdqa         " "ymm3, [rdi + 0x20]"),
        Q!("    vmovdqa         " "ymm4, [rdi + 0x40]"),
        Q!("    vmovdqa         " "ymm5, [rdi + 0x60]"),
        Q!("    vmovdqa         " "ymm6, [rdi + 0x80]"),
        Q!("    vmovdqa         " "ymm7, [rdi + 0xa0]"),
        Q!("    vmovdqa         " "ymm8, [rdi + 0xc0]"),
        Q!("    vmovdqa         " "ymm9, [rdi + 0xe0]"),

        Q!("    vpmulhw         " "ymm12, ymm2, ymm1"),
        Q!("    vpsraw          " "ymm12, ymm12, 0xa"),
        Q!("    vpmullw         " "ymm12, ymm12, ymm0"),
        Q!("    vpsubw          " "ymm2, ymm2, ymm12"),
        Q!("    vpmulhw         " "ymm12, ymm3, ymm1"),
        Q!("    vpsraw          " "ymm12, ymm12, 0xa"),
        Q!("    vpmullw         " "ymm12, ymm12, ymm0"),
        Q!("    vpsubw          " "ymm3, ymm3, ymm12"),
        Q!("    vpmulhw         " "ymm12, ymm4, ymm1"),
        Q!("    vpsraw          " "ymm12, ymm12, 0xa"),
        Q!("    vpmullw         " "ymm12, ymm12, ymm0"),
        Q!("    vpsubw          " "ymm4, ymm4, ymm12"),
        Q!("    vpmulhw         " "ymm12, ymm5, ymm1"),
        Q!("    vpsraw          " "ymm12, ymm12, 0xa"),
        Q!("    vpmullw         " "ymm12, ymm12, ymm0"),
        Q!("    vpsubw          " "ymm5, ymm5, ymm12"),
        Q!("    vpmulhw         " "ymm12, ymm6, ymm1"),
        Q!("    vpsraw          " "ymm12, ymm12, 0xa"),
        Q!("    vpmullw         " "ymm12, ymm12, ymm0"),
        Q!("    vpsubw          " "ymm6, ymm6, ymm12"),
        Q!("    vpmulhw         " "ymm12, ymm7, ymm1"),
        Q!("    vpsraw          " "ymm12, ymm12, 0xa"),
        Q!("    vpmullw         " "ymm12, ymm12, ymm0"),
        Q!("    vpsubw          " "ymm7, ymm7, ymm12"),
        Q!("    vpmulhw         " "ymm12, ymm8, ymm1"),
        Q!("    vpsraw          " "ymm12, ymm12, 0xa"),
        Q!("    vpmullw         " "ymm12, ymm12, ymm0"),
        Q!("    vpsubw          " "ymm8, ymm8, ymm12"),
        Q!("    vpmulhw         " "ymm12, ymm9, ymm1"),
        Q!("    vpsraw          " "ymm12, ymm12, 0xa"),
        Q!("    vpmullw         " "ymm12, ymm12, ymm0"),
        Q!("    vpsubw          " "ymm9, ymm9, ymm12"),

        Q!("    vpsubw          " "ymm2, ymm2, ymm0"),
        Q!("    vpsraw          " "ymm12, ymm2, 0xf"),
        Q!("    vpand           " "ymm12, ymm12, ymm0"),
        Q!("    vpaddw          " "ymm2, ymm2, ymm12"),
        Q!("    vpsubw          " "ymm3, ymm3, ymm0"),
        Q!("    vpsraw          " "ymm12, ymm3, 0xf"),
        Q!("    vpand           " "ymm12, ymm12, ymm0"),
        Q!("    vpaddw          " "ymm3, ymm3, ymm12"),
        Q!("    vpsubw          " "ymm4, ymm4, ymm0"),
        Q!("    vpsraw          " "ymm12, ymm4, 0xf"),
        Q!("    vpand           " "ymm12, ymm12, ymm0"),
        Q!("    vpaddw          " "ymm4, ymm4, ymm12"),
        Q!("    vpsubw          " "ymm5, ymm5, ymm0"),
        Q!("    vpsraw          " "ymm12, ymm5, 0xf"),
        Q!("    vpand           " "ymm12, ymm12, ymm0"),
        Q!("    vpaddw          " "ymm5, ymm5, ymm12"),
        Q!("    vpsubw          " "ymm6, ymm6, ymm0"),
        Q!("    vpsraw          " "ymm12, ymm6, 0xf"),
        Q!("    vpand           " "ymm12, ymm12, ymm0"),
        Q!("    vpaddw          " "ymm6, ymm6, ymm12"),
        Q!("    vpsubw          " "ymm7, ymm7, ymm0"),
        Q!("    vpsraw          " "ymm12, ymm7, 0xf"),
        Q!("    vpand           " "ymm12, ymm12, ymm0"),
        Q!("    vpaddw          " "ymm7, ymm7, ymm12"),
        Q!("    vpsubw          " "ymm8, ymm8, ymm0"),
        Q!("    vpsraw          " "ymm12, ymm8, 0xf"),
        Q!("    vpand           " "ymm12, ymm12, ymm0"),
        Q!("    vpaddw          " "ymm8, ymm8, ymm12"),
        Q!("    vpsubw          " "ymm9, ymm9, ymm0"),
        Q!("    vpsraw          " "ymm12, ymm9, 0xf"),
        Q!("    vpand           " "ymm12, ymm12, ymm0"),
        Q!("    vpaddw          " "ymm9, ymm9, ymm12"),

        Q!("    vmovdqa         " "[rdi + 0x00], ymm2"),
        Q!("    vmovdqa         " "[rdi + 0x20], ymm3"),
        Q!("    vmovdqa         " "[rdi + 0x40], ymm4"),
        Q!("    vmovdqa         " "[rdi + 0x60], ymm5"),
        Q!("    vmovdqa         " "[rdi + 0x80], ymm6"),
        Q!("    vmovdqa         " "[rdi + 0xa0], ymm7"),
        Q!("    vmovdqa         " "[rdi + 0xc0], ymm8"),
        Q!("    vmovdqa         " "[rdi + 0xe0], ymm9"),

        // Reduce the second 128 coefficients:
        Q!("    vmovdqa         " "ymm2, [rdi + 0x100]"),
        Q!("    vmovdqa         " "ymm3, [rdi + 0x120]"),
        Q!("    vmovdqa         " "ymm4, [rdi + 0x140]"),
        Q!("    vmovdqa         " "ymm5, [rdi + 0x160]"),
        Q!("    vmovdqa         " "ymm6, [rdi + 0x180]"),
        Q!("    vmovdqa         " "ymm7, [rdi + 0x1a0]"),
        Q!("    vmovdqa         " "ymm8, [rdi + 0x1c0]"),
        Q!("    vmovdqa         " "ymm9, [rdi + 0x1e0]"),

        Q!("    vpmulhw         " "ymm12, ymm2, ymm1"),
        Q!("    vpsraw          " "ymm12, ymm12, 0xa"),
        Q!("    vpmullw         " "ymm12, ymm12, ymm0"),
        Q!("    vpsubw          " "ymm2, ymm2, ymm12"),
        Q!("    vpmulhw         " "ymm12, ymm3, ymm1"),
        Q!("    vpsraw          " "ymm12, ymm12, 0xa"),
        Q!("    vpmullw         " "ymm12, ymm12, ymm0"),
        Q!("    vpsubw          " "ymm3, ymm3, ymm12"),
        Q!("    vpmulhw         " "ymm12, ymm4, ymm1"),
        Q!("    vpsraw          " "ymm12, ymm12, 0xa"),
        Q!("    vpmullw         " "ymm12, ymm12, ymm0"),
        Q!("    vpsubw          " "ymm4, ymm4, ymm12"),
        Q!("    vpmulhw         " "ymm12, ymm5, ymm1"),
        Q!("    vpsraw          " "ymm12, ymm12, 0xa"),
        Q!("    vpmullw         " "ymm12, ymm12, ymm0"),
        Q!("    vpsubw          " "ymm5, ymm5, ymm12"),
        Q!("    vpmulhw         " "ymm12, ymm6, ymm1"),
        Q!("    vpsraw          " "ymm12, ymm12, 0xa"),
        Q!("    vpmullw         " "ymm12, ymm12, ymm0"),
        Q!("    vpsubw          " "ymm6, ymm6, ymm12"),
        Q!("    vpmulhw         " "ymm12, ymm7, ymm1"),
        Q!("    vpsraw          " "ymm12, ymm12, 0xa"),
        Q!("    vpmullw         " "ymm12, ymm12, ymm0"),
        Q!("    vpsubw          " "ymm7, ymm7, ymm12"),
        Q!("    vpmulhw         " "ymm12, ymm8, ymm1"),
        Q!("    vpsraw          " "ymm12, ymm12, 0xa"),
        Q!("    vpmullw         " "ymm12, ymm12, ymm0"),
        Q!("    vpsubw          " "ymm8, ymm8, ymm12"),
        Q!("    vpmulhw         " "ymm12, ymm9, ymm1"),
        Q!("    vpsraw          " "ymm12, ymm12, 0xa"),
        Q!("    vpmullw         " "ymm12, ymm12, ymm0"),
        Q!("    vpsubw          " "ymm9, ymm9, ymm12"),

        Q!("    vpsubw          " "ymm2, ymm2, ymm0"),
        Q!("    vpsraw          " "ymm12, ymm2, 0xf"),
        Q!("    vpand           " "ymm12, ymm12, ymm0"),
        Q!("    vpaddw          " "ymm2, ymm2, ymm12"),
        Q!("    vpsubw          " "ymm3, ymm3, ymm0"),
        Q!("    vpsraw          " "ymm12, ymm3, 0xf"),
        Q!("    vpand           " "ymm12, ymm12, ymm0"),
        Q!("    vpaddw          " "ymm3, ymm3, ymm12"),
        Q!("    vpsubw          " "ymm4, ymm4, ymm0"),
        Q!("    vpsraw          " "ymm12, ymm4, 0xf"),
        Q!("    vpand           " "ymm12, ymm12, ymm0"),
        Q!("    vpaddw          " "ymm4, ymm4, ymm12"),
        Q!("    vpsubw          " "ymm5, ymm5, ymm0"),
        Q!("    vpsraw          " "ymm12, ymm5, 0xf"),
        Q!("    vpand           " "ymm12, ymm12, ymm0"),
        Q!("    vpaddw          " "ymm5, ymm5, ymm12"),
        Q!("    vpsubw          " "ymm6, ymm6, ymm0"),
        Q!("    vpsraw          " "ymm12, ymm6, 0xf"),
        Q!("    vpand           " "ymm12, ymm12, ymm0"),
        Q!("    vpaddw          " "ymm6, ymm6, ymm12"),
        Q!("    vpsubw          " "ymm7, ymm7, ymm0"),
        Q!("    vpsraw          " "ymm12, ymm7, 0xf"),
        Q!("    vpand           " "ymm12, ymm12, ymm0"),
        Q!("    vpaddw          " "ymm7, ymm7, ymm12"),
        Q!("    vpsubw          " "ymm8, ymm8, ymm0"),
        Q!("    vpsraw          " "ymm12, ymm8, 0xf"),
        Q!("    vpand           " "ymm12, ymm12, ymm0"),
        Q!("    vpaddw          " "ymm8, ymm8, ymm12"),
        Q!("    vpsubw          " "ymm9, ymm9, ymm0"),
        Q!("    vpsraw          " "ymm12, ymm9, 0xf"),
        Q!("    vpand           " "ymm12, ymm12, ymm0"),
        Q!("    vpaddw          " "ymm9, ymm9, ymm12"),

        Q!("    vmovdqa         " "[rdi + 0x100], ymm2"),
        Q!("    vmovdqa         " "[rdi + 0x120], ymm3"),
        Q!("    vmovdqa         " "[rdi + 0x140], ymm4"),
        Q!("    vmovdqa         " "[rdi + 0x160], ymm5"),
        Q!("    vmovdqa         " "[rdi + 0x180], ymm6"),
        Q!("    vmovdqa         " "[rdi + 0x1a0], ymm7"),
        Q!("    vmovdqa         " "[rdi + 0x1c0], ymm8"),
        Q!("    vmovdqa         " "[rdi + 0x1e0], ymm9"),

        inout("rdi") a.as_mut_ptr() => _,
        // clobbers
        out("rax") _,
        out("zmm0") _,
        out("zmm1") _,
        out("zmm12") _,
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
