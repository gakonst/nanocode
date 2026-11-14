// generated source. do not edit.
#![allow(non_upper_case_globals, unused_macros, unused_imports)]
use crate::low::macros::*;

// Copyright (c) 2024 The mlkem-native project authors
// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT

// ----------------------------------------------------------------------------
// Precompute the mulcache data for a polynomial in the NTT domain
// Inputs a[256], qdata[624] (all signed 16-bit words); output x[128] (signed 16-bit words)
//
// The input array a is assumed to represent 128 linear polynomials
// in the NTT domain, p_i = a[f(i)] + a[g(i)] * X where each p_i is in
// Fq[X]/(X^2-zeta^i'), with zeta^i' being a power of zeta = 17, with i
// bit-reversed as used for NTTs, and f(i) and g(i) are ordering functions
// that map the coefficients per the specific order used in the AVX2
// implementation. For each such polynomial, the mulcache value is
// a[g(i)] * zeta^i' (modulo 3329 as usual), a value useful to
// perform base multiplication of polynomials efficiently. The The second
// parameter is expected to point to a table of constants whose definitions
// can be found in the mlkem-native repo or our "tests/test.c".
//
// extern void mlkem_mulcache_compute_x86
//      (int16_t x[static 128],const int16_t a[static 256],
//       const int16_t qdata[static 624]);
//
// Standard x86-64 ABI: RDI = x, RSI = a, RDX = qdata
// Microsoft x64 ABI:   RCX = x, RDX = a, R8 = qdata
// ----------------------------------------------------------------------------

/// Precompute the mulcache data for a polynomial in the NTT domain
///
/// Inputs a[256], qdata[624] (all signed 16-bit words); output x[128] (signed 16-bit words)
///
/// The input array a is assumed to represent 128 linear polynomials
/// in the NTT domain, p_i = a[f(i)] + a[g(i)] * X where each p_i is in
/// Fq[X]/(X^2-zeta^i'), with zeta^i' being a power of zeta = 17, with i
/// bit-reversed as used for NTTs, and f(i) and g(i) are ordering functions
/// that map the coefficients per the specific order used in the AVX2
/// implementation. For each such polynomial, the mulcache value is
/// a[g(i)] * zeta^i' (modulo 3329 as usual), a value useful to
/// perform base multiplication of polynomials efficiently. The The second
/// parameter is expected to point to a table of constants whose definitions
/// can be found in the mlkem-native repo or our "tests/test.c".
pub(crate) fn mlkem_mulcache_compute(x: &mut [i16; 128], a: &[i16; 256], qdata: &[i16; 624]) {
    // SAFETY: inline assembly. see [crate::low::inline_assembly_safety] for safety info.
    unsafe {
        core::arch::asm!(

        Q!("    endbr64         " ),

        Q!("    mov             " "eax, 0xd010d01"),
        Q!("    vmovd           " "xmm0, eax"),
        Q!("    vpbroadcastd    " "ymm0, xmm0"),
        Q!("    vmovdqa         " "ymm2, [32 + rsi]"),
        Q!("    vmovdqa         " "ymm3, [96 + rsi]"),
        Q!("    vmovdqa         " "ymm4, [992 + rdx]"),
        Q!("    vmovdqa         " "ymm1, [1120 + rdx]"),
        Q!("    vpmullw         " "ymm5, ymm1, ymm2"),
        Q!("    vpmullw         " "ymm6, ymm1, ymm3"),
        Q!("    vpmulhw         " "ymm7, ymm4, ymm2"),
        Q!("    vpmulhw         " "ymm8, ymm4, ymm3"),
        Q!("    vpmulhw         " "ymm9, ymm0, ymm5"),
        Q!("    vpmulhw         " "ymm10, ymm0, ymm6"),
        Q!("    vpsubw          " "ymm7, ymm7, ymm9"),
        Q!("    vpsubw          " "ymm8, ymm8, ymm10"),
        Q!("    vmovdqa         " "[rdi], ymm7"),
        Q!("    vmovdqa         " "[32 + rdi], ymm8"),
        Q!("    vmovdqa         " "ymm2, [160 + rsi]"),
        Q!("    vmovdqa         " "ymm3, [224 + rsi]"),
        Q!("    vmovdqa         " "ymm4, [1024 + rdx]"),
        Q!("    vmovdqa         " "ymm1, [1152 + rdx]"),
        Q!("    vpmullw         " "ymm5, ymm1, ymm2"),
        Q!("    vpmullw         " "ymm6, ymm1, ymm3"),
        Q!("    vpmulhw         " "ymm7, ymm4, ymm2"),
        Q!("    vpmulhw         " "ymm8, ymm4, ymm3"),
        Q!("    vpmulhw         " "ymm9, ymm0, ymm5"),
        Q!("    vpmulhw         " "ymm10, ymm0, ymm6"),
        Q!("    vpsubw          " "ymm7, ymm7, ymm9"),
        Q!("    vpsubw          " "ymm8, ymm8, ymm10"),
        Q!("    vmovdqa         " "[64 + rdi], ymm7"),
        Q!("    vmovdqa         " "[96 + rdi], ymm8"),
        Q!("    vmovdqa         " "ymm2, [288 + rsi]"),
        Q!("    vmovdqa         " "ymm3, [352 + rsi]"),
        Q!("    vmovdqa         " "ymm4, [1056 + rdx]"),
        Q!("    vmovdqa         " "ymm1, [1184 + rdx]"),
        Q!("    vpmullw         " "ymm5, ymm1, ymm2"),
        Q!("    vpmullw         " "ymm6, ymm1, ymm3"),
        Q!("    vpmulhw         " "ymm7, ymm4, ymm2"),
        Q!("    vpmulhw         " "ymm8, ymm4, ymm3"),
        Q!("    vpmulhw         " "ymm9, ymm0, ymm5"),
        Q!("    vpmulhw         " "ymm10, ymm0, ymm6"),
        Q!("    vpsubw          " "ymm7, ymm7, ymm9"),
        Q!("    vpsubw          " "ymm8, ymm8, ymm10"),
        Q!("    vmovdqa         " "[128 + rdi], ymm7"),
        Q!("    vmovdqa         " "[160 + rdi], ymm8"),
        Q!("    vmovdqa         " "ymm2, [416 + rsi]"),
        Q!("    vmovdqa         " "ymm3, [480 + rsi]"),
        Q!("    vmovdqa         " "ymm4, [1088 + rdx]"),
        Q!("    vmovdqa         " "ymm1, [1216 + rdx]"),
        Q!("    vpmullw         " "ymm5, ymm1, ymm2"),
        Q!("    vpmullw         " "ymm6, ymm1, ymm3"),
        Q!("    vpmulhw         " "ymm7, ymm4, ymm2"),
        Q!("    vpmulhw         " "ymm8, ymm4, ymm3"),
        Q!("    vpmulhw         " "ymm9, ymm0, ymm5"),
        Q!("    vpmulhw         " "ymm10, ymm0, ymm6"),
        Q!("    vpsubw          " "ymm7, ymm7, ymm9"),
        Q!("    vpsubw          " "ymm8, ymm8, ymm10"),
        Q!("    vmovdqa         " "[192 + rdi], ymm7"),
        Q!("    vmovdqa         " "[224 + rdi], ymm8"),

        inout("rdi") x.as_mut_ptr() => _,
        inout("rsi") a.as_ptr() => _,
        inout("rdx") qdata.as_ptr() => _,
        // clobbers
        out("rax") _,
        out("zmm0") _,
        out("zmm1") _,
        out("zmm10") _,
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
