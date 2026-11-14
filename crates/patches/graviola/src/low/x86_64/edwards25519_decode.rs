// generated source. do not edit.
#![allow(non_upper_case_globals, unused_macros, unused_imports)]
use crate::low::macros::*;

// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

// ----------------------------------------------------------------------------
// Decode compressed 256-bit form of edwards25519 point
// Input c[32] (bytes); output function return and z[8]
//
// extern uint64_t edwards25519_decode(uint64_t z[static 8], const uint8_t c[static 32]);
//
// This interprets the input byte string as a little-endian number
// representing a point (x,y) on the edwards25519 curve, encoded as
// 2^255 * x_0 + y where x_0 is the least significant bit of x. It
// returns the full pair of coordinates x (at z) and y (at z+4). The
// return code is 0 for success and 1 for failure, which means that
// the input does not correspond to the encoding of any edwards25519
// point. This can happen for three reasons, where y = the lowest
// 255 bits of the input:
//
//  * y >= p_25519
//    Input y coordinate is not reduced
//  * (y^2 - 1) * (1 + d_25519 * y^2) has no modular square root
//    There is no x such that (x,y) is on the curve
//  * y^2 = 1 and top bit of input is set
//    Cannot be the canonical encoding of (0,1) or (0,-1)
//
// Standard x86-64 ABI: RDI = z, RSI = c
// Microsoft x64 ABI:   RCX = z, RDX = c
// ----------------------------------------------------------------------------

// Size in bytes of a 64-bit word

macro_rules! N {
    () => {
        "8"
    };
}

// Pointer-offset pairs for temporaries on stack

macro_rules! y {
    () => {
        "rsp + 0"
    };
}
macro_rules! s { () => { Q!("rsp + (4 * " N!() ")") } }
macro_rules! t { () => { Q!("rsp + (8 * " N!() ")") } }
macro_rules! u { () => { Q!("rsp + (12 * " N!() ")") } }
macro_rules! v { () => { Q!("rsp + (16 * " N!() ")") } }
macro_rules! w { () => { Q!("rsp + (20 * " N!() ")") } }
macro_rules! q { () => { Q!("rsp + (24 * " N!() ")") } }
macro_rules! res { () => { Q!("QWORD PTR [rsp + (28 * " N!() ")]") } }
macro_rules! sgnbit { () => { Q!("QWORD PTR [rsp + (29 * " N!() ")]") } }
macro_rules! badun { () => { Q!("QWORD PTR [rsp + (30 * " N!() ")]") } }

// Total size to reserve on the stack

macro_rules! NSPACE { () => { Q!("32 * " N!()) } }

// Corrupted versions when stack is down 8 more

macro_rules! q8 { () => { Q!("rsp + (25 * " N!() ")") } }

// Syntactic variants to make x86_att version simpler to generate

macro_rules! Y {
    () => {
        "0"
    };
}
macro_rules! S { () => { Q!("(4 * " N!() ")") } }
macro_rules! T { () => { Q!("(8 * " N!() ")") } }
macro_rules! U { () => { Q!("(12 * " N!() ")") } }
macro_rules! V { () => { Q!("(16 * " N!() ")") } }
macro_rules! W { () => { Q!("(20 * " N!() ")") } }
macro_rules! Q8 { () => { Q!("(25 * " N!() ")") } }

/// Decode compressed 256-bit form of edwards25519 point
///
/// Input c[32] (bytes); output function return and z[8]
///
/// This interprets the input byte string as a little-endian number
/// representing a point (x,y) on the edwards25519 curve, encoded as
/// 2^255 * x_0 + y where x_0 is the least significant bit of x. It
/// returns the full pair of coordinates x (at z) and y (at z+4). The
/// return code is 0 for success and 1 for failure, which means that
/// the input does not correspond to the encoding of any edwards25519
/// point. This can happen for three reasons, where y = the lowest
/// 255 bits of the input:
///
///  * y >= p_25519
///    Input y coordinate is not reduced
///  * (y^2 - 1) * (1 + d_25519 * y^2) has no modular square root
///    There is no x such that (x,y) is on the curve
///  * y^2 = 1 and top bit of input is set
///    Cannot be the canonical encoding of (0,1) or (0,-1)
pub(crate) fn edwards25519_decode(z: &mut [u64; 8], c: &[u8; 32]) -> bool {
    let ret: u64;
    // SAFETY: inline assembly. see [crate::low::inline_assembly_safety] for safety info.
    unsafe {
        core::arch::asm!(

        Q!("    endbr64         " ),

        // In this case the Windows form literally makes a subroutine call.
        // This avoids hassle arising from subroutine offsets

        // Save registers and make room for temporaries

        Q!("    push            " "rbx"),
        Q!("    push            " "rbp"),
        Q!("    push            " "r12"),
        Q!("    push            " "r13"),
        Q!("    push            " "r14"),
        Q!("    push            " "r15"),

        Q!("    sub             " "rsp, " NSPACE!()),

        // Save the return pointer for the end so we can overwrite rdi later

        Q!("    mov             " res!() ", rdi"),

        // Load the inputs, which can be done word-wise since x86 is little-endian.
        // Let y be the lowest 255 bits of the input and sgnbit the desired parity.
        // If y >= p_25519 then already flag the input as invalid (badun = 1).

        Q!("    mov             " "rax, [rsi]"),
        Q!("    mov             " "[rsp + " Y!() "], rax"),
        Q!("    mov             " "rbx, [rsi + 8]"),
        Q!("    mov             " "[rsp + " Y!() "+ 8], rbx"),
        Q!("    xor             " "ebp, ebp"),
        Q!("    mov             " "rcx, [rsi + 16]"),
        Q!("    mov             " "[rsp + " Y!() "+ 16], rcx"),
        Q!("    mov             " "rdx, [rsi + 24]"),
        Q!("    btr             " "rdx, 63"),
        Q!("    mov             " "[rsp + " Y!() "+ 24], rdx"),
        Q!("    adc             " "rbp, rbp"),
        Q!("    mov             " sgnbit!() ", rbp"),

        Q!("    add             " "rax, 19"),
        Q!("    adc             " "rbx, 0"),
        Q!("    adc             " "rcx, 0"),
        Q!("    adc             " "rdx, 0"),
        Q!("    shr             " "rdx, 63"),
        Q!("    mov             " badun!() ", rdx"),

        // u = y^2 - 1 (actually y + 2^255-20, not reduced modulo)
        // v = 1 + d * y^2 (not reduced modulo from the +1)
        // w = u * v

        Q!("    lea             " "rdi, [rsp + " V!() "]"),
        Q!("    mov             " "rsi, 1"),
        Q!("    lea             " "rdx, [rsp + " Y!() "]"),
        Q!("    call            " Label!("Ledwards25519_decode_nsqr_p25519", 2, After)),
        Q!("    mov             " "rax, [rsp + " V!() "]"),
        Q!("    sub             " "rax, 20"),
        Q!("    mov             " "rbx, [rsp + " V!() "+ 8]"),
        Q!("    sbb             " "rbx, 0"),
        Q!("    mov             " "rcx, [rsp + " V!() "+ 16]"),
        Q!("    sbb             " "rcx, 0"),
        Q!("    mov             " "rdx, [rsp + " V!() "+ 24]"),
        Q!("    sbb             " "rdx, 0"),
        Q!("    btc             " "rdx, 63"),
        Q!("    mov             " "[rsp + " U!() "], rax"),
        Q!("    mov             " "[rsp + " U!() "+ 8], rbx"),
        Q!("    mov             " "[rsp + " U!() "+ 16], rcx"),
        Q!("    mov             " "[rsp + " U!() "+ 24], rdx"),

        Q!("    mov             " "rax, 0x75eb4dca135978a3"),
        Q!("    mov             " "[rsp + " W!() "], rax"),
        Q!("    mov             " "rax, 0x00700a4d4141d8ab"),
        Q!("    mov             " "[rsp + " W!() "+ 8], rax"),
        Q!("    mov             " "rax, 0x8cc740797779e898"),
        Q!("    mov             " "[rsp + " W!() "+ 16], rax"),
        Q!("    mov             " "rax, 0x52036cee2b6ffe73"),
        Q!("    mov             " "[rsp + " W!() "+ 24], rax"),
        Q!("    lea             " "rdi, [rsp + " V!() "]"),
        Q!("    lea             " "rsi, [rsp + " W!() "]"),
        Q!("    lea             " "rdx, [rsp + " V!() "]"),
        Q!("    call            " Label!("Ledwards25519_decode_mul_p25519", 3, After)),
        Q!("    mov             " "rax, [rsp + " V!() "]"),
        Q!("    add             " "rax, 1"),
        Q!("    mov             " "rbx, [rsp + " V!() "+ 8]"),
        Q!("    adc             " "rbx, 0"),
        Q!("    mov             " "rcx, [rsp + " V!() "+ 16]"),
        Q!("    adc             " "rcx, 0"),
        Q!("    mov             " "rdx, [rsp + " V!() "+ 24]"),
        Q!("    adc             " "rdx, 0"),
        Q!("    mov             " "[rsp + " V!() "], rax"),
        Q!("    mov             " "[rsp + " V!() "+ 8], rbx"),
        Q!("    mov             " "[rsp + " V!() "+ 16], rcx"),
        Q!("    mov             " "[rsp + " V!() "+ 24], rdx"),

        Q!("    lea             " "rdi, [rsp + " W!() "]"),
        Q!("    lea             " "rsi, [rsp + " U!() "]"),
        Q!("    lea             " "rdx, [rsp + " V!() "]"),
        Q!("    call            " Label!("Ledwards25519_decode_mul_p25519", 3, After)),

        // Get s = w^{252-3} as a candidate inverse square root 1/sqrt(w).
        // This power tower computation is the same as bignum_invsqrt_p25519

        Q!("    lea             " "rdi, [rsp + " T!() "]"),
        Q!("    mov             " "rsi, 1"),
        Q!("    lea             " "rdx, [rsp + " W!() "]"),
        Q!("    call            " Label!("Ledwards25519_decode_nsqr_p25519", 2, After)),

        Q!("    lea             " "rdi, [rsp + " T!() "]"),
        Q!("    lea             " "rsi, [rsp + " T!() "]"),
        Q!("    lea             " "rdx, [rsp + " W!() "]"),
        Q!("    call            " Label!("Ledwards25519_decode_mul_p25519", 3, After)),

        Q!("    lea             " "rdi, [rsp + " S!() "]"),
        Q!("    mov             " "rsi, 2"),
        Q!("    lea             " "rdx, [rsp + " T!() "]"),
        Q!("    call            " Label!("Ledwards25519_decode_nsqr_p25519", 2, After)),

        Q!("    lea             " "rdi, [rsp + " T!() "]"),
        Q!("    lea             " "rsi, [rsp + " S!() "]"),
        Q!("    lea             " "rdx, [rsp + " T!() "]"),
        Q!("    call            " Label!("Ledwards25519_decode_mul_p25519", 3, After)),

        Q!("    lea             " "rdi, [rsp + " S!() "]"),
        Q!("    mov             " "rsi, 1"),
        Q!("    lea             " "rdx, [rsp + " T!() "]"),
        Q!("    call            " Label!("Ledwards25519_decode_nsqr_p25519", 2, After)),

        Q!("    lea             " "rdi, [rsp + " V!() "]"),
        Q!("    lea             " "rsi, [rsp + " S!() "]"),
        Q!("    lea             " "rdx, [rsp + " W!() "]"),
        Q!("    call            " Label!("Ledwards25519_decode_mul_p25519", 3, After)),

        Q!("    lea             " "rdi, [rsp + " S!() "]"),
        Q!("    mov             " "rsi, 5"),
        Q!("    lea             " "rdx, [rsp + " V!() "]"),
        Q!("    call            " Label!("Ledwards25519_decode_nsqr_p25519", 2, After)),

        Q!("    lea             " "rdi, [rsp + " T!() "]"),
        Q!("    lea             " "rsi, [rsp + " S!() "]"),
        Q!("    lea             " "rdx, [rsp + " V!() "]"),
        Q!("    call            " Label!("Ledwards25519_decode_mul_p25519", 3, After)),

        Q!("    lea             " "rdi, [rsp + " S!() "]"),
        Q!("    mov             " "rsi, 10"),
        Q!("    lea             " "rdx, [rsp + " T!() "]"),
        Q!("    call            " Label!("Ledwards25519_decode_nsqr_p25519", 2, After)),

        Q!("    lea             " "rdi, [rsp + " T!() "]"),
        Q!("    lea             " "rsi, [rsp + " S!() "]"),
        Q!("    lea             " "rdx, [rsp + " T!() "]"),
        Q!("    call            " Label!("Ledwards25519_decode_mul_p25519", 3, After)),

        Q!("    lea             " "rdi, [rsp + " S!() "]"),
        Q!("    mov             " "rsi, 5"),
        Q!("    lea             " "rdx, [rsp + " T!() "]"),
        Q!("    call            " Label!("Ledwards25519_decode_nsqr_p25519", 2, After)),

        Q!("    lea             " "rdi, [rsp + " V!() "]"),
        Q!("    lea             " "rsi, [rsp + " S!() "]"),
        Q!("    lea             " "rdx, [rsp + " V!() "]"),
        Q!("    call            " Label!("Ledwards25519_decode_mul_p25519", 3, After)),

        Q!("    lea             " "rdi, [rsp + " S!() "]"),
        Q!("    mov             " "rsi, 25"),
        Q!("    lea             " "rdx, [rsp + " V!() "]"),
        Q!("    call            " Label!("Ledwards25519_decode_nsqr_p25519", 2, After)),

        Q!("    lea             " "rdi, [rsp + " T!() "]"),
        Q!("    lea             " "rsi, [rsp + " S!() "]"),
        Q!("    lea             " "rdx, [rsp + " V!() "]"),
        Q!("    call            " Label!("Ledwards25519_decode_mul_p25519", 3, After)),

        Q!("    lea             " "rdi, [rsp + " S!() "]"),
        Q!("    mov             " "rsi, 50"),
        Q!("    lea             " "rdx, [rsp + " T!() "]"),
        Q!("    call            " Label!("Ledwards25519_decode_nsqr_p25519", 2, After)),

        Q!("    lea             " "rdi, [rsp + " T!() "]"),
        Q!("    lea             " "rsi, [rsp + " S!() "]"),
        Q!("    lea             " "rdx, [rsp + " T!() "]"),
        Q!("    call            " Label!("Ledwards25519_decode_mul_p25519", 3, After)),

        Q!("    lea             " "rdi, [rsp + " S!() "]"),
        Q!("    mov             " "rsi, 25"),
        Q!("    lea             " "rdx, [rsp + " T!() "]"),
        Q!("    call            " Label!("Ledwards25519_decode_nsqr_p25519", 2, After)),

        Q!("    lea             " "rdi, [rsp + " V!() "]"),
        Q!("    lea             " "rsi, [rsp + " S!() "]"),
        Q!("    lea             " "rdx, [rsp + " V!() "]"),
        Q!("    call            " Label!("Ledwards25519_decode_mul_p25519", 3, After)),

        Q!("    lea             " "rdi, [rsp + " S!() "]"),
        Q!("    mov             " "rsi, 125"),
        Q!("    lea             " "rdx, [rsp + " V!() "]"),
        Q!("    call            " Label!("Ledwards25519_decode_nsqr_p25519", 2, After)),

        Q!("    lea             " "rdi, [rsp + " V!() "]"),
        Q!("    lea             " "rsi, [rsp + " S!() "]"),
        Q!("    lea             " "rdx, [rsp + " V!() "]"),
        Q!("    call            " Label!("Ledwards25519_decode_mul_p25519", 3, After)),

        Q!("    lea             " "rdi, [rsp + " S!() "]"),
        Q!("    mov             " "rsi, 2"),
        Q!("    lea             " "rdx, [rsp + " V!() "]"),
        Q!("    call            " Label!("Ledwards25519_decode_nsqr_p25519", 2, After)),

        Q!("    lea             " "rdi, [rsp + " S!() "]"),
        Q!("    lea             " "rsi, [rsp + " S!() "]"),
        Q!("    lea             " "rdx, [rsp + " W!() "]"),
        Q!("    call            " Label!("Ledwards25519_decode_mul_p25519", 3, After)),

        // Compute v' = s^2 * w to discriminate whether the square root sqrt(u/v)
        // exists, in which case we should get 0, 1 or -1.

        Q!("    lea             " "rdi, [rsp + " V!() "]"),
        Q!("    mov             " "rsi, 1"),
        Q!("    lea             " "rdx, [rsp + " S!() "]"),
        Q!("    call            " Label!("Ledwards25519_decode_nsqr_p25519", 2, After)),

        Q!("    lea             " "rdi, [rsp + " V!() "]"),
        Q!("    lea             " "rsi, [rsp + " V!() "]"),
        Q!("    lea             " "rdx, [rsp + " W!() "]"),
        Q!("    call            " Label!("Ledwards25519_decode_mul_p25519", 3, After)),

        // Get the two candidates for sqrt(u / v), one being s = u * w^{252-3}
        // and the other being t = s * j_25519 where j_25519 = sqrt(-1).

        Q!("    lea             " "rdi, [rsp + " S!() "]"),
        Q!("    lea             " "rsi, [rsp + " U!() "]"),
        Q!("    lea             " "rdx, [rsp + " S!() "]"),
        Q!("    call            " Label!("Ledwards25519_decode_mul_p25519", 3, After)),
        Q!("    mov             " "rax, 0xc4ee1b274a0ea0b0"),
        Q!("    mov             " "[rsp + " T!() "], rax"),
        Q!("    mov             " "rax, 0x2f431806ad2fe478"),
        Q!("    mov             " "[rsp + " T!() "+ 8], rax"),
        Q!("    mov             " "rax, 0x2b4d00993dfbd7a7"),
        Q!("    mov             " "[rsp + " T!() "+ 16], rax"),
        Q!("    mov             " "rax, 0x2b8324804fc1df0b"),
        Q!("    mov             " "[rsp + " T!() "+ 24], rax"),
        Q!("    lea             " "rdi, [rsp + " T!() "]"),
        Q!("    lea             " "rsi, [rsp + " S!() "]"),
        Q!("    lea             " "rdx, [rsp + " T!() "]"),
        Q!("    call            " Label!("Ledwards25519_decode_mul_p25519", 3, After)),

        // rax = 0 <=> s^2 * w = 0 or 1

        Q!("    mov             " "r8, [rsp + " V!() "]"),
        Q!("    mov             " "r9, [rsp + " V!() "+ 8]"),
        Q!("    mov             " "r10, [rsp + " V!() "+ 16]"),
        Q!("    mov             " "r11, [rsp + " V!() "+ 24]"),
        Q!("    mov             " "eax, 1"),
        Q!("    not             " "rax"),
        Q!("    and             " "rax, r8"),
        Q!("    or              " "rax, r9"),
        Q!("    or              " "rax, r10"),
        Q!("    or              " "rax, r11"),

        // r8 = 0 <=> s^2 * w = -1 (mod p_25519, i.e. s^2 * w = 2^255 - 20)

        Q!("    add             " "r8, 20"),
        Q!("    not             " "r9"),
        Q!("    not             " "r10"),
        Q!("    bts             " "r11, 63"),
        Q!("    add             " "r11, 1"),
        Q!("    or              " "r8, r9"),
        Q!("    or              " "r10, r11"),
        Q!("    or              " "r8, r10"),

        // If s^2 * w is not 0 or 1 then replace s by t

        Q!("    test            " "rax, rax"),

        Q!("    mov             " "r12, [rsp + " S!() "]"),
        Q!("    mov             " "rbx, [rsp + " T!() "]"),
        Q!("    cmovnz          " "r12, rbx"),
        Q!("    mov             " "r13, [rsp + " S!() "+ 8]"),
        Q!("    mov             " "rbx, [rsp + " T!() "+ 8]"),
        Q!("    cmovnz          " "r13, rbx"),
        Q!("    mov             " "r14, [rsp + " S!() "+ 16]"),
        Q!("    mov             " "rbx, [rsp + " T!() "+ 16]"),
        Q!("    cmovnz          " "r14, rbx"),
        Q!("    mov             " "r15, [rsp + " S!() "+ 24]"),
        Q!("    mov             " "rbx, [rsp + " T!() "+ 24]"),
        Q!("    cmovnz          " "r15, rbx"),
        Q!("    mov             " "[rsp + " S!() "], r12"),
        Q!("    mov             " "[rsp + " S!() "+ 8], r13"),
        Q!("    mov             " "[rsp + " S!() "+ 16], r14"),
        Q!("    mov             " "[rsp + " S!() "+ 24], r15"),

        // Check invalidity, occurring if s^2 * w is not in {0,1,-1}

        Q!("    cmovz           " "r8, rax"),
        Q!("    neg             " "r8"),
        Q!("    sbb             " "r8, r8"),
        Q!("    neg             " "r8"),
        Q!("    or              " badun!() ", r8"),

        // Let [r11;r10;r9;r8] = s and [r15;r14;r13;r12] = p_25519 - s

        Q!("    mov             " "r8, [rsp + " S!() "]"),
        Q!("    mov             " "r12, -19"),
        Q!("    sub             " "r12, r8"),
        Q!("    mov             " "r9, [rsp + " S!() "+ 8]"),
        Q!("    mov             " "r13, -1"),
        Q!("    sbb             " "r13, r9"),
        Q!("    mov             " "r10, [rsp + " S!() "+ 16]"),
        Q!("    mov             " "r14, -1"),
        Q!("    sbb             " "r14, r10"),
        Q!("    mov             " "r11, [rsp + " S!() "+ 24]"),
        Q!("    mov             " "r15, 0x7FFFFFFFFFFFFFFF"),
        Q!("    sbb             " "r15, r11"),

        // Decide whether a flip is apparently indicated, s_0 <=> sgnbit
        // Decide also if s = 0 by OR-ing its digits. Now if a flip is indicated:
        //  - if s = 0 then mark as invalid
        //  - if s <> 0 then indeed flip

        Q!("    mov             " "ecx, 1"),
        Q!("    and             " "rcx, r8"),
        Q!("    xor             " "rcx, " sgnbit!()),
        Q!("    mov             " "rdx, " badun!()),
        Q!("    mov             " "rsi, rdx"),
        Q!("    or              " "rdx, rcx"),
        Q!("    xor             " "ebp, ebp"),
        Q!("    mov             " "rax, r8"),
        Q!("    mov             " "rbx, r9"),
        Q!("    or              " "rax, r10"),
        Q!("    or              " "rbx, r11"),
        Q!("    or              " "rax, rbx"),
        Q!("    cmovz           " "rcx, rbp"),
        Q!("    cmovnz          " "rdx, rsi"),

        // Actual selection of x as s or -s, copying of y and return of validity

        Q!("    test            " "rcx, rcx"),

        Q!("    cmovnz          " "r8, r12"),
        Q!("    cmovnz          " "r9, r13"),
        Q!("    cmovnz          " "r10, r14"),
        Q!("    cmovnz          " "r11, r15"),

        Q!("    mov             " "rdi, " res!()),
        Q!("    mov             " "[rdi], r8"),
        Q!("    mov             " "[rdi + 8], r9"),
        Q!("    mov             " "[rdi + 16], r10"),
        Q!("    mov             " "[rdi + 24], r11"),
        Q!("    mov             " "rcx, [rsp + " Y!() "]"),
        Q!("    mov             " "[rdi + 32], rcx"),
        Q!("    mov             " "rcx, [rsp + " Y!() "+ 8]"),
        Q!("    mov             " "[rdi + 40], rcx"),
        Q!("    mov             " "rcx, [rsp + " Y!() "+ 16]"),
        Q!("    mov             " "[rdi + 48], rcx"),
        Q!("    mov             " "rcx, [rsp + " Y!() "+ 24]"),
        Q!("    mov             " "[rdi + 56], rcx"),

        Q!("    mov             " "rax, rdx"),

        // Restore stack and registers

        Q!("    add             " "rsp, " NSPACE!()),

        Q!("    pop             " "r15"),
        Q!("    pop             " "r14"),
        Q!("    pop             " "r13"),
        Q!("    pop             " "r12"),
        Q!("    pop             " "rbp"),
        Q!("    pop             " "rbx"),
        // proc hoisting in -> ret after Ledwards25519_decode_loop
        Q!("    jmp             " Label!("hoist_finish", 4, After)),

        // *************************************************************
        // Local z = x * y
        // *************************************************************

        Q!(Label!("Ledwards25519_decode_mul_p25519", 3) ":"),

        Q!("    mov             " "rcx, rdx"),
        Q!("    xor             " "ebp, ebp"),
        Q!("    mov             " "rdx, [rcx]"),
        Q!("    mulx            " "r9, r8, [rsi]"),
        Q!("    mulx            " "r10, rax, [rsi + 0x8]"),
        Q!("    add             " "r9, rax"),
        Q!("    mulx            " "r11, rax, [rsi + 0x10]"),
        Q!("    adc             " "r10, rax"),
        Q!("    mulx            " "r12, rax, [rsi + 0x18]"),
        Q!("    adc             " "r11, rax"),
        Q!("    adc             " "r12, rbp"),
        Q!("    xor             " "ebp, ebp"),
        Q!("    mov             " "rdx, [rcx + 0x8]"),
        Q!("    mulx            " "rbx, rax, [rsi]"),
        Q!("    adcx            " "r9, rax"),
        Q!("    adox            " "r10, rbx"),
        Q!("    mulx            " "rbx, rax, [rsi + 0x8]"),
        Q!("    adcx            " "r10, rax"),
        Q!("    adox            " "r11, rbx"),
        Q!("    mulx            " "rbx, rax, [rsi + 0x10]"),
        Q!("    adcx            " "r11, rax"),
        Q!("    adox            " "r12, rbx"),
        Q!("    mulx            " "r13, rax, [rsi + 0x18]"),
        Q!("    adcx            " "r12, rax"),
        Q!("    adox            " "r13, rbp"),
        Q!("    adc             " "r13, rbp"),
        Q!("    xor             " "ebp, ebp"),
        Q!("    mov             " "rdx, [rcx + 0x10]"),
        Q!("    mulx            " "rbx, rax, [rsi]"),
        Q!("    adcx            " "r10, rax"),
        Q!("    adox            " "r11, rbx"),
        Q!("    mulx            " "rbx, rax, [rsi + 0x8]"),
        Q!("    adcx            " "r11, rax"),
        Q!("    adox            " "r12, rbx"),
        Q!("    mulx            " "rbx, rax, [rsi + 0x10]"),
        Q!("    adcx            " "r12, rax"),
        Q!("    adox            " "r13, rbx"),
        Q!("    mulx            " "r14, rax, [rsi + 0x18]"),
        Q!("    adcx            " "r13, rax"),
        Q!("    adox            " "r14, rbp"),
        Q!("    adc             " "r14, rbp"),
        Q!("    xor             " "ebp, ebp"),
        Q!("    mov             " "rdx, [rcx + 0x18]"),
        Q!("    mulx            " "rbx, rax, [rsi]"),
        Q!("    adcx            " "r11, rax"),
        Q!("    adox            " "r12, rbx"),
        Q!("    mulx            " "r15, rcx, [rsi + 0x18]"),
        Q!("    mulx            " "rbx, rax, [rsi + 0x8]"),
        Q!("    adcx            " "r12, rax"),
        Q!("    adox            " "r13, rbx"),
        Q!("    mulx            " "rbx, rax, [rsi + 0x10]"),
        Q!("    adcx            " "r13, rax"),
        Q!("    adox            " "r14, rbx"),
        Q!("    mov             " "edx, 0x26"),
        Q!("    mulx            " "rbx, rax, r15"),
        Q!("    adcx            " "r14, rcx"),
        Q!("    adox            " "r15, rbp"),
        Q!("    adc             " "r15, rbp"),
        Q!("    add             " "rax, r11"),
        Q!("    adc             " "rbx, rbp"),
        Q!("    bt              " "rax, 0x3f"),
        Q!("    adc             " "rbx, rbx"),
        Q!("    lea             " "rcx, [rbx + 0x1]"),
        Q!("    imul            " "rcx, rcx, 0x13"),
        Q!("    xor             " "ebp, ebp"),
        Q!("    adox            " "r8, rcx"),
        Q!("    mulx            " "rbx, rax, r12"),
        Q!("    adcx            " "r8, rax"),
        Q!("    adox            " "r9, rbx"),
        Q!("    mulx            " "rbx, rax, r13"),
        Q!("    adcx            " "r9, rax"),
        Q!("    adox            " "r10, rbx"),
        Q!("    mulx            " "rbx, rax, r14"),
        Q!("    adcx            " "r10, rax"),
        Q!("    adox            " "r11, rbx"),
        Q!("    mulx            " "rbx, rax, r15"),
        Q!("    adc             " "r11, rax"),
        Q!("    shl             " "rcx, 0x3f"),
        Q!("    cmp             " "r11, rcx"),
        Q!("    mov             " "eax, 0x13"),
        Q!("    cmovns          " "rax, rbp"),
        Q!("    sub             " "r8, rax"),
        Q!("    sbb             " "r9, rbp"),
        Q!("    sbb             " "r10, rbp"),
        Q!("    sbb             " "r11, rbp"),
        Q!("    btr             " "r11, 0x3f"),
        Q!("    mov             " "[rdi], r8"),
        Q!("    mov             " "[rdi + 0x8], r9"),
        Q!("    mov             " "[rdi + 0x10], r10"),
        Q!("    mov             " "[rdi + 0x18], r11"),
        Q!("    ret             " ),

        // *************************************************************
        // Local z = 2^n * x
        // *************************************************************

        Q!(Label!("Ledwards25519_decode_nsqr_p25519", 2) ":"),

        // Copy input argument into q

        Q!("    mov             " "rax, [rdx]"),
        Q!("    mov             " "rbx, [rdx + 8]"),
        Q!("    mov             " "rcx, [rdx + 16]"),
        Q!("    mov             " "rdx, [rdx + 24]"),
        Q!("    mov             " "[rsp + " Q8!() "], rax"),
        Q!("    mov             " "[rsp + " Q8!() "+ 8], rbx"),
        Q!("    mov             " "[rsp + " Q8!() "+ 16], rcx"),
        Q!("    mov             " "[rsp + " Q8!() "+ 24], rdx"),

        // Main squaring loop, accumulating in u consistently  and
        // only ensuring the intermediates are < 2 * p_25519 = 2^256 - 38

        Q!(Label!("Ledwards25519_decode_loop", 5) ":"),
        Q!("    mov             " "rdx, [rsp + " Q8!() "]"),
        Q!("    mulx            " "r15, r8, rdx"),
        Q!("    mulx            " "r10, r9, [rsp + " Q8!() "+ 0x8]"),
        Q!("    mulx            " "r12, r11, [rsp + " Q8!() "+ 0x18]"),
        Q!("    mov             " "rdx, [rsp + " Q8!() "+ 0x10]"),
        Q!("    mulx            " "r14, r13, [rsp + " Q8!() "+ 0x18]"),
        Q!("    xor             " "ebx, ebx"),
        Q!("    mulx            " "rcx, rax, [rsp + " Q8!() "]"),
        Q!("    adcx            " "r10, rax"),
        Q!("    adox            " "r11, rcx"),
        Q!("    mulx            " "rcx, rax, [rsp + " Q8!() "+ 0x8]"),
        Q!("    adcx            " "r11, rax"),
        Q!("    adox            " "r12, rcx"),
        Q!("    mov             " "rdx, [rsp + " Q8!() "+ 0x18]"),
        Q!("    mulx            " "rcx, rax, [rsp + " Q8!() "+ 0x8]"),
        Q!("    adcx            " "r12, rax"),
        Q!("    adox            " "r13, rcx"),
        Q!("    adcx            " "r13, rbx"),
        Q!("    adox            " "r14, rbx"),
        Q!("    adc             " "r14, rbx"),
        Q!("    xor             " "ebx, ebx"),
        Q!("    adcx            " "r9, r9"),
        Q!("    adox            " "r9, r15"),
        Q!("    mov             " "rdx, [rsp + " Q8!() "+ 0x8]"),
        Q!("    mulx            " "rdx, rax, rdx"),
        Q!("    adcx            " "r10, r10"),
        Q!("    adox            " "r10, rax"),
        Q!("    adcx            " "r11, r11"),
        Q!("    adox            " "r11, rdx"),
        Q!("    mov             " "rdx, [rsp + " Q8!() "+ 0x10]"),
        Q!("    mulx            " "rdx, rax, rdx"),
        Q!("    adcx            " "r12, r12"),
        Q!("    adox            " "r12, rax"),
        Q!("    adcx            " "r13, r13"),
        Q!("    adox            " "r13, rdx"),
        Q!("    mov             " "rdx, [rsp + " Q8!() "+ 0x18]"),
        Q!("    mulx            " "r15, rax, rdx"),
        Q!("    adcx            " "r14, r14"),
        Q!("    adox            " "r14, rax"),
        Q!("    adcx            " "r15, rbx"),
        Q!("    adox            " "r15, rbx"),
        Q!("    mov             " "edx, 0x26"),
        Q!("    xor             " "ebx, ebx"),
        Q!("    mulx            " "rcx, rax, r12"),
        Q!("    adcx            " "r8, rax"),
        Q!("    adox            " "r9, rcx"),
        Q!("    mulx            " "rcx, rax, r13"),
        Q!("    adcx            " "r9, rax"),
        Q!("    adox            " "r10, rcx"),
        Q!("    mulx            " "rcx, rax, r14"),
        Q!("    adcx            " "r10, rax"),
        Q!("    adox            " "r11, rcx"),
        Q!("    mulx            " "r12, rax, r15"),
        Q!("    adcx            " "r11, rax"),
        Q!("    adox            " "r12, rbx"),
        Q!("    adcx            " "r12, rbx"),
        Q!("    shld            " "r12, r11, 0x1"),
        Q!("    btr             " "r11, 0x3f"),
        Q!("    mov             " "edx, 0x13"),
        Q!("    imul            " "rdx, r12"),
        Q!("    add             " "r8, rdx"),
        Q!("    adc             " "r9, rbx"),
        Q!("    adc             " "r10, rbx"),
        Q!("    adc             " "r11, rbx"),
        Q!("    mov             " "[rsp + " Q8!() "], r8"),
        Q!("    mov             " "[rsp + " Q8!() "+ 0x8], r9"),
        Q!("    mov             " "[rsp + " Q8!() "+ 0x10], r10"),
        Q!("    mov             " "[rsp + " Q8!() "+ 0x18], r11"),

        // Loop as applicable

        Q!("    dec             " "rsi"),
        Q!("    jnz             " Label!("Ledwards25519_decode_loop", 5, Before)),

        // We know the intermediate result x < 2^256 - 38, and now we do strict
        // modular reduction mod 2^255 - 19. Note x < 2^255 - 19 <=> x + 19 < 2^255
        // which is equivalent to a "ns" condition. We just use the results where
        // they were in registers [r11;r10;r9;r8] instead of re-loading them.

        Q!("    mov             " "eax, 19"),
        Q!("    xor             " "ebx, ebx"),
        Q!("    xor             " "ecx, ecx"),
        Q!("    xor             " "edx, edx"),
        Q!("    add             " "rax, r8"),
        Q!("    adc             " "rbx, r9"),
        Q!("    adc             " "rcx, r10"),
        Q!("    adc             " "rdx, r11"),

        Q!("    cmovns          " "rax, r8"),
        Q!("    cmovns          " "rbx, r9"),
        Q!("    cmovns          " "rcx, r10"),
        Q!("    cmovns          " "rdx, r11"),
        Q!("    btr             " "rdx, 63"),
        Q!("    mov             " "[rdi], rax"),
        Q!("    mov             " "[rdi + 8], rbx"),
        Q!("    mov             " "[rdi + 16], rcx"),
        Q!("    mov             " "[rdi + 24], rdx"),
        Q!("    ret             " ),
        Q!(Label!("hoist_finish", 4) ":"),
        inout("rdi") z.as_mut_ptr() => _,
        inout("rsi") c.as_ptr() => _,
        out("rax") ret,
        // clobbers
        out("r10") _,
        out("r11") _,
        out("r12") _,
        out("r13") _,
        out("r14") _,
        out("r15") _,
        out("r8") _,
        out("r9") _,
        out("rcx") _,
        out("rdx") _,
            )
    };
    ret == 0
}
