// generated source. do not edit.
#![allow(non_upper_case_globals, unused_macros, unused_imports)]
use crate::low::macros::*;

// Copyright (c) 2024 The mlkem-native project authors
// Copyright (c) 2021-2022 Arm Limited
// Copyright (c) 2022 Matthias Kannwischer
// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT

// ----------------------------------------------------------------------------
// Keccak-f1600 permutation for SHA3, batch of two independent operations
// Input a[50], rc[24]; output a[50]
//
// The input/output argument is in effect two 25-element Keccak arrays
// a[0...24] and a[25..49], which could be considered as type a[25][2].
//
// Thinking of each such input/output array as a row-major flattening of a
// 5x5 matrix of 64-bit words, this performs the Keccak-f1600 permutation,
// all 24 rounds with the distinct round constants rc[i] for each one. For
// correct operation, the input pointer rc should point at the standard
// round constants as in the specification:
//
//   https://keccak.team/keccak_specs_summary.html#roundConstants
//
// This operation is at the core of SHA3 and is fully specified here:
//
//   https://keccak.team/files/Keccak-reference-3.0.pdf
//
// extern void sha3_keccak2_f1600(uint64_t a[static 50],
//                                const uint64_t rc[static 24]);
//
// Standard ARM ABI: X0 = a, X1 = rc
// ----------------------------------------------------------------------------

/// Keccak-f1600 permutation for SHA3, batch of two independent operations
///
/// Input a[50], rc[24]; output a[50]
///
/// The input/output argument is in effect two 25-element Keccak arrays
/// a[0...24] and a[25..49], which could be considered as type a[25][2].
///
/// Thinking of each such input/output array as a row-major flattening of a
/// 5x5 matrix of 64-bit words, this performs the Keccak-f1600 permutation,
/// all 24 rounds with the distinct round constants rc[i] for each one. For
/// correct operation, the input pointer rc should point at the standard
/// round constants as in the specification:
///
///   https://keccak.team/keccak_specs_summary.html#roundConstants
///
/// This operation is at the core of SHA3 and is fully specified here:
///
///   https://keccak.team/files/Keccak-reference-3.0.pdf
#[target_feature(enable = "sha3")]
pub(crate) fn sha3_keccak2_f1600(a: &mut [[u64; 25]; 2], rc: &[u64; 24]) {
    // SAFETY: inline assembly. see [crate::low::inline_assembly_safety] for safety info.
    unsafe {
        core::arch::asm!(

        // This is similar to the code in the mlkem-native repository here:
        //
        //   mlkem/fips202/native/aarch64/src/keccak_f1600_x2_v84a_asm.S
        //
        // The main difference is the avoidance of ld2/st2 in favour of explicit
        // transposition operations and conventional loads and stores.

        Q!("    sub             " "sp, sp, # (64 + 0)"),
        Q!("    stp             " "d8, d9, [sp, # (0 + 0)]"),
        Q!("    stp             " "d10, d11, [sp, # (16 + 0)]"),
        Q!("    stp             " "d12, d13, [sp, # (32 + 0)]"),
        Q!("    stp             " "d14, d15, [sp, # (48 + 0)]"),

        // Load the Keccak initial state into upper and lower parts of Q0..Q24

        Q!("    add             " "x2, x0, #0xc8"),

        Q!("    ldp             " "q24, q25, [x0]"),
        Q!("    ldp             " "q26, q27, [x2]"),
        Q!("    trn1            " "v0.2d, v24.2d, v26.2d"),
        Q!("    trn2            " "v1.2d, v24.2d, v26.2d"),
        Q!("    trn1            " "v2.2d, v25.2d, v27.2d"),
        Q!("    trn2            " "v3.2d, v25.2d, v27.2d"),

        Q!("    ldp             " "q24, q25, [x0, #0x20]"),
        Q!("    ldp             " "q26, q27, [x2, #0x20]"),
        Q!("    trn1            " "v4.2d, v24.2d, v26.2d"),
        Q!("    trn2            " "v5.2d, v24.2d, v26.2d"),
        Q!("    trn1            " "v6.2d, v25.2d, v27.2d"),
        Q!("    trn2            " "v7.2d, v25.2d, v27.2d"),

        Q!("    ldp             " "q24, q25, [x0, #0x40]"),
        Q!("    ldp             " "q26, q27, [x2, #0x40]"),
        Q!("    trn1            " "v8.2d, v24.2d, v26.2d"),
        Q!("    trn2            " "v9.2d, v24.2d, v26.2d"),
        Q!("    trn1            " "v10.2d, v25.2d, v27.2d"),
        Q!("    trn2            " "v11.2d, v25.2d, v27.2d"),

        Q!("    ldp             " "q24, q25, [x0, #0x60]"),
        Q!("    ldp             " "q26, q27, [x2, #0x60]"),
        Q!("    trn1            " "v12.2d, v24.2d, v26.2d"),
        Q!("    trn2            " "v13.2d, v24.2d, v26.2d"),
        Q!("    trn1            " "v14.2d, v25.2d, v27.2d"),
        Q!("    trn2            " "v15.2d, v25.2d, v27.2d"),

        Q!("    ldp             " "q24, q25, [x0, #0x80]"),
        Q!("    ldp             " "q26, q27, [x2, #0x80]"),
        Q!("    trn1            " "v16.2d, v24.2d, v26.2d"),
        Q!("    trn2            " "v17.2d, v24.2d, v26.2d"),
        Q!("    trn1            " "v18.2d, v25.2d, v27.2d"),
        Q!("    trn2            " "v19.2d, v25.2d, v27.2d"),

        Q!("    ldp             " "q24, q25, [x0, #0xa0]"),
        Q!("    ldp             " "q26, q27, [x2, #0xa0]"),
        Q!("    trn1            " "v20.2d, v24.2d, v26.2d"),
        Q!("    trn2            " "v21.2d, v24.2d, v26.2d"),
        Q!("    trn1            " "v22.2d, v25.2d, v27.2d"),
        Q!("    trn2            " "v23.2d, v25.2d, v27.2d"),

        Q!("    ldr             " "d24, [x0, #0xc0]"),
        Q!("    ldr             " "d25, [x2, #0xc0]"),
        Q!("    trn1            " "v24.2d, v24.2d, v25.2d"),

        // Now 24 rounds of the iteration

        Q!("    mov             " "x2, #24"),

        Q!(Label!("Lsha3_keccak2_f1600_loop", 2) ":"),
        Q!("    eor3            " "v30.16b, v0.16b, v5.16b, v10.16b"),
        Q!("    eor3            " "v29.16b, v1.16b, v6.16b, v11.16b"),
        Q!("    eor3            " "v28.16b, v2.16b, v7.16b, v12.16b"),
        Q!("    eor3            " "v27.16b, v3.16b, v8.16b, v13.16b"),
        Q!("    eor3            " "v26.16b, v4.16b, v9.16b, v14.16b"),
        Q!("    eor3            " "v30.16b, v30.16b, v15.16b, v20.16b"),
        Q!("    eor3            " "v29.16b, v29.16b, v16.16b, v21.16b"),
        Q!("    eor3            " "v28.16b, v28.16b, v17.16b, v22.16b"),
        Q!("    eor3            " "v27.16b, v27.16b, v18.16b, v23.16b"),
        Q!("    eor3            " "v26.16b, v26.16b, v19.16b, v24.16b"),
        Q!("    rax1            " "v25.2d, v30.2d, v28.2d"),
        Q!("    rax1            " "v28.2d, v28.2d, v26.2d"),
        Q!("    rax1            " "v26.2d, v26.2d, v29.2d"),
        Q!("    rax1            " "v29.2d, v29.2d, v27.2d"),
        Q!("    rax1            " "v27.2d, v27.2d, v30.2d"),
        Q!("    eor             " "v30.16b, v0.16b, v26.16b"),
        Q!("    xar             " "v0.2d, v2.2d, v29.2d, #0x2"),
        Q!("    xar             " "v2.2d, v12.2d, v29.2d, #0x15"),
        Q!("    xar             " "v12.2d, v13.2d, v28.2d, #0x27"),
        Q!("    xar             " "v13.2d, v19.2d, v27.2d, #0x38"),
        Q!("    xar             " "v19.2d, v23.2d, v28.2d, #0x8"),
        Q!("    xar             " "v23.2d, v15.2d, v26.2d, #0x17"),
        Q!("    xar             " "v15.2d, v1.2d, v25.2d, #0x3f"),
        Q!("    xar             " "v1.2d, v8.2d, v28.2d, #0x9"),
        Q!("    xar             " "v8.2d, v16.2d, v25.2d, #0x13"),
        Q!("    xar             " "v16.2d, v7.2d, v29.2d, #0x3a"),
        Q!("    xar             " "v7.2d, v10.2d, v26.2d, #0x3d"),
        Q!("    xar             " "v10.2d, v3.2d, v28.2d, #0x24"),
        Q!("    xar             " "v3.2d, v18.2d, v28.2d, #0x2b"),
        Q!("    xar             " "v18.2d, v17.2d, v29.2d, #0x31"),
        Q!("    xar             " "v17.2d, v11.2d, v25.2d, #0x36"),
        Q!("    xar             " "v11.2d, v9.2d, v27.2d, #0x2c"),
        Q!("    xar             " "v9.2d, v22.2d, v29.2d, #0x3"),
        Q!("    xar             " "v22.2d, v14.2d, v27.2d, #0x19"),
        Q!("    xar             " "v14.2d, v20.2d, v26.2d, #0x2e"),
        Q!("    xar             " "v20.2d, v4.2d, v27.2d, #0x25"),
        Q!("    xar             " "v4.2d, v24.2d, v27.2d, #0x32"),
        Q!("    xar             " "v24.2d, v21.2d, v25.2d, #0x3e"),
        Q!("    xar             " "v21.2d, v5.2d, v26.2d, #0x1c"),
        Q!("    xar             " "v27.2d, v6.2d, v25.2d, #0x14"),
        Q!("    ld1r            " "{{ v31.2d }}, [x1], #8"),
        Q!("    bcax            " "v5.16b, v10.16b, v7.16b, v11.16b"),
        Q!("    bcax            " "v6.16b, v11.16b, v8.16b, v7.16b"),
        Q!("    bcax            " "v7.16b, v7.16b, v9.16b, v8.16b"),
        Q!("    bcax            " "v8.16b, v8.16b, v10.16b, v9.16b"),
        Q!("    bcax            " "v9.16b, v9.16b, v11.16b, v10.16b"),
        Q!("    bcax            " "v10.16b, v15.16b, v12.16b, v16.16b"),
        Q!("    bcax            " "v11.16b, v16.16b, v13.16b, v12.16b"),
        Q!("    bcax            " "v12.16b, v12.16b, v14.16b, v13.16b"),
        Q!("    bcax            " "v13.16b, v13.16b, v15.16b, v14.16b"),
        Q!("    bcax            " "v14.16b, v14.16b, v16.16b, v15.16b"),
        Q!("    bcax            " "v15.16b, v20.16b, v17.16b, v21.16b"),
        Q!("    bcax            " "v16.16b, v21.16b, v18.16b, v17.16b"),
        Q!("    bcax            " "v17.16b, v17.16b, v19.16b, v18.16b"),
        Q!("    bcax            " "v18.16b, v18.16b, v20.16b, v19.16b"),
        Q!("    bcax            " "v19.16b, v19.16b, v21.16b, v20.16b"),
        Q!("    bcax            " "v20.16b, v0.16b, v22.16b, v1.16b"),
        Q!("    bcax            " "v21.16b, v1.16b, v23.16b, v22.16b"),
        Q!("    bcax            " "v22.16b, v22.16b, v24.16b, v23.16b"),
        Q!("    bcax            " "v23.16b, v23.16b, v0.16b, v24.16b"),
        Q!("    bcax            " "v24.16b, v24.16b, v1.16b, v0.16b"),
        Q!("    bcax            " "v0.16b, v30.16b, v2.16b, v27.16b"),
        Q!("    bcax            " "v1.16b, v27.16b, v3.16b, v2.16b"),
        Q!("    bcax            " "v2.16b, v2.16b, v4.16b, v3.16b"),
        Q!("    bcax            " "v3.16b, v3.16b, v30.16b, v4.16b"),
        Q!("    bcax            " "v4.16b, v4.16b, v27.16b, v30.16b"),
        Q!("    eor             " "v0.16b, v0.16b, v31.16b"),
        Q!("    sub             " "x2, x2, #0x1"),
        Q!("    cbnz            " "x2, " Label!("Lsha3_keccak2_f1600_loop", 2, Before)),

        // Store back the state

        Q!("    add             " "x2, x0, #0xc8"),

        Q!("    trn1            " "v25.2d, v0.2d, v1.2d"),
        Q!("    trn1            " "v26.2d, v2.2d, v3.2d"),
        Q!("    stp             " "q25, q26, [x0]"),
        Q!("    trn2            " "v25.2d, v0.2d, v1.2d"),
        Q!("    trn2            " "v26.2d, v2.2d, v3.2d"),
        Q!("    stp             " "q25, q26, [x2]"),

        Q!("    trn1            " "v25.2d, v4.2d, v5.2d"),
        Q!("    trn1            " "v26.2d, v6.2d, v7.2d"),
        Q!("    stp             " "q25, q26, [x0, #0x20]"),
        Q!("    trn2            " "v25.2d, v4.2d, v5.2d"),
        Q!("    trn2            " "v26.2d, v6.2d, v7.2d"),
        Q!("    stp             " "q25, q26, [x2, #0x20]"),

        Q!("    trn1            " "v25.2d, v8.2d, v9.2d"),
        Q!("    trn1            " "v26.2d, v10.2d, v11.2d"),
        Q!("    stp             " "q25, q26, [x0, #0x40]"),
        Q!("    trn2            " "v25.2d, v8.2d, v9.2d"),
        Q!("    trn2            " "v26.2d, v10.2d, v11.2d"),
        Q!("    stp             " "q25, q26, [x2, #0x40]"),

        Q!("    trn1            " "v25.2d, v12.2d, v13.2d"),
        Q!("    trn1            " "v26.2d, v14.2d, v15.2d"),
        Q!("    stp             " "q25, q26, [x0, #0x60]"),
        Q!("    trn2            " "v25.2d, v12.2d, v13.2d"),
        Q!("    trn2            " "v26.2d, v14.2d, v15.2d"),
        Q!("    stp             " "q25, q26, [x2, #0x60]"),

        Q!("    trn1            " "v25.2d, v16.2d, v17.2d"),
        Q!("    trn1            " "v26.2d, v18.2d, v19.2d"),
        Q!("    stp             " "q25, q26, [x0, #0x80]"),
        Q!("    trn2            " "v25.2d, v16.2d, v17.2d"),
        Q!("    trn2            " "v26.2d, v18.2d, v19.2d"),
        Q!("    stp             " "q25, q26, [x2, #0x80]"),

        Q!("    trn1            " "v25.2d, v20.2d, v21.2d"),
        Q!("    trn1            " "v26.2d, v22.2d, v23.2d"),
        Q!("    stp             " "q25, q26, [x0, #0xa0]"),
        Q!("    trn2            " "v25.2d, v20.2d, v21.2d"),
        Q!("    trn2            " "v26.2d, v22.2d, v23.2d"),
        Q!("    stp             " "q25, q26, [x2, #0xa0]"),

        Q!("    str             " "d24, [x0, #0xc0]"),
        Q!("    trn2            " "v24.2d, v24.2d, v24.2d"),
        Q!("    str             " "d24, [x2, #0xc0]"),

        // Restore registers and return

        Q!("    ldp             " "d8, d9, [sp, # (0 + 0)]"),
        Q!("    ldp             " "d10, d11, [sp, # (16 + 0)]"),
        Q!("    ldp             " "d12, d13, [sp, # (32 + 0)]"),
        Q!("    ldp             " "d14, d15, [sp, # (48 + 0)]"),
        Q!("    add             " "sp, sp, # (64 + 0)"),
        inout("x0") a.as_mut_ptr() => _,
        inout("x1") rc.as_ptr() => _,
        // clobbers
        out("v0") _,
        out("v1") _,
        out("v10") _,
        out("v11") _,
        out("v12") _,
        out("v13") _,
        out("v14") _,
        out("v15") _,
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
        out("v8") _,
        out("v9") _,
        out("x2") _,
            )
    };
}
