// generated source. do not edit.
#![allow(non_upper_case_globals, unused_macros, unused_imports)]
use crate::low::macros::*;

// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

// ----------------------------------------------------------------------------
// Montgomery multiply, z := (x * y / 2^384) mod p_384
// Inputs x[6], y[6]; output z[6]
//
//    extern void bignum_montmul_p384(uint64_t z[static 6],
//                                    const uint64_t x[static 6],
//                                    const uint64_t y[static 6]);
//
// Does z := (2^{-384} * x * y) mod p_384, assuming that the inputs x and y
// satisfy x * y <= 2^384 * p_384 (in particular this is true if we are in
// the "usual" case x < p_384 and y < p_384).
//
// Standard ARM ABI: X0 = z, X1 = x, X2 = y
// ----------------------------------------------------------------------------

// bignum_montmul_p384 is functionally equivalent to
// unopt/bignum_montmul_p384_base.
// It is written in a way that
// 1. A subset of scalar multiplications in bignum_montmul_p384 are carefully
//    chosen and vectorized
// 2. The vectorized assembly is rescheduled using the SLOTHY superoptimizer.
//    https://github.com/slothy-optimizer/slothy
//
// The output program of step 1. is as follows:
//
//        stp	x19, x20, [sp, #-16]!
//        stp	x21, x22, [sp, #-16]!
//        stp	x23, x24, [sp, #-16]!
//        ldp x3, x21, [x1]
//        ldr q30, [x1]
//        ldp x8, x24, [x1, #16]
//        ldp x5, x10, [x1, #32]
//        ldp x13, x23, [x2]
//        ldr q19, [x2]
//        ldp x6, x14, [x2, #16]
//        ldp x15, x17, [x2, #32]
//        ldr q1, [x1, #32]
//        ldr q28, [x2, #32]
//        uzp1 v5.4S, v19.4S, v30.4S
//        rev64 v19.4S, v19.4S
//        uzp1 v0.4S, v30.4S, v30.4S
//        mul v21.4S, v19.4S, v30.4S
//        uaddlp v19.2D, v21.4S
//        shl v19.2D, v19.2D, #32
//        umlal v19.2D, v0.2S, v5.2S
//        mov x12, v19.d[0]
//        mov x16, v19.d[1]
//        mul x20, x8, x6
//        umulh x4, x3, x13
//        umulh x1, x21, x23
//        umulh x2, x8, x6
//        adds x4, x4, x16
//        adcs x19, x1, x20
//        adc x20, x2, xzr
//        adds x11, x4, x12
//        adcs x16, x19, x4
//        adcs x1, x20, x19
//        adc x2, x20, xzr
//        adds x7, x16, x12
//        adcs x4, x1, x4
//        adcs x9, x2, x19
//        adc x19, x20, xzr
//        subs x2, x3, x21
//        cneg x20, x2, cc
//        csetm x16, cc
//        subs x2, x23, x13
//        cneg x2, x2, cc
//        mul x1, x20, x2
//        umulh x2, x20, x2
//        cinv x16, x16, cc
//        eor x1, x1, x16
//        eor x2, x2, x16
//        cmn x16, #0x1
//        adcs x11, x11, x1
//        adcs x7, x7, x2
//        adcs x4, x4, x16
//        adcs x9, x9, x16
//        adc x19, x19, x16
//        subs x2, x3, x8
//        cneg x20, x2, cc
//        csetm x16, cc
//        subs x2, x6, x13
//        cneg x2, x2, cc
//        mul x1, x20, x2
//        umulh x2, x20, x2
//        cinv x16, x16, cc
//        eor x1, x1, x16
//        eor x2, x2, x16
//        cmn x16, #0x1
//        adcs x7, x7, x1
//        adcs x4, x4, x2
//        adcs x9, x9, x16
//        adc x19, x19, x16
//        subs x2, x21, x8
//        cneg x20, x2, cc
//        csetm x16, cc
//        subs x2, x6, x23
//        cneg x2, x2, cc
//        mul x1, x20, x2
//        umulh x2, x20, x2
//        cinv x16, x16, cc
//        eor x1, x1, x16
//        eor x2, x2, x16
//        cmn x16, #0x1
//        adcs x4, x4, x1
//        adcs x20, x9, x2
//        adc x16, x19, x16
//        lsl x2, x12, #32
//        add x19, x2, x12
//        lsr x2, x19, #32
//        subs x1, x2, x19
//        sbc x2, x19, xzr
//        extr x1, x2, x1, #32
//        lsr x2, x2, #32
//        adds x12, x2, x19
//        adc x2, xzr, xzr
//        subs x1, x11, x1
//        sbcs x7, x7, x12
//        sbcs x4, x4, x2
//        sbcs x20, x20, xzr
//        sbcs x16, x16, xzr
//        sbc x9, x19, xzr
//        lsl x2, x1, #32
//        add x19, x2, x1
//        lsr x2, x19, #32
//        subs x1, x2, x19
//        sbc x2, x19, xzr
//        extr x1, x2, x1, #32
//        lsr x2, x2, #32
//        adds x12, x2, x19
//        adc x2, xzr, xzr
//        subs x1, x7, x1
//        sbcs x4, x4, x12
//        sbcs x20, x20, x2
//        sbcs x16, x16, xzr
//        sbcs x7, x9, xzr
//        sbc x9, x19, xzr
//        lsl x2, x1, #32
//        add x19, x2, x1
//        lsr x2, x19, #32
//        subs x1, x2, x19
//        sbc x2, x19, xzr
//        extr x12, x2, x1, #32
//        lsr x2, x2, #32
//        adds x1, x2, x19
//        adc x2, xzr, xzr
//        subs x4, x4, x12
//        sbcs x20, x20, x1
//        sbcs x16, x16, x2
//        sbcs x12, x7, xzr
//        sbcs x1, x9, xzr
//        sbc x2, x19, xzr
//        stp x4, x20, [x0]                       // @slothy:writes=buffer0
//        stp x16, x12, [x0, #16]                 // @slothy:writes=buffer16
//        stp x1, x2, [x0, #32]                   // @slothy:writes=buffer32
//        mul x22, x24, x14
//        movi v31.2D, #0x00000000ffffffff
//        uzp2 v16.4S, v28.4S, v28.4S
//        xtn v6.2S, v1.2D
//        xtn v30.2S, v28.2D
//        rev64 v28.4S, v28.4S
//        umull v5.2D, v6.2S, v30.2S
//        umull v0.2D, v6.2S, v16.2S
//        uzp2 v19.4S, v1.4S, v1.4S
//        mul v20.4S, v28.4S, v1.4S
//        usra v0.2D, v5.2D, #32
//        umull v1.2D, v19.2S, v16.2S
//        uaddlp v24.2D, v20.4S
//        and v5.16B, v0.16B, v31.16B
//        umlal v5.2D, v19.2S, v30.2S
//        shl v19.2D, v24.2D, #32
//        usra v1.2D, v0.2D, #32
//        umlal v19.2D, v6.2S, v30.2S
//        usra v1.2D, v5.2D, #32
//        mov x20, v19.d[0]
//        mov x16, v19.d[1]
//        umulh x12, x24, x14
//        mov x1, v1.d[0]
//        mov x2, v1.d[1]
//        adds x4, x12, x20
//        adcs x20, x1, x16
//        adc x16, x2, xzr
//        adds x7, x4, x22
//        adcs x12, x20, x4
//        adcs x1, x16, x20
//        adc x2, x16, xzr
//        adds x9, x12, x22
//        adcs x19, x1, x4
//        adcs x4, x2, x20
//        adc x20, x16, xzr
//        subs x2, x24, x5
//        cneg x16, x2, cc
//        csetm x12, cc
//        subs x2, x15, x14
//        cneg x2, x2, cc
//        mul x1, x16, x2
//        umulh x2, x16, x2
//        cinv x12, x12, cc
//        eor x1, x1, x12
//        eor x2, x2, x12
//        cmn x12, #0x1
//        adcs x11, x7, x1
//        adcs x9, x9, x2
//        adcs x19, x19, x12
//        adcs x4, x4, x12
//        adc x20, x20, x12
//        subs x2, x24, x10
//        cneg x16, x2, cc
//        csetm x12, cc
//        subs x2, x17, x14
//        cneg x2, x2, cc
//        mul x1, x16, x2
//        umulh x2, x16, x2
//        cinv x12, x12, cc
//        eor x1, x1, x12
//        eor x2, x2, x12
//        cmn x12, #0x1
//        adcs x7, x9, x1
//        adcs x19, x19, x2
//        adcs x4, x4, x12
//        adc x20, x20, x12
//        subs x2, x5, x10
//        cneg x16, x2, cc
//        csetm x12, cc
//        subs x2, x17, x15
//        cneg x2, x2, cc
//        mul x1, x16, x2
//        umulh x2, x16, x2
//        cinv x16, x12, cc
//        eor x1, x1, x16
//        eor x2, x2, x16
//        cmn x16, #0x1
//        adcs x19, x19, x1
//        adcs x12, x4, x2
//        adc x1, x20, x16
//        subs x2, x24, x3
//        sbcs x24, x5, x21
//        sbcs x21, x10, x8
//        ngc x5, xzr
//        cmn x5, #0x1
//        eor x2, x2, x5
//        adcs x4, x2, xzr
//        eor x2, x24, x5
//        adcs x20, x2, xzr
//        eor x2, x21, x5
//        adc x16, x2, xzr
//        subs x2, x13, x14
//        sbcs x24, x23, x15
//        sbcs x8, x6, x17
//        ngc x21, xzr
//        cmn x21, #0x1
//        eor x2, x2, x21
//        adcs x15, x2, xzr
//        eor x2, x24, x21
//        adcs x14, x2, xzr
//        eor x2, x8, x21
//        adc x6, x2, xzr
//        eor x9, x5, x21
//        ldp x21, x2, [x0]                       // @slothy:reads=buffer0
//        adds x10, x22, x21
//        adcs x5, x11, x2
//        ldp x21, x2, [x0, #16]                  // @slothy:reads=buffer16
//        adcs x24, x7, x21
//        adcs x8, x19, x2
//        ldp x21, x2, [x0, #32]                  // @slothy:reads=buffer32
//        adcs x21, x12, x21
//        adcs x2, x1, x2
//        adc x19, xzr, xzr
//        stp x10, x5, [x0]                       // @slothy:writes=buffer0
//        stp x24, x8, [x0, #16]                  // @slothy:writes=buffer16
//        stp x21, x2, [x0, #32]                  // @slothy:writes=buffer32
//        mul x12, x4, x15
//        mul x5, x20, x14
//        mul x24, x16, x6
//        umulh x8, x4, x15
//        umulh x21, x20, x14
//        umulh x2, x16, x6
//        adds x10, x8, x5
//        adcs x5, x21, x24
//        adc x24, x2, xzr
//        adds x23, x10, x12
//        adcs x8, x5, x10
//        adcs x21, x24, x5
//        adc x2, x24, xzr
//        adds x13, x8, x12
//        adcs x1, x21, x10
//        adcs x10, x2, x5
//        adc x5, x24, xzr
//        subs x2, x4, x20
//        cneg x24, x2, cc
//        csetm x8, cc
//        subs x2, x14, x15
//        cneg x2, x2, cc
//        mul x21, x24, x2
//        umulh x2, x24, x2
//        cinv x8, x8, cc
//        eor x21, x21, x8
//        eor x2, x2, x8
//        cmn x8, #0x1
//        adcs x23, x23, x21
//        adcs x13, x13, x2
//        adcs x1, x1, x8
//        adcs x10, x10, x8
//        adc x5, x5, x8
//        subs x2, x4, x16
//        cneg x24, x2, cc
//        csetm x8, cc
//        subs x2, x6, x15
//        cneg x2, x2, cc
//        mul x21, x24, x2
//        umulh x2, x24, x2
//        cinv x8, x8, cc
//        eor x21, x21, x8
//        eor x2, x2, x8
//        cmn x8, #0x1
//        adcs x4, x13, x21
//        adcs x13, x1, x2
//        adcs x1, x10, x8
//        adc x10, x5, x8
//        subs x2, x20, x16
//        cneg x24, x2, cc
//        csetm x8, cc
//        subs x2, x6, x14
//        cneg x2, x2, cc
//        mul x21, x24, x2
//        umulh x2, x24, x2
//        cinv x5, x8, cc
//        eor x21, x21, x5
//        eor x2, x2, x5
//        cmn x5, #0x1
//        adcs x24, x13, x21
//        adcs x8, x1, x2
//        adc x21, x10, x5
//        ldp x20, x16, [x0]                      // @slothy:reads=buffer0
//        ldp x17, x15, [x0, #16]                 // @slothy:reads=buffer16
//        ldp x14, x6, [x0, #32]                  // @slothy:reads=buffer32
//        cmn x9, #0x1
//        eor x2, x12, x9
//        adcs x12, x2, x20
//        eor x2, x23, x9
//        adcs x23, x2, x16
//        eor x2, x4, x9
//        adcs x13, x2, x17
//        eor x2, x24, x9
//        adcs x10, x2, x15
//        eor x2, x8, x9
//        adcs x5, x2, x14
//        eor x2, x21, x9
//        adcs x24, x2, x6
//        adcs x1, x9, x19
//        adcs x8, x9, xzr
//        adcs x21, x9, xzr
//        adc x2, x9, xzr
//        adds x10, x10, x20
//        adcs x5, x5, x16
//        adcs x24, x24, x17
//        adcs x17, x1, x15
//        adcs x15, x8, x14
//        adcs x14, x21, x6
//        adc x6, x2, x19
//        lsl x2, x12, #32
//        add x1, x2, x12
//        lsr x2, x1, #32
//        subs x21, x2, x1
//        sbc x2, x1, xzr
//        extr x21, x2, x21, #32
//        lsr x2, x2, #32
//        adds x8, x2, x1
//        adc x2, xzr, xzr
//        subs x21, x23, x21
//        sbcs x23, x13, x8
//        sbcs x10, x10, x2
//        sbcs x5, x5, xzr
//        sbcs x24, x24, xzr
//        sbc x13, x1, xzr
//        lsl x2, x21, #32
//        add x1, x2, x21
//        lsr x2, x1, #32
//        subs x21, x2, x1
//        sbc x2, x1, xzr
//        extr x21, x2, x21, #32
//        lsr x2, x2, #32
//        adds x8, x2, x1
//        adc x2, xzr, xzr
//        subs x21, x23, x21
//        sbcs x10, x10, x8
//        sbcs x5, x5, x2
//        sbcs x24, x24, xzr
//        sbcs x23, x13, xzr
//        sbc x13, x1, xzr
//        lsl x2, x21, #32
//        add x1, x2, x21
//        lsr x2, x1, #32
//        subs x21, x2, x1
//        sbc x2, x1, xzr
//        extr x8, x2, x21, #32
//        lsr x2, x2, #32
//        adds x21, x2, x1
//        adc x2, xzr, xzr
//        subs x10, x10, x8
//        sbcs x5, x5, x21
//        sbcs x24, x24, x2
//        sbcs x8, x23, xzr
//        sbcs x21, x13, xzr
//        sbc x2, x1, xzr
//        adds x23, x17, x8
//        adcs x13, x15, x21
//        adcs x1, x14, x2
//        adc x2, x6, xzr
//        add x8, x2, #0x1
//        lsl x2, x8, #32
//        subs x21, x8, x2
//        sbc x2, x2, xzr
//        adds x10, x10, x21
//        adcs x5, x5, x2
//        adcs x24, x24, x8
//        adcs x8, x23, xzr
//        adcs x21, x13, xzr
//        adcs x13, x1, xzr
//        csetm x1, cc
//        mov x2, #0xffffffff
//        and x2, x2, x1
//        adds x10, x10, x2
//        eor x2, x2, x1
//        adcs x5, x5, x2
//        mov x2, #0xfffffffffffffffe
//        and x2, x2, x1
//        adcs x24, x24, x2
//        adcs x8, x8, x1
//        adcs x21, x21, x1
//        adc x2, x13, x1
//        stp x10, x5, [x0]                       // @slothy:writes=buffer0
//        stp x24, x8, [x0, #16]                  // @slothy:writes=buffer16
//        stp x21, x2, [x0, #32]                  // @slothy:writes=buffer32
//        ldp	x23, x24, [sp], #16
//        ldp	x21, x22, [sp], #16
//        ldp	x19, x20, [sp], #16
//        ret
//
// The bash script used for step 2 is as follows:
//
//        # Store the assembly instructions except the last 'ret' and
//        # callee-register store/loads as, say, 'input.S'.
//        export OUTPUTS="[hint_buffer0,hint_buffer16,hint_buffer32]"
//        export RESERVED_REGS="[x18,x25,x26,x27,x28,x29,x30,sp,q8,q9,q10,q11,q12,q13,q14,q15,v8,v9,v10,v11,v12,v13,v14,v15]"
//        <s2n-bignum>/tools/external/slothy.sh input.S my_out_dir
//        # my_out_dir/3.opt.s is the optimized assembly. Its output may differ
//        # from this file since the sequence is non-deterministically chosen.
//        # Please add 'ret' at the end of the output assembly.

/// Montgomery multiply, z := (x * y / 2^384) mod p_384
///
/// Inputs x[6], y[6]; output z[6]
///
/// Does z := (2^{-384} * x * y) mod p_384, assuming that the inputs x and y
/// satisfy x * y <= 2^384 * p_384 (in particular this is true if we are in
/// the "usual" case x < p_384 and y < p_384).
pub(crate) fn bignum_montmul_p384(z: &mut [u64; 6], x: &[u64; 6], y: &[u64; 6]) {
    // SAFETY: inline assembly. see [crate::low::inline_assembly_safety] for safety info.
    unsafe {
        core::arch::asm!(

        // Save some registers

        Q!("    stp             " "x19, x20, [sp, #-16] !"),
        Q!("    stp             " "x21, x22, [sp, #-16] !"),
        Q!("    stp             " "x23, x24, [sp, #-16] !"),

        Q!("    ldr             " "q3, [x1]"),
        Q!("    ldr             " "q25, [x2]"),
        Q!("    ldp             " "x13, x23, [x2]"),
        Q!("    ldp             " "x3, x21, [x1]"),
        Q!("    rev64           " "v23.4S, v25.4S"),
        Q!("    uzp1            " "v17.4S, v25.4S, v3.4S"),
        Q!("    umulh           " "x15, x3, x13"),
        Q!("    mul             " "v6.4S, v23.4S, v3.4S"),
        Q!("    uzp1            " "v3.4S, v3.4S, v3.4S"),
        Q!("    ldr             " "q27, [x2, #32]"),
        Q!("    ldp             " "x8, x24, [x1, #16]"),
        Q!("    subs            " "x6, x3, x21"),
        Q!("    ldr             " "q0, [x1, #32]"),
        Q!("    movi            " "v23.2D, #0x00000000ffffffff"),
        Q!("    csetm           " "x10, cc"),
        Q!("    umulh           " "x19, x21, x23"),
        Q!("    rev64           " "v4.4S, v27.4S"),
        Q!("    uzp2            " "v25.4S, v27.4S, v27.4S"),
        Q!("    cneg            " "x4, x6, cc"),
        Q!("    subs            " "x7, x23, x13"),
        Q!("    xtn             " "v22.2S, v0.2D"),
        Q!("    xtn             " "v24.2S, v27.2D"),
        Q!("    cneg            " "x20, x7, cc"),
        Q!("    ldp             " "x6, x14, [x2, #16]"),
        Q!("    mul             " "v27.4S, v4.4S, v0.4S"),
        Q!("    uaddlp          " "v20.2D, v6.4S"),
        Q!("    cinv            " "x5, x10, cc"),
        Q!("    mul             " "x16, x4, x20"),
        Q!("    uzp2            " "v6.4S, v0.4S, v0.4S"),
        Q!("    umull           " "v21.2D, v22.2S, v25.2S"),
        Q!("    shl             " "v0.2D, v20.2D, #32"),
        Q!("    umlal           " "v0.2D, v3.2S, v17.2S"),
        Q!("    mul             " "x22, x8, x6"),
        Q!("    umull           " "v1.2D, v6.2S, v25.2S"),
        Q!("    subs            " "x12, x3, x8"),
        Q!("    umull           " "v20.2D, v22.2S, v24.2S"),
        Q!("    cneg            " "x17, x12, cc"),
        Q!("    umulh           " "x9, x8, x6"),
        Q!("    mov             " "x12, v0.d[1]"),
        Q!("    eor             " "x11, x16, x5"),
        Q!("    mov             " "x7, v0.d[0]"),
        Q!("    csetm           " "x10, cc"),
        Q!("    usra            " "v21.2D, v20.2D, #32"),
        Q!("    adds            " "x15, x15, x12"),
        Q!("    adcs            " "x12, x19, x22"),
        Q!("    umulh           " "x20, x4, x20"),
        Q!("    adc             " "x19, x9, xzr"),
        Q!("    usra            " "v1.2D, v21.2D, #32"),
        Q!("    adds            " "x22, x15, x7"),
        Q!("    and             " "v26.16B, v21.16B, v23.16B"),
        Q!("    adcs            " "x16, x12, x15"),
        Q!("    uaddlp          " "v25.2D, v27.4S"),
        Q!("    adcs            " "x9, x19, x12"),
        Q!("    umlal           " "v26.2D, v6.2S, v24.2S"),
        Q!("    adc             " "x4, x19, xzr"),
        Q!("    adds            " "x16, x16, x7"),
        Q!("    shl             " "v27.2D, v25.2D, #32"),
        Q!("    adcs            " "x9, x9, x15"),
        Q!("    adcs            " "x4, x4, x12"),
        Q!("    eor             " "x12, x20, x5"),
        Q!("    adc             " "x15, x19, xzr"),
        Q!("    subs            " "x20, x6, x13"),
        Q!("    cneg            " "x20, x20, cc"),
        Q!("    cinv            " "x10, x10, cc"),
        Q!("    cmn             " "x5, #0x1"),
        Q!("    mul             " "x19, x17, x20"),
        Q!("    adcs            " "x11, x22, x11"),
        Q!("    adcs            " "x12, x16, x12"),
        Q!("    adcs            " "x9, x9, x5"),
        Q!("    umulh           " "x17, x17, x20"),
        Q!("    adcs            " "x22, x4, x5"),
        Q!("    adc             " "x5, x15, x5"),
        Q!("    subs            " "x16, x21, x8"),
        Q!("    cneg            " "x20, x16, cc"),
        Q!("    eor             " "x19, x19, x10"),
        Q!("    csetm           " "x4, cc"),
        Q!("    subs            " "x16, x6, x23"),
        Q!("    cneg            " "x16, x16, cc"),
        Q!("    umlal           " "v27.2D, v22.2S, v24.2S"),
        Q!("    mul             " "x15, x20, x16"),
        Q!("    cinv            " "x4, x4, cc"),
        Q!("    cmn             " "x10, #0x1"),
        Q!("    usra            " "v1.2D, v26.2D, #32"),
        Q!("    adcs            " "x19, x12, x19"),
        Q!("    eor             " "x17, x17, x10"),
        Q!("    adcs            " "x9, x9, x17"),
        Q!("    adcs            " "x22, x22, x10"),
        Q!("    lsl             " "x12, x7, #32"),
        Q!("    umulh           " "x20, x20, x16"),
        Q!("    eor             " "x16, x15, x4"),
        Q!("    ldp             " "x15, x17, [x2, #32]"),
        Q!("    add             " "x2, x12, x7"),
        Q!("    adc             " "x7, x5, x10"),
        Q!("    ldp             " "x5, x10, [x1, #32]"),
        Q!("    lsr             " "x1, x2, #32"),
        Q!("    eor             " "x12, x20, x4"),
        Q!("    subs            " "x1, x1, x2"),
        Q!("    sbc             " "x20, x2, xzr"),
        Q!("    cmn             " "x4, #0x1"),
        Q!("    adcs            " "x9, x9, x16"),
        Q!("    extr            " "x1, x20, x1, #32"),
        Q!("    lsr             " "x20, x20, #32"),
        Q!("    adcs            " "x22, x22, x12"),
        Q!("    adc             " "x16, x7, x4"),
        Q!("    adds            " "x12, x20, x2"),
        Q!("    umulh           " "x7, x24, x14"),
        Q!("    adc             " "x4, xzr, xzr"),
        Q!("    subs            " "x1, x11, x1"),
        Q!("    sbcs            " "x20, x19, x12"),
        Q!("    sbcs            " "x12, x9, x4"),
        Q!("    lsl             " "x9, x1, #32"),
        Q!("    add             " "x1, x9, x1"),
        Q!("    sbcs            " "x9, x22, xzr"),
        Q!("    mul             " "x22, x24, x14"),
        Q!("    sbcs            " "x16, x16, xzr"),
        Q!("    lsr             " "x4, x1, #32"),
        Q!("    sbc             " "x19, x2, xzr"),
        Q!("    subs            " "x4, x4, x1"),
        Q!("    sbc             " "x11, x1, xzr"),
        Q!("    extr            " "x2, x11, x4, #32"),
        Q!("    lsr             " "x4, x11, #32"),
        Q!("    adds            " "x4, x4, x1"),
        Q!("    adc             " "x11, xzr, xzr"),
        Q!("    subs            " "x2, x20, x2"),
        Q!("    sbcs            " "x4, x12, x4"),
        Q!("    sbcs            " "x20, x9, x11"),
        Q!("    lsl             " "x12, x2, #32"),
        Q!("    add             " "x2, x12, x2"),
        Q!("    sbcs            " "x9, x16, xzr"),
        Q!("    lsr             " "x11, x2, #32"),
        Q!("    sbcs            " "x19, x19, xzr"),
        Q!("    sbc             " "x1, x1, xzr"),
        Q!("    subs            " "x16, x11, x2"),
        Q!("    sbc             " "x12, x2, xzr"),
        Q!("    extr            " "x16, x12, x16, #32"),
        Q!("    lsr             " "x12, x12, #32"),
        Q!("    adds            " "x11, x12, x2"),
        Q!("    adc             " "x12, xzr, xzr"),
        Q!("    subs            " "x16, x4, x16"),
        Q!("    mov             " "x4, v27.d[0]"),
        Q!("    sbcs            " "x11, x20, x11"),
        Q!("    sbcs            " "x20, x9, x12"),
        Q!("    stp             " "x16, x11, [x0]"),
        Q!("    sbcs            " "x11, x19, xzr"),
        Q!("    sbcs            " "x9, x1, xzr"),
        Q!("    stp             " "x20, x11, [x0, #16]"),
        Q!("    mov             " "x1, v1.d[0]"),
        Q!("    sbc             " "x20, x2, xzr"),
        Q!("    subs            " "x12, x24, x5"),
        Q!("    mov             " "x11, v27.d[1]"),
        Q!("    cneg            " "x16, x12, cc"),
        Q!("    csetm           " "x2, cc"),
        Q!("    subs            " "x19, x15, x14"),
        Q!("    mov             " "x12, v1.d[1]"),
        Q!("    cinv            " "x2, x2, cc"),
        Q!("    cneg            " "x19, x19, cc"),
        Q!("    stp             " "x9, x20, [x0, #32]"),
        Q!("    mul             " "x9, x16, x19"),
        Q!("    adds            " "x4, x7, x4"),
        Q!("    adcs            " "x11, x1, x11"),
        Q!("    adc             " "x1, x12, xzr"),
        Q!("    adds            " "x20, x4, x22"),
        Q!("    umulh           " "x19, x16, x19"),
        Q!("    adcs            " "x7, x11, x4"),
        Q!("    eor             " "x16, x9, x2"),
        Q!("    adcs            " "x9, x1, x11"),
        Q!("    adc             " "x12, x1, xzr"),
        Q!("    adds            " "x7, x7, x22"),
        Q!("    adcs            " "x4, x9, x4"),
        Q!("    adcs            " "x9, x12, x11"),
        Q!("    adc             " "x12, x1, xzr"),
        Q!("    cmn             " "x2, #0x1"),
        Q!("    eor             " "x1, x19, x2"),
        Q!("    adcs            " "x11, x20, x16"),
        Q!("    adcs            " "x19, x7, x1"),
        Q!("    adcs            " "x1, x4, x2"),
        Q!("    adcs            " "x20, x9, x2"),
        Q!("    adc             " "x2, x12, x2"),
        Q!("    subs            " "x12, x24, x10"),
        Q!("    cneg            " "x16, x12, cc"),
        Q!("    csetm           " "x12, cc"),
        Q!("    subs            " "x9, x17, x14"),
        Q!("    cinv            " "x12, x12, cc"),
        Q!("    cneg            " "x9, x9, cc"),
        Q!("    subs            " "x3, x24, x3"),
        Q!("    sbcs            " "x21, x5, x21"),
        Q!("    mul             " "x24, x16, x9"),
        Q!("    sbcs            " "x4, x10, x8"),
        Q!("    ngc             " "x8, xzr"),
        Q!("    subs            " "x10, x5, x10"),
        Q!("    eor             " "x5, x24, x12"),
        Q!("    csetm           " "x7, cc"),
        Q!("    cneg            " "x24, x10, cc"),
        Q!("    subs            " "x10, x17, x15"),
        Q!("    cinv            " "x7, x7, cc"),
        Q!("    cneg            " "x10, x10, cc"),
        Q!("    subs            " "x14, x13, x14"),
        Q!("    sbcs            " "x15, x23, x15"),
        Q!("    eor             " "x13, x21, x8"),
        Q!("    mul             " "x23, x24, x10"),
        Q!("    sbcs            " "x17, x6, x17"),
        Q!("    eor             " "x6, x3, x8"),
        Q!("    ngc             " "x21, xzr"),
        Q!("    umulh           " "x9, x16, x9"),
        Q!("    cmn             " "x8, #0x1"),
        Q!("    eor             " "x3, x23, x7"),
        Q!("    adcs            " "x23, x6, xzr"),
        Q!("    adcs            " "x13, x13, xzr"),
        Q!("    eor             " "x16, x4, x8"),
        Q!("    adc             " "x16, x16, xzr"),
        Q!("    eor             " "x4, x17, x21"),
        Q!("    umulh           " "x17, x24, x10"),
        Q!("    cmn             " "x21, #0x1"),
        Q!("    eor             " "x24, x14, x21"),
        Q!("    eor             " "x6, x15, x21"),
        Q!("    adcs            " "x15, x24, xzr"),
        Q!("    adcs            " "x14, x6, xzr"),
        Q!("    adc             " "x6, x4, xzr"),
        Q!("    cmn             " "x12, #0x1"),
        Q!("    eor             " "x4, x9, x12"),
        Q!("    adcs            " "x19, x19, x5"),
        Q!("    umulh           " "x5, x23, x15"),
        Q!("    adcs            " "x1, x1, x4"),
        Q!("    adcs            " "x10, x20, x12"),
        Q!("    eor             " "x4, x17, x7"),
        Q!("    ldp             " "x20, x9, [x0]"),
        Q!("    adc             " "x2, x2, x12"),
        Q!("    cmn             " "x7, #0x1"),
        Q!("    adcs            " "x12, x1, x3"),
        Q!("    ldp             " "x17, x24, [x0, #16]"),
        Q!("    mul             " "x1, x16, x6"),
        Q!("    adcs            " "x3, x10, x4"),
        Q!("    adc             " "x2, x2, x7"),
        Q!("    ldp             " "x7, x4, [x0, #32]"),
        Q!("    adds            " "x20, x22, x20"),
        Q!("    mul             " "x10, x13, x14"),
        Q!("    adcs            " "x11, x11, x9"),
        Q!("    eor             " "x9, x8, x21"),
        Q!("    adcs            " "x21, x19, x17"),
        Q!("    stp             " "x20, x11, [x0]"),
        Q!("    adcs            " "x12, x12, x24"),
        Q!("    mul             " "x8, x23, x15"),
        Q!("    adcs            " "x3, x3, x7"),
        Q!("    stp             " "x21, x12, [x0, #16]"),
        Q!("    adcs            " "x12, x2, x4"),
        Q!("    adc             " "x19, xzr, xzr"),
        Q!("    subs            " "x21, x23, x16"),
        Q!("    umulh           " "x2, x16, x6"),
        Q!("    stp             " "x3, x12, [x0, #32]"),
        Q!("    cneg            " "x3, x21, cc"),
        Q!("    csetm           " "x24, cc"),
        Q!("    umulh           " "x11, x13, x14"),
        Q!("    subs            " "x21, x13, x16"),
        Q!("    eor             " "x7, x8, x9"),
        Q!("    cneg            " "x17, x21, cc"),
        Q!("    csetm           " "x16, cc"),
        Q!("    subs            " "x21, x6, x15"),
        Q!("    cneg            " "x22, x21, cc"),
        Q!("    cinv            " "x21, x24, cc"),
        Q!("    subs            " "x20, x23, x13"),
        Q!("    umulh           " "x12, x3, x22"),
        Q!("    cneg            " "x23, x20, cc"),
        Q!("    csetm           " "x24, cc"),
        Q!("    subs            " "x20, x14, x15"),
        Q!("    cinv            " "x24, x24, cc"),
        Q!("    mul             " "x22, x3, x22"),
        Q!("    cneg            " "x3, x20, cc"),
        Q!("    subs            " "x13, x6, x14"),
        Q!("    cneg            " "x20, x13, cc"),
        Q!("    cinv            " "x15, x16, cc"),
        Q!("    adds            " "x13, x5, x10"),
        Q!("    mul             " "x4, x23, x3"),
        Q!("    adcs            " "x11, x11, x1"),
        Q!("    adc             " "x14, x2, xzr"),
        Q!("    adds            " "x5, x13, x8"),
        Q!("    adcs            " "x16, x11, x13"),
        Q!("    umulh           " "x23, x23, x3"),
        Q!("    adcs            " "x3, x14, x11"),
        Q!("    adc             " "x1, x14, xzr"),
        Q!("    adds            " "x10, x16, x8"),
        Q!("    adcs            " "x6, x3, x13"),
        Q!("    adcs            " "x8, x1, x11"),
        Q!("    umulh           " "x13, x17, x20"),
        Q!("    eor             " "x1, x4, x24"),
        Q!("    adc             " "x4, x14, xzr"),
        Q!("    cmn             " "x24, #0x1"),
        Q!("    adcs            " "x1, x5, x1"),
        Q!("    eor             " "x16, x23, x24"),
        Q!("    eor             " "x11, x1, x9"),
        Q!("    adcs            " "x23, x10, x16"),
        Q!("    eor             " "x2, x22, x21"),
        Q!("    adcs            " "x3, x6, x24"),
        Q!("    mul             " "x14, x17, x20"),
        Q!("    eor             " "x17, x13, x15"),
        Q!("    adcs            " "x13, x8, x24"),
        Q!("    adc             " "x8, x4, x24"),
        Q!("    cmn             " "x21, #0x1"),
        Q!("    adcs            " "x6, x23, x2"),
        Q!("    mov             " "x16, #0xfffffffffffffffe"),
        Q!("    eor             " "x20, x12, x21"),
        Q!("    adcs            " "x20, x3, x20"),
        Q!("    eor             " "x23, x14, x15"),
        Q!("    adcs            " "x2, x13, x21"),
        Q!("    adc             " "x8, x8, x21"),
        Q!("    cmn             " "x15, #0x1"),
        Q!("    ldp             " "x5, x4, [x0]"),
        Q!("    ldp             " "x21, x12, [x0, #16]"),
        Q!("    adcs            " "x22, x20, x23"),
        Q!("    eor             " "x23, x22, x9"),
        Q!("    adcs            " "x17, x2, x17"),
        Q!("    adc             " "x22, x8, x15"),
        Q!("    cmn             " "x9, #0x1"),
        Q!("    adcs            " "x15, x7, x5"),
        Q!("    ldp             " "x10, x14, [x0, #32]"),
        Q!("    eor             " "x1, x6, x9"),
        Q!("    lsl             " "x2, x15, #32"),
        Q!("    adcs            " "x8, x11, x4"),
        Q!("    adcs            " "x13, x1, x21"),
        Q!("    eor             " "x1, x22, x9"),
        Q!("    adcs            " "x24, x23, x12"),
        Q!("    eor             " "x11, x17, x9"),
        Q!("    adcs            " "x23, x11, x10"),
        Q!("    adcs            " "x7, x1, x14"),
        Q!("    adcs            " "x17, x9, x19"),
        Q!("    adcs            " "x20, x9, xzr"),
        Q!("    add             " "x1, x2, x15"),
        Q!("    lsr             " "x3, x1, #32"),
        Q!("    adcs            " "x11, x9, xzr"),
        Q!("    adc             " "x9, x9, xzr"),
        Q!("    subs            " "x3, x3, x1"),
        Q!("    sbc             " "x6, x1, xzr"),
        Q!("    adds            " "x24, x24, x5"),
        Q!("    adcs            " "x4, x23, x4"),
        Q!("    extr            " "x3, x6, x3, #32"),
        Q!("    lsr             " "x6, x6, #32"),
        Q!("    adcs            " "x21, x7, x21"),
        Q!("    adcs            " "x15, x17, x12"),
        Q!("    adcs            " "x7, x20, x10"),
        Q!("    adcs            " "x20, x11, x14"),
        Q!("    mov             " "x14, #0xffffffff"),
        Q!("    adc             " "x22, x9, x19"),
        Q!("    adds            " "x12, x6, x1"),
        Q!("    adc             " "x10, xzr, xzr"),
        Q!("    subs            " "x3, x8, x3"),
        Q!("    sbcs            " "x12, x13, x12"),
        Q!("    lsl             " "x9, x3, #32"),
        Q!("    add             " "x3, x9, x3"),
        Q!("    sbcs            " "x10, x24, x10"),
        Q!("    sbcs            " "x24, x4, xzr"),
        Q!("    lsr             " "x9, x3, #32"),
        Q!("    sbcs            " "x21, x21, xzr"),
        Q!("    sbc             " "x1, x1, xzr"),
        Q!("    subs            " "x9, x9, x3"),
        Q!("    sbc             " "x13, x3, xzr"),
        Q!("    extr            " "x9, x13, x9, #32"),
        Q!("    lsr             " "x13, x13, #32"),
        Q!("    adds            " "x13, x13, x3"),
        Q!("    adc             " "x6, xzr, xzr"),
        Q!("    subs            " "x12, x12, x9"),
        Q!("    sbcs            " "x17, x10, x13"),
        Q!("    lsl             " "x2, x12, #32"),
        Q!("    sbcs            " "x10, x24, x6"),
        Q!("    add             " "x9, x2, x12"),
        Q!("    sbcs            " "x6, x21, xzr"),
        Q!("    lsr             " "x5, x9, #32"),
        Q!("    sbcs            " "x21, x1, xzr"),
        Q!("    sbc             " "x13, x3, xzr"),
        Q!("    subs            " "x8, x5, x9"),
        Q!("    sbc             " "x19, x9, xzr"),
        Q!("    lsr             " "x12, x19, #32"),
        Q!("    extr            " "x3, x19, x8, #32"),
        Q!("    adds            " "x8, x12, x9"),
        Q!("    adc             " "x1, xzr, xzr"),
        Q!("    subs            " "x2, x17, x3"),
        Q!("    sbcs            " "x12, x10, x8"),
        Q!("    sbcs            " "x5, x6, x1"),
        Q!("    sbcs            " "x3, x21, xzr"),
        Q!("    sbcs            " "x19, x13, xzr"),
        Q!("    sbc             " "x24, x9, xzr"),
        Q!("    adds            " "x23, x15, x3"),
        Q!("    adcs            " "x8, x7, x19"),
        Q!("    adcs            " "x11, x20, x24"),
        Q!("    adc             " "x9, x22, xzr"),
        Q!("    add             " "x24, x9, #0x1"),
        Q!("    lsl             " "x7, x24, #32"),
        Q!("    subs            " "x21, x24, x7"),
        Q!("    sbc             " "x10, x7, xzr"),
        Q!("    adds            " "x6, x2, x21"),
        Q!("    adcs            " "x7, x12, x10"),
        Q!("    adcs            " "x24, x5, x24"),
        Q!("    adcs            " "x13, x23, xzr"),
        Q!("    adcs            " "x8, x8, xzr"),
        Q!("    adcs            " "x15, x11, xzr"),
        Q!("    csetm           " "x23, cc"),
        Q!("    and             " "x11, x16, x23"),
        Q!("    and             " "x20, x14, x23"),
        Q!("    adds            " "x22, x6, x20"),
        Q!("    eor             " "x3, x20, x23"),
        Q!("    adcs            " "x5, x7, x3"),
        Q!("    adcs            " "x14, x24, x11"),
        Q!("    stp             " "x22, x5, [x0]"),
        Q!("    adcs            " "x5, x13, x23"),
        Q!("    adcs            " "x21, x8, x23"),
        Q!("    stp             " "x14, x5, [x0, #16]"),
        Q!("    adc             " "x12, x15, x23"),
        Q!("    stp             " "x21, x12, [x0, #32]"),

        // Restore registers and return

        Q!("    ldp             " "x23, x24, [sp], #16"),
        Q!("    ldp             " "x21, x22, [sp], #16"),
        Q!("    ldp             " "x19, x20, [sp], #16"),

        inout("x0") z.as_mut_ptr() => _,
        inout("x1") x.as_ptr() => _,
        inout("x2") y.as_ptr() => _,
        // clobbers
        out("v0") _,
        out("v1") _,
        out("v17") _,
        out("v20") _,
        out("v21") _,
        out("v22") _,
        out("v23") _,
        out("v24") _,
        out("v25") _,
        out("v26") _,
        out("v27") _,
        out("v3") _,
        out("v4") _,
        out("v6") _,
        out("x10") _,
        out("x11") _,
        out("x12") _,
        out("x13") _,
        out("x14") _,
        out("x15") _,
        out("x16") _,
        out("x17") _,
        out("x20") _,
        out("x21") _,
        out("x22") _,
        out("x23") _,
        out("x24") _,
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
