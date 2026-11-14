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
// Standard ARM ABI: X0 = a
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

        // This matches the code in the mlkem-native repository
        // https://github.com/pq-code-package/mlkem-native/blob/main/mlkem/native/aarch64/src/poly_reduce_asm.S

        Q!("    mov             " "w2, #0xd01"),
        Q!("    dup             " "v3.8h, w2"),
        Q!("    mov             " "w2, #0x4ebf"),
        Q!("    dup             " "v4.8h, w2"),
        Q!("    mov             " "x1, #0x8"),
        Q!("    ldr             " "q21, [x0, #0x20]"),
        Q!("    ldr             " "q23, [x0, #0x30]"),
        Q!("    sqdmulh         " "v7.8h, v21.8h, v4.h[0]"),
        Q!("    sqdmulh         " "v30.8h, v23.8h, v4.h[0]"),
        Q!("    srshr           " "v7.8h, v7.8h, #0xb"),
        Q!("    srshr           " "v30.8h, v30.8h, #0xb"),
        Q!("    mls             " "v21.8h, v7.8h, v3.h[0]"),
        Q!("    mls             " "v23.8h, v30.8h, v3.h[0]"),
        Q!("    ldr             " "q5, [x0, #0x10]"),
        Q!("    sshr            " "v7.8h, v21.8h, #0xf"),
        Q!("    sshr            " "v30.8h, v23.8h, #0xf"),
        Q!("    and             " "v7.16b, v3.16b, v7.16b"),
        Q!("    add             " "v21.8h, v21.8h, v7.8h"),
        Q!("    and             " "v7.16b, v3.16b, v30.16b"),
        Q!("    add             " "v16.8h, v23.8h, v7.8h"),
        Q!("    sub             " "x1, x1, #0x1"),

        Q!(Label!("mlkem_reduce_loop", 2) ":"),
        Q!("    ldr             " "q6, [x0], #0x40"),
        Q!("    ldr             " "q30, [x0, #0x20]"),
        Q!("    sqdmulh         " "v31.8h, v6.8h, v4.h[0]"),
        Q!("    sqdmulh         " "v29.8h, v5.8h, v4.h[0]"),
        Q!("    sqdmulh         " "v22.8h, v30.8h, v4.h[0]"),
        Q!("    stur            " "q16, [x0, #-0x10]"),
        Q!("    srshr           " "v20.8h, v31.8h, #0xb"),
        Q!("    srshr           " "v28.8h, v29.8h, #0xb"),
        Q!("    stur            " "q21, [x0, #-0x20]"),
        Q!("    mls             " "v6.8h, v20.8h, v3.h[0]"),
        Q!("    mls             " "v5.8h, v28.8h, v3.h[0]"),
        Q!("    ldr             " "q2, [x0, #0x30]"),
        Q!("    sshr            " "v31.8h, v6.8h, #0xf"),
        Q!("    srshr           " "v19.8h, v22.8h, #0xb"),
        Q!("    and             " "v22.16b, v3.16b, v31.16b"),
        Q!("    add             " "v0.8h, v6.8h, v22.8h"),
        Q!("    mls             " "v30.8h, v19.8h, v3.h[0]"),
        Q!("    sshr            " "v26.8h, v5.8h, #0xf"),
        Q!("    sqdmulh         " "v25.8h, v2.8h, v4.h[0]"),
        Q!("    and             " "v17.16b, v3.16b, v26.16b"),
        Q!("    add             " "v1.8h, v5.8h, v17.8h"),
        Q!("    sshr            " "v31.8h, v30.8h, #0xf"),
        Q!("    srshr           " "v25.8h, v25.8h, #0xb"),
        Q!("    stur            " "q1, [x0, #-0x30]"),
        Q!("    and             " "v18.16b, v3.16b, v31.16b"),
        Q!("    mls             " "v2.8h, v25.8h, v3.h[0]"),
        Q!("    add             " "v21.8h, v30.8h, v18.8h"),
        Q!("    ldr             " "q5, [x0, #0x10]"),
        Q!("    sshr            " "v18.8h, v2.8h, #0xf"),
        Q!("    stur            " "q0, [x0, #-0x40]"),
        Q!("    and             " "v27.16b, v3.16b, v18.16b"),
        Q!("    add             " "v16.8h, v2.8h, v27.8h"),
        Q!("    sub             " "x1, x1, #0x1"),
        Q!("    cbnz            " "x1, " Label!("mlkem_reduce_loop", 2, Before)),
        Q!("    sqdmulh         " "v20.8h, v5.8h, v4.h[0]"),
        Q!("    ldr             " "q24, [x0], #0x40"),
        Q!("    stur            " "q21, [x0, #-0x20]"),
        Q!("    srshr           " "v20.8h, v20.8h, #0xb"),
        Q!("    sqdmulh         " "v25.8h, v24.8h, v4.h[0]"),
        Q!("    stur            " "q16, [x0, #-0x10]"),
        Q!("    mls             " "v5.8h, v20.8h, v3.h[0]"),
        Q!("    srshr           " "v20.8h, v25.8h, #0xb"),
        Q!("    sshr            " "v2.8h, v5.8h, #0xf"),
        Q!("    mls             " "v24.8h, v20.8h, v3.h[0]"),
        Q!("    and             " "v20.16b, v3.16b, v2.16b"),
        Q!("    add             " "v31.8h, v5.8h, v20.8h"),
        Q!("    sshr            " "v20.8h, v24.8h, #0xf"),
        Q!("    stur            " "q31, [x0, #-0x30]"),
        Q!("    and             " "v31.16b, v3.16b, v20.16b"),
        Q!("    add             " "v24.8h, v24.8h, v31.8h"),
        Q!("    stur            " "q24, [x0, #-0x40]"),
        inout("x0") a.as_mut_ptr() => _,
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
        out("v28") _,
        out("v29") _,
        out("v3") _,
        out("v30") _,
        out("v31") _,
        out("v4") _,
        out("v5") _,
        out("v6") _,
        out("v7") _,
        out("x1") _,
        out("x2") _,
            )
    };
}
