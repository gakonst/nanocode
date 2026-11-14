// generated source. do not edit.
#![allow(non_upper_case_globals, unused_macros, unused_imports)]
use crate::low::macros::*;

// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

// ----------------------------------------------------------------------------
// Keccak-f1600 permutation for SHA3, batch of four independent operations
// Input a[100], rc[24], rho8[4], rho56[4]; output a[100]
//
// The input/output argument is in effect four 25-element Keccak arrays
// a[0...24], a[25..49], a[50..74] and a[75..99], which could be considered
// as type a[25][4].
//
// Keccak-f1600 permutation operation is at the core of SHA3 and SHAKE
// and is fully specified here:
//
//   https://keccak.team/files/Keccak-reference-3.0.pdf
//
//    extern void sha3_keccak4_f1600_alt(uint64_t a[100], const uint64_t rc[24], const uint64_t rho8[4], const uint64_t rho56[4]);
//
// Standard x86-64 ABI: RDI = a, RSI = rc, RDX = rho8, RCX = rho56
// Microsoft x64 ABI:   RCX = a, RDX = rc, R8 = rho8, R9 = rho56
// ----------------------------------------------------------------------------

/// Keccak-f1600 permutation for SHA3, batch of four independent operations
///
/// Input a[100], rc[24], rho8[4], rho56[4]; output a[100]
///
/// The input/output argument is in effect four 25-element Keccak arrays
/// a[0...24], a[25..49], a[50..74] and a[75..99], which could be considered
/// as type a[25][4].
///
/// Keccak-f1600 permutation operation is at the core of SHA3 and SHAKE
/// and is fully specified here:
///
///   https://keccak.team/files/Keccak-reference-3.0.pdf
pub(crate) fn sha3_keccak4_f1600(
    a: &mut [[u64; 25]; 4],
    rc: &[u64; 24],
    rho8: &[u64; 4],
    rho56: &[u64; 4],
) {
    // SAFETY: inline assembly. see [crate::low::inline_assembly_safety] for safety info.
    unsafe {
        core::arch::asm!(

        Q!("    endbr64         " ),

        // **** Bitstates Allocation Map **** //
        // 0x0(%rsp)     A[0]    [state0[0], state1[0], state2[0], state3[0]]     Input (%rdi) offsets: 0x00, 0xC8, 0x190, 0x258
        // 0x20(%rsp)    A[1]    [state0[1], state1[1], state2[1], state3[1]]     Input (%rdi) offsets: 0x08, 0xD0, 0x198, 0x260
        // 0x40(%rsp)    A[2]    [state0[2], state1[2], state2[2], state3[2]]     Input (%rdi) offsets: 0x10, 0xD8, 0x1A0, 0x268
        // 0x60(%rsp)    A[3]    [state0[3], state1[3], state2[3], state3[3]]     Input (%rdi) offsets: 0x18, 0xE0, 0x1A8, 0x270
        // 0x80(%rsp)    A[4]    [state0[4], state1[4], state2[4], state3[4]]     Input (%rdi) offsets: 0x20, 0xE8, 0x1B0, 0x278
        // 0xa0(%rsp)    A[5]    [state0[5], state1[5], state2[5], state3[5]]     Input (%rdi) offsets: 0x28, 0xF0, 0x1B8, 0x280
        // 0xc0(%rsp)    A[6]    [state0[6], state1[6], state2[6], state3[6]]     Input (%rdi) offsets: 0x30, 0xF8, 0x1C0, 0x288
        // ymm10         A[7]    [state0[7], state1[7], state2[7], state3[7]]     Input (%rdi) offsets: 0x38, 0x100, 0x1C8, 0x290
        // ymm14         A[8]    [state0[8], state1[8], state2[8], state3[8]]     Input (%rdi) offsets: 0x40, 0x108, 0x1D0, 0x298
        // 0xe0(%rsp)    A[9]    [state0[9], state1[9], state2[9], state3[9]]     Input (%rdi) offsets: 0x48, 0x110, 0x1D8, 0x2A0
        // 0x100(%rsp)   A[10]   [state0[10], state1[10], state2[10], state3[10]] Input (%rdi) offsets: 0x50, 0x118, 0x1E0, 0x2A8
        // ymm8          A[11]   [state0[11], state1[11], state2[11], state3[11]] Input (%rdi) offsets: 0x58, 0x120, 0x1E8, 0x2B0
        // ymm15         A[12]   [state0[12], state1[12], state2[12], state3[12]] Input (%rdi) offsets: 0x60, 0x128, 0x1F0, 0x2B8
        // 0x120(%rsp)   A[13]   [state0[13], state1[13], state2[13], state3[13]] Input (%rdi) offsets: 0x68, 0x130, 0x1F8, 0x2C0
        // 0x140(%rsp)   A[14]   [state0[14], state1[14], state2[14], state3[14]] Input (%rdi) offsets: 0x70, 0x138, 0x200, 0x2C8
        // ymm9          A[15]   [state0[15], state1[15], state2[15], state3[15]] Input (%rdi) offsets: 0x78, 0x140, 0x208, 0x2D0
        // 0x160(%rsp)   A[16]   [state0[16], state1[16], state2[16], state3[16]] Input (%rdi) offsets: 0x80, 0x148, 0x210, 0x2D8
        // 0x180(%rsp)   A[17]   [state0[17], state1[17], state2[17], state3[17]] Input (%rdi) offsets: 0x88, 0x150, 0x218, 0x2E0
        // ymm13         A[18]   [state0[18], state1[18], state2[18], state3[18]] Input (%rdi) offsets: 0x90, 0x158, 0x220, 0x2E8
        // 0x1a0(%rsp)   A[19]   [state0[19], state1[19], state2[19], state3[19]] Input (%rdi) offsets: 0x98, 0x160, 0x228, 0x2F0
        // 0x1c0(%rsp)   A[20]   [state0[20], state1[20], state2[20], state3[20]] Input (%rdi) offsets: 0xA0, 0x168, 0x230, 0x2F8
        // ymm3          A[21]   [state0[21], state1[21], state2[21], state3[21]] Input (%rdi) offsets: 0xA8, 0x170, 0x238, 0x300
        // ymm7          A[22]   [state0[22], state1[22], state2[22], state3[22]] Input (%rdi) offsets: 0xB0, 0x178, 0x240, 0x308
        // 0x1e0(%rsp)   A[23]   [state0[23], state1[23], state2[23], state3[23]] Input (%rdi) offsets: 0xB8, 0x180, 0x248, 0x310
        // ymm2          A[24]   [state0[24], state1[24], state2[24], state3[24]] Input (%rdi) offsets: 0xC0, 0x188, 0x250, 0x318

        Q!("    mov             " "r11, rsp"),
        Q!("    and             " "rsp, 0xffffffffffffffe0"),
        Q!("    sub             " "rsp, 0x300"),

        // Load 32 bytes from each of the 4 states (A[0-3])
        Q!("    vmovdqu         " "ymm0, [rdi]"),
        Q!("    vmovdqu         " "ymm3, [rdi + 0xc8]"),
        Q!("    vmovdqu         " "ymm1, [rdi + 0x190]"),
        Q!("    vmovdqu         " "ymm4, [rdi + 0x258]"),

        // Interleave low and high qwords from ymm0(state0[0,1,2,3]) and ymm3(state1[0,1,2,3])
        Q!("    vpunpcklqdq     " "ymm2, ymm0, ymm3"),
        Q!("    vpunpckhqdq     " "ymm0, ymm0, ymm3"),

        // Interleave low and high qwords from ymm1(state2[0,1,2,3]) and ymm4(state3[0,1,2,3])
        Q!("    vpunpcklqdq     " "ymm3, ymm1, ymm4"),

        // Permute 128-bit lanes to complete the interleave for A[0] and A[2]
        Q!("    vperm2i128      " "ymm7, ymm2, ymm3, 0x20"),
        Q!("    vpunpckhqdq     " "ymm1, ymm1, ymm4"),
        Q!("    vperm2i128      " "ymm3, ymm2, ymm3, 0x31"),
        Q!("    vmovdqu         " "ymm4, [rdi + 0x278]"),
        Q!("    vmovdqu         " "[rsp + 0x40], ymm3"),

        // Permute 128-bit lanes to complete the interleave for A[3] and A[1]
        Q!("    vperm2i128      " "ymm3, ymm0, ymm1, 0x31"),
        Q!("    vmovdqu         " "[rsp + 0x0], ymm7"),
        Q!("    vperm2i128      " "ymm7, ymm0, ymm1, 0x20"),

        Q!("    vmovdqu         " "ymm0, [rdi + 0x20]"),
        Q!("    vmovdqu         " "ymm1, [rdi + 0x1b0]"),
        Q!("    vmovdqu         " "[rsp + 0x60], ymm3"),
        Q!("    vmovdqu         " "ymm3, [rdi + 0xe8]"),
        Q!("    vmovdqu         " "[rsp + 0x20], ymm7"),

        // Load, Interleave, and Store 32 bytes from each of the 4 states (A[4-7])
        Q!("    vpunpcklqdq     " "ymm2, ymm0, ymm3"),
        Q!("    vpunpckhqdq     " "ymm0, ymm0, ymm3"),
        Q!("    vpunpcklqdq     " "ymm3, ymm1, ymm4"),
        Q!("    vperm2i128      " "ymm7, ymm2, ymm3, 0x20"),
        Q!("    vpunpckhqdq     " "ymm1, ymm1, ymm4"),
        Q!("    vperm2i128      " "ymm3, ymm2, ymm3, 0x31"),
        Q!("    vmovdqu         " "ymm4, [rdi + 0x298]"),
        Q!("    vperm2i128      " "ymm14, ymm0, ymm1, 0x31"),
        Q!("    vmovdqu         " "[rsp + 0x80], ymm7"),
        Q!("    vperm2i128      " "ymm7, ymm0, ymm1, 0x20"),
        Q!("    vmovdqu         " "ymm0, [rdi + 0x40]"),
        Q!("    vmovdqu         " "ymm1, [rdi + 0x1d0]"),
        Q!("    vmovdqu         " "[rsp + 0xc0], ymm3"),
        Q!("    vmovdqu         " "ymm3, [rdi + 0x108]"),
        Q!("    vmovdqu         " "ymm10, ymm14"),
        Q!("    vmovdqu         " "[rsp + 0xa0], ymm7"),

        // Load, Interleave, and Store 32 bytes from each of the 4 states (A[8-11])
        Q!("    vpunpcklqdq     " "ymm2, ymm0, ymm3"),
        Q!("    vpunpckhqdq     " "ymm0, ymm0, ymm3"),
        Q!("    vpunpcklqdq     " "ymm3, ymm1, ymm4"),
        Q!("    vpunpckhqdq     " "ymm1, ymm1, ymm4"),
        Q!("    vperm2i128      " "ymm11, ymm2, ymm3, 0x20"),
        Q!("    vperm2i128      " "ymm3, ymm2, ymm3, 0x31"),
        Q!("    vperm2i128      " "ymm7, ymm0, ymm1, 0x20"),
        Q!("    vmovdqu         " "[rsp + 0x100], ymm3"),
        Q!("    vperm2i128      " "ymm8, ymm0, ymm1, 0x31"),
        Q!("    vmovdqu         " "ymm3, [rdi + 0x128]"),
        Q!("    vmovdqu         " "ymm0, [rdi + 0x60]"),
        Q!("    vmovdqu         " "ymm1, [rdi + 0x1f0]"),
        Q!("    vmovdqu         " "[rsp + 0xe0], ymm7"),
        Q!("    vmovdqu         " "ymm14, ymm11"),
        Q!("    vmovdqu         " "ymm4, [rdi + 0x2b8]"),
        Q!("    vmovdqu         " "ymm5, [rdi + 0x2f8]"),

        // Load, Interleave, and Store 32 bytes from each of the 4 states (A[12-15])
        Q!("    vpunpcklqdq     " "ymm2, ymm0, ymm3"),
        Q!("    vpunpckhqdq     " "ymm0, ymm0, ymm3"),
        Q!("    vpunpcklqdq     " "ymm3, ymm1, ymm4"),
        Q!("    vpunpckhqdq     " "ymm1, ymm1, ymm4"),
        Q!("    vmovdqu         " "ymm4, [rdi + 0x2d8]"),
        Q!("    vperm2i128      " "ymm15, ymm2, ymm3, 0x20"),
        Q!("    vperm2i128      " "ymm3, ymm2, ymm3, 0x31"),
        Q!("    vperm2i128      " "ymm7, ymm0, ymm1, 0x20"),
        Q!("    vperm2i128      " "ymm9, ymm0, ymm1, 0x31"),
        Q!("    vmovdqu         " "[rsp + 0x140], ymm3"),
        Q!("    vmovdqu         " "ymm0, [rdi + 0x80]"),
        Q!("    vmovdqu         " "ymm3, [rdi + 0x148]"),
        Q!("    vmovdqu         " "ymm1, [rdi + 0x210]"),
        Q!("    vmovdqu         " "[rsp + 0x120], ymm7"),

        // Load, Interleave, and Store 32 bytes from each of the 4 states (A[16-19])
        Q!("    vpunpcklqdq     " "ymm2, ymm0, ymm3"),
        Q!("    vpunpckhqdq     " "ymm0, ymm0, ymm3"),
        Q!("    vpunpcklqdq     " "ymm3, ymm1, ymm4"),
        Q!("    vpunpckhqdq     " "ymm1, ymm1, ymm4"),
        Q!("    vperm2i128      " "ymm7, ymm2, ymm3, 0x20"),
        Q!("    vperm2i128      " "ymm13, ymm2, ymm3, 0x31"),
        Q!("    vperm2i128      " "ymm3, ymm0, ymm1, 0x31"),
        Q!("    vmovdqu         " "[rsp + 0x160], ymm7"),
        Q!("    vperm2i128      " "ymm7, ymm0, ymm1, 0x20"),
        Q!("    vmovdqu         " "ymm0, [rdi + 0xa0]"),
        Q!("    vmovdqu         " "ymm1, [rdi + 0x230]"),
        Q!("    vmovdqu         " "[rsp + 0x1a0], ymm3"),
        Q!("    vmovdqu         " "ymm3, [rdi + 0x168]"),

        // Load, Interleave, and Store 32 bytes from each of the 4 states (A[20-23])
        Q!("    vpunpcklqdq     " "ymm4, ymm1, ymm5"),
        Q!("    vpunpckhqdq     " "ymm1, ymm1, ymm5"),
        Q!("    vmovdqu         " "[rsp + 0x180], ymm7"),
        Q!("    vpunpcklqdq     " "ymm2, ymm0, ymm3"),
        Q!("    vpunpckhqdq     " "ymm0, ymm0, ymm3"),
        Q!("    vperm2i128      " "ymm12, ymm2, ymm4, 0x20"),
        Q!("    vperm2i128      " "ymm3, ymm0, ymm1, 0x20"),
        Q!("    vperm2i128      " "ymm7, ymm2, ymm4, 0x31"),
        Q!("    vperm2i128      " "ymm4, ymm0, ymm1, 0x31"),

        // Load, Interleave, and Store 8 bytes from each of the 4 states (A[24])
        // A[24] is the last element (only 8 bytes per state)
        Q!("    vmovq           " "xmm0, [rdi + 0x250]"),
        Q!("    vmovq           " "xmm1, [rdi + 0xc0]"),
        Q!("    vmovdqu         " "[rsp + 0x1c0], ymm12"),
        Q!("    vmovdqu         " "[rsp + 0x1e0], ymm4"),
        Q!("    vpinsrq         " "xmm0, xmm0, [rdi + 0x318], 0x1"),
        Q!("    vpinsrq         " "xmm1, xmm1, [rdi + 0x188], 0x1"),
        Q!("    vinserti128     " "ymm2, ymm1, xmm0, 0x1"),

        // Initialize the loop counter
        Q!("    mov             " "r10, 0"),

        Q!(Label!("Lsha3_keccak4_f1600_alt", 2) ":"),

        // =====================================================================
        // Theta Step
        // =====================================================================
        // Compute the column parities C[x] = A[x,0] xor A[x,1] xor A[x,2] xor A[x,3] xor A[x,4]
        // Then D[x] = C[x-1] xor ROL(C[x+1], 1)
        // Then A'[x,y] = A[x,y] xor D[x]

        // Theta step
        Q!("    vmovdqu         " "ymm4, [rsp + 0xa0]"),
        Q!("    vpxor           " "ymm0, ymm9, [rsp + 0x1c0]"),
        Q!("    vmovdqu         " "[rsp + 0x200], ymm9"),
        Q!("    vmovdqu         " "ymm9, ymm10"),
        Q!("    vmovdqu         " "ymm11, [rsp + 0xc0]"),
        Q!("    vmovdqu         " "ymm12, [rsp + 0x160]"),
        Q!("    vmovdqu         " "[rsp + 0x240], ymm3"),
        Q!("    vpxor           " "ymm1, ymm4, [rsp + 0x100]"),
        Q!("    vmovdqu         " "ymm10, [rsp + 0x40]"),
        Q!("    vmovdqu         " "[rsp + 0x220], ymm4"),
        Q!("    vpxor           " "ymm12, ymm12, ymm3"),
        Q!("    vmovdqu         " "ymm6, [rsp + 0x20]"),
        Q!("    vmovdqu         " "ymm4, [rsp + 0x140]"),
        Q!("    vmovdqu         " "[rsp + 0x2a0], ymm14"),
        Q!("    vpxor           " "ymm0, ymm0, ymm1"),
        Q!("    vpxor           " "ymm1, ymm11, ymm8"),
        Q!("    vpxor           " "ymm11, ymm7, [rsp + 0x180]"),
        Q!("    vmovdqu         " "[rsp + 0x280], ymm10"),
        Q!("    vpxor           " "ymm12, ymm12, ymm1"),
        Q!("    vpxor           " "ymm1, ymm9, ymm15"),
        Q!("    vmovdqu         " "ymm3, [rsp + 0xe0]"),
        Q!("    vmovdqu         " "[rsp + 0x260], ymm8"),
        Q!("    vpxor           " "ymm11, ymm11, ymm1"),
        Q!("    vpxor           " "ymm1, ymm14, [rsp + 0x120]"),
        Q!("    vpxor           " "ymm12, ymm12, ymm6"),
        Q!("    vmovdqu         " "ymm8, [rsp + 0x60]"),
        Q!("    vpxor           " "ymm11, ymm11, ymm10"),
        Q!("    vpxor           " "ymm10, ymm13, [rsp + 0x1e0]"),
        Q!("    vpxor           " "ymm3, ymm3, ymm4"),
        Q!("    vmovdqu         " "[rsp + 0x2c0], ymm4"),
        Q!("    vpsrlq          " "ymm4, ymm12, 0x3f"),
        Q!("    vpsrlq          " "ymm5, ymm11, 0x3f"),
        Q!("    vpxor           " "ymm0, ymm0, [rsp + 0x0]"),
        Q!("    vpxor           " "ymm10, ymm10, ymm1"),
        Q!("    vmovdqu         " "ymm1, [rsp + 0x80]"),
        Q!("    vpxor           " "ymm10, ymm10, ymm8"),
        Q!("    vmovdqu         " "ymm14, ymm1"),
        Q!("    vpxor           " "ymm1, ymm2, [rsp + 0x1a0]"),
        Q!("    vmovdqu         " "[rsp + 0x2e0], ymm14"),
        Q!("    vpxor           " "ymm1, ymm1, ymm3"),
        Q!("    vpsllq          " "ymm3, ymm12, 0x1"),
        Q!("    vpor            " "ymm3, ymm3, ymm4"),
        Q!("    vpsllq          " "ymm4, ymm11, 0x1"),
        Q!("    vpxor           " "ymm1, ymm1, ymm14"),

        // C[0] = ymm0
        // C[1] = ymm12
        // C[2] = ymm11
        // C[3] = ymm10
        // C[4] = ymm1

        Q!("    vpor            " "ymm4, ymm4, ymm5"),
        Q!("    vpsrlq          " "ymm14, ymm10, 0x3f"),
        Q!("    vpxor           " "ymm3, ymm3, ymm1"),
        Q!("    vpsllq          " "ymm5, ymm10, 0x1"),
        Q!("    vpxor           " "ymm4, ymm4, ymm0"),
        Q!("    vpor            " "ymm5, ymm5, ymm14"),
        Q!("    vpxor           " "ymm6, ymm4, ymm6"),
        Q!("    vpxor           " "ymm5, ymm5, ymm12"),
        Q!("    vpsrlq          " "ymm12, ymm1, 0x3f"),
        Q!("    vpsllq          " "ymm1, ymm1, 0x1"),
        Q!("    vpxor           " "ymm7, ymm5, ymm7"),
        Q!("    vpxor           " "ymm9, ymm5, ymm9"),
        Q!("    vpor            " "ymm1, ymm1, ymm12"),
        Q!("    vpxor           " "ymm12, ymm3, [rsp + 0x0]"),
        Q!("    vpxor           " "ymm1, ymm1, ymm11"),
        Q!("    vpsrlq          " "ymm11, ymm0, 0x3f"),
        Q!("    vpsllq          " "ymm0, ymm0, 0x1"),
        Q!("    vpxor           " "ymm13, ymm1, ymm13"),
        Q!("    vpxor           " "ymm8, ymm1, ymm8"),
        Q!("    vpor            " "ymm0, ymm0, ymm11"),
        Q!("    vpxor           " "ymm0, ymm0, ymm10"),

        // D[0] = ymm3
        // D[1] = ymm4
        // D[2] = ymm5
        // D[3] = ymm1
        // D[4] = ymm0

        Q!("    vpxor           " "ymm10, ymm4, [rsp + 0xc0]"),
        Q!("    vpxor           " "ymm2, ymm0, ymm2"),

        // Rho, Pi, and Chi Steps (interleaved for performance)
        // B[x,y] = ROL(A'[...], rotation_constant) placed at position determined by Pi
        // A''[x,y] = B[x,y] XOR ((NOT B[x+1,y]) AND B[x+2,y])

        Q!("    vpsrlq          " "ymm11, ymm10, 0x14"),
        Q!("    vpsllq          " "ymm10, ymm10, 0x2c"),
        Q!("    vpor            " "ymm10, ymm10, ymm11"),

        Q!("    vpxor           " "ymm11, ymm5, ymm15"),
        Q!("    vpbroadcastq    " "ymm15, [rsi]"),
        Q!("    vpsrlq          " "ymm14, ymm11, 0x15"),
        Q!("    vpsllq          " "ymm11, ymm11, 0x2b"),
        Q!("    vpor            " "ymm11, ymm11, ymm14"),

        Q!("    vpandn          " "ymm14, ymm10, ymm11"),
        Q!("    vpxor           " "ymm14, ymm14, ymm15"),
        Q!("    vpxor           " "ymm15, ymm14, ymm12"),

        Q!("    vpsrlq          " "ymm14, ymm13, 0x2b"),
        Q!("    vpsllq          " "ymm13, ymm13, 0x15"),
        Q!("    vmovdqu         " "[rsp + 0x0], ymm15"),
        Q!("    vpor            " "ymm13, ymm13, ymm14"),

        Q!("    vpandn          " "ymm14, ymm11, ymm13"),
        Q!("    vpxor           " "ymm15, ymm14, ymm10"),

        Q!("    vpsrlq          " "ymm14, ymm2, 0x32"),
        Q!("    vpsllq          " "ymm2, ymm2, 0xe"),
        Q!("    vmovdqu         " "[rsp + 0x20], ymm15"),
        Q!("    vpor            " "ymm2, ymm2, ymm14"),

        // **** B[0]-B[4] Register Allocation Map ****
        // B[0]  (B[0,0])    ymm12   (A'[0,0] (A'[0]) unchanged, no rotation)
        // B[1]  (B[1,0])    ymm10   ROL(A'[1,1] (A'[6]),  44)
        // B[2]  (B[2,0])    ymm11   ROL(A'[2,2] (A'[12]), 43)
        // B[3]  (B[3,0])    ymm13   ROL(A'[3,3] (A'[18]), 21)
        // B[4]  (B[4,0])    ymm2    ROL(A'[4,4] (A'[24]), 14)

        Q!("    vpandn          " "ymm14, ymm13, ymm2"),
        Q!("    vpxor           " "ymm11, ymm14, ymm11"),
        Q!("    vmovdqu         " "[rsp + 0x40], ymm11"),
        Q!("    vpandn          " "ymm11, ymm2, ymm12"),
        Q!("    vpandn          " "ymm12, ymm12, ymm10"),
        Q!("    vpxor           " "ymm11, ymm11, ymm13"),
        Q!("    vmovdqu         " "[rsp + 0x60], ymm11"),
        Q!("    vpxor           " "ymm11, ymm12, ymm2"),

        Q!("    vpsrlq          " "ymm2, ymm8, 0x24"),
        Q!("    vpsllq          " "ymm8, ymm8, 0x1c"),
        Q!("    vmovdqu         " "[rsp + 0x80], ymm11"),
        Q!("    vpor            " "ymm8, ymm8, ymm2"),

        Q!("    vpxor           " "ymm2, ymm0, [rsp + 0xe0]"),
        Q!("    vpsrlq          " "ymm10, ymm2, 0x2c"),
        Q!("    vpsllq          " "ymm2, ymm2, 0x14"),
        Q!("    vpor            " "ymm2, ymm2, ymm10"),

        Q!("    vpxor           " "ymm10, ymm3, [rsp + 0x100]"),
        Q!("    vpsrlq          " "ymm11, ymm10, 0x3d"),
        Q!("    vpsllq          " "ymm10, ymm10, 0x3"),
        Q!("    vpor            " "ymm10, ymm10, ymm11"),

        Q!("    vpandn          " "ymm11, ymm2, ymm10"),
        Q!("    vpxor           " "ymm11, ymm11, ymm8"),
        Q!("    vmovdqu         " "[rsp + 0xa0], ymm11"),

        Q!("    vpxor           " "ymm11, ymm4, [rsp + 0x160]"),
        Q!("    vpsrlq          " "ymm12, ymm11, 0x13"),
        Q!("    vpsllq          " "ymm11, ymm11, 0x2d"),
        Q!("    vpor            " "ymm11, ymm11, ymm12"),

        Q!("    vpandn          " "ymm12, ymm10, ymm11"),
        Q!("    vpxor           " "ymm12, ymm12, ymm2"),
        Q!("    vmovdqu         " "[rsp + 0xc0], ymm12"),

        Q!("    vpsrlq          " "ymm12, ymm7, 0x3"),
        Q!("    vpsllq          " "ymm7, ymm7, 0x3d"),
        Q!("    vpor            " "ymm7, ymm7, ymm12"),

        // **** B[5]-B[9] Register Allocation Map ****
        // B[5]  (B[0,1])    ymm8    ROL(A'[3,0] (A'[3]), 28)
        // B[6]  (B[1,1])    ymm2    ROL(A'[4,1] (A'[9]), 20)
        // B[7]  (B[2,1])    ymm10   ROL(A'[0,2] (A'[10]), 3)
        // B[8]  (B[3,1])    ymm11   ROL(A'[1,3] (A'[16]), 45)
        // B[9]  (B[4,1])    ymm7    ROL(A'[2,4] (A'[22]), 61)

        Q!("    vpandn          " "ymm12, ymm11, ymm7"),
        Q!("    vpxor           " "ymm10, ymm12, ymm10"),

        Q!("    vpandn          " "ymm12, ymm7, ymm8"),
        Q!("    vpandn          " "ymm8, ymm8, ymm2"),

        Q!("    vpsrlq          " "ymm2, ymm6, 0x3f"),
        Q!("    vpsllq          " "ymm6, ymm6, 0x1"),
        Q!("    vpxor           " "ymm14, ymm12, ymm11"),
        Q!("    vpor            " "ymm6, ymm6, ymm2"),

        Q!("    vpsrlq          " "ymm2, ymm9, 0x3a"),
        Q!("    vpxor           " "ymm12, ymm8, ymm7"),
        Q!("    vpsllq          " "ymm9, ymm9, 0x6"),
        Q!("    vmovdqu         " "[rsp + 0xe0], ymm12"),

        Q!("    vpxor           " "ymm7, ymm0, [rsp + 0x1a0]"),
        Q!("    vpor            " "ymm9, ymm9, ymm2"),

        Q!("    vpxor           " "ymm2, ymm1, [rsp + 0x120]"),
        Q!("    vpshufb         " "ymm7, ymm7, [rdx]"),
        Q!("    vpsrlq          " "ymm11, ymm2, 0x27"),
        Q!("    vpsllq          " "ymm2, ymm2, 0x19"),
        Q!("    vpor            " "ymm11, ymm11, ymm2"),

        Q!("    vpandn          " "ymm2, ymm9, ymm11"),
        Q!("    vpandn          " "ymm8, ymm11, ymm7"),
        Q!("    vpxor           " "ymm12, ymm2, ymm6"),

        Q!("    vpxor           " "ymm2, ymm3, [rsp + 0x1c0]"),
        Q!("    vpxor           " "ymm8, ymm8, ymm9"),
        Q!("    vmovdqu         " "[rsp + 0x100], ymm12"),
        Q!("    vpsrlq          " "ymm12, ymm2, 0x2e"),
        Q!("    vpsllq          " "ymm2, ymm2, 0x12"),
        Q!("    vpor            " "ymm2, ymm12, ymm2"),

        // **** B[10]-B[14] Register Allocation Map ****
        // B[10] (B[0,2])    ymm6    ROL(A'[1,0] (A'[1]), 1)
        // B[11] (B[1,2])    ymm9    ROL(A'[2,1] (A'[7]), 6)
        // B[12] (B[2,2])    ymm11   ROL(A'[3,2] (A'[13]), 25)
        // B[13] (B[3,2])    ymm7    ROL(A'[4,3] (A'[19]), 8)
        // B[14] (B[4,2])    ymm2    ROL(A'[0,4] (A'[20]), 18)

        Q!("    vpandn          " "ymm12, ymm7, ymm2"),
        Q!("    vpxor           " "ymm15, ymm12, ymm11"),

        Q!("    vpandn          " "ymm11, ymm2, ymm6"),
        Q!("    vpandn          " "ymm6, ymm6, ymm9"),
        Q!("    vpxor           " "ymm12, ymm11, ymm7"),
        Q!("    vmovdqu         " "[rsp + 0x120], ymm12"),

        Q!("    vpxor           " "ymm12, ymm6, ymm2"),

        Q!("    vpxor           " "ymm6, ymm0, [rsp + 0x2e0]"),
        Q!("    vpxor           " "ymm0, ymm0, [rsp + 0x2c0]"),
        Q!("    vmovdqu         " "[rsp + 0x140], ymm12"),
        Q!("    vpsrlq          " "ymm2, ymm6, 0x25"),
        Q!("    vpsllq          " "ymm6, ymm6, 0x1b"),
        Q!("    vpor            " "ymm2, ymm2, ymm6"),

        Q!("    vpxor           " "ymm6, ymm3, [rsp + 0x220]"),
        Q!("    vpxor           " "ymm3, ymm3, [rsp + 0x200]"),
        Q!("    vpsrlq          " "ymm7, ymm6, 0x1c"),
        Q!("    vpsllq          " "ymm6, ymm6, 0x24"),
        Q!("    vpor            " "ymm7, ymm7, ymm6"),

        Q!("    vpxor           " "ymm6, ymm4, [rsp + 0x260]"),
        Q!("    vpxor           " "ymm4, ymm4, [rsp + 0x240]"),
        Q!("    vpsrlq          " "ymm12, ymm6, 0x36"),
        Q!("    vpsllq          " "ymm6, ymm6, 0xa"),
        Q!("    vpor            " "ymm12, ymm12, ymm6"),

        Q!("    vpxor           " "ymm6, ymm5, [rsp + 0x180]"),
        Q!("    vpxor           " "ymm5, ymm5, [rsp + 0x280]"),

        Q!("    vpandn          " "ymm9, ymm7, ymm12"),
        Q!("    vpsrlq          " "ymm11, ymm6, 0x31"),
        Q!("    vpsllq          " "ymm6, ymm6, 0xf"),
        Q!("    vpxor           " "ymm9, ymm9, ymm2"),
        Q!("    vpor            " "ymm11, ymm11, ymm6"),

        Q!("    vpandn          " "ymm6, ymm12, ymm11"),
        Q!("    vpxor           " "ymm6, ymm6, ymm7"),
        Q!("    vmovdqu         " "[rsp + 0x160], ymm6"),

        Q!("    vpxor           " "ymm6, ymm1, [rsp + 0x1e0]"),
        Q!("    vpxor           " "ymm1, ymm1, [rsp + 0x2a0]"),
        Q!("    vpshufb         " "ymm6, ymm6, [rcx]"),

        // **** B[15]-B[19] Register Allocation Map ****
        // B[15] (B[0,3])    ymm2    ROL(A'[4,0] (A'[4]), 27)
        // B[16] (B[1,3])    ymm7    ROL(A'[0,1] (A'[5]), 36)
        // B[17] (B[2,3])    ymm12   ROL(A'[1,2] (A'[11]), 10)
        // B[18] (B[3,3])    ymm11   ROL(A'[2,3] (A'[17]), 15)
        // B[19] (B[4,3])    ymm6    ROL(A'[3,4] (A'[23]), 56)

        Q!("    vpandn          " "ymm13, ymm11, ymm6"),
        Q!("    vpxor           " "ymm13, ymm13, ymm12"),
        Q!("    vmovdqu         " "[rsp + 0x180], ymm13"),

        Q!("    vpandn          " "ymm13, ymm6, ymm2"),
        Q!("    vpandn          " "ymm2, ymm2, ymm7"),
        Q!("    vpxor           " "ymm2, ymm2, ymm6"),

        Q!("    vpsrlq          " "ymm6, ymm4, 0x3e"),
        Q!("    vpxor           " "ymm13, ymm13, ymm11"),
        Q!("    vmovdqu         " "[rsp + 0x1a0], ymm2"),
        Q!("    vpsrlq          " "ymm2, ymm5, 0x2"),
        Q!("    vpsllq          " "ymm5, ymm5, 0x3e"),
        Q!("    vpor            " "ymm2, ymm2, ymm5"),

        Q!("    vpsrlq          " "ymm5, ymm1, 0x9"),
        Q!("    vpsllq          " "ymm1, ymm1, 0x37"),

        Q!("    vpsllq          " "ymm4, ymm4, 0x2"),
        Q!("    vpor            " "ymm1, ymm5, ymm1"),

        Q!("    vpsrlq          " "ymm5, ymm0, 0x19"),
        Q!("    vpor            " "ymm4, ymm6, ymm4"),
        Q!("    vpsllq          " "ymm0, ymm0, 0x27"),
        Q!("    vpor            " "ymm5, ymm5, ymm0"),

        Q!("    vpandn          " "ymm0, ymm1, ymm5"),
        Q!("    vpxor           " "ymm0, ymm0, ymm2"),
        Q!("    vmovdqu         " "[rsp + 0x1c0], ymm0"),

        Q!("    vpsrlq          " "ymm0, ymm3, 0x17"),
        Q!("    vpsllq          " "ymm3, ymm3, 0x29"),
        Q!("    vpor            " "ymm0, ymm0, ymm3"),

        // **** B[20]-B[24] Register Allocation Map ****
        // B[20] (B[0,4])    ymm2    ROL(A'[2,0] (A'[2]), 62)
        // B[21] (B[1,4])    ymm1    ROL(A'[3,1] (A'[8]), 55)
        // B[22] (B[2,4])    ymm5    ROL(A'[4,2] (A'[14]), 39)
        // B[23] (B[3,4])    ymm4    ROL(A'[1,4] (A'[21]), 2)
        // B[24] (B[4,4])    ymm0    ROL(A'[0,3] (A'[15]), 41)

        Q!("    vpandn          " "ymm7, ymm0, ymm4"),
        Q!("    vpandn          " "ymm3, ymm5, ymm0"),
        Q!("    vpxor           " "ymm7, ymm7, ymm5"),

        Q!("    vpandn          " "ymm5, ymm4, ymm2"),
        Q!("    vpandn          " "ymm2, ymm2, ymm1"),
        Q!("    vpxor           " "ymm5, ymm5, ymm0"),

        Q!("    vpxor           " "ymm3, ymm3, ymm1"),

        Q!("    vpxor           " "ymm2, ymm2, ymm4"),
        Q!("    vmovdqu         " "[rsp + 0x1e0], ymm5"),

        Q!("    add             " "rsi, 8"),
        Q!("    add             " "r10, 1"),
        Q!("    cmp             " "r10, 0x18"),
        Q!("    jne             " Label!("Lsha3_keccak4_f1600_alt", 2, Before)),

        // Load, De-interleave, and Store 32 bytes to each of the 4 states (A[0-3])
        Q!("    vmovdqu         " "ymm4, [rsp + 0x0]"),
        Q!("    vmovdqu         " "ymm5, [rsp + 0x40]"),
        Q!("    vmovdqu         " "ymm0, [rsp + 0x20]"),
        Q!("    vmovdqu         " "ymm1, [rsp + 0x60]"),
        Q!("    vmovdqu         " "ymm12, [rsp + 0x1c0]"),
        Q!("    vmovdqu         " "[rsp + 0x1c0], ymm2"),

        // De-interleave ymm4(A[0]) and ymm0(A[1])
        Q!("    vpunpcklqdq     " "ymm2, ymm4, ymm0"),
        Q!("    vpunpckhqdq     " "ymm0, ymm4, ymm0"),
        // De-interleave ymm5(A[2]) and ymm1(A[3])
        Q!("    vpunpcklqdq     " "ymm4, ymm5, ymm1"),
        Q!("    vpunpckhqdq     " "ymm1, ymm5, ymm1"),

        // Permute 128-bit lanes to complete the de-interleave
        Q!("    vperm2i128      " "ymm6, ymm2, ymm4, 0x20"),
        Q!("    vperm2i128      " "ymm2, ymm2, ymm4, 0x31"),
        Q!("    vmovdqu         " "ymm4, [rsp + 0x80]"),
        Q!("    vperm2i128      " "ymm5, ymm0, ymm1, 0x20"),
        Q!("    vperm2i128      " "ymm0, ymm0, ymm1, 0x31"),

        // Store de-interleaved results back to output
        Q!("    vmovdqu         " "[rdi], ymm6"),
        Q!("    vmovdqu         " "[rdi + 0xc8], ymm5"),
        Q!("    vmovdqu         " "[rdi + 0x190], ymm2"),
        Q!("    vmovdqu         " "[rdi + 0x258], ymm0"),

        // Load, De-interleave, and Store 32 bytes to each of the 4 states (A[4-7])
        Q!("    vmovdqu         " "ymm0, [rsp + 0xa0]"),
        Q!("    vpunpcklqdq     " "ymm2, ymm4, ymm0"),
        Q!("    vpunpckhqdq     " "ymm1, ymm4, ymm0"),
        Q!("    vmovdqu         " "ymm0, [rsp + 0xc0]"),
        Q!("    vpunpcklqdq     " "ymm4, ymm0, ymm10"),
        Q!("    vpunpckhqdq     " "ymm0, ymm0, ymm10"),
        Q!("    vperm2i128      " "ymm6, ymm2, ymm4, 0x20"),
        Q!("    vperm2i128      " "ymm5, ymm1, ymm0, 0x20"),
        Q!("    vperm2i128      " "ymm2, ymm2, ymm4, 0x31"),
        Q!("    vmovdqu         " "ymm4, [rsp + 0xe0]"),
        Q!("    vperm2i128      " "ymm1, ymm1, ymm0, 0x31"),
        Q!("    vmovdqu         " "ymm0, [rsp + 0x100]"),
        Q!("    vmovdqu         " "[rdi + 0x1b0], ymm2"),
        Q!("    vmovdqu         " "[rdi + 0x278], ymm1"),

        // Load, De-interleave, and Store 32 bytes to each of the 4 states (A[8-11])
        Q!("    vpunpcklqdq     " "ymm2, ymm14, ymm4"),
        Q!("    vpunpckhqdq     " "ymm1, ymm14, ymm4"),
        Q!("    vpunpcklqdq     " "ymm4, ymm0, ymm8"),
        Q!("    vpunpckhqdq     " "ymm0, ymm0, ymm8"),
        Q!("    vmovdqu         " "[rdi + 0x20], ymm6"),
        Q!("    vmovdqu         " "[rdi + 0xe8], ymm5"),
        Q!("    vperm2i128      " "ymm6, ymm2, ymm4, 0x20"),
        Q!("    vperm2i128      " "ymm5, ymm1, ymm0, 0x20"),
        Q!("    vperm2i128      " "ymm2, ymm2, ymm4, 0x31"),
        Q!("    vperm2i128      " "ymm1, ymm1, ymm0, 0x31"),
        Q!("    vmovdqu         " "ymm4, [rsp + 0x120]"),
        Q!("    vmovdqu         " "ymm0, [rsp + 0x140]"),
        Q!("    vmovdqu         " "[rdi + 0x1d0], ymm2"),
        Q!("    vmovdqu         " "[rdi + 0x298], ymm1"),

        // Load, De-interleave, and Store 32 bytes to each of the 4 states (A[12-15])
        Q!("    vpunpcklqdq     " "ymm2, ymm15, ymm4"),
        Q!("    vpunpckhqdq     " "ymm1, ymm15, ymm4"),
        Q!("    vpunpcklqdq     " "ymm4, ymm0, ymm9"),
        Q!("    vmovdqu         " "[rdi + 0x108], ymm5"),
        Q!("    vpunpckhqdq     " "ymm0, ymm0, ymm9"),
        Q!("    vmovdqu         " "[rdi + 0x40], ymm6"),
        Q!("    vperm2i128      " "ymm6, ymm2, ymm4, 0x20"),
        Q!("    vperm2i128      " "ymm2, ymm2, ymm4, 0x31"),
        Q!("    vperm2i128      " "ymm5, ymm1, ymm0, 0x20"),
        Q!("    vmovdqu         " "ymm4, [rsp + 0x160]"),
        Q!("    vperm2i128      " "ymm1, ymm1, ymm0, 0x31"),
        Q!("    vmovdqu         " "ymm0, [rsp + 0x180]"),
        Q!("    vmovdqu         " "[rdi + 0x128], ymm5"),
        Q!("    vmovdqu         " "ymm5, [rsp + 0x1a0]"),
        Q!("    vmovdqu         " "[rdi + 0x1f0], ymm2"),

        // Load, De-interleave, and Store 32 bytes to each of the 4 states (A[16-19])
        Q!("    vpunpcklqdq     " "ymm2, ymm4, ymm0"),
        Q!("    vpunpckhqdq     " "ymm0, ymm4, ymm0"),
        Q!("    vpunpcklqdq     " "ymm4, ymm13, ymm5"),
        Q!("    vmovdqu         " "[rdi + 0x60], ymm6"),
        Q!("    vperm2i128      " "ymm6, ymm2, ymm4, 0x20"),
        Q!("    vmovdqu         " "[rdi + 0x2b8], ymm1"),
        Q!("    vperm2i128      " "ymm2, ymm2, ymm4, 0x31"),
        Q!("    vpunpckhqdq     " "ymm1, ymm13, ymm5"),
        Q!("    vmovdqu         " "[rdi + 0x80], ymm6"),
        Q!("    vmovdqu         " "ymm4, [rsp + 0x1e0]"),
        Q!("    vperm2i128      " "ymm5, ymm0, ymm1, 0x20"),
        Q!("    vperm2i128      " "ymm0, ymm0, ymm1, 0x31"),
        Q!("    vmovdqu         " "[rdi + 0x210], ymm2"),

        // Load, De-interleave, and Store 32 bytes to each of the 4 states (A[20-23])
        Q!("    vpunpcklqdq     " "ymm2, ymm12, ymm3"),
        Q!("    vmovdqu         " "[rdi + 0x2d8], ymm0"),
        Q!("    vpunpckhqdq     " "ymm0, ymm12, ymm3"),
        Q!("    vpunpcklqdq     " "ymm3, ymm7, ymm4"),
        Q!("    vpunpckhqdq     " "ymm1, ymm7, ymm4"),
        Q!("    vmovdqu         " "[rdi + 0x148], ymm5"),
        Q!("    vperm2i128      " "ymm5, ymm2, ymm3, 0x20"),
        Q!("    vperm2i128      " "ymm2, ymm2, ymm3, 0x31"),
        Q!("    vmovdqu         " "ymm3, [rsp + 0x1c0]"),
        Q!("    vperm2i128      " "ymm4, ymm0, ymm1, 0x20"),
        Q!("    vperm2i128      " "ymm0, ymm0, ymm1, 0x31"),

        // Store de-interleaved results back to output
        Q!("    vmovdqu         " "[rdi + 0xa0], ymm5"),
        Q!("    vextracti128    " "xmm15, ymm3, 0x1"),
        Q!("    vmovdqu         " "[rdi + 0x168], ymm4"),
        Q!("    vmovdqu         " "[rdi + 0x230], ymm2"),
        Q!("    vmovdqu         " "[rdi + 0x2f8], ymm0"),

        // Load, De-interleave, and Store 8 bytes to each of the 4 states (A[24])
        // A[24] is the last element (only 8 bytes per state)
        Q!("    vmovq           " "[rdi + 0xc0], xmm3"),
        Q!("    vmovhpd         " "[rdi + 0x188], xmm3"),
        Q!("    vmovq           " "[rdi + 0x250], xmm15"),
        Q!("    vmovhpd         " "[rdi + 0x318], xmm15"),
        Q!("    mov             " "rsp, r11"),

        inout("rdi") a.as_mut_ptr() => _,
        inout("rsi") rc.as_ptr() => _,
        inout("rdx") rho8.as_ptr() => _,
        inout("rcx") rho56.as_ptr() => _,
        // clobbers
        out("r10") _,
        out("r11") _,
        out("zmm0") _,
        out("zmm1") _,
        out("zmm10") _,
        out("zmm11") _,
        out("zmm12") _,
        out("zmm13") _,
        out("zmm14") _,
        out("zmm15") _,
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
