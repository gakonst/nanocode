// generated source. do not edit.
#![allow(non_upper_case_globals, unused_macros, unused_imports)]
use crate::low::macros::*;

// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

// ----------------------------------------------------------------------------
// Montgomery multiply, z := (x * y / 2^256) mod p_256
// Inputs x[4], y[4]; output z[4]
//
//    extern void bignum_montmul_p256(uint64_t z[static 4],
//                                    const uint64_t x[static 4],
//                                    const uint64_t y[static 4]);
//
// Does z := (2^{-256} * x * y) mod p_256, assuming that the inputs x and y
// satisfy x * y <= 2^256 * p_256 (in particular this is true if we are in
// the "usual" case x < p_256 and y < p_256).
//
// Standard ARM ABI: X0 = z, X1 = x, X2 = y
// ----------------------------------------------------------------------------

// bignum_montmul_p256 is functionally equivalent to
// unopt/bignum_montmul_p256_base.
// It is written in a way that
// 1. A subset of scalar multiplications in bignum_montmul_p256_base are carefully
//    chosen and vectorized
// 2. The vectorized assembly is rescheduled using the SLOTHY superoptimizer.
//    https://github.com/slothy-optimizer/slothy
//
// The output program of step 1. is as follows:
//
//        ldp x7, x13, [x1]
//        ldr q16, [x1]
//        ldp x9, x15, [x1, #16]
//        ldp x14, x4, [x2]
//        ldr q19, [x2]
//        ldp x12, x16, [x2, #16]
//        ldr q29, [x1, #16]
//        ldr q30, [x2, #16]
//        uzp1 v17.4S, v19.4S, v16.4S
//        rev64 v18.4S, v19.4S
//        uzp1 v28.4S, v16.4S, v16.4S
//        mul v24.4S, v18.4S, v16.4S
//        uaddlp v18.2D, v24.4S
//        shl v16.2D, v18.2D, #32
//        umlal v16.2D, v28.2S, v17.2S
//        mov x2, v16.d[0]
//        mov x1, v16.d[1]
//        umulh x5, x7, x14
//        adds x17, x2, x1
//        umulh x3, x13, x4
//        adcs x8, x5, x3
//        adcs x10, x3, xzr
//        adds x5, x5, x17
//        adcs x1, x1, x8
//        adcs x8, x10, xzr
//        subs x17, x7, x13
//        cneg x3, x17, cc
//        csetm x11, cc
//        subs x10, x4, x14
//        cneg x6, x10, cc
//        mul x17, x3, x6
//        umulh x6, x3, x6
//        cinv x11, x11, cc
//        eor x17, x17, x11
//        eor x3, x6, x11
//        cmn x11, #0x1
//        adcs x5, x5, x17
//        adcs x10, x1, x3
//        adc x1, x8, x11
//        lsl x3, x2, #32
//        subs x17, x2, x3
//        lsr x11, x2, #32
//        sbc x8, x2, x11
//        adds x2, x5, x3
//        adcs x6, x10, x11
//        adcs x3, x1, x17
//        adc x10, x8, xzr
//        lsl x5, x2, #32
//        subs x17, x2, x5
//        lsr x11, x2, #32
//        sbc x8, x2, x11
//        adds x2, x6, x5
//        adcs x6, x3, x11
//        adcs x1, x10, x17
//        adc x17, x8, xzr
//        stp x2, x6, [x0]                        // @slothy:writes=buffer0
//        stp x1, x17, [x0, #16]                  // @slothy:writes=buffer16
//        movi v28.2D, #0x00000000ffffffff
//        uzp2 v22.4S, v30.4S, v30.4S
//        xtn v4.2S, v29.2D
//        xtn v27.2S, v30.2D
//        rev64 v23.4S, v30.4S
//        umull v17.2D, v4.2S, v27.2S
//        umull v7.2D, v4.2S, v22.2S
//        uzp2 v16.4S, v29.4S, v29.4S
//        mul v29.4S, v23.4S, v29.4S
//        usra v7.2D, v17.2D, #32
//        umull v30.2D, v16.2S, v22.2S
//        uaddlp v20.2D, v29.4S
//        and v18.16B, v7.16B, v28.16B
//        umlal v18.2D, v16.2S, v27.2S
//        shl v16.2D, v20.2D, #32
//        usra v30.2D, v7.2D, #32
//        umlal v16.2D, v4.2S, v27.2S
//        usra v30.2D, v18.2D, #32
//        mov x11, v16.d[0]
//        mov x5, v16.d[1]
//        mov x2, v30.d[0]
//        adds x3, x11, x5
//        mov x17, v30.d[1]
//        adcs x8, x2, x17
//        adcs x1, x17, xzr
//        adds x17, x2, x3
//        adcs x8, x5, x8
//        adcs x1, x1, xzr
//        subs x2, x9, x15
//        cneg x6, x2, cc
//        csetm x3, cc
//        subs x2, x16, x12
//        cneg x5, x2, cc
//        mul x10, x6, x5
//        umulh x5, x6, x5
//        cinv x3, x3, cc
//        eor x10, x10, x3
//        eor x6, x5, x3
//        cmn x3, #0x1
//        adcs x2, x17, x10
//        adcs x6, x8, x6
//        adc x5, x1, x3
//        subs x7, x9, x7
//        sbcs x3, x15, x13
//        ngc x17, xzr
//        cmn x17, #0x1
//        eor x8, x7, x17
//        adcs x13, x8, xzr
//        eor x15, x3, x17
//        adcs x1, x15, xzr
//        subs x9, x14, x12
//        sbcs x14, x4, x16
//        ngc x3, xzr
//        cmn x3, #0x1
//        eor x12, x9, x3
//        adcs x7, x12, xzr
//        eor x12, x14, x3
//        adcs x12, x12, xzr
//        eor x10, x17, x3
//        ldp x4, x15, [x0]                       // @slothy:reads=buffer0
//        adds x17, x11, x4
//        adcs x16, x2, x15
//        ldp x3, x15, [x0, #16]                  // @slothy:reads=buffer16
//        adcs x11, x6, x3
//        adcs x9, x5, x15
//        adc x14, xzr, xzr
//        mul x6, x13, x7
//        mul x8, x1, x12
//        umulh x5, x13, x7
//        adds x3, x6, x8
//        umulh x2, x1, x12
//        adcs x4, x5, x2
//        adcs x15, x2, xzr
//        adds x3, x5, x3
//        adcs x4, x8, x4
//        adcs x15, x15, xzr
//        subs x1, x13, x1
//        cneg x8, x1, cc
//        csetm x5, cc
//        subs x1, x12, x7
//        cneg x2, x1, cc
//        mul x7, x8, x2
//        umulh x2, x8, x2
//        cinv x13, x5, cc
//        eor x7, x7, x13
//        eor x2, x2, x13
//        cmn x13, #0x1
//        adcs x3, x3, x7
//        adcs x4, x4, x2
//        adc x5, x15, x13
//        cmn x10, #0x1
//        eor x8, x6, x10
//        adcs x15, x8, x17
//        eor x2, x3, x10
//        adcs x2, x2, x16
//        eor x6, x4, x10
//        adcs x3, x6, x11
//        eor x7, x5, x10
//        adcs x1, x7, x9
//        adcs x13, x14, x10
//        adcs x12, x10, xzr
//        adc x10, x10, xzr
//        adds x5, x3, x17
//        adcs x8, x1, x16
//        adcs x13, x13, x11
//        adcs x6, x12, x9
//        adc x4, x10, x14
//        lsl x9, x15, #32
//        subs x7, x15, x9
//        lsr x1, x15, #32
//        sbc x14, x15, x1
//        adds x10, x2, x9
//        adcs x15, x5, x1
//        adcs x5, x8, x7
//        adc x7, x14, xzr
//        lsl x12, x10, #32
//        subs x17, x10, x12
//        lsr x9, x10, #32
//        sbc x3, x10, x9
//        adds x12, x15, x12
//        adcs x5, x5, x9
//        adcs x14, x7, x17
//        adc x2, x3, xzr
//        adds x14, x13, x14
//        adcs x6, x6, x2
//        adc x17, x4, xzr
//        add x7, x17, #0x1
//        lsl x16, x7, #32
//        adds x3, x6, x16
//        adc x1, x17, xzr
//        neg x15, x7
//        sub x13, x16, #0x1
//        subs x9, x12, x15
//        sbcs x8, x5, x13
//        sbcs x15, x14, xzr
//        sbcs x3, x3, x7
//        sbcs x7, x1, x7
//        adds x4, x9, x7
//        mov x6, #0xffffffff
//        and x17, x6, x7
//        adcs x8, x8, x17
//        adcs x5, x15, xzr
//        mov x10, #0xffffffff00000001
//        and x1, x10, x7
//        adc x12, x3, x1
//        stp x4, x8, [x0]                        // @slothy:writes=buffer0
//        stp x5, x12, [x0, #16]                  // @slothy:writes=buffer16
//        ret
//
// The bash script used for step 2 is as follows:
//
//        # Store the assembly instructions except the last 'ret' as, say, 'input.S'
//        export OUTPUTS="[hint_buffer0,hint_buffer16]"
//        export RESERVED_REGS="[x18,x19,x20,x21,x22,x23,x24,x25,x26,x27,x28,x29,x30,sp,q8,q9,q10,q11,q12,q13,q14,q15,v8,v9,v10,v11,v12,v13,v14,v15]"
//        <s2n-bignum>/tools/external/slothy.sh input.S my_out_dir
//        # my_out_dir/3.opt.s is the optimized assembly. Its output may differ
//        # from this file since the sequence is non-deterministically chosen.
//        # Please add 'ret' at the end of the output assembly.

/// Montgomery multiply, z := (x * y / 2^256) mod p_256
///
/// Inputs x[4], y[4]; output z[4]
///
/// Does z := (2^{-256} * x * y) mod p_256, assuming that the inputs x and y
/// satisfy x * y <= 2^256 * p_256 (in particular this is true if we are in
/// the "usual" case x < p_256 and y < p_256).
pub(crate) fn bignum_montmul_p256(z: &mut [u64; 4], x: &[u64; 4], y: &[u64; 4]) {
    // SAFETY: inline assembly. see [crate::low::inline_assembly_safety] for safety info.
    unsafe {
        core::arch::asm!(

        Q!("    ldr             " "q20, [x2]"),
        Q!("    ldp             " "x7, x17, [x1]"),
        Q!("    ldr             " "q0, [x1]"),
        Q!("    ldp             " "x6, x10, [x2]"),
        Q!("    ldp             " "x11, x15, [x1, #16]"),
        Q!("    rev64           " "v16.4S, v20.4S"),
        Q!("    subs            " "x4, x7, x17"),
        Q!("    csetm           " "x3, cc"),
        Q!("    cneg            " "x13, x4, cc"),
        Q!("    mul             " "v16.4S, v16.4S, v0.4S"),
        Q!("    umulh           " "x12, x17, x10"),
        Q!("    uzp1            " "v28.4S, v20.4S, v0.4S"),
        Q!("    subs            " "x14, x11, x7"),
        Q!("    ldr             " "q20, [x2, #16]"),
        Q!("    sbcs            " "x5, x15, x17"),
        Q!("    ngc             " "x17, xzr"),
        Q!("    subs            " "x8, x11, x15"),
        Q!("    uaddlp          " "v27.2D, v16.4S"),
        Q!("    umulh           " "x4, x7, x6"),
        Q!("    uzp1            " "v21.4S, v0.4S, v0.4S"),
        Q!("    cneg            " "x11, x8, cc"),
        Q!("    shl             " "v17.2D, v27.2D, #32"),
        Q!("    csetm           " "x15, cc"),
        Q!("    subs            " "x9, x10, x6"),
        Q!("    eor             " "x7, x14, x17"),
        Q!("    umlal           " "v17.2D, v21.2S, v28.2S"),
        Q!("    cneg            " "x8, x9, cc"),
        Q!("    cinv            " "x9, x3, cc"),
        Q!("    cmn             " "x17, #0x1"),
        Q!("    ldr             " "q28, [x1, #16]"),
        Q!("    adcs            " "x14, x7, xzr"),
        Q!("    mul             " "x7, x13, x8"),
        Q!("    eor             " "x1, x5, x17"),
        Q!("    adcs            " "x5, x1, xzr"),
        Q!("    xtn             " "v1.2S, v20.2D"),
        Q!("    mov             " "x1, v17.d[0]"),
        Q!("    mov             " "x3, v17.d[1]"),
        Q!("    uzp2            " "v16.4S, v20.4S, v20.4S"),
        Q!("    umulh           " "x16, x13, x8"),
        Q!("    eor             " "x13, x7, x9"),
        Q!("    adds            " "x8, x1, x3"),
        Q!("    adcs            " "x7, x4, x12"),
        Q!("    xtn             " "v0.2S, v28.2D"),
        Q!("    adcs            " "x12, x12, xzr"),
        Q!("    adds            " "x8, x4, x8"),
        Q!("    adcs            " "x3, x3, x7"),
        Q!("    ldp             " "x7, x2, [x2, #16]"),
        Q!("    adcs            " "x12, x12, xzr"),
        Q!("    cmn             " "x9, #0x1"),
        Q!("    adcs            " "x8, x8, x13"),
        Q!("    eor             " "x13, x16, x9"),
        Q!("    adcs            " "x16, x3, x13"),
        Q!("    lsl             " "x3, x1, #32"),
        Q!("    adc             " "x13, x12, x9"),
        Q!("    subs            " "x12, x6, x7"),
        Q!("    sbcs            " "x9, x10, x2"),
        Q!("    lsr             " "x10, x1, #32"),
        Q!("    ngc             " "x4, xzr"),
        Q!("    subs            " "x6, x2, x7"),
        Q!("    cinv            " "x2, x15, cc"),
        Q!("    cneg            " "x6, x6, cc"),
        Q!("    subs            " "x7, x1, x3"),
        Q!("    eor             " "x9, x9, x4"),
        Q!("    sbc             " "x1, x1, x10"),
        Q!("    adds            " "x15, x8, x3"),
        Q!("    adcs            " "x3, x16, x10"),
        Q!("    mul             " "x16, x11, x6"),
        Q!("    adcs            " "x8, x13, x7"),
        Q!("    eor             " "x13, x12, x4"),
        Q!("    adc             " "x10, x1, xzr"),
        Q!("    cmn             " "x4, #0x1"),
        Q!("    umulh           " "x6, x11, x6"),
        Q!("    adcs            " "x11, x13, xzr"),
        Q!("    adcs            " "x1, x9, xzr"),
        Q!("    lsl             " "x13, x15, #32"),
        Q!("    subs            " "x12, x15, x13"),
        Q!("    lsr             " "x7, x15, #32"),
        Q!("    sbc             " "x15, x15, x7"),
        Q!("    adds            " "x9, x3, x13"),
        Q!("    adcs            " "x3, x8, x7"),
        Q!("    umulh           " "x8, x14, x11"),
        Q!("    umull           " "v21.2D, v0.2S, v1.2S"),
        Q!("    adcs            " "x12, x10, x12"),
        Q!("    umull           " "v3.2D, v0.2S, v16.2S"),
        Q!("    adc             " "x15, x15, xzr"),
        Q!("    rev64           " "v24.4S, v20.4S"),
        Q!("    stp             " "x12, x15, [x0, #16]"),
        Q!("    movi            " "v2.2D, #0x00000000ffffffff"),
        Q!("    mul             " "x10, x14, x11"),
        Q!("    mul             " "v4.4S, v24.4S, v28.4S"),
        Q!("    subs            " "x13, x14, x5"),
        Q!("    uzp2            " "v19.4S, v28.4S, v28.4S"),
        Q!("    csetm           " "x15, cc"),
        Q!("    usra            " "v3.2D, v21.2D, #32"),
        Q!("    mul             " "x7, x5, x1"),
        Q!("    umull           " "v21.2D, v19.2S, v16.2S"),
        Q!("    cneg            " "x13, x13, cc"),
        Q!("    uaddlp          " "v5.2D, v4.4S"),
        Q!("    subs            " "x11, x1, x11"),
        Q!("    and             " "v16.16B, v3.16B, v2.16B"),
        Q!("    umulh           " "x5, x5, x1"),
        Q!("    shl             " "v24.2D, v5.2D, #32"),
        Q!("    cneg            " "x11, x11, cc"),
        Q!("    umlal           " "v16.2D, v19.2S, v1.2S"),
        Q!("    cinv            " "x12, x15, cc"),
        Q!("    umlal           " "v24.2D, v0.2S, v1.2S"),
        Q!("    adds            " "x15, x10, x7"),
        Q!("    mul             " "x14, x13, x11"),
        Q!("    eor             " "x1, x6, x2"),
        Q!("    adcs            " "x6, x8, x5"),
        Q!("    stp             " "x9, x3, [x0]"),
        Q!("    usra            " "v21.2D, v3.2D, #32"),
        Q!("    adcs            " "x9, x5, xzr"),
        Q!("    umulh           " "x11, x13, x11"),
        Q!("    adds            " "x15, x8, x15"),
        Q!("    adcs            " "x7, x7, x6"),
        Q!("    eor             " "x8, x14, x12"),
        Q!("    usra            " "v21.2D, v16.2D, #32"),
        Q!("    adcs            " "x13, x9, xzr"),
        Q!("    cmn             " "x12, #0x1"),
        Q!("    mov             " "x9, v24.d[1]"),
        Q!("    adcs            " "x14, x15, x8"),
        Q!("    eor             " "x6, x11, x12"),
        Q!("    adcs            " "x6, x7, x6"),
        Q!("    mov             " "x5, v24.d[0]"),
        Q!("    mov             " "x11, v21.d[1]"),
        Q!("    mov             " "x7, v21.d[0]"),
        Q!("    adc             " "x3, x13, x12"),
        Q!("    adds            " "x12, x5, x9"),
        Q!("    adcs            " "x13, x7, x11"),
        Q!("    ldp             " "x15, x8, [x0]"),
        Q!("    adcs            " "x11, x11, xzr"),
        Q!("    adds            " "x12, x7, x12"),
        Q!("    eor             " "x16, x16, x2"),
        Q!("    adcs            " "x7, x9, x13"),
        Q!("    adcs            " "x11, x11, xzr"),
        Q!("    cmn             " "x2, #0x1"),
        Q!("    ldp             " "x9, x13, [x0, #16]"),
        Q!("    adcs            " "x16, x12, x16"),
        Q!("    adcs            " "x1, x7, x1"),
        Q!("    adc             " "x2, x11, x2"),
        Q!("    adds            " "x7, x5, x15"),
        Q!("    adcs            " "x15, x16, x8"),
        Q!("    eor             " "x5, x17, x4"),
        Q!("    adcs            " "x9, x1, x9"),
        Q!("    eor             " "x1, x10, x5"),
        Q!("    adcs            " "x16, x2, x13"),
        Q!("    adc             " "x2, xzr, xzr"),
        Q!("    cmn             " "x5, #0x1"),
        Q!("    eor             " "x13, x14, x5"),
        Q!("    adcs            " "x14, x1, x7"),
        Q!("    eor             " "x1, x6, x5"),
        Q!("    adcs            " "x6, x13, x15"),
        Q!("    adcs            " "x10, x1, x9"),
        Q!("    eor             " "x4, x3, x5"),
        Q!("    mov             " "x1, #0xffffffff"),
        Q!("    adcs            " "x8, x4, x16"),
        Q!("    lsr             " "x13, x14, #32"),
        Q!("    adcs            " "x17, x2, x5"),
        Q!("    adcs            " "x11, x5, xzr"),
        Q!("    adc             " "x4, x5, xzr"),
        Q!("    adds            " "x12, x10, x7"),
        Q!("    adcs            " "x7, x8, x15"),
        Q!("    adcs            " "x5, x17, x9"),
        Q!("    adcs            " "x9, x11, x16"),
        Q!("    lsl             " "x11, x14, #32"),
        Q!("    adc             " "x10, x4, x2"),
        Q!("    subs            " "x17, x14, x11"),
        Q!("    sbc             " "x4, x14, x13"),
        Q!("    adds            " "x11, x6, x11"),
        Q!("    adcs            " "x12, x12, x13"),
        Q!("    lsl             " "x15, x11, #32"),
        Q!("    adcs            " "x17, x7, x17"),
        Q!("    lsr             " "x7, x11, #32"),
        Q!("    adc             " "x13, x4, xzr"),
        Q!("    subs            " "x4, x11, x15"),
        Q!("    sbc             " "x11, x11, x7"),
        Q!("    adds            " "x8, x12, x15"),
        Q!("    adcs            " "x15, x17, x7"),
        Q!("    adcs            " "x4, x13, x4"),
        Q!("    adc             " "x11, x11, xzr"),
        Q!("    adds            " "x7, x5, x4"),
        Q!("    adcs            " "x17, x9, x11"),
        Q!("    adc             " "x13, x10, xzr"),
        Q!("    add             " "x12, x13, #0x1"),
        Q!("    neg             " "x11, x12"),
        Q!("    lsl             " "x4, x12, #32"),
        Q!("    adds            " "x17, x17, x4"),
        Q!("    sub             " "x4, x4, #0x1"),
        Q!("    adc             " "x13, x13, xzr"),
        Q!("    subs            " "x11, x8, x11"),
        Q!("    sbcs            " "x4, x15, x4"),
        Q!("    sbcs            " "x7, x7, xzr"),
        Q!("    sbcs            " "x17, x17, x12"),
        Q!("    sbcs            " "x13, x13, x12"),
        Q!("    mov             " "x12, #0xffffffff00000001"),
        Q!("    adds            " "x11, x11, x13"),
        Q!("    and             " "x1, x1, x13"),
        Q!("    adcs            " "x4, x4, x1"),
        Q!("    and             " "x1, x12, x13"),
        Q!("    stp             " "x11, x4, [x0]"),
        Q!("    adcs            " "x4, x7, xzr"),
        Q!("    adc             " "x1, x17, x1"),
        Q!("    stp             " "x4, x1, [x0, #16]"),
        inout("x0") z.as_mut_ptr() => _,
        inout("x1") x.as_ptr() => _,
        inout("x2") y.as_ptr() => _,
        // clobbers
        out("v0") _,
        out("v1") _,
        out("v16") _,
        out("v17") _,
        out("v19") _,
        out("v2") _,
        out("v20") _,
        out("v21") _,
        out("v24") _,
        out("v27") _,
        out("v28") _,
        out("v3") _,
        out("v4") _,
        out("v5") _,
        out("x10") _,
        out("x11") _,
        out("x12") _,
        out("x13") _,
        out("x14") _,
        out("x15") _,
        out("x16") _,
        out("x17") _,
        out("x3") _,
        out("x4") _,
        out("x5") _,
        out("x6") _,
        out("x7") _,
        out("x8") _,
        out("x9") _,
            )
    };
}
