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
// Standard ARM ABI: X0 = a
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

        // This matches the code in the mlkem-native repository
        // https://github.com/pq-code-package/mlkem-native/blob/main/mlkem/native/aarch64/src/poly_tomont_asm.S

        Q!("    mov             " "w2, #0xd01"),
        Q!("    dup             " "v4.8h, w2"),
        Q!("    mov             " "w2, #0x4ebf"),
        Q!("    dup             " "v5.8h, w2"),
        Q!("    mov             " "w2, #-0x414"),
        Q!("    dup             " "v2.8h, w2"),
        Q!("    mov             " "w2, #-0x2824"),
        Q!("    dup             " "v3.8h, w2"),
        Q!("    mov             " "x1, #0x8"),
        Q!("    ldr             " "q26, [x0, #0x30]"),
        Q!("    ldr             " "q23, [x0, #0x10]"),
        Q!("    mul             " "v17.8h, v26.8h, v2.8h"),
        Q!("    sqrdmulh        " "v7.8h, v26.8h, v3.8h"),
        Q!("    ldr             " "q27, [x0, #0x20]"),
        Q!("    sub             " "x1, x1, #0x1"),

        Q!(Label!("mlkem_tomont_loop", 2) ":"),
        Q!("    mls             " "v17.8h, v7.8h, v4.h[0]"),
        Q!("    sqrdmulh        " "v5.8h, v23.8h, v3.8h"),
        Q!("    ldr             " "q7, [x0], #0x40"),
        Q!("    stur            " "q17, [x0, #-0x10]"),
        Q!("    sqrdmulh        " "v29.8h, v27.8h, v3.8h"),
        Q!("    sqrdmulh        " "v19.8h, v7.8h, v3.8h"),
        Q!("    mul             " "v25.8h, v23.8h, v2.8h"),
        Q!("    mul             " "v0.8h, v7.8h, v2.8h"),
        Q!("    mul             " "v26.8h, v27.8h, v2.8h"),
        Q!("    ldr             " "q7, [x0, #0x30]"),
        Q!("    mls             " "v25.8h, v5.8h, v4.h[0]"),
        Q!("    ldr             " "q23, [x0, #0x10]"),
        Q!("    mls             " "v26.8h, v29.8h, v4.h[0]"),
        Q!("    mls             " "v0.8h, v19.8h, v4.h[0]"),
        Q!("    stur            " "q25, [x0, #-0x30]"),
        Q!("    mul             " "v17.8h, v7.8h, v2.8h"),
        Q!("    sqrdmulh        " "v7.8h, v7.8h, v3.8h"),
        Q!("    stur            " "q0, [x0, #-0x40]"),
        Q!("    ldr             " "q27, [x0, #0x20]"),
        Q!("    stur            " "q26, [x0, #-0x20]"),
        Q!("    sub             " "x1, x1, #0x1"),
        Q!("    cbnz            " "x1, " Label!("mlkem_tomont_loop", 2, Before)),

        Q!("    mls             " "v17.8h, v7.8h, v4.h[0]"),
        Q!("    sqrdmulh        " "v7.8h, v23.8h, v3.8h"),
        Q!("    mul             " "v26.8h, v23.8h, v2.8h"),
        Q!("    sqrdmulh        " "v25.8h, v27.8h, v3.8h"),
        Q!("    ldr             " "q23, [x0], #0x40"),
        Q!("    mul             " "v27.8h, v27.8h, v2.8h"),
        Q!("    mls             " "v26.8h, v7.8h, v4.h[0]"),
        Q!("    sqrdmulh        " "v7.8h, v23.8h, v3.8h"),
        Q!("    mul             " "v23.8h, v23.8h, v2.8h"),
        Q!("    stur            " "q17, [x0, #-0x10]"),
        Q!("    mls             " "v27.8h, v25.8h, v4.h[0]"),
        Q!("    stur            " "q26, [x0, #-0x30]"),
        Q!("    mls             " "v23.8h, v7.8h, v4.h[0]"),
        Q!("    stur            " "q27, [x0, #-0x20]"),
        Q!("    stur            " "q23, [x0, #-0x40]"),
        inout("x0") a.as_mut_ptr() => _,
        // clobbers
        out("v0") _,
        out("v17") _,
        out("v19") _,
        out("v2") _,
        out("v23") _,
        out("v25") _,
        out("v26") _,
        out("v27") _,
        out("v29") _,
        out("v3") _,
        out("v4") _,
        out("v5") _,
        out("v7") _,
        out("x1") _,
        out("x2") _,
            )
    };
}
