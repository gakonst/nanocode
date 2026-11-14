// generated source. do not edit.
#![allow(non_upper_case_globals, unused_macros, unused_imports)]
use crate::low::macros::*;

// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

// ----------------------------------------------------------------------------
// Montgomery square, z := (x^2 / 2^384) mod p_384
// Input x[6]; output z[6]
//
//    extern void bignum_montsqr_p384(uint64_t z[static 6],
//                                    const uint64_t x[static 6]);
//
// Does z := (x^2 / 2^384) mod p_384, assuming x^2 <= 2^384 * p_384, which is
// guaranteed in particular if x < p_384 initially (the "intended" case).
//
// Standard ARM ABI: X0 = z, X1 = x
// ----------------------------------------------------------------------------

// bignum_montsqr_p384 is functionally equivalent to
// unopt/bignum_montsqr_p384_base.
// It is written in a way that
// 1. A subset of scalar multiplications in bignum_montsqr_p384 are carefully
//    chosen and vectorized
// 2. The vectorized assembly is rescheduled using the SLOTHY superoptimizer.
//    https://github.com/slothy-optimizer/slothy
//
// The output program of step 1. is as follows:
//
//        ldp x9, x2, [x1]
//        ldr q18, [x1]
//        ldr q19, [x1]
//        ldp x4, x6, [x1, #16]
//        ldp x5, x10, [x1, #32]
//        ldr q21, [x1, #32]
//        ldr q28, [x1, #32]
//        mul x12, x9, x2
//        mul x1, x9, x4
//        mul x13, x2, x4
//        movi v0.2D, #0x00000000ffffffff
//        uzp2 v5.4S, v19.4S, v19.4S
//        xtn v25.2S, v18.2D
//        xtn v4.2S, v19.2D
//        rev64 v23.4S, v19.4S
//        umull v20.2D, v25.2S, v4.2S
//        umull v30.2D, v25.2S, v5.2S
//        uzp2 v19.4S, v18.4S, v18.4S
//        mul v22.4S, v23.4S, v18.4S
//        usra v30.2D, v20.2D, #32
//        umull v18.2D, v19.2S, v5.2S
//        uaddlp v22.2D, v22.4S
//        and v20.16B, v30.16B, v0.16B
//        umlal v20.2D, v19.2S, v4.2S
//        shl v19.2D, v22.2D, #32
//        usra v18.2D, v30.2D, #32
//        umlal v19.2D, v25.2S, v4.2S
//        usra v18.2D, v20.2D, #32
//        mov x7, v19.d[0]
//        mov x17, v19.d[1]
//        mul x16, x4, x4
//        umulh x3, x9, x2
//        adds x15, x1, x3
//        umulh x1, x9, x4
//        adcs x13, x13, x1
//        umulh x1, x2, x4
//        adcs x8, x1, xzr
//        mov x11, v18.d[0]
//        mov x14, v18.d[1]
//        umulh x1, x4, x4
//        adds x3, x12, x12
//        adcs x15, x15, x15
//        adcs x13, x13, x13
//        adcs x12, x8, x8
//        adc x1, x1, xzr
//        adds x11, x11, x3
//        adcs x3, x17, x15
//        adcs x17, x14, x13
//        adcs x15, x16, x12
//        adc x13, x1, xzr
//        lsl x1, x7, #32
//        add x16, x1, x7
//        lsr x1, x16, #32
//        subs x12, x1, x16
//        sbc x1, x16, xzr
//        extr x12, x1, x12, #32
//        lsr x1, x1, #32
//        adds x7, x1, x16
//        adc x1, xzr, xzr
//        subs x12, x11, x12
//        sbcs x11, x3, x7
//        sbcs x17, x17, x1
//        sbcs x15, x15, xzr
//        sbcs x13, x13, xzr
//        sbc x3, x16, xzr
//        lsl x1, x12, #32
//        add x16, x1, x12
//        lsr x1, x16, #32
//        subs x12, x1, x16
//        sbc x1, x16, xzr
//        extr x12, x1, x12, #32
//        lsr x1, x1, #32
//        adds x7, x1, x16
//        adc x1, xzr, xzr
//        subs x12, x11, x12
//        sbcs x17, x17, x7
//        sbcs x15, x15, x1
//        sbcs x13, x13, xzr
//        sbcs x11, x3, xzr
//        sbc x3, x16, xzr
//        lsl x1, x12, #32
//        add x16, x1, x12
//        lsr x1, x16, #32
//        subs x12, x1, x16
//        sbc x1, x16, xzr
//        extr x7, x1, x12, #32
//        lsr x1, x1, #32
//        adds x12, x1, x16
//        adc x1, xzr, xzr
//        subs x17, x17, x7
//        sbcs x15, x15, x12
//        sbcs x13, x13, x1
//        sbcs x7, x11, xzr
//        sbcs x12, x3, xzr
//        sbc x1, x16, xzr
//        stp x17, x15, [x0]                     // @slothy:writes=buffer0
//        stp x13, x7, [x0, #16]                 // @slothy:writes=buffer16
//        stp x12, x1, [x0, #32]                 // @slothy:writes=buffer32
//        mul x14, x9, x6
//        mul x15, x2, x5
//        mul x13, x4, x10
//        umulh x7, x9, x6
//        umulh x12, x2, x5
//        umulh x1, x4, x10
//        adds x15, x7, x15
//        adcs x16, x12, x13
//        adc x13, x1, xzr
//        adds x11, x15, x14
//        adcs x7, x16, x15
//        adcs x12, x13, x16
//        adc x1, x13, xzr
//        adds x17, x7, x14
//        adcs x15, x12, x15
//        adcs x3, x1, x16
//        adc x16, x13, xzr
//        subs x1, x9, x2
//        cneg x13, x1, cc
//        csetm x7, cc
//        subs x1, x5, x6
//        cneg x1, x1, cc
//        mul x12, x13, x1
//        umulh x1, x13, x1
//        cinv x7, x7, cc
//        eor x12, x12, x7
//        eor x1, x1, x7
//        cmn x7, #0x1
//        adcs x11, x11, x12
//        adcs x17, x17, x1
//        adcs x15, x15, x7
//        adcs x3, x3, x7
//        adc x16, x16, x7
//        subs x9, x9, x4
//        cneg x13, x9, cc
//        csetm x7, cc
//        subs x1, x10, x6
//        cneg x1, x1, cc
//        mul x12, x13, x1
//        umulh x1, x13, x1
//        cinv x7, x7, cc
//        eor x12, x12, x7
//        eor x1, x1, x7
//        cmn x7, #0x1
//        adcs x17, x17, x12
//        adcs x15, x15, x1
//        adcs x13, x3, x7
//        adc x7, x16, x7
//        subs x2, x2, x4
//        cneg x12, x2, cc
//        csetm x1, cc
//        subs x2, x10, x5
//        cneg x2, x2, cc
//        mul x4, x12, x2
//        umulh x2, x12, x2
//        cinv x1, x1, cc
//        eor x4, x4, x1
//        eor x2, x2, x1
//        cmn x1, #0x1
//        adcs x12, x15, x4
//        adcs x4, x13, x2
//        adc x2, x7, x1
//        adds x1, x14, x14
//        adcs x16, x11, x11
//        adcs x17, x17, x17
//        adcs x15, x12, x12
//        adcs x13, x4, x4
//        adcs x7, x2, x2
//        adc x12, xzr, xzr
//        ldp x4, x2, [x0]                       // @slothy:reads=buffer0
//        adds x1, x1, x4
//        adcs x16, x16, x2
//        ldp x4, x2, [x0, #16]                  // @slothy:reads=buffer16
//        adcs x17, x17, x4
//        adcs x15, x15, x2
//        ldp x4, x2, [x0, #32]                  // @slothy:reads=buffer32
//        adcs x13, x13, x4
//        adcs x7, x7, x2
//        adc x11, x12, xzr
//        lsl x2, x1, #32
//        add x12, x2, x1
//        lsr x2, x12, #32
//        subs x4, x2, x12
//        sbc x2, x12, xzr
//        extr x4, x2, x4, #32
//        lsr x2, x2, #32
//        adds x1, x2, x12
//        adc x2, xzr, xzr
//        subs x4, x16, x4
//        sbcs x16, x17, x1
//        sbcs x17, x15, x2
//        sbcs x15, x13, xzr
//        sbcs x13, x7, xzr
//        sbc x7, x12, xzr
//        lsl x2, x4, #32
//        add x12, x2, x4
//        lsr x2, x12, #32
//        subs x4, x2, x12
//        sbc x2, x12, xzr
//        extr x4, x2, x4, #32
//        lsr x2, x2, #32
//        adds x1, x2, x12
//        adc x2, xzr, xzr
//        subs x4, x16, x4
//        sbcs x16, x17, x1
//        sbcs x17, x15, x2
//        sbcs x15, x13, xzr
//        sbcs x13, x7, xzr
//        sbc x7, x12, xzr
//        lsl x2, x4, #32
//        add x12, x2, x4
//        lsr x2, x12, #32
//        subs x4, x2, x12
//        sbc x2, x12, xzr
//        extr x1, x2, x4, #32
//        lsr x2, x2, #32
//        adds x4, x2, x12
//        adc x2, xzr, xzr
//        subs x3, x16, x1
//        sbcs x17, x17, x4
//        sbcs x15, x15, x2
//        sbcs x1, x13, xzr
//        sbcs x4, x7, xzr
//        sbc x2, x12, xzr
//        adds x13, x11, x1
//        adcs x7, x4, xzr
//        adcs x12, x2, xzr
//        adcs x16, xzr, xzr
//        mul x2, x6, x6
//        adds x3, x3, x2
//        xtn v30.2S, v28.2D
//        shrn v26.2S, v28.2D, #32
//        umull v26.2D, v30.2S, v26.2S
//        shl v19.2D, v26.2D, #33
//        umlal v19.2D, v30.2S, v30.2S
//        mov x1, v19.d[0]
//        mov x4, v19.d[1]
//        umulh x2, x6, x6
//        adcs x17, x17, x2
//        umulh x2, x5, x5
//        adcs x15, x15, x1
//        adcs x13, x13, x2
//        umulh x2, x10, x10
//        adcs x7, x7, x4
//        adcs x12, x12, x2
//        adc x16, x16, xzr
//        dup v28.2D, x6
//        movi v0.2D, #0x00000000ffffffff
//        uzp2 v5.4S, v21.4S, v21.4S
//        xtn v25.2S, v28.2D
//        xtn v4.2S, v21.2D
//        rev64 v19.4S, v21.4S
//        umull v30.2D, v25.2S, v4.2S
//        umull v23.2D, v25.2S, v5.2S
//        uzp2 v20.4S, v28.4S, v28.4S
//        mul v19.4S, v19.4S, v28.4S
//        usra v23.2D, v30.2D, #32
//        umull v18.2D, v20.2S, v5.2S
//        uaddlp v19.2D, v19.4S
//        and v30.16B, v23.16B, v0.16B
//        umlal v30.2D, v20.2S, v4.2S
//        shl v19.2D, v19.2D, #32
//        usra v18.2D, v23.2D, #32
//        umlal v19.2D, v25.2S, v4.2S
//        usra v18.2D, v30.2D, #32
//        mov x6, v19.d[0]
//        mov x1, v19.d[1]
//        mul x4, x5, x10
//        mov x2, v18.d[0]
//        adds x1, x1, x2
//        mov x2, v18.d[1]
//        adcs x4, x4, x2
//        umulh x5, x5, x10
//        adc x2, x5, xzr
//        adds x5, x6, x6
//        adcs x6, x1, x1
//        adcs x1, x4, x4
//        adcs x4, x2, x2
//        adc x2, xzr, xzr
//        adds x17, x17, x5
//        adcs x15, x15, x6
//        adcs x13, x13, x1
//        adcs x7, x7, x4
//        adcs x12, x12, x2
//        adc x2, x16, xzr
//        mov x5, #0xffffffff00000001
//        mov x6, #0xffffffff
//        mov x1, #0x1
//        cmn x3, x5
//        adcs xzr, x17, x6
//        adcs xzr, x15, x1
//        adcs xzr, x13, xzr
//        adcs xzr, x7, xzr
//        adcs xzr, x12, xzr
//        adc x2, x2, xzr
//        neg x4, x2
//        and x2, x5, x4
//        adds x10, x3, x2
//        and x2, x6, x4
//        adcs x5, x17, x2
//        and x2, x1, x4
//        adcs x6, x15, x2
//        adcs x1, x13, xzr
//        adcs x4, x7, xzr
//        adc x2, x12, xzr
//        stp x10, x5, [x0]                      // @slothy:writes=buffer0
//        stp x6, x1, [x0, #16]                  // @slothy:writes=buffer16
//        stp x4, x2, [x0, #32]                  // @slothy:writes=buffer32
//        ret
//
// The bash script used for step 2 is as follows:
//
//        # Store the assembly instructions except the last 'ret' as, say, 'input.S'.
//        export OUTPUTS="[hint_buffer0,hint_buffer16,hint_buffer32]"
//        export RESERVED_REGS="[x18,x19,x20,x21,x22,x23,x24,x25,x26,x27,x28,x29,x30,sp,q8,q9,q10,q11,q12,q13,q14,q15,v8,v9,v10,v11,v12,v13,v14,v15]"
//        <s2n-bignum>/tools/external/slothy.sh input.S my_out_dir
//        # my_out_dir/3.opt.s is the optimized assembly. Its output may differ
//        # from this file since the sequence is non-deterministically chosen.
//        # Please add 'ret' at the end of the output assembly.

/// Montgomery square, z := (x^2 / 2^384) mod p_384
///
/// Input x[6]; output z[6]
///
/// Does z := (x^2 / 2^384) mod p_384, assuming x^2 <= 2^384 * p_384, which is
/// guaranteed in particular if x < p_384 initially (the "intended" case).
pub(crate) fn bignum_montsqr_p384(z: &mut [u64; 6], x: &[u64; 6]) {
    // SAFETY: inline assembly. see [crate::low::inline_assembly_safety] for safety info.
    unsafe {
        core::arch::asm!(

        Q!("    ldr             " "q1, [x1]"),
        Q!("    ldp             " "x9, x2, [x1]"),
        Q!("    ldr             " "q0, [x1]"),
        Q!("    ldp             " "x4, x6, [x1, #16]"),
        Q!("    rev64           " "v21.4S, v1.4S"),
        Q!("    uzp2            " "v28.4S, v1.4S, v1.4S"),
        Q!("    umulh           " "x7, x9, x2"),
        Q!("    xtn             " "v17.2S, v1.2D"),
        Q!("    mul             " "v27.4S, v21.4S, v0.4S"),
        Q!("    ldr             " "q20, [x1, #32]"),
        Q!("    xtn             " "v30.2S, v0.2D"),
        Q!("    ldr             " "q1, [x1, #32]"),
        Q!("    uzp2            " "v31.4S, v0.4S, v0.4S"),
        Q!("    ldp             " "x5, x10, [x1, #32]"),
        Q!("    umulh           " "x8, x9, x4"),
        Q!("    uaddlp          " "v3.2D, v27.4S"),
        Q!("    umull           " "v16.2D, v30.2S, v17.2S"),
        Q!("    mul             " "x16, x9, x4"),
        Q!("    umull           " "v27.2D, v30.2S, v28.2S"),
        Q!("    shrn            " "v0.2S, v20.2D, #32"),
        Q!("    xtn             " "v7.2S, v20.2D"),
        Q!("    shl             " "v20.2D, v3.2D, #32"),
        Q!("    umull           " "v3.2D, v31.2S, v28.2S"),
        Q!("    mul             " "x3, x2, x4"),
        Q!("    umlal           " "v20.2D, v30.2S, v17.2S"),
        Q!("    umull           " "v22.2D, v7.2S, v0.2S"),
        Q!("    usra            " "v27.2D, v16.2D, #32"),
        Q!("    umulh           " "x11, x2, x4"),
        Q!("    movi            " "v21.2D, #0x00000000ffffffff"),
        Q!("    uzp2            " "v28.4S, v1.4S, v1.4S"),
        Q!("    adds            " "x15, x16, x7"),
        Q!("    and             " "v5.16B, v27.16B, v21.16B"),
        Q!("    adcs            " "x3, x3, x8"),
        Q!("    usra            " "v3.2D, v27.2D, #32"),
        Q!("    dup             " "v29.2D, x6"),
        Q!("    adcs            " "x16, x11, xzr"),
        Q!("    mov             " "x14, v20.d[0]"),
        Q!("    umlal           " "v5.2D, v31.2S, v17.2S"),
        Q!("    mul             " "x8, x9, x2"),
        Q!("    mov             " "x7, v20.d[1]"),
        Q!("    shl             " "v19.2D, v22.2D, #33"),
        Q!("    xtn             " "v25.2S, v29.2D"),
        Q!("    rev64           " "v31.4S, v1.4S"),
        Q!("    lsl             " "x13, x14, #32"),
        Q!("    uzp2            " "v6.4S, v29.4S, v29.4S"),
        Q!("    umlal           " "v19.2D, v7.2S, v7.2S"),
        Q!("    usra            " "v3.2D, v5.2D, #32"),
        Q!("    adds            " "x1, x8, x8"),
        Q!("    umulh           " "x8, x4, x4"),
        Q!("    add             " "x12, x13, x14"),
        Q!("    mul             " "v17.4S, v31.4S, v29.4S"),
        Q!("    xtn             " "v4.2S, v1.2D"),
        Q!("    adcs            " "x14, x15, x15"),
        Q!("    lsr             " "x13, x12, #32"),
        Q!("    adcs            " "x15, x3, x3"),
        Q!("    umull           " "v31.2D, v25.2S, v28.2S"),
        Q!("    adcs            " "x11, x16, x16"),
        Q!("    umull           " "v21.2D, v25.2S, v4.2S"),
        Q!("    mov             " "x17, v3.d[0]"),
        Q!("    umull           " "v18.2D, v6.2S, v28.2S"),
        Q!("    adc             " "x16, x8, xzr"),
        Q!("    uaddlp          " "v16.2D, v17.4S"),
        Q!("    movi            " "v1.2D, #0x00000000ffffffff"),
        Q!("    subs            " "x13, x13, x12"),
        Q!("    usra            " "v31.2D, v21.2D, #32"),
        Q!("    sbc             " "x8, x12, xzr"),
        Q!("    adds            " "x17, x17, x1"),
        Q!("    mul             " "x1, x4, x4"),
        Q!("    shl             " "v28.2D, v16.2D, #32"),
        Q!("    mov             " "x3, v3.d[1]"),
        Q!("    adcs            " "x14, x7, x14"),
        Q!("    extr            " "x7, x8, x13, #32"),
        Q!("    adcs            " "x13, x3, x15"),
        Q!("    and             " "v3.16B, v31.16B, v1.16B"),
        Q!("    adcs            " "x11, x1, x11"),
        Q!("    lsr             " "x1, x8, #32"),
        Q!("    umlal           " "v3.2D, v6.2S, v4.2S"),
        Q!("    usra            " "v18.2D, v31.2D, #32"),
        Q!("    adc             " "x3, x16, xzr"),
        Q!("    adds            " "x1, x1, x12"),
        Q!("    umlal           " "v28.2D, v25.2S, v4.2S"),
        Q!("    adc             " "x16, xzr, xzr"),
        Q!("    subs            " "x15, x17, x7"),
        Q!("    sbcs            " "x7, x14, x1"),
        Q!("    lsl             " "x1, x15, #32"),
        Q!("    sbcs            " "x16, x13, x16"),
        Q!("    add             " "x8, x1, x15"),
        Q!("    usra            " "v18.2D, v3.2D, #32"),
        Q!("    sbcs            " "x14, x11, xzr"),
        Q!("    lsr             " "x1, x8, #32"),
        Q!("    sbcs            " "x17, x3, xzr"),
        Q!("    sbc             " "x11, x12, xzr"),
        Q!("    subs            " "x13, x1, x8"),
        Q!("    umulh           " "x12, x4, x10"),
        Q!("    sbc             " "x1, x8, xzr"),
        Q!("    extr            " "x13, x1, x13, #32"),
        Q!("    lsr             " "x1, x1, #32"),
        Q!("    adds            " "x15, x1, x8"),
        Q!("    adc             " "x1, xzr, xzr"),
        Q!("    subs            " "x7, x7, x13"),
        Q!("    sbcs            " "x13, x16, x15"),
        Q!("    lsl             " "x3, x7, #32"),
        Q!("    umulh           " "x16, x2, x5"),
        Q!("    sbcs            " "x15, x14, x1"),
        Q!("    add             " "x7, x3, x7"),
        Q!("    sbcs            " "x3, x17, xzr"),
        Q!("    lsr             " "x1, x7, #32"),
        Q!("    sbcs            " "x14, x11, xzr"),
        Q!("    sbc             " "x11, x8, xzr"),
        Q!("    subs            " "x8, x1, x7"),
        Q!("    sbc             " "x1, x7, xzr"),
        Q!("    extr            " "x8, x1, x8, #32"),
        Q!("    lsr             " "x1, x1, #32"),
        Q!("    adds            " "x1, x1, x7"),
        Q!("    adc             " "x17, xzr, xzr"),
        Q!("    subs            " "x13, x13, x8"),
        Q!("    umulh           " "x8, x9, x6"),
        Q!("    sbcs            " "x1, x15, x1"),
        Q!("    sbcs            " "x15, x3, x17"),
        Q!("    sbcs            " "x3, x14, xzr"),
        Q!("    mul             " "x17, x2, x5"),
        Q!("    sbcs            " "x11, x11, xzr"),
        Q!("    stp             " "x13, x1, [x0]"),
        Q!("    sbc             " "x14, x7, xzr"),
        Q!("    mul             " "x7, x4, x10"),
        Q!("    subs            " "x1, x9, x2"),
        Q!("    stp             " "x15, x3, [x0, #16]"),
        Q!("    csetm           " "x15, cc"),
        Q!("    cneg            " "x1, x1, cc"),
        Q!("    stp             " "x11, x14, [x0, #32]"),
        Q!("    mul             " "x14, x9, x6"),
        Q!("    adds            " "x17, x8, x17"),
        Q!("    adcs            " "x7, x16, x7"),
        Q!("    adc             " "x13, x12, xzr"),
        Q!("    subs            " "x12, x5, x6"),
        Q!("    cneg            " "x3, x12, cc"),
        Q!("    cinv            " "x16, x15, cc"),
        Q!("    mul             " "x8, x1, x3"),
        Q!("    umulh           " "x1, x1, x3"),
        Q!("    eor             " "x12, x8, x16"),
        Q!("    adds            " "x11, x17, x14"),
        Q!("    adcs            " "x3, x7, x17"),
        Q!("    adcs            " "x15, x13, x7"),
        Q!("    adc             " "x8, x13, xzr"),
        Q!("    adds            " "x3, x3, x14"),
        Q!("    adcs            " "x15, x15, x17"),
        Q!("    adcs            " "x17, x8, x7"),
        Q!("    eor             " "x1, x1, x16"),
        Q!("    adc             " "x13, x13, xzr"),
        Q!("    subs            " "x9, x9, x4"),
        Q!("    csetm           " "x8, cc"),
        Q!("    cneg            " "x9, x9, cc"),
        Q!("    subs            " "x4, x2, x4"),
        Q!("    cneg            " "x4, x4, cc"),
        Q!("    csetm           " "x7, cc"),
        Q!("    subs            " "x2, x10, x6"),
        Q!("    cinv            " "x8, x8, cc"),
        Q!("    cneg            " "x2, x2, cc"),
        Q!("    cmn             " "x16, #0x1"),
        Q!("    adcs            " "x11, x11, x12"),
        Q!("    mul             " "x12, x9, x2"),
        Q!("    adcs            " "x3, x3, x1"),
        Q!("    adcs            " "x15, x15, x16"),
        Q!("    umulh           " "x9, x9, x2"),
        Q!("    adcs            " "x17, x17, x16"),
        Q!("    adc             " "x13, x13, x16"),
        Q!("    subs            " "x1, x10, x5"),
        Q!("    cinv            " "x2, x7, cc"),
        Q!("    cneg            " "x1, x1, cc"),
        Q!("    eor             " "x9, x9, x8"),
        Q!("    cmn             " "x8, #0x1"),
        Q!("    eor             " "x7, x12, x8"),
        Q!("    mul             " "x12, x4, x1"),
        Q!("    adcs            " "x3, x3, x7"),
        Q!("    adcs            " "x7, x15, x9"),
        Q!("    adcs            " "x15, x17, x8"),
        Q!("    ldp             " "x9, x17, [x0, #16]"),
        Q!("    umulh           " "x4, x4, x1"),
        Q!("    adc             " "x8, x13, x8"),
        Q!("    cmn             " "x2, #0x1"),
        Q!("    eor             " "x1, x12, x2"),
        Q!("    adcs            " "x1, x7, x1"),
        Q!("    ldp             " "x7, x16, [x0]"),
        Q!("    eor             " "x12, x4, x2"),
        Q!("    adcs            " "x4, x15, x12"),
        Q!("    ldp             " "x15, x12, [x0, #32]"),
        Q!("    adc             " "x8, x8, x2"),
        Q!("    adds            " "x13, x14, x14"),
        Q!("    umulh           " "x14, x5, x10"),
        Q!("    adcs            " "x2, x11, x11"),
        Q!("    adcs            " "x3, x3, x3"),
        Q!("    adcs            " "x1, x1, x1"),
        Q!("    adcs            " "x4, x4, x4"),
        Q!("    adcs            " "x11, x8, x8"),
        Q!("    adc             " "x8, xzr, xzr"),
        Q!("    adds            " "x13, x13, x7"),
        Q!("    adcs            " "x2, x2, x16"),
        Q!("    mul             " "x16, x5, x10"),
        Q!("    adcs            " "x3, x3, x9"),
        Q!("    adcs            " "x1, x1, x17"),
        Q!("    umulh           " "x5, x5, x5"),
        Q!("    lsl             " "x9, x13, #32"),
        Q!("    add             " "x9, x9, x13"),
        Q!("    adcs            " "x4, x4, x15"),
        Q!("    mov             " "x13, v28.d[1]"),
        Q!("    adcs            " "x15, x11, x12"),
        Q!("    lsr             " "x7, x9, #32"),
        Q!("    adc             " "x11, x8, xzr"),
        Q!("    subs            " "x7, x7, x9"),
        Q!("    umulh           " "x10, x10, x10"),
        Q!("    sbc             " "x17, x9, xzr"),
        Q!("    extr            " "x7, x17, x7, #32"),
        Q!("    lsr             " "x17, x17, #32"),
        Q!("    adds            " "x17, x17, x9"),
        Q!("    adc             " "x12, xzr, xzr"),
        Q!("    subs            " "x8, x2, x7"),
        Q!("    sbcs            " "x17, x3, x17"),
        Q!("    lsl             " "x7, x8, #32"),
        Q!("    sbcs            " "x2, x1, x12"),
        Q!("    add             " "x3, x7, x8"),
        Q!("    sbcs            " "x12, x4, xzr"),
        Q!("    lsr             " "x1, x3, #32"),
        Q!("    sbcs            " "x7, x15, xzr"),
        Q!("    sbc             " "x15, x9, xzr"),
        Q!("    subs            " "x1, x1, x3"),
        Q!("    sbc             " "x4, x3, xzr"),
        Q!("    lsr             " "x9, x4, #32"),
        Q!("    extr            " "x8, x4, x1, #32"),
        Q!("    adds            " "x9, x9, x3"),
        Q!("    adc             " "x4, xzr, xzr"),
        Q!("    subs            " "x1, x17, x8"),
        Q!("    lsl             " "x17, x1, #32"),
        Q!("    sbcs            " "x8, x2, x9"),
        Q!("    sbcs            " "x9, x12, x4"),
        Q!("    add             " "x17, x17, x1"),
        Q!("    mov             " "x1, v18.d[1]"),
        Q!("    lsr             " "x2, x17, #32"),
        Q!("    sbcs            " "x7, x7, xzr"),
        Q!("    mov             " "x12, v18.d[0]"),
        Q!("    sbcs            " "x15, x15, xzr"),
        Q!("    sbc             " "x3, x3, xzr"),
        Q!("    subs            " "x4, x2, x17"),
        Q!("    sbc             " "x2, x17, xzr"),
        Q!("    adds            " "x12, x13, x12"),
        Q!("    adcs            " "x16, x16, x1"),
        Q!("    lsr             " "x13, x2, #32"),
        Q!("    extr            " "x1, x2, x4, #32"),
        Q!("    adc             " "x2, x14, xzr"),
        Q!("    adds            " "x4, x13, x17"),
        Q!("    mul             " "x13, x6, x6"),
        Q!("    adc             " "x14, xzr, xzr"),
        Q!("    subs            " "x1, x8, x1"),
        Q!("    sbcs            " "x4, x9, x4"),
        Q!("    mov             " "x9, v28.d[0]"),
        Q!("    sbcs            " "x7, x7, x14"),
        Q!("    sbcs            " "x8, x15, xzr"),
        Q!("    sbcs            " "x3, x3, xzr"),
        Q!("    sbc             " "x14, x17, xzr"),
        Q!("    adds            " "x17, x9, x9"),
        Q!("    adcs            " "x12, x12, x12"),
        Q!("    mov             " "x15, v19.d[0]"),
        Q!("    adcs            " "x9, x16, x16"),
        Q!("    umulh           " "x6, x6, x6"),
        Q!("    adcs            " "x16, x2, x2"),
        Q!("    adc             " "x2, xzr, xzr"),
        Q!("    adds            " "x11, x11, x8"),
        Q!("    adcs            " "x3, x3, xzr"),
        Q!("    adcs            " "x14, x14, xzr"),
        Q!("    adcs            " "x8, xzr, xzr"),
        Q!("    adds            " "x13, x1, x13"),
        Q!("    mov             " "x1, v19.d[1]"),
        Q!("    adcs            " "x6, x4, x6"),
        Q!("    mov             " "x4, #0xffffffff"),
        Q!("    adcs            " "x15, x7, x15"),
        Q!("    adcs            " "x7, x11, x5"),
        Q!("    adcs            " "x1, x3, x1"),
        Q!("    adcs            " "x14, x14, x10"),
        Q!("    adc             " "x11, x8, xzr"),
        Q!("    adds            " "x6, x6, x17"),
        Q!("    adcs            " "x8, x15, x12"),
        Q!("    adcs            " "x3, x7, x9"),
        Q!("    adcs            " "x15, x1, x16"),
        Q!("    mov             " "x16, #0xffffffff00000001"),
        Q!("    adcs            " "x14, x14, x2"),
        Q!("    mov             " "x2, #0x1"),
        Q!("    adc             " "x17, x11, xzr"),
        Q!("    cmn             " "x13, x16"),
        Q!("    adcs            " "xzr, x6, x4"),
        Q!("    adcs            " "xzr, x8, x2"),
        Q!("    adcs            " "xzr, x3, xzr"),
        Q!("    adcs            " "xzr, x15, xzr"),
        Q!("    adcs            " "xzr, x14, xzr"),
        Q!("    adc             " "x1, x17, xzr"),
        Q!("    neg             " "x9, x1"),
        Q!("    and             " "x1, x16, x9"),
        Q!("    adds            " "x11, x13, x1"),
        Q!("    and             " "x13, x4, x9"),
        Q!("    adcs            " "x5, x6, x13"),
        Q!("    and             " "x1, x2, x9"),
        Q!("    adcs            " "x7, x8, x1"),
        Q!("    stp             " "x11, x5, [x0]"),
        Q!("    adcs            " "x11, x3, xzr"),
        Q!("    adcs            " "x2, x15, xzr"),
        Q!("    stp             " "x7, x11, [x0, #16]"),
        Q!("    adc             " "x17, x14, xzr"),
        Q!("    stp             " "x2, x17, [x0, #32]"),

        inout("x0") z.as_mut_ptr() => _,
        inout("x1") x.as_ptr() => _,
        // clobbers
        out("v0") _,
        out("v1") _,
        out("v16") _,
        out("v17") _,
        out("v18") _,
        out("v19") _,
        out("v20") _,
        out("v21") _,
        out("v22") _,
        out("v25") _,
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
        out("x10") _,
        out("x11") _,
        out("x12") _,
        out("x13") _,
        out("x14") _,
        out("x15") _,
        out("x16") _,
        out("x17") _,
        out("x2") _,
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
