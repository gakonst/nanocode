// generated source. do not edit.
#![allow(non_upper_case_globals, unused_macros, unused_imports)]
use crate::low::macros::*;

// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

// ----------------------------------------------------------------------------
// Modular inverse modulo p_256 = 2^256 - 2^224 + 2^192 + 2^96 - 1
// Input x[4]; output z[4]
//
// extern void bignum_inv_p256(uint64_t z[static 4],const uint64_t x[static 4]);
//
// If the 4-digit input x is coprime to p_256, i.e. is not divisible
// by it, returns z < p_256 such that x * z == 1 (mod p_256). Note that
// x does not need to be reduced modulo p_256, but the output always is.
// If the input is divisible (i.e. is 0 or p_256), then there can be no
// modular inverse and z = 0 is returned.
//
// Standard x86-64 ABI: RDI = z, RSI = x
// Microsoft x64 ABI:   RCX = z, RDX = x
// ----------------------------------------------------------------------------

// Size in bytes of a 64-bit word

macro_rules! N {
    () => {
        "8"
    };
}

// Pointer-offset pairs for temporaries on stack

macro_rules! f {
    () => {
        "rsp + 0"
    };
}
macro_rules! g { () => { Q!("rsp + (5 * " N!() ")") } }
macro_rules! u { () => { Q!("rsp + (10 * " N!() ")") } }
macro_rules! v { () => { Q!("rsp + (15 * " N!() ")") } }
macro_rules! tmp { () => { Q!("QWORD PTR [rsp + (20 * " N!() ")]") } }
macro_rules! tmp2 { () => { Q!("QWORD PTR [rsp + (21 * " N!() ")]") } }
macro_rules! i { () => { Q!("QWORD PTR [rsp + (22 * " N!() ")]") } }
macro_rules! d { () => { Q!("QWORD PTR [rsp + (23 * " N!() ")]") } }

macro_rules! mat { () => { Q!("rsp + (24 * " N!() ")") } }

// Backup for the input pointer

macro_rules! res { () => { Q!("QWORD PTR [rsp + (28 * " N!() ")]") } }

// Total size to reserve on the stack

macro_rules! NSPACE { () => { Q!("30 * " N!()) } }

// Syntactic variants to make x86_att version simpler to generate

macro_rules! F {
    () => {
        "0"
    };
}
macro_rules! G { () => { Q!("(5 * " N!() ")") } }
macro_rules! U { () => { Q!("(10 * " N!() ")") } }
macro_rules! V { () => { Q!("(15 * " N!() ")") } }
macro_rules! MAT { () => { Q!("(24 * " N!() ")") } }

macro_rules! ff {
    () => {
        "QWORD PTR [rsp]"
    };
}
macro_rules! gg { () => { Q!("QWORD PTR [rsp + (5 * " N!() ")]") } }

// ---------------------------------------------------------------------------
// Core signed almost-Montgomery reduction macro from u[4..0] to u[3..0].
// ---------------------------------------------------------------------------

macro_rules! amontred {
    ($P:expr) => { Q!(
        /* We only know the input is -2^316 < x < 2^316. To do traditional  */
        /* unsigned Montgomery reduction, start by adding 2^61 * p_256.     */
        "mov r8, 0xe000000000000000;\n"
        "add r8, [" $P "];\n"
        "mov r9, 0xffffffffffffffff;\n"
        "adc r9, [" $P "+ 8];\n"
        "mov r10, 0x000000001fffffff;\n"
        "adc r10, [" $P "+ 16];\n"
        "mov r11, 0x2000000000000000;\n"
        "adc r11, [" $P "+ 24];\n"
        "mov r12, 0x1fffffffe0000000;\n"
        "adc r12, [" $P "+ 32];\n"
        /* Let [r8;rbx] = 2^32 * w and [rdx;rax] = (2^64 - 2^32 + 1) * w */
        /* where w is the lowest word */
        "mov rbx, r8;\n"
        "shl rbx, 32;\n"
        "mov rax, 0xffffffff00000001;\n"
        "mul r8;\n"
        "shr r8, 32;\n"
        /* Hence basic addition of (2^256 - 2^224 + 2^192 + 2^96) * w */
        "add r9, rbx;\n"
        "adc r10, r8;\n"
        "adc r11, rax;\n"
        "adc r12, rdx;\n"
        /* Now capture carry and subtract p_256 if set (almost-Montgomery) */
        "sbb rax, rax;\n"
        "mov ebx, 0x00000000ffffffff;\n"
        "and rbx, rax;\n"
        "mov rdx, 0xffffffff00000001;\n"
        "and rdx, rax;\n"
        "sub r9, rax;\n"
        "mov [" $P "], r9;\n"
        "sbb r10, rbx;\n"
        "mov [" $P "+ 8], r10;\n"
        "sbb r11, 0;\n"
        "mov [" $P "+ 16], r11;\n"
        "sbb r12, rdx;\n"
        "mov [" $P "+ 24], r12"
    )}
}

// Very similar to a subroutine call to the s2n-bignum word_divstep59.
// But different in register usage and returning the final matrix as
//
// [ r8   r10]
// [ r12  r14]
//
// and also returning the matrix still negated (which doesn't matter)

macro_rules! divstep59 {
    ($din:expr, $fin:expr, $gin:expr) => { Q!(
        "mov rsi, " $din ";\n"
        "mov rdx, " $fin ";\n"
        "mov rcx, " $gin ";\n"
        "mov rbx, rdx;\n"
        "and rbx, 0xfffff;\n"
        "movabs rax, 0xfffffe0000000000;\n"
        "or rbx, rax;\n"
        "and rcx, 0xfffff;\n"
        "movabs rax, 0xc000000000000000;\n"
        "or rcx, rax;\n"
        "mov rax, 0xfffffffffffffffe;\n"
        "xor ebp, ebp;\n"
        "mov edx, 0x2;\n"
        "mov rdi, rbx;\n"
        "mov r8, rax;\n"
        "test rsi, rsi;\n"
        "cmovs r8, rbp;\n"
        "test rcx, 0x1;\n"
        "cmove r8, rbp;\n"
        "cmove rdi, rbp;\n"
        "xor rdi, r8;\n"
        "xor rsi, r8;\n"
        "bt r8, 0x3f;\n"
        "cmovb rbx, rcx;\n"
        "mov r8, rax;\n"
        "sub rsi, rax;\n"
        "lea rcx, [rcx + rdi];\n"
        "cmovs r8, rbp;\n"
        "mov rdi, rbx;\n"
        "test rcx, rdx;\n"
        "cmove r8, rbp;\n"
        "cmove rdi, rbp;\n"
        "sar rcx, 1;\n"
        "xor rdi, r8;\n"
        "xor rsi, r8;\n"
        "bt r8, 0x3f;\n"
        "cmovb rbx, rcx;\n"
        "mov r8, rax;\n"
        "sub rsi, rax;\n"
        "lea rcx, [rcx + rdi];\n"
        "cmovs r8, rbp;\n"
        "mov rdi, rbx;\n"
        "test rcx, rdx;\n"
        "cmove r8, rbp;\n"
        "cmove rdi, rbp;\n"
        "sar rcx, 1;\n"
        "xor rdi, r8;\n"
        "xor rsi, r8;\n"
        "bt r8, 0x3f;\n"
        "cmovb rbx, rcx;\n"
        "mov r8, rax;\n"
        "sub rsi, rax;\n"
        "lea rcx, [rcx + rdi];\n"
        "cmovs r8, rbp;\n"
        "mov rdi, rbx;\n"
        "test rcx, rdx;\n"
        "cmove r8, rbp;\n"
        "cmove rdi, rbp;\n"
        "sar rcx, 1;\n"
        "xor rdi, r8;\n"
        "xor rsi, r8;\n"
        "bt r8, 0x3f;\n"
        "cmovb rbx, rcx;\n"
        "mov r8, rax;\n"
        "sub rsi, rax;\n"
        "lea rcx, [rcx + rdi];\n"
        "cmovs r8, rbp;\n"
        "mov rdi, rbx;\n"
        "test rcx, rdx;\n"
        "cmove r8, rbp;\n"
        "cmove rdi, rbp;\n"
        "sar rcx, 1;\n"
        "xor rdi, r8;\n"
        "xor rsi, r8;\n"
        "bt r8, 0x3f;\n"
        "cmovb rbx, rcx;\n"
        "mov r8, rax;\n"
        "sub rsi, rax;\n"
        "lea rcx, [rcx + rdi];\n"
        "cmovs r8, rbp;\n"
        "mov rdi, rbx;\n"
        "test rcx, rdx;\n"
        "cmove r8, rbp;\n"
        "cmove rdi, rbp;\n"
        "sar rcx, 1;\n"
        "xor rdi, r8;\n"
        "xor rsi, r8;\n"
        "bt r8, 0x3f;\n"
        "cmovb rbx, rcx;\n"
        "mov r8, rax;\n"
        "sub rsi, rax;\n"
        "lea rcx, [rcx + rdi];\n"
        "cmovs r8, rbp;\n"
        "mov rdi, rbx;\n"
        "test rcx, rdx;\n"
        "cmove r8, rbp;\n"
        "cmove rdi, rbp;\n"
        "sar rcx, 1;\n"
        "xor rdi, r8;\n"
        "xor rsi, r8;\n"
        "bt r8, 0x3f;\n"
        "cmovb rbx, rcx;\n"
        "mov r8, rax;\n"
        "sub rsi, rax;\n"
        "lea rcx, [rcx + rdi];\n"
        "cmovs r8, rbp;\n"
        "mov rdi, rbx;\n"
        "test rcx, rdx;\n"
        "cmove r8, rbp;\n"
        "cmove rdi, rbp;\n"
        "sar rcx, 1;\n"
        "xor rdi, r8;\n"
        "xor rsi, r8;\n"
        "bt r8, 0x3f;\n"
        "cmovb rbx, rcx;\n"
        "mov r8, rax;\n"
        "sub rsi, rax;\n"
        "lea rcx, [rcx + rdi];\n"
        "cmovs r8, rbp;\n"
        "mov rdi, rbx;\n"
        "test rcx, rdx;\n"
        "cmove r8, rbp;\n"
        "cmove rdi, rbp;\n"
        "sar rcx, 1;\n"
        "xor rdi, r8;\n"
        "xor rsi, r8;\n"
        "bt r8, 0x3f;\n"
        "cmovb rbx, rcx;\n"
        "mov r8, rax;\n"
        "sub rsi, rax;\n"
        "lea rcx, [rcx + rdi];\n"
        "cmovs r8, rbp;\n"
        "mov rdi, rbx;\n"
        "test rcx, rdx;\n"
        "cmove r8, rbp;\n"
        "cmove rdi, rbp;\n"
        "sar rcx, 1;\n"
        "xor rdi, r8;\n"
        "xor rsi, r8;\n"
        "bt r8, 0x3f;\n"
        "cmovb rbx, rcx;\n"
        "mov r8, rax;\n"
        "sub rsi, rax;\n"
        "lea rcx, [rcx + rdi];\n"
        "cmovs r8, rbp;\n"
        "mov rdi, rbx;\n"
        "test rcx, rdx;\n"
        "cmove r8, rbp;\n"
        "cmove rdi, rbp;\n"
        "sar rcx, 1;\n"
        "xor rdi, r8;\n"
        "xor rsi, r8;\n"
        "bt r8, 0x3f;\n"
        "cmovb rbx, rcx;\n"
        "mov r8, rax;\n"
        "sub rsi, rax;\n"
        "lea rcx, [rcx + rdi];\n"
        "cmovs r8, rbp;\n"
        "mov rdi, rbx;\n"
        "test rcx, rdx;\n"
        "cmove r8, rbp;\n"
        "cmove rdi, rbp;\n"
        "sar rcx, 1;\n"
        "xor rdi, r8;\n"
        "xor rsi, r8;\n"
        "bt r8, 0x3f;\n"
        "cmovb rbx, rcx;\n"
        "mov r8, rax;\n"
        "sub rsi, rax;\n"
        "lea rcx, [rcx + rdi];\n"
        "cmovs r8, rbp;\n"
        "mov rdi, rbx;\n"
        "test rcx, rdx;\n"
        "cmove r8, rbp;\n"
        "cmove rdi, rbp;\n"
        "sar rcx, 1;\n"
        "xor rdi, r8;\n"
        "xor rsi, r8;\n"
        "bt r8, 0x3f;\n"
        "cmovb rbx, rcx;\n"
        "mov r8, rax;\n"
        "sub rsi, rax;\n"
        "lea rcx, [rcx + rdi];\n"
        "cmovs r8, rbp;\n"
        "mov rdi, rbx;\n"
        "test rcx, rdx;\n"
        "cmove r8, rbp;\n"
        "cmove rdi, rbp;\n"
        "sar rcx, 1;\n"
        "xor rdi, r8;\n"
        "xor rsi, r8;\n"
        "bt r8, 0x3f;\n"
        "cmovb rbx, rcx;\n"
        "mov r8, rax;\n"
        "sub rsi, rax;\n"
        "lea rcx, [rcx + rdi];\n"
        "cmovs r8, rbp;\n"
        "mov rdi, rbx;\n"
        "test rcx, rdx;\n"
        "cmove r8, rbp;\n"
        "cmove rdi, rbp;\n"
        "sar rcx, 1;\n"
        "xor rdi, r8;\n"
        "xor rsi, r8;\n"
        "bt r8, 0x3f;\n"
        "cmovb rbx, rcx;\n"
        "mov r8, rax;\n"
        "sub rsi, rax;\n"
        "lea rcx, [rcx + rdi];\n"
        "cmovs r8, rbp;\n"
        "mov rdi, rbx;\n"
        "test rcx, rdx;\n"
        "cmove r8, rbp;\n"
        "cmove rdi, rbp;\n"
        "sar rcx, 1;\n"
        "xor rdi, r8;\n"
        "xor rsi, r8;\n"
        "bt r8, 0x3f;\n"
        "cmovb rbx, rcx;\n"
        "mov r8, rax;\n"
        "sub rsi, rax;\n"
        "lea rcx, [rcx + rdi];\n"
        "cmovs r8, rbp;\n"
        "mov rdi, rbx;\n"
        "test rcx, rdx;\n"
        "cmove r8, rbp;\n"
        "cmove rdi, rbp;\n"
        "sar rcx, 1;\n"
        "xor rdi, r8;\n"
        "xor rsi, r8;\n"
        "bt r8, 0x3f;\n"
        "cmovb rbx, rcx;\n"
        "mov r8, rax;\n"
        "sub rsi, rax;\n"
        "lea rcx, [rcx + rdi];\n"
        "cmovs r8, rbp;\n"
        "mov rdi, rbx;\n"
        "test rcx, rdx;\n"
        "cmove r8, rbp;\n"
        "cmove rdi, rbp;\n"
        "sar rcx, 1;\n"
        "xor rdi, r8;\n"
        "xor rsi, r8;\n"
        "bt r8, 0x3f;\n"
        "cmovb rbx, rcx;\n"
        "mov r8, rax;\n"
        "sub rsi, rax;\n"
        "lea rcx, [rcx + rdi];\n"
        "cmovs r8, rbp;\n"
        "mov rdi, rbx;\n"
        "test rcx, rdx;\n"
        "cmove r8, rbp;\n"
        "cmove rdi, rbp;\n"
        "sar rcx, 1;\n"
        "xor rdi, r8;\n"
        "xor rsi, r8;\n"
        "bt r8, 0x3f;\n"
        "cmovb rbx, rcx;\n"
        "mov r8, rax;\n"
        "sub rsi, rax;\n"
        "lea rcx, [rcx + rdi];\n"
        "cmovs r8, rbp;\n"
        "mov rdi, rbx;\n"
        "test rcx, rdx;\n"
        "cmove r8, rbp;\n"
        "cmove rdi, rbp;\n"
        "sar rcx, 1;\n"
        "xor rdi, r8;\n"
        "xor rsi, r8;\n"
        "bt r8, 0x3f;\n"
        "cmovb rbx, rcx;\n"
        "mov r8, rax;\n"
        "sub rsi, rax;\n"
        "lea rcx, [rcx + rdi];\n"
        "sar rcx, 1;\n"
        "mov eax, 0x100000;\n"
        "lea rdx, [rbx + rax];\n"
        "lea rdi, [rcx + rax];\n"
        "shl rdx, 0x16;\n"
        "shl rdi, 0x16;\n"
        "sar rdx, 0x2b;\n"
        "sar rdi, 0x2b;\n"
        "movabs rax, 0x20000100000;\n"
        "lea rbx, [rbx + rax];\n"
        "lea rcx, [rcx + rax];\n"
        "sar rbx, 0x2a;\n"
        "sar rcx, 0x2a;\n"
        "mov [rsp + " MAT!() "], rdx;\n"
        "mov [rsp + " MAT!() "+ 0x8], rbx;\n"
        "mov [rsp + " MAT!() "+ 0x10], rdi;\n"
        "mov [rsp + " MAT!() "+ 0x18], rcx;\n"
        "mov r12, " $fin ";\n"
        "imul rdi, r12;\n"
        "imul r12, rdx;\n"
        "mov r13, " $gin ";\n"
        "imul rbx, r13;\n"
        "imul r13, rcx;\n"
        "add r12, rbx;\n"
        "add r13, rdi;\n"
        "sar r12, 0x14;\n"
        "sar r13, 0x14;\n"
        "mov rbx, r12;\n"
        "and rbx, 0xfffff;\n"
        "movabs rax, 0xfffffe0000000000;\n"
        "or rbx, rax;\n"
        "mov rcx, r13;\n"
        "and rcx, 0xfffff;\n"
        "movabs rax, 0xc000000000000000;\n"
        "or rcx, rax;\n"
        "mov rax, 0xfffffffffffffffe;\n"
        "mov edx, 0x2;\n"
        "mov rdi, rbx;\n"
        "mov r8, rax;\n"
        "test rsi, rsi;\n"
        "cmovs r8, rbp;\n"
        "test rcx, 0x1;\n"
        "cmove r8, rbp;\n"
        "cmove rdi, rbp;\n"
        "xor rdi, r8;\n"
        "xor rsi, r8;\n"
        "bt r8, 0x3f;\n"
        "cmovb rbx, rcx;\n"
        "mov r8, rax;\n"
        "sub rsi, rax;\n"
        "lea rcx, [rcx + rdi];\n"
        "cmovs r8, rbp;\n"
        "mov rdi, rbx;\n"
        "test rcx, rdx;\n"
        "cmove r8, rbp;\n"
        "cmove rdi, rbp;\n"
        "sar rcx, 1;\n"
        "xor rdi, r8;\n"
        "xor rsi, r8;\n"
        "bt r8, 0x3f;\n"
        "cmovb rbx, rcx;\n"
        "mov r8, rax;\n"
        "sub rsi, rax;\n"
        "lea rcx, [rcx + rdi];\n"
        "cmovs r8, rbp;\n"
        "mov rdi, rbx;\n"
        "test rcx, rdx;\n"
        "cmove r8, rbp;\n"
        "cmove rdi, rbp;\n"
        "sar rcx, 1;\n"
        "xor rdi, r8;\n"
        "xor rsi, r8;\n"
        "bt r8, 0x3f;\n"
        "cmovb rbx, rcx;\n"
        "mov r8, rax;\n"
        "sub rsi, rax;\n"
        "lea rcx, [rcx + rdi];\n"
        "cmovs r8, rbp;\n"
        "mov rdi, rbx;\n"
        "test rcx, rdx;\n"
        "cmove r8, rbp;\n"
        "cmove rdi, rbp;\n"
        "sar rcx, 1;\n"
        "xor rdi, r8;\n"
        "xor rsi, r8;\n"
        "bt r8, 0x3f;\n"
        "cmovb rbx, rcx;\n"
        "mov r8, rax;\n"
        "sub rsi, rax;\n"
        "lea rcx, [rcx + rdi];\n"
        "cmovs r8, rbp;\n"
        "mov rdi, rbx;\n"
        "test rcx, rdx;\n"
        "cmove r8, rbp;\n"
        "cmove rdi, rbp;\n"
        "sar rcx, 1;\n"
        "xor rdi, r8;\n"
        "xor rsi, r8;\n"
        "bt r8, 0x3f;\n"
        "cmovb rbx, rcx;\n"
        "mov r8, rax;\n"
        "sub rsi, rax;\n"
        "lea rcx, [rcx + rdi];\n"
        "cmovs r8, rbp;\n"
        "mov rdi, rbx;\n"
        "test rcx, rdx;\n"
        "cmove r8, rbp;\n"
        "cmove rdi, rbp;\n"
        "sar rcx, 1;\n"
        "xor rdi, r8;\n"
        "xor rsi, r8;\n"
        "bt r8, 0x3f;\n"
        "cmovb rbx, rcx;\n"
        "mov r8, rax;\n"
        "sub rsi, rax;\n"
        "lea rcx, [rcx + rdi];\n"
        "cmovs r8, rbp;\n"
        "mov rdi, rbx;\n"
        "test rcx, rdx;\n"
        "cmove r8, rbp;\n"
        "cmove rdi, rbp;\n"
        "sar rcx, 1;\n"
        "xor rdi, r8;\n"
        "xor rsi, r8;\n"
        "bt r8, 0x3f;\n"
        "cmovb rbx, rcx;\n"
        "mov r8, rax;\n"
        "sub rsi, rax;\n"
        "lea rcx, [rcx + rdi];\n"
        "cmovs r8, rbp;\n"
        "mov rdi, rbx;\n"
        "test rcx, rdx;\n"
        "cmove r8, rbp;\n"
        "cmove rdi, rbp;\n"
        "sar rcx, 1;\n"
        "xor rdi, r8;\n"
        "xor rsi, r8;\n"
        "bt r8, 0x3f;\n"
        "cmovb rbx, rcx;\n"
        "mov r8, rax;\n"
        "sub rsi, rax;\n"
        "lea rcx, [rcx + rdi];\n"
        "cmovs r8, rbp;\n"
        "mov rdi, rbx;\n"
        "test rcx, rdx;\n"
        "cmove r8, rbp;\n"
        "cmove rdi, rbp;\n"
        "sar rcx, 1;\n"
        "xor rdi, r8;\n"
        "xor rsi, r8;\n"
        "bt r8, 0x3f;\n"
        "cmovb rbx, rcx;\n"
        "mov r8, rax;\n"
        "sub rsi, rax;\n"
        "lea rcx, [rcx + rdi];\n"
        "cmovs r8, rbp;\n"
        "mov rdi, rbx;\n"
        "test rcx, rdx;\n"
        "cmove r8, rbp;\n"
        "cmove rdi, rbp;\n"
        "sar rcx, 1;\n"
        "xor rdi, r8;\n"
        "xor rsi, r8;\n"
        "bt r8, 0x3f;\n"
        "cmovb rbx, rcx;\n"
        "mov r8, rax;\n"
        "sub rsi, rax;\n"
        "lea rcx, [rcx + rdi];\n"
        "cmovs r8, rbp;\n"
        "mov rdi, rbx;\n"
        "test rcx, rdx;\n"
        "cmove r8, rbp;\n"
        "cmove rdi, rbp;\n"
        "sar rcx, 1;\n"
        "xor rdi, r8;\n"
        "xor rsi, r8;\n"
        "bt r8, 0x3f;\n"
        "cmovb rbx, rcx;\n"
        "mov r8, rax;\n"
        "sub rsi, rax;\n"
        "lea rcx, [rcx + rdi];\n"
        "cmovs r8, rbp;\n"
        "mov rdi, rbx;\n"
        "test rcx, rdx;\n"
        "cmove r8, rbp;\n"
        "cmove rdi, rbp;\n"
        "sar rcx, 1;\n"
        "xor rdi, r8;\n"
        "xor rsi, r8;\n"
        "bt r8, 0x3f;\n"
        "cmovb rbx, rcx;\n"
        "mov r8, rax;\n"
        "sub rsi, rax;\n"
        "lea rcx, [rcx + rdi];\n"
        "cmovs r8, rbp;\n"
        "mov rdi, rbx;\n"
        "test rcx, rdx;\n"
        "cmove r8, rbp;\n"
        "cmove rdi, rbp;\n"
        "sar rcx, 1;\n"
        "xor rdi, r8;\n"
        "xor rsi, r8;\n"
        "bt r8, 0x3f;\n"
        "cmovb rbx, rcx;\n"
        "mov r8, rax;\n"
        "sub rsi, rax;\n"
        "lea rcx, [rcx + rdi];\n"
        "cmovs r8, rbp;\n"
        "mov rdi, rbx;\n"
        "test rcx, rdx;\n"
        "cmove r8, rbp;\n"
        "cmove rdi, rbp;\n"
        "sar rcx, 1;\n"
        "xor rdi, r8;\n"
        "xor rsi, r8;\n"
        "bt r8, 0x3f;\n"
        "cmovb rbx, rcx;\n"
        "mov r8, rax;\n"
        "sub rsi, rax;\n"
        "lea rcx, [rcx + rdi];\n"
        "cmovs r8, rbp;\n"
        "mov rdi, rbx;\n"
        "test rcx, rdx;\n"
        "cmove r8, rbp;\n"
        "cmove rdi, rbp;\n"
        "sar rcx, 1;\n"
        "xor rdi, r8;\n"
        "xor rsi, r8;\n"
        "bt r8, 0x3f;\n"
        "cmovb rbx, rcx;\n"
        "mov r8, rax;\n"
        "sub rsi, rax;\n"
        "lea rcx, [rcx + rdi];\n"
        "cmovs r8, rbp;\n"
        "mov rdi, rbx;\n"
        "test rcx, rdx;\n"
        "cmove r8, rbp;\n"
        "cmove rdi, rbp;\n"
        "sar rcx, 1;\n"
        "xor rdi, r8;\n"
        "xor rsi, r8;\n"
        "bt r8, 0x3f;\n"
        "cmovb rbx, rcx;\n"
        "mov r8, rax;\n"
        "sub rsi, rax;\n"
        "lea rcx, [rcx + rdi];\n"
        "cmovs r8, rbp;\n"
        "mov rdi, rbx;\n"
        "test rcx, rdx;\n"
        "cmove r8, rbp;\n"
        "cmove rdi, rbp;\n"
        "sar rcx, 1;\n"
        "xor rdi, r8;\n"
        "xor rsi, r8;\n"
        "bt r8, 0x3f;\n"
        "cmovb rbx, rcx;\n"
        "mov r8, rax;\n"
        "sub rsi, rax;\n"
        "lea rcx, [rcx + rdi];\n"
        "cmovs r8, rbp;\n"
        "mov rdi, rbx;\n"
        "test rcx, rdx;\n"
        "cmove r8, rbp;\n"
        "cmove rdi, rbp;\n"
        "sar rcx, 1;\n"
        "xor rdi, r8;\n"
        "xor rsi, r8;\n"
        "bt r8, 0x3f;\n"
        "cmovb rbx, rcx;\n"
        "mov r8, rax;\n"
        "sub rsi, rax;\n"
        "lea rcx, [rcx + rdi];\n"
        "cmovs r8, rbp;\n"
        "mov rdi, rbx;\n"
        "test rcx, rdx;\n"
        "cmove r8, rbp;\n"
        "cmove rdi, rbp;\n"
        "sar rcx, 1;\n"
        "xor rdi, r8;\n"
        "xor rsi, r8;\n"
        "bt r8, 0x3f;\n"
        "cmovb rbx, rcx;\n"
        "mov r8, rax;\n"
        "sub rsi, rax;\n"
        "lea rcx, [rcx + rdi];\n"
        "cmovs r8, rbp;\n"
        "mov rdi, rbx;\n"
        "test rcx, rdx;\n"
        "cmove r8, rbp;\n"
        "cmove rdi, rbp;\n"
        "sar rcx, 1;\n"
        "xor rdi, r8;\n"
        "xor rsi, r8;\n"
        "bt r8, 0x3f;\n"
        "cmovb rbx, rcx;\n"
        "mov r8, rax;\n"
        "sub rsi, rax;\n"
        "lea rcx, [rcx + rdi];\n"
        "sar rcx, 1;\n"
        "mov eax, 0x100000;\n"
        "lea r8, [rbx + rax];\n"
        "lea r10, [rcx + rax];\n"
        "shl r8, 0x16;\n"
        "shl r10, 0x16;\n"
        "sar r8, 0x2b;\n"
        "sar r10, 0x2b;\n"
        "movabs rax, 0x20000100000;\n"
        "lea r15, [rbx + rax];\n"
        "lea r11, [rcx + rax];\n"
        "sar r15, 0x2a;\n"
        "sar r11, 0x2a;\n"
        "mov rbx, r13;\n"
        "mov rcx, r12;\n"
        "imul r12, r8;\n"
        "imul rbx, r15;\n"
        "add r12, rbx;\n"
        "imul r13, r11;\n"
        "imul rcx, r10;\n"
        "add r13, rcx;\n"
        "sar r12, 0x14;\n"
        "sar r13, 0x14;\n"
        "mov rbx, r12;\n"
        "and rbx, 0xfffff;\n"
        "movabs rax, 0xfffffe0000000000;\n"
        "or rbx, rax;\n"
        "mov rcx, r13;\n"
        "and rcx, 0xfffff;\n"
        "movabs rax, 0xc000000000000000;\n"
        "or rcx, rax;\n"
        "mov rax, [rsp + " MAT!() "];\n"
        "imul rax, r8;\n"
        "mov rdx, [rsp + " MAT!() "+ 0x10];\n"
        "imul rdx, r15;\n"
        "imul r8, [rsp + " MAT!() "+ 0x8];\n"
        "imul r15, [rsp + " MAT!() "+ 0x18];\n"
        "add r15, r8;\n"
        "lea r9, [rax + rdx];\n"
        "mov rax, [rsp + " MAT!() "];\n"
        "imul rax, r10;\n"
        "mov rdx, [rsp + " MAT!() "+ 0x10];\n"
        "imul rdx, r11;\n"
        "imul r10, [rsp + " MAT!() "+ 0x8];\n"
        "imul r11, [rsp + " MAT!() "+ 0x18];\n"
        "add r11, r10;\n"
        "lea r13, [rax + rdx];\n"
        "mov rax, 0xfffffffffffffffe;\n"
        "mov edx, 0x2;\n"
        "mov rdi, rbx;\n"
        "mov r8, rax;\n"
        "test rsi, rsi;\n"
        "cmovs r8, rbp;\n"
        "test rcx, 0x1;\n"
        "cmove r8, rbp;\n"
        "cmove rdi, rbp;\n"
        "xor rdi, r8;\n"
        "xor rsi, r8;\n"
        "bt r8, 0x3f;\n"
        "cmovb rbx, rcx;\n"
        "mov r8, rax;\n"
        "sub rsi, rax;\n"
        "lea rcx, [rcx + rdi];\n"
        "cmovs r8, rbp;\n"
        "mov rdi, rbx;\n"
        "test rcx, rdx;\n"
        "cmove r8, rbp;\n"
        "cmove rdi, rbp;\n"
        "sar rcx, 1;\n"
        "xor rdi, r8;\n"
        "xor rsi, r8;\n"
        "bt r8, 0x3f;\n"
        "cmovb rbx, rcx;\n"
        "mov r8, rax;\n"
        "sub rsi, rax;\n"
        "lea rcx, [rcx + rdi];\n"
        "cmovs r8, rbp;\n"
        "mov rdi, rbx;\n"
        "test rcx, rdx;\n"
        "cmove r8, rbp;\n"
        "cmove rdi, rbp;\n"
        "sar rcx, 1;\n"
        "xor rdi, r8;\n"
        "xor rsi, r8;\n"
        "bt r8, 0x3f;\n"
        "cmovb rbx, rcx;\n"
        "mov r8, rax;\n"
        "sub rsi, rax;\n"
        "lea rcx, [rcx + rdi];\n"
        "cmovs r8, rbp;\n"
        "mov rdi, rbx;\n"
        "test rcx, rdx;\n"
        "cmove r8, rbp;\n"
        "cmove rdi, rbp;\n"
        "sar rcx, 1;\n"
        "xor rdi, r8;\n"
        "xor rsi, r8;\n"
        "bt r8, 0x3f;\n"
        "cmovb rbx, rcx;\n"
        "mov r8, rax;\n"
        "sub rsi, rax;\n"
        "lea rcx, [rcx + rdi];\n"
        "cmovs r8, rbp;\n"
        "mov rdi, rbx;\n"
        "test rcx, rdx;\n"
        "cmove r8, rbp;\n"
        "cmove rdi, rbp;\n"
        "sar rcx, 1;\n"
        "xor rdi, r8;\n"
        "xor rsi, r8;\n"
        "bt r8, 0x3f;\n"
        "cmovb rbx, rcx;\n"
        "mov r8, rax;\n"
        "sub rsi, rax;\n"
        "lea rcx, [rcx + rdi];\n"
        "cmovs r8, rbp;\n"
        "mov rdi, rbx;\n"
        "test rcx, rdx;\n"
        "cmove r8, rbp;\n"
        "cmove rdi, rbp;\n"
        "sar rcx, 1;\n"
        "xor rdi, r8;\n"
        "xor rsi, r8;\n"
        "bt r8, 0x3f;\n"
        "cmovb rbx, rcx;\n"
        "mov r8, rax;\n"
        "sub rsi, rax;\n"
        "lea rcx, [rcx + rdi];\n"
        "cmovs r8, rbp;\n"
        "mov rdi, rbx;\n"
        "test rcx, rdx;\n"
        "cmove r8, rbp;\n"
        "cmove rdi, rbp;\n"
        "sar rcx, 1;\n"
        "xor rdi, r8;\n"
        "xor rsi, r8;\n"
        "bt r8, 0x3f;\n"
        "cmovb rbx, rcx;\n"
        "mov r8, rax;\n"
        "sub rsi, rax;\n"
        "lea rcx, [rcx + rdi];\n"
        "cmovs r8, rbp;\n"
        "mov rdi, rbx;\n"
        "test rcx, rdx;\n"
        "cmove r8, rbp;\n"
        "cmove rdi, rbp;\n"
        "sar rcx, 1;\n"
        "xor rdi, r8;\n"
        "xor rsi, r8;\n"
        "bt r8, 0x3f;\n"
        "cmovb rbx, rcx;\n"
        "mov r8, rax;\n"
        "sub rsi, rax;\n"
        "lea rcx, [rcx + rdi];\n"
        "cmovs r8, rbp;\n"
        "mov rdi, rbx;\n"
        "test rcx, rdx;\n"
        "cmove r8, rbp;\n"
        "cmove rdi, rbp;\n"
        "sar rcx, 1;\n"
        "xor rdi, r8;\n"
        "xor rsi, r8;\n"
        "bt r8, 0x3f;\n"
        "cmovb rbx, rcx;\n"
        "mov r8, rax;\n"
        "sub rsi, rax;\n"
        "lea rcx, [rcx + rdi];\n"
        "cmovs r8, rbp;\n"
        "mov rdi, rbx;\n"
        "test rcx, rdx;\n"
        "cmove r8, rbp;\n"
        "cmove rdi, rbp;\n"
        "sar rcx, 1;\n"
        "xor rdi, r8;\n"
        "xor rsi, r8;\n"
        "bt r8, 0x3f;\n"
        "cmovb rbx, rcx;\n"
        "mov r8, rax;\n"
        "sub rsi, rax;\n"
        "lea rcx, [rcx + rdi];\n"
        "cmovs r8, rbp;\n"
        "mov rdi, rbx;\n"
        "test rcx, rdx;\n"
        "cmove r8, rbp;\n"
        "cmove rdi, rbp;\n"
        "sar rcx, 1;\n"
        "xor rdi, r8;\n"
        "xor rsi, r8;\n"
        "bt r8, 0x3f;\n"
        "cmovb rbx, rcx;\n"
        "mov r8, rax;\n"
        "sub rsi, rax;\n"
        "lea rcx, [rcx + rdi];\n"
        "cmovs r8, rbp;\n"
        "mov rdi, rbx;\n"
        "test rcx, rdx;\n"
        "cmove r8, rbp;\n"
        "cmove rdi, rbp;\n"
        "sar rcx, 1;\n"
        "xor rdi, r8;\n"
        "xor rsi, r8;\n"
        "bt r8, 0x3f;\n"
        "cmovb rbx, rcx;\n"
        "mov r8, rax;\n"
        "sub rsi, rax;\n"
        "lea rcx, [rcx + rdi];\n"
        "cmovs r8, rbp;\n"
        "mov rdi, rbx;\n"
        "test rcx, rdx;\n"
        "cmove r8, rbp;\n"
        "cmove rdi, rbp;\n"
        "sar rcx, 1;\n"
        "xor rdi, r8;\n"
        "xor rsi, r8;\n"
        "bt r8, 0x3f;\n"
        "cmovb rbx, rcx;\n"
        "mov r8, rax;\n"
        "sub rsi, rax;\n"
        "lea rcx, [rcx + rdi];\n"
        "cmovs r8, rbp;\n"
        "mov rdi, rbx;\n"
        "test rcx, rdx;\n"
        "cmove r8, rbp;\n"
        "cmove rdi, rbp;\n"
        "sar rcx, 1;\n"
        "xor rdi, r8;\n"
        "xor rsi, r8;\n"
        "bt r8, 0x3f;\n"
        "cmovb rbx, rcx;\n"
        "mov r8, rax;\n"
        "sub rsi, rax;\n"
        "lea rcx, [rcx + rdi];\n"
        "cmovs r8, rbp;\n"
        "mov rdi, rbx;\n"
        "test rcx, rdx;\n"
        "cmove r8, rbp;\n"
        "cmove rdi, rbp;\n"
        "sar rcx, 1;\n"
        "xor rdi, r8;\n"
        "xor rsi, r8;\n"
        "bt r8, 0x3f;\n"
        "cmovb rbx, rcx;\n"
        "mov r8, rax;\n"
        "sub rsi, rax;\n"
        "lea rcx, [rcx + rdi];\n"
        "cmovs r8, rbp;\n"
        "mov rdi, rbx;\n"
        "test rcx, rdx;\n"
        "cmove r8, rbp;\n"
        "cmove rdi, rbp;\n"
        "sar rcx, 1;\n"
        "xor rdi, r8;\n"
        "xor rsi, r8;\n"
        "bt r8, 0x3f;\n"
        "cmovb rbx, rcx;\n"
        "mov r8, rax;\n"
        "sub rsi, rax;\n"
        "lea rcx, [rcx + rdi];\n"
        "cmovs r8, rbp;\n"
        "mov rdi, rbx;\n"
        "test rcx, rdx;\n"
        "cmove r8, rbp;\n"
        "cmove rdi, rbp;\n"
        "sar rcx, 1;\n"
        "xor rdi, r8;\n"
        "xor rsi, r8;\n"
        "bt r8, 0x3f;\n"
        "cmovb rbx, rcx;\n"
        "mov r8, rax;\n"
        "sub rsi, rax;\n"
        "lea rcx, [rcx + rdi];\n"
        "cmovs r8, rbp;\n"
        "mov rdi, rbx;\n"
        "test rcx, rdx;\n"
        "cmove r8, rbp;\n"
        "cmove rdi, rbp;\n"
        "sar rcx, 1;\n"
        "xor rdi, r8;\n"
        "xor rsi, r8;\n"
        "bt r8, 0x3f;\n"
        "cmovb rbx, rcx;\n"
        "mov r8, rax;\n"
        "sub rsi, rax;\n"
        "lea rcx, [rcx + rdi];\n"
        "cmovs r8, rbp;\n"
        "mov rdi, rbx;\n"
        "test rcx, rdx;\n"
        "cmove r8, rbp;\n"
        "cmove rdi, rbp;\n"
        "sar rcx, 1;\n"
        "xor rdi, r8;\n"
        "xor rsi, r8;\n"
        "bt r8, 0x3f;\n"
        "cmovb rbx, rcx;\n"
        "mov r8, rax;\n"
        "sub rsi, rax;\n"
        "lea rcx, [rcx + rdi];\n"
        "sar rcx, 1;\n"
        "mov eax, 0x100000;\n"
        "lea r8, [rbx + rax];\n"
        "lea r12, [rcx + rax];\n"
        "shl r8, 0x15;\n"
        "shl r12, 0x15;\n"
        "sar r8, 0x2b;\n"
        "sar r12, 0x2b;\n"
        "movabs rax, 0x20000100000;\n"
        "lea r10, [rbx + rax];\n"
        "lea r14, [rcx + rax];\n"
        "sar r10, 0x2b;\n"
        "sar r14, 0x2b;\n"
        "mov rax, r9;\n"
        "imul rax, r8;\n"
        "mov rdx, r13;\n"
        "imul rdx, r10;\n"
        "imul r8, r15;\n"
        "imul r10, r11;\n"
        "add r10, r8;\n"
        "lea r8, [rax + rdx];\n"
        "mov rax, r9;\n"
        "imul rax, r12;\n"
        "mov rdx, r13;\n"
        "imul rdx, r14;\n"
        "imul r12, r15;\n"
        "imul r14, r11;\n"
        "add r14, r12;\n"
        "lea r12, [rax + rdx]"
    )}
}

/// Modular inverse modulo p_256 = 2^256 - 2^224 + 2^192 + 2^96 - 1
///
/// Input x[4]; output z[4]
///
/// If the 4-digit input x is coprime to p_256, i.e. is not divisible
/// by it, returns z < p_256 such that x * z == 1 (mod p_256). Note that
/// x does not need to be reduced modulo p_256, but the output always is.
/// If the input is divisible (i.e. is 0 or p_256), then there can be no
/// modular inverse and z = 0 is returned.
pub(crate) fn bignum_inv_p256(z: &mut [u64; 4], x: &[u64; 4]) {
    // SAFETY: inline assembly. see [crate::low::inline_assembly_safety] for safety info.
    unsafe {
        core::arch::asm!(

        Q!("    endbr64         " ),

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

        // Create constant [rdx;rcx;rbx;rax] = p_256 and copy it into the variable f
        // including the 5th zero digit

        Q!("    xor             " "ecx, ecx"),
        Q!("    mov             " "edx, 0x00000000ffffffff"),
        Q!("    mov             " "rbx, rdx"),
        Q!("    lea             " "rax, [rcx -1]"),
        Q!("    neg             " "rdx"),
        Q!("    mov             " "[rsp + " F!() "], rax"),
        Q!("    mov             " "[rsp + " F!() "+ 8], rbx"),
        Q!("    mov             " "[rsp + " F!() "+ 16], rcx"),
        Q!("    mov             " "[rsp + " F!() "+ 24], rdx"),
        Q!("    mov             " "[rsp + " F!() "+ 32], rcx"),

        // Now reduce the input modulo p_256, first negating the constant to get
        // [rdx;rcx;rbx;rax] = 2^256 - p_256, adding it to x and hence getting
        // the comparison x < p_256 <=> (2^256 - p_256) + x < 2^256 and choosing
        // g accordingly.

        Q!("    mov             " "r8, [rsi]"),
        Q!("    mov             " "r9, [rsi + 8]"),
        Q!("    mov             " "r10, [rsi + 16]"),
        Q!("    mov             " "r11, [rsi + 24]"),

        Q!("    lea             " "rax, [rcx + 1]"),
        Q!("    add             " "rax, r8"),
        Q!("    lea             " "rbx, [rdx -1]"),
        Q!("    adc             " "rbx, r9"),
        Q!("    not             " "rcx"),
        Q!("    adc             " "rcx, r10"),
        Q!("    not             " "rdx"),
        Q!("    adc             " "rdx, r11"),

        Q!("    cmovnc          " "rax, r8"),
        Q!("    cmovnc          " "rbx, r9"),
        Q!("    cmovnc          " "rcx, r10"),
        Q!("    cmovnc          " "rdx, r11"),

        Q!("    mov             " "[rsp + " G!() "], rax"),
        Q!("    mov             " "[rsp + " G!() "+ 8], rbx"),
        Q!("    mov             " "[rsp + " G!() "+ 16], rcx"),
        Q!("    mov             " "[rsp + " G!() "+ 24], rdx"),
        Q!("    xor             " "eax, eax"),
        Q!("    mov             " "[rsp + " G!() "+ 32], rax"),

        // Also maintain reduced < 2^256 vector [u,v] such that
        // [f,g] == x * 2^{5*i-50} * [u,v] (mod p_256)
        // starting with [p_256,x] == x * 2^{5*0-50} * [0,2^50] (mod p_256)
        // The weird-looking 5*i modifications come in because we are doing
        // 64-bit word-sized Montgomery reductions at each stage, which is
        // 5 bits more than the 59-bit requirement to keep things stable.

        Q!("    xor             " "eax, eax"),
        Q!("    mov             " "[rsp + " U!() "], rax"),
        Q!("    mov             " "[rsp + " U!() "+ 8], rax"),
        Q!("    mov             " "[rsp + " U!() "+ 16], rax"),
        Q!("    mov             " "[rsp + " U!() "+ 24], rax"),

        Q!("    mov             " "rcx, 0x0004000000000000"),
        Q!("    mov             " "[rsp + " V!() "], rcx"),
        Q!("    mov             " "[rsp + " V!() "+ 8], rax"),
        Q!("    mov             " "[rsp + " V!() "+ 16], rax"),
        Q!("    mov             " "[rsp + " V!() "+ 24], rax"),

        // Start of main loop. We jump into the middle so that the divstep
        // portion is common to the special tenth iteration after a uniform
        // first 9.

        Q!("    mov             " i!() ", 10"),
        Q!("    mov             " d!() ", 1"),
        Q!("    jmp             " Label!("Lbignum_inv_p256_midloop", 2, After)),

        Q!(Label!("Lbignum_inv_p256_loop", 3) ":"),

        // Separate out the matrix into sign-magnitude pairs

        Q!("    mov             " "r9, r8"),
        Q!("    sar             " "r9, 63"),
        Q!("    xor             " "r8, r9"),
        Q!("    sub             " "r8, r9"),

        Q!("    mov             " "r11, r10"),
        Q!("    sar             " "r11, 63"),
        Q!("    xor             " "r10, r11"),
        Q!("    sub             " "r10, r11"),

        Q!("    mov             " "r13, r12"),
        Q!("    sar             " "r13, 63"),
        Q!("    xor             " "r12, r13"),
        Q!("    sub             " "r12, r13"),

        Q!("    mov             " "r15, r14"),
        Q!("    sar             " "r15, 63"),
        Q!("    xor             " "r14, r15"),
        Q!("    sub             " "r14, r15"),

        // Adjust the initial values to allow for complement instead of negation
        // This initial offset is the same for [f,g] and [u,v] compositions.
        // Save it in temporary storage for the [u,v] part and do [f,g] first.

        Q!("    mov             " "rax, r8"),
        Q!("    and             " "rax, r9"),
        Q!("    mov             " "rdi, r10"),
        Q!("    and             " "rdi, r11"),
        Q!("    add             " "rdi, rax"),
        Q!("    mov             " tmp!() ", rdi"),

        Q!("    mov             " "rax, r12"),
        Q!("    and             " "rax, r13"),
        Q!("    mov             " "rsi, r14"),
        Q!("    and             " "rsi, r15"),
        Q!("    add             " "rsi, rax"),
        Q!("    mov             " tmp2!() ", rsi"),

        // Now the computation of the updated f and g values. This maintains a
        // 2-word carry between stages so we can conveniently insert the shift
        // right by 59 before storing back, and not overwrite digits we need
        // again of the old f and g values.
        //
        // Digit 0 of [f,g]

        Q!("    xor             " "ebx, ebx"),
        Q!("    mov             " "rax, [rsp + " F!() "]"),
        Q!("    xor             " "rax, r9"),
        Q!("    mul             " "r8"),
        Q!("    add             " "rdi, rax"),
        Q!("    adc             " "rbx, rdx"),
        Q!("    mov             " "rax, [rsp + " G!() "]"),
        Q!("    xor             " "rax, r11"),
        Q!("    mul             " "r10"),
        Q!("    add             " "rdi, rax"),
        Q!("    adc             " "rbx, rdx"),

        Q!("    xor             " "ebp, ebp"),
        Q!("    mov             " "rax, [rsp + " F!() "]"),
        Q!("    xor             " "rax, r13"),
        Q!("    mul             " "r12"),
        Q!("    add             " "rsi, rax"),
        Q!("    adc             " "rbp, rdx"),
        Q!("    mov             " "rax, [rsp + " G!() "]"),
        Q!("    xor             " "rax, r15"),
        Q!("    mul             " "r14"),
        Q!("    add             " "rsi, rax"),
        Q!("    adc             " "rbp, rdx"),

        // Digit 1 of [f,g]

        Q!("    xor             " "ecx, ecx"),
        Q!("    mov             " "rax, [rsp + " F!() "+ " N!() "]"),
        Q!("    xor             " "rax, r9"),
        Q!("    mul             " "r8"),
        Q!("    add             " "rbx, rax"),
        Q!("    adc             " "rcx, rdx"),
        Q!("    mov             " "rax, [rsp + " G!() "+ " N!() "]"),
        Q!("    xor             " "rax, r11"),
        Q!("    mul             " "r10"),
        Q!("    add             " "rbx, rax"),
        Q!("    adc             " "rcx, rdx"),
        Q!("    shrd            " "rdi, rbx, 59"),
        Q!("    mov             " "[rsp + " F!() "], rdi"),

        Q!("    xor             " "edi, edi"),
        Q!("    mov             " "rax, [rsp + " F!() "+ " N!() "]"),
        Q!("    xor             " "rax, r13"),
        Q!("    mul             " "r12"),
        Q!("    add             " "rbp, rax"),
        Q!("    adc             " "rdi, rdx"),
        Q!("    mov             " "rax, [rsp + " G!() "+ " N!() "]"),
        Q!("    xor             " "rax, r15"),
        Q!("    mul             " "r14"),
        Q!("    add             " "rbp, rax"),
        Q!("    adc             " "rdi, rdx"),
        Q!("    shrd            " "rsi, rbp, 59"),
        Q!("    mov             " "[rsp + " G!() "], rsi"),

        // Digit 2 of [f,g]

        Q!("    xor             " "esi, esi"),
        Q!("    mov             " "rax, [rsp + " F!() "+ 2 * " N!() "]"),
        Q!("    xor             " "rax, r9"),
        Q!("    mul             " "r8"),
        Q!("    add             " "rcx, rax"),
        Q!("    adc             " "rsi, rdx"),
        Q!("    mov             " "rax, [rsp + " G!() "+ 2 * " N!() "]"),
        Q!("    xor             " "rax, r11"),
        Q!("    mul             " "r10"),
        Q!("    add             " "rcx, rax"),
        Q!("    adc             " "rsi, rdx"),
        Q!("    shrd            " "rbx, rcx, 59"),
        Q!("    mov             " "[rsp + " F!() "+ " N!() "], rbx"),

        Q!("    xor             " "ebx, ebx"),
        Q!("    mov             " "rax, [rsp + " F!() "+ 2 * " N!() "]"),
        Q!("    xor             " "rax, r13"),
        Q!("    mul             " "r12"),
        Q!("    add             " "rdi, rax"),
        Q!("    adc             " "rbx, rdx"),
        Q!("    mov             " "rax, [rsp + " G!() "+ 2 * " N!() "]"),
        Q!("    xor             " "rax, r15"),
        Q!("    mul             " "r14"),
        Q!("    add             " "rdi, rax"),
        Q!("    adc             " "rbx, rdx"),
        Q!("    shrd            " "rbp, rdi, 59"),
        Q!("    mov             " "[rsp + " G!() "+ " N!() "], rbp"),

        // Digits 3 and 4 of [f,g]

        Q!("    mov             " "rax, [rsp + " F!() "+ 3 * " N!() "]"),
        Q!("    xor             " "rax, r9"),
        Q!("    mov             " "rbp, [rsp + " F!() "+ 4 * " N!() "]"),
        Q!("    xor             " "rbp, r9"),
        Q!("    and             " "rbp, r8"),
        Q!("    neg             " "rbp"),
        Q!("    mul             " "r8"),
        Q!("    add             " "rsi, rax"),
        Q!("    adc             " "rbp, rdx"),
        Q!("    mov             " "rax, [rsp + " G!() "+ 3 * " N!() "]"),
        Q!("    xor             " "rax, r11"),
        Q!("    mov             " "rdx, [rsp + " G!() "+ 4 * " N!() "]"),
        Q!("    xor             " "rdx, r11"),
        Q!("    and             " "rdx, r10"),
        Q!("    sub             " "rbp, rdx"),
        Q!("    mul             " "r10"),
        Q!("    add             " "rsi, rax"),
        Q!("    adc             " "rbp, rdx"),
        Q!("    shrd            " "rcx, rsi, 59"),
        Q!("    mov             " "[rsp + " F!() "+ 2 * " N!() "], rcx"),
        Q!("    shrd            " "rsi, rbp, 59"),
        Q!("    sar             " "rbp, 59"),

        Q!("    mov             " "rax, [rsp + " F!() "+ 3 * " N!() "]"),
        Q!("    mov             " "[rsp + " F!() "+ 3 * " N!() "], rsi"),

        Q!("    mov             " "rsi, [rsp + " F!() "+ 4 * " N!() "]"),
        Q!("    mov             " "[rsp + " F!() "+ 4 * " N!() "], rbp"),

        Q!("    xor             " "rax, r13"),
        Q!("    xor             " "rsi, r13"),
        Q!("    and             " "rsi, r12"),
        Q!("    neg             " "rsi"),
        Q!("    mul             " "r12"),
        Q!("    add             " "rbx, rax"),
        Q!("    adc             " "rsi, rdx"),
        Q!("    mov             " "rax, [rsp + " G!() "+ 3 * " N!() "]"),
        Q!("    xor             " "rax, r15"),
        Q!("    mov             " "rdx, [rsp + " G!() "+ 4 * " N!() "]"),
        Q!("    xor             " "rdx, r15"),
        Q!("    and             " "rdx, r14"),
        Q!("    sub             " "rsi, rdx"),
        Q!("    mul             " "r14"),
        Q!("    add             " "rbx, rax"),
        Q!("    adc             " "rsi, rdx"),
        Q!("    shrd            " "rdi, rbx, 59"),
        Q!("    mov             " "[rsp + " G!() "+ 2 * " N!() "], rdi"),
        Q!("    shrd            " "rbx, rsi, 59"),
        Q!("    mov             " "[rsp + " G!() "+ 3 * " N!() "], rbx"),
        Q!("    sar             " "rsi, 59"),
        Q!("    mov             " "[rsp + " G!() "+ 4 * " N!() "], rsi"),

        // Get the initial carries back from storage and do the [u,v] accumulation

        Q!("    mov             " "rbx, " tmp!()),
        Q!("    mov             " "rbp, " tmp2!()),

        // Digit 0 of [u,v]

        Q!("    xor             " "ecx, ecx"),
        Q!("    mov             " "rax, [rsp + " U!() "]"),
        Q!("    xor             " "rax, r9"),
        Q!("    mul             " "r8"),
        Q!("    add             " "rbx, rax"),
        Q!("    adc             " "rcx, rdx"),
        Q!("    mov             " "rax, [rsp + " V!() "]"),
        Q!("    xor             " "rax, r11"),
        Q!("    mul             " "r10"),
        Q!("    add             " "rbx, rax"),
        Q!("    adc             " "rcx, rdx"),

        Q!("    xor             " "esi, esi"),
        Q!("    mov             " "rax, [rsp + " U!() "]"),
        Q!("    xor             " "rax, r13"),
        Q!("    mul             " "r12"),
        Q!("    mov             " "[rsp + " U!() "], rbx"),
        Q!("    add             " "rbp, rax"),
        Q!("    adc             " "rsi, rdx"),
        Q!("    mov             " "rax, [rsp + " V!() "]"),
        Q!("    xor             " "rax, r15"),
        Q!("    mul             " "r14"),
        Q!("    add             " "rbp, rax"),
        Q!("    adc             " "rsi, rdx"),
        Q!("    mov             " "[rsp + " V!() "], rbp"),

        // Digit 1 of [u,v]

        Q!("    xor             " "ebx, ebx"),
        Q!("    mov             " "rax, [rsp + " U!() "+ " N!() "]"),
        Q!("    xor             " "rax, r9"),
        Q!("    mul             " "r8"),
        Q!("    add             " "rcx, rax"),
        Q!("    adc             " "rbx, rdx"),
        Q!("    mov             " "rax, [rsp + " V!() "+ " N!() "]"),
        Q!("    xor             " "rax, r11"),
        Q!("    mul             " "r10"),
        Q!("    add             " "rcx, rax"),
        Q!("    adc             " "rbx, rdx"),

        Q!("    xor             " "ebp, ebp"),
        Q!("    mov             " "rax, [rsp + " U!() "+ " N!() "]"),
        Q!("    xor             " "rax, r13"),
        Q!("    mul             " "r12"),
        Q!("    mov             " "[rsp + " U!() "+ " N!() "], rcx"),
        Q!("    add             " "rsi, rax"),
        Q!("    adc             " "rbp, rdx"),
        Q!("    mov             " "rax, [rsp + " V!() "+ " N!() "]"),
        Q!("    xor             " "rax, r15"),
        Q!("    mul             " "r14"),
        Q!("    add             " "rsi, rax"),
        Q!("    adc             " "rbp, rdx"),
        Q!("    mov             " "[rsp + " V!() "+ " N!() "], rsi"),

        // Digit 2 of [u,v]

        Q!("    xor             " "ecx, ecx"),
        Q!("    mov             " "rax, [rsp + " U!() "+ 2 * " N!() "]"),
        Q!("    xor             " "rax, r9"),
        Q!("    mul             " "r8"),
        Q!("    add             " "rbx, rax"),
        Q!("    adc             " "rcx, rdx"),
        Q!("    mov             " "rax, [rsp + " V!() "+ 2 * " N!() "]"),
        Q!("    xor             " "rax, r11"),
        Q!("    mul             " "r10"),
        Q!("    add             " "rbx, rax"),
        Q!("    adc             " "rcx, rdx"),

        Q!("    xor             " "esi, esi"),
        Q!("    mov             " "rax, [rsp + " U!() "+ 2 * " N!() "]"),
        Q!("    xor             " "rax, r13"),
        Q!("    mul             " "r12"),
        Q!("    mov             " "[rsp + " U!() "+ 2 * " N!() "], rbx"),
        Q!("    add             " "rbp, rax"),
        Q!("    adc             " "rsi, rdx"),
        Q!("    mov             " "rax, [rsp + " V!() "+ 2 * " N!() "]"),
        Q!("    xor             " "rax, r15"),
        Q!("    mul             " "r14"),
        Q!("    add             " "rbp, rax"),
        Q!("    adc             " "rsi, rdx"),
        Q!("    mov             " "[rsp + " V!() "+ 2 * " N!() "], rbp"),

        // Digits 3 and 4 of u (top is unsigned)

        Q!("    mov             " "rax, [rsp + " U!() "+ 3 * " N!() "]"),
        Q!("    xor             " "rax, r9"),
        Q!("    mov             " "rbx, r9"),
        Q!("    and             " "rbx, r8"),
        Q!("    neg             " "rbx"),
        Q!("    mul             " "r8"),
        Q!("    add             " "rcx, rax"),
        Q!("    adc             " "rbx, rdx"),
        Q!("    mov             " "rax, [rsp + " V!() "+ 3 * " N!() "]"),
        Q!("    xor             " "rax, r11"),
        Q!("    mov             " "rdx, r11"),
        Q!("    and             " "rdx, r10"),
        Q!("    sub             " "rbx, rdx"),
        Q!("    mul             " "r10"),
        Q!("    add             " "rcx, rax"),
        Q!("    adc             " "rdx, rbx"),

        // Preload for last use of old u digit 3

        Q!("    mov             " "rax, [rsp + " U!() "+ 3 * " N!() "]"),
        Q!("    mov             " "[rsp + " U!() "+ 3 * " N!() "], rcx"),
        Q!("    mov             " "[rsp + " U!() "+ 4 * " N!() "], rdx"),

        // Digits 3 and 4 of v (top is unsigned)

        Q!("    xor             " "rax, r13"),
        Q!("    mov             " "rcx, r13"),
        Q!("    and             " "rcx, r12"),
        Q!("    neg             " "rcx"),
        Q!("    mul             " "r12"),
        Q!("    add             " "rsi, rax"),
        Q!("    adc             " "rcx, rdx"),
        Q!("    mov             " "rax, [rsp + " V!() "+ 3 * " N!() "]"),
        Q!("    xor             " "rax, r15"),
        Q!("    mov             " "rdx, r15"),
        Q!("    and             " "rdx, r14"),
        Q!("    sub             " "rcx, rdx"),
        Q!("    mul             " "r14"),
        Q!("    add             " "rsi, rax"),
        Q!("    adc             " "rdx, rcx"),
        Q!("    mov             " "[rsp + " V!() "+ 3 * " N!() "], rsi"),
        Q!("    mov             " "[rsp + " V!() "+ 4 * " N!() "], rdx"),

        // Montgomery reduction of u

        amontred!(u!()),

        // Montgomery reduction of v

        amontred!(v!()),

        Q!(Label!("Lbignum_inv_p256_midloop", 2) ":"),

        divstep59!(d!(), ff!(), gg!()),
        Q!("    mov             " d!() ", rsi"),

        // Next iteration

        Q!("    dec             " i!()),
        Q!("    jnz             " Label!("Lbignum_inv_p256_loop", 3, Before)),

        // The 10th and last iteration does not need anything except the
        // u value and the sign of f; the latter can be obtained from the
        // lowest word of f. So it's done differently from the main loop.
        // Find the sign of the new f. For this we just need one digit
        // since we know (for in-scope cases) that f is either +1 or -1.
        // We don't explicitly shift right by 59 either, but looking at
        // bit 63 (or any bit >= 60) of the unshifted result is enough
        // to distinguish -1 from +1; this is then made into a mask.

        Q!("    mov             " "rax, [rsp + " F!() "]"),
        Q!("    mov             " "rcx, [rsp + " G!() "]"),
        Q!("    imul            " "rax, r8"),
        Q!("    imul            " "rcx, r10"),
        Q!("    add             " "rax, rcx"),
        Q!("    sar             " "rax, 63"),

        // Now separate out the matrix into sign-magnitude pairs
        // and adjust each one based on the sign of f.
        //
        // Note that at this point we expect |f|=1 and we got its
        // sign above, so then since [f,0] == x * [u,v] (mod p_256)
        // we want to flip the sign of u according to that of f.

        Q!("    mov             " "r9, r8"),
        Q!("    sar             " "r9, 63"),
        Q!("    xor             " "r8, r9"),
        Q!("    sub             " "r8, r9"),
        Q!("    xor             " "r9, rax"),

        Q!("    mov             " "r11, r10"),
        Q!("    sar             " "r11, 63"),
        Q!("    xor             " "r10, r11"),
        Q!("    sub             " "r10, r11"),
        Q!("    xor             " "r11, rax"),

        Q!("    mov             " "r13, r12"),
        Q!("    sar             " "r13, 63"),
        Q!("    xor             " "r12, r13"),
        Q!("    sub             " "r12, r13"),
        Q!("    xor             " "r13, rax"),

        Q!("    mov             " "r15, r14"),
        Q!("    sar             " "r15, 63"),
        Q!("    xor             " "r14, r15"),
        Q!("    sub             " "r14, r15"),
        Q!("    xor             " "r15, rax"),

        // Adjust the initial value to allow for complement instead of negation

        Q!("    mov             " "rax, r8"),
        Q!("    and             " "rax, r9"),
        Q!("    mov             " "r12, r10"),
        Q!("    and             " "r12, r11"),
        Q!("    add             " "r12, rax"),

        // Digit 0 of [u]

        Q!("    xor             " "r13d, r13d"),
        Q!("    mov             " "rax, [rsp + " U!() "]"),
        Q!("    xor             " "rax, r9"),
        Q!("    mul             " "r8"),
        Q!("    add             " "r12, rax"),
        Q!("    adc             " "r13, rdx"),
        Q!("    mov             " "rax, [rsp + " V!() "]"),
        Q!("    xor             " "rax, r11"),
        Q!("    mul             " "r10"),
        Q!("    add             " "r12, rax"),
        Q!("    adc             " "r13, rdx"),

        // Digit 1 of [u]

        Q!("    xor             " "r14d, r14d"),
        Q!("    mov             " "rax, [rsp + " U!() "+ " N!() "]"),
        Q!("    xor             " "rax, r9"),
        Q!("    mul             " "r8"),
        Q!("    add             " "r13, rax"),
        Q!("    adc             " "r14, rdx"),
        Q!("    mov             " "rax, [rsp + " V!() "+ " N!() "]"),
        Q!("    xor             " "rax, r11"),
        Q!("    mul             " "r10"),
        Q!("    add             " "r13, rax"),
        Q!("    adc             " "r14, rdx"),

        // Digit 2 of [u]

        Q!("    xor             " "r15d, r15d"),
        Q!("    mov             " "rax, [rsp + " U!() "+ 2 * " N!() "]"),
        Q!("    xor             " "rax, r9"),
        Q!("    mul             " "r8"),
        Q!("    add             " "r14, rax"),
        Q!("    adc             " "r15, rdx"),
        Q!("    mov             " "rax, [rsp + " V!() "+ 2 * " N!() "]"),
        Q!("    xor             " "rax, r11"),
        Q!("    mul             " "r10"),
        Q!("    add             " "r14, rax"),
        Q!("    adc             " "r15, rdx"),

        // Digits 3 and 4 of u (top is unsigned)

        Q!("    mov             " "rax, [rsp + " U!() "+ 3 * " N!() "]"),
        Q!("    xor             " "rax, r9"),
        Q!("    and             " "r9, r8"),
        Q!("    neg             " "r9"),
        Q!("    mul             " "r8"),
        Q!("    add             " "r15, rax"),
        Q!("    adc             " "r9, rdx"),
        Q!("    mov             " "rax, [rsp + " V!() "+ 3 * " N!() "]"),
        Q!("    xor             " "rax, r11"),
        Q!("    mov             " "rdx, r11"),
        Q!("    and             " "rdx, r10"),
        Q!("    sub             " "r9, rdx"),
        Q!("    mul             " "r10"),
        Q!("    add             " "r15, rax"),
        Q!("    adc             " "r9, rdx"),

        // Store back and Montgomery reduce u

        Q!("    mov             " "[rsp + " U!() "], r12"),
        Q!("    mov             " "[rsp + " U!() "+ " N!() "], r13"),
        Q!("    mov             " "[rsp + " U!() "+ 2 * " N!() "], r14"),
        Q!("    mov             " "[rsp + " U!() "+ 3 * " N!() "], r15"),
        Q!("    mov             " "[rsp + " U!() "+ 4 * " N!() "], r9"),

        amontred!(u!()),

        // Perform final strict reduction mod p_256 and copy to output

        Q!("    mov             " "r8, [rsp + " U!() "]"),
        Q!("    mov             " "r9, [rsp + " U!() "+ " N!() "]"),
        Q!("    mov             " "r10, [rsp + " U!() "+ 2 * " N!() "]"),
        Q!("    mov             " "r11, [rsp + " U!() "+ 3 * " N!() "]"),

        Q!("    mov             " "eax, 1"),
        Q!("    mov             " "ebx, 0xffffffff"),
        Q!("    lea             " "rcx, [rax -2]"),
        Q!("    lea             " "rdx, [rbx -1]"),
        Q!("    not             " "rbx"),

        Q!("    add             " "rax, r8"),
        Q!("    adc             " "rbx, r9"),
        Q!("    adc             " "rcx, r10"),
        Q!("    adc             " "rdx, r11"),

        Q!("    cmovnc          " "rax, r8"),
        Q!("    cmovnc          " "rbx, r9"),
        Q!("    cmovnc          " "rcx, r10"),
        Q!("    cmovnc          " "rdx, r11"),

        Q!("    mov             " "rdi, " res!()),
        Q!("    mov             " "[rdi], rax"),
        Q!("    mov             " "[rdi + " N!() "], rbx"),
        Q!("    mov             " "[rdi + 2 * " N!() "], rcx"),
        Q!("    mov             " "[rdi + 3 * " N!() "], rdx"),

        // Restore stack and registers

        Q!("    add             " "rsp, " NSPACE!()),

        Q!("    pop             " "r15"),
        Q!("    pop             " "r14"),
        Q!("    pop             " "r13"),
        Q!("    pop             " "r12"),
        Q!("    pop             " "rbp"),
        Q!("    pop             " "rbx"),

        inout("rdi") z.as_mut_ptr() => _,
        inout("rsi") x.as_ptr() => _,
        // clobbers
        out("r10") _,
        out("r11") _,
        out("r12") _,
        out("r13") _,
        out("r14") _,
        out("r15") _,
        out("r8") _,
        out("r9") _,
        out("rax") _,
        out("rcx") _,
        out("rdx") _,
            )
    };
}
