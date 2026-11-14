// generated source. do not edit.
#![allow(non_upper_case_globals, unused_macros, unused_imports)]
use crate::low::macros::*;

// Copyright (c) 2024 The mlkem-native project authors
// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT

// ----------------------------------------------------------------------------
// Precompute the mulcache data for a polynomial in the NTT domain
// Inputs a[256], z[128] and t[128] (signed 16-bit words); output x[128] (signed 16-bit words)
//
// The input array a is assumed to represent 128 linear polynomials
// in the NTT domain, p_i = a[2i] + a[2i+1] * X where each p_i is in
// Fq[X]/(X^2-zeta^i'), with zeta^i' being a power of zeta = 17, with i
// bit-reversed as used for NTTs. For each such polynomial, the mulcache
// value is a[2i+1] * zeta^i' (modulo 3329 as usual), a value useful to
// perform base multiplication of polynomials efficiently. The two other
// table arguments z = zetas and t = twisted zetas are expected to point
// to tables of zeta-related constants whose definitions can be found in
// the mlkem-native repo (mlkem/native/aarch64/src/aarch64_zetas.c) or
// our "tests/test.c", as "mulcache_zetas" and "mulcache_zetas_twisted"
//
// extern void mlkem_mulcache_compute
//      (int16_t x[static 128],const int16_t a[static 256],
//       const int16_t z[static 128],const int16_t t[static 128]);
//
// Standard ARM ABI: X0 = x, X1 = a, X2 = z, X3 = t
// ----------------------------------------------------------------------------

/// Precompute the mulcache data for a polynomial in the NTT domain
///
/// Inputs a[256], z[128] and t[128] (signed 16-bit words); output x[128] (signed 16-bit words)
///
/// The input array a is assumed to represent 128 linear polynomials
/// in the NTT domain, p_i = a[2i] + a[2i+1] * X where each p_i is in
/// Fq[X]/(X^2-zeta^i'), with zeta^i' being a power of zeta = 17, with i
/// bit-reversed as used for NTTs. For each such polynomial, the mulcache
/// value is a[2i+1] * zeta^i' (modulo 3329 as usual), a value useful to
/// perform base multiplication of polynomials efficiently. The two other
/// table arguments z = zetas and t = twisted zetas are expected to point
/// to tables of zeta-related constants whose definitions can be found in
/// the mlkem-native repo (mlkem/native/aarch64/src/aarch64_zetas.c) or
/// our "tests/test.c", as "mulcache_zetas" and "mulcache_zetas_twisted"
pub(crate) fn mlkem_mulcache_compute(
    x: &mut [i16; 128],
    a: &[i16; 256],
    z: &[i16; 128],
    t: &[i16; 128],
) {
    // SAFETY: inline assembly. see [crate::low::inline_assembly_safety] for safety info.
    unsafe {
        core::arch::asm!(

        // This matches the code in the mlkem-native repository
        // https://github.com/pq-code-package/mlkem-native/blob/main/mlkem/native/aarch64/src/poly_mulcache_compute_asm.S

        Q!("    mov             " "w5, #0xd01"),
        Q!("    dup             " "v6.8h, w5"),
        Q!("    mov             " "w5, #0x4ebf"),
        Q!("    dup             " "v7.8h, w5"),
        Q!("    mov             " "x4, #0x10"),
        Q!("    ldr             " "q1, [x1, #0x10]"),
        Q!("    ldr             " "q27, [x1], #0x20"),
        Q!("    ldr             " "q23, [x2], #0x10"),
        Q!("    uzp2            " "v27.8h, v27.8h, v1.8h"),
        Q!("    ldr             " "q1, [x3], #0x10"),
        Q!("    mul             " "v2.8h, v27.8h, v23.8h"),
        Q!("    sqrdmulh        " "v27.8h, v27.8h, v1.8h"),
        Q!("    sub             " "x4, x4, #0x1"),

        Q!(Label!("mlkem_mulcache_compute_loop", 2) ":"),
        Q!("    ldr             " "q29, [x1, #0x10]"),
        Q!("    ldr             " "q21, [x2], #0x10"),
        Q!("    mls             " "v2.8h, v27.8h, v6.h[0]"),
        Q!("    ldr             " "q27, [x1], #0x20"),
        Q!("    ldr             " "q7, [x3], #0x10"),
        Q!("    uzp2            " "v28.8h, v27.8h, v29.8h"),
        Q!("    str             " "q2, [x0], #0x10"),
        Q!("    mul             " "v2.8h, v28.8h, v21.8h"),
        Q!("    sqrdmulh        " "v27.8h, v28.8h, v7.8h"),
        Q!("    sub             " "x4, x4, #0x1"),
        Q!("    cbnz            " "x4, " Label!("mlkem_mulcache_compute_loop", 2, Before)),

        Q!("    mls             " "v2.8h, v27.8h, v6.h[0]"),
        Q!("    str             " "q2, [x0], #0x10"),
        inout("x0") x.as_mut_ptr() => _,
        inout("x1") a.as_ptr() => _,
        inout("x2") z.as_ptr() => _,
        inout("x3") t.as_ptr() => _,
        // clobbers
        out("v1") _,
        out("v2") _,
        out("v21") _,
        out("v23") _,
        out("v27") _,
        out("v28") _,
        out("v29") _,
        out("v6") _,
        out("v7") _,
        out("x4") _,
        out("x5") _,
            )
    };
}
