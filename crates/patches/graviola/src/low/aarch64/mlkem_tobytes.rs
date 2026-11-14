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
//
// Standard ARM ABI: X0 = r, X1 = a
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

        // This code is essentially a verbatim copy of the mlkem-native version
        // https://github.com/pq-code-package/mlkem-native/blob/main/mlkem/native/aarch64/src/poly_tobytes_asm.S

        Q!("    mov             " "x2, #0x10"),
        Q!("    ldr             " "q6, [x1], #0x20"),
        Q!("    ldur            " "q24, [x1, #-0x10]"),
        Q!("    ldr             " "q30, [x1], #0x20"),
        Q!("    ldur            " "q22, [x1, #-0x10]"),
        Q!("    ldr             " "q5, [x1], #0x20"),
        Q!("    ldur            " "q17, [x1, #-0x10]"),
        Q!("    ldr             " "q19, [x1], #0x20"),
        Q!("    ldur            " "q4, [x1, #-0x10]"),
        Q!("    lsr             " "x2, x2, #2"),
        Q!("    sub             " "x2, x2, #0x1"),

        Q!(Label!("mlkem_tobytes_asm_asm_loop_start", 2) ":"),
        Q!("    uzp1            " "v25.8h, v6.8h, v24.8h"),
        Q!("    uzp2            " "v6.8h, v6.8h, v24.8h"),
        Q!("    xtn             " "v24.8b, v25.8h"),
        Q!("    shrn            " "v25.8b, v25.8h, #0x8"),
        Q!("    xtn             " "v18.8b, v6.8h"),
        Q!("    shrn            " "v26.8b, v6.8h, #0x4"),
        Q!("    sli             " "v25.8b, v18.8b, #0x4"),
        Q!("    st3             " "{{ v24.8b, v25.8b, v26.8b }}, [x0], #24"),
        Q!("    uzp1            " "v25.8h, v30.8h, v22.8h"),
        Q!("    uzp2            " "v6.8h, v30.8h, v22.8h"),
        Q!("    xtn             " "v24.8b, v25.8h"),
        Q!("    xtn             " "v18.8b, v6.8h"),
        Q!("    uzp1            " "v30.8h, v5.8h, v17.8h"),
        Q!("    uzp2            " "v22.8h, v5.8h, v17.8h"),
        Q!("    xtn             " "v5.8b, v30.8h"),
        Q!("    xtn             " "v17.8b, v22.8h"),
        Q!("    uzp1            " "v28.8h, v19.8h, v4.8h"),
        Q!("    uzp2            " "v19.8h, v19.8h, v4.8h"),
        Q!("    xtn             " "v4.8b, v28.8h"),
        Q!("    xtn             " "v20.8b, v19.8h"),
        Q!("    shrn            " "v25.8b, v25.8h, #0x8"),
        Q!("    sli             " "v25.8b, v18.8b, #0x4"),
        Q!("    shrn            " "v26.8b, v6.8h, #0x4"),
        Q!("    st3             " "{{ v24.8b, v25.8b, v26.8b }}, [x0], #24"),
        Q!("    shrn            " "v6.8b, v30.8h, #0x8"),
        Q!("    sli             " "v6.8b, v17.8b, #0x4"),
        Q!("    shrn            " "v7.8b, v22.8h, #0x4"),
        Q!("    st3             " "{{ v5.8b, v6.8b, v7.8b }}, [x0], #24"),
        Q!("    shrn            " "v5.8b, v28.8h, #0x8"),
        Q!("    shrn            " "v6.8b, v19.8h, #0x4"),
        Q!("    sli             " "v5.8b, v20.8b, #0x4"),
        Q!("    st3             " "{{ v4.8b, v5.8b, v6.8b }}, [x0], #24"),
        Q!("    ldr             " "q6, [x1], #0x20"),
        Q!("    ldur            " "q24, [x1, #-0x10]"),
        Q!("    ldr             " "q30, [x1], #0x20"),
        Q!("    ldur            " "q22, [x1, #-0x10]"),
        Q!("    ldr             " "q5, [x1], #0x20"),
        Q!("    ldur            " "q17, [x1, #-0x10]"),
        Q!("    ldr             " "q19, [x1], #0x20"),
        Q!("    ldur            " "q4, [x1, #-0x10]"),
        Q!("    sub             " "x2, x2, #0x1"),
        Q!("    cbnz            " "x2, " Label!("mlkem_tobytes_asm_asm_loop_start", 2, Before)),
        Q!("    uzp1            " "v25.8h, v30.8h, v22.8h"),
        Q!("    uzp2            " "v18.8h, v30.8h, v22.8h"),
        Q!("    uzp1            " "v30.8h, v6.8h, v24.8h"),
        Q!("    uzp2            " "v6.8h, v6.8h, v24.8h"),
        Q!("    uzp1            " "v24.8h, v5.8h, v17.8h"),
        Q!("    uzp2            " "v22.8h, v5.8h, v17.8h"),
        Q!("    uzp1            " "v5.8h, v19.8h, v4.8h"),
        Q!("    uzp2            " "v17.8h, v19.8h, v4.8h"),
        Q!("    xtn             " "v19.8b, v25.8h"),
        Q!("    shrn            " "v20.8b, v25.8h, #0x8"),
        Q!("    xtn             " "v25.8b, v18.8h"),
        Q!("    shrn            " "v21.8b, v18.8h, #0x4"),
        Q!("    xtn             " "v28.8b, v30.8h"),
        Q!("    shrn            " "v29.8b, v30.8h, #0x8"),
        Q!("    xtn             " "v18.8b, v6.8h"),
        Q!("    shrn            " "v30.8b, v6.8h, #0x4"),
        Q!("    xtn             " "v1.8b, v24.8h"),
        Q!("    shrn            " "v2.8b, v24.8h, #0x8"),
        Q!("    xtn             " "v6.8b, v22.8h"),
        Q!("    shrn            " "v3.8b, v22.8h, #0x4"),
        Q!("    xtn             " "v22.8b, v5.8h"),
        Q!("    shrn            " "v23.8b, v5.8h, #0x8"),
        Q!("    xtn             " "v5.8b, v17.8h"),
        Q!("    shrn            " "v24.8b, v17.8h, #0x4"),
        Q!("    sli             " "v20.8b, v25.8b, #0x4"),
        Q!("    sli             " "v29.8b, v18.8b, #0x4"),
        Q!("    st3             " "{{ v28.8b, v29.8b, v30.8b }}, [x0], #24"),
        Q!("    st3             " "{{ v19.8b, v20.8b, v21.8b }}, [x0], #24"),
        Q!("    sli             " "v2.8b, v6.8b, #0x4"),
        Q!("    st3             " "{{ v1.8b, v2.8b, v3.8b }}, [x0], #24"),
        Q!("    sli             " "v23.8b, v5.8b, #0x4"),
        Q!("    st3             " "{{ v22.8b, v23.8b, v24.8b }}, [x0], #24"),
        inout("x0") r.as_mut_ptr() => _,
        inout("x1") a.as_ptr() => _,
        // clobbers
        out("v1") _,
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
        out("v28") _,
        out("v29") _,
        out("v3") _,
        out("v30") _,
        out("v4") _,
        out("v5") _,
        out("v6") _,
        out("v7") _,
        out("x2") _,
            )
    };
}
