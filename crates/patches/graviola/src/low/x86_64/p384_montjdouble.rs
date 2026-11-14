// generated source. do not edit.
#![allow(non_upper_case_globals, unused_macros, unused_imports)]
use crate::low::macros::*;

// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

// ----------------------------------------------------------------------------
// Point doubling on NIST curve P-384 in Montgomery-Jacobian coordinates
//
//    extern void p384_montjdouble(uint64_t p3[static 18],
//                                 const uint64_t p1[static 18]);
//
// Does p3 := 2 * p1 where all points are regarded as Jacobian triples with
// each coordinate in the Montgomery domain, i.e. x' = (2^384 * x) mod p_384.
// A Jacobian triple (x',y',z') represents affine point (x/z^2,y/z^3).
//
// Standard x86-64 ABI: RDI = p3, RSI = p1
// Microsoft x64 ABI:   RCX = p3, RDX = p1
// ----------------------------------------------------------------------------

// Size of individual field elements

macro_rules! NUMSIZE {
    () => {
        "48"
    };
}

// Pointer-offset pairs for inputs and outputs
// These assume rdi = p3, rsi = p1. The latter stays true
// but montsqr below modifies rdi as well. Thus, we need
// to save rdi and restore it before the writes to outputs.

macro_rules! x_1 {
    () => {
        "rsi + 0"
    };
}
macro_rules! y_1 { () => { Q!("rsi + " NUMSIZE!()) } }
macro_rules! z_1 { () => { Q!("rsi + (2 * " NUMSIZE!() ")") } }

macro_rules! x_3 {
    () => {
        "rdi + 0"
    };
}
macro_rules! y_3 { () => { Q!("rdi + " NUMSIZE!()) } }
macro_rules! z_3 { () => { Q!("rdi + (2 * " NUMSIZE!() ")") } }

// Pointer-offset pairs for temporaries, with some aliasing
// NSPACE is the total stack needed for these temporaries

macro_rules! z2 { () => { Q!("rsp + (" NUMSIZE!() "* 0)") } }
macro_rules! y2 { () => { Q!("rsp + (" NUMSIZE!() "* 1)") } }
macro_rules! x2p { () => { Q!("rsp + (" NUMSIZE!() "* 2)") } }
macro_rules! xy2 { () => { Q!("rsp + (" NUMSIZE!() "* 3)") } }

macro_rules! y4 { () => { Q!("rsp + (" NUMSIZE!() "* 4)") } }
macro_rules! t2 { () => { Q!("rsp + (" NUMSIZE!() "* 4)") } }

macro_rules! dx2 { () => { Q!("rsp + (" NUMSIZE!() "* 5)") } }
macro_rules! t1 { () => { Q!("rsp + (" NUMSIZE!() "* 5)") } }

macro_rules! d { () => { Q!("rsp + (" NUMSIZE!() "* 6)") } }
macro_rules! x4p { () => { Q!("rsp + (" NUMSIZE!() "* 6)") } }

// Safe place for pointer to the output

macro_rules! input_z { () => { Q!("[rsp + (" NUMSIZE!() "* 7)]") } }

macro_rules! NSPACE {
    () => {
        "344"
    };
}

// Corresponds exactly to bignum_montmul_p384

macro_rules! montmul_p384 {
    ($P0:expr, $P1:expr, $P2:expr) => { Q!(
        "mov rdx, [" $P2 "];\n"
        "xor r15d, r15d;\n"
        "mulx r9, r8, [" $P1 "];\n"
        "mulx r10, rbx, [" $P1 "+ 0x8];\n"
        "add r9, rbx;\n"
        "mulx r11, rbx, [" $P1 "+ 0x10];\n"
        "adc r10, rbx;\n"
        "mulx r12, rbx, [" $P1 "+ 0x18];\n"
        "adc r11, rbx;\n"
        "mulx r13, rbx, [" $P1 "+ 0x20];\n"
        "adc r12, rbx;\n"
        "mulx r14, rbx, [" $P1 "+ 0x28];\n"
        "adc r13, rbx;\n"
        "adc r14, r15;\n"
        "mov rdx, r8;\n"
        "shl rdx, 0x20;\n"
        "add rdx, r8;\n"
        "xor ebp, ebp;\n"
        "mov rax, 0xffffffff00000001;\n"
        "mulx rax, rbx, rax;\n"
        "mov ebx, 0xffffffff;\n"
        "mulx rbx, r8, rbx;\n"
        "adc rax, r8;\n"
        "adc rbx, rdx;\n"
        "adc ebp, ebp;\n"
        "sub r9, rax;\n"
        "sbb r10, rbx;\n"
        "sbb r11, rbp;\n"
        "sbb r12, 0x0;\n"
        "sbb r13, 0x0;\n"
        "sbb rdx, 0x0;\n"
        "add r14, rdx;\n"
        "adc r15, 0x0;\n"
        "mov rdx, [" $P2 "+ 0x8];\n"
        "xor r8d, r8d;\n"
        "mulx rbx, rax, [" $P1 "];\n"
        "adcx r9, rax;\n"
        "adox r10, rbx;\n"
        "mulx rbx, rax, [" $P1 "+ 0x8];\n"
        "adcx r10, rax;\n"
        "adox r11, rbx;\n"
        "mulx rbx, rax, [" $P1 "+ 0x10];\n"
        "adcx r11, rax;\n"
        "adox r12, rbx;\n"
        "mulx rbx, rax, [" $P1 "+ 0x18];\n"
        "adcx r12, rax;\n"
        "adox r13, rbx;\n"
        "mulx rbx, rax, [" $P1 "+ 0x20];\n"
        "adcx r13, rax;\n"
        "adox r14, rbx;\n"
        "adox r15, r8;\n"
        "mulx rbx, rax, [" $P1 "+ 0x28];\n"
        "adc r14, rax;\n"
        "adc r15, rbx;\n"
        "adc r8, r8;\n"
        "mov rdx, r9;\n"
        "shl rdx, 0x20;\n"
        "add rdx, r9;\n"
        "xor ebp, ebp;\n"
        "mov rax, 0xffffffff00000001;\n"
        "mulx rax, rbx, rax;\n"
        "mov ebx, 0xffffffff;\n"
        "mulx rbx, r9, rbx;\n"
        "adc rax, r9;\n"
        "adc rbx, rdx;\n"
        "adc ebp, ebp;\n"
        "sub r10, rax;\n"
        "sbb r11, rbx;\n"
        "sbb r12, rbp;\n"
        "sbb r13, 0x0;\n"
        "sbb r14, 0x0;\n"
        "sbb rdx, 0x0;\n"
        "add r15, rdx;\n"
        "adc r8, 0x0;\n"
        "mov rdx, [" $P2 "+ 0x10];\n"
        "xor r9d, r9d;\n"
        "mulx rbx, rax, [" $P1 "];\n"
        "adcx r10, rax;\n"
        "adox r11, rbx;\n"
        "mulx rbx, rax, [" $P1 "+ 0x8];\n"
        "adcx r11, rax;\n"
        "adox r12, rbx;\n"
        "mulx rbx, rax, [" $P1 "+ 0x10];\n"
        "adcx r12, rax;\n"
        "adox r13, rbx;\n"
        "mulx rbx, rax, [" $P1 "+ 0x18];\n"
        "adcx r13, rax;\n"
        "adox r14, rbx;\n"
        "mulx rbx, rax, [" $P1 "+ 0x20];\n"
        "adcx r14, rax;\n"
        "adox r15, rbx;\n"
        "adox r8, r9;\n"
        "mulx rbx, rax, [" $P1 "+ 0x28];\n"
        "adc r15, rax;\n"
        "adc r8, rbx;\n"
        "adc r9, r9;\n"
        "mov rdx, r10;\n"
        "shl rdx, 0x20;\n"
        "add rdx, r10;\n"
        "xor ebp, ebp;\n"
        "mov rax, 0xffffffff00000001;\n"
        "mulx rax, rbx, rax;\n"
        "mov ebx, 0xffffffff;\n"
        "mulx rbx, r10, rbx;\n"
        "adc rax, r10;\n"
        "adc rbx, rdx;\n"
        "adc ebp, ebp;\n"
        "sub r11, rax;\n"
        "sbb r12, rbx;\n"
        "sbb r13, rbp;\n"
        "sbb r14, 0x0;\n"
        "sbb r15, 0x0;\n"
        "sbb rdx, 0x0;\n"
        "add r8, rdx;\n"
        "adc r9, 0x0;\n"
        "mov rdx, [" $P2 "+ 0x18];\n"
        "xor r10d, r10d;\n"
        "mulx rbx, rax, [" $P1 "];\n"
        "adcx r11, rax;\n"
        "adox r12, rbx;\n"
        "mulx rbx, rax, [" $P1 "+ 0x8];\n"
        "adcx r12, rax;\n"
        "adox r13, rbx;\n"
        "mulx rbx, rax, [" $P1 "+ 0x10];\n"
        "adcx r13, rax;\n"
        "adox r14, rbx;\n"
        "mulx rbx, rax, [" $P1 "+ 0x18];\n"
        "adcx r14, rax;\n"
        "adox r15, rbx;\n"
        "mulx rbx, rax, [" $P1 "+ 0x20];\n"
        "adcx r15, rax;\n"
        "adox r8, rbx;\n"
        "adox r9, r10;\n"
        "mulx rbx, rax, [" $P1 "+ 0x28];\n"
        "adc r8, rax;\n"
        "adc r9, rbx;\n"
        "adc r10, r10;\n"
        "mov rdx, r11;\n"
        "shl rdx, 0x20;\n"
        "add rdx, r11;\n"
        "xor ebp, ebp;\n"
        "mov rax, 0xffffffff00000001;\n"
        "mulx rax, rbx, rax;\n"
        "mov ebx, 0xffffffff;\n"
        "mulx rbx, r11, rbx;\n"
        "adc rax, r11;\n"
        "adc rbx, rdx;\n"
        "adc ebp, ebp;\n"
        "sub r12, rax;\n"
        "sbb r13, rbx;\n"
        "sbb r14, rbp;\n"
        "sbb r15, 0x0;\n"
        "sbb r8, 0x0;\n"
        "sbb rdx, 0x0;\n"
        "add r9, rdx;\n"
        "adc r10, 0x0;\n"
        "mov rdx, [" $P2 "+ 0x20];\n"
        "xor r11d, r11d;\n"
        "mulx rbx, rax, [" $P1 "];\n"
        "adcx r12, rax;\n"
        "adox r13, rbx;\n"
        "mulx rbx, rax, [" $P1 "+ 0x8];\n"
        "adcx r13, rax;\n"
        "adox r14, rbx;\n"
        "mulx rbx, rax, [" $P1 "+ 0x10];\n"
        "adcx r14, rax;\n"
        "adox r15, rbx;\n"
        "mulx rbx, rax, [" $P1 "+ 0x18];\n"
        "adcx r15, rax;\n"
        "adox r8, rbx;\n"
        "mulx rbx, rax, [" $P1 "+ 0x20];\n"
        "adcx r8, rax;\n"
        "adox r9, rbx;\n"
        "adox r10, r11;\n"
        "mulx rbx, rax, [" $P1 "+ 0x28];\n"
        "adc r9, rax;\n"
        "adc r10, rbx;\n"
        "adc r11, r11;\n"
        "mov rdx, r12;\n"
        "shl rdx, 0x20;\n"
        "add rdx, r12;\n"
        "xor ebp, ebp;\n"
        "mov rax, 0xffffffff00000001;\n"
        "mulx rax, rbx, rax;\n"
        "mov ebx, 0xffffffff;\n"
        "mulx rbx, r12, rbx;\n"
        "adc rax, r12;\n"
        "adc rbx, rdx;\n"
        "adc ebp, ebp;\n"
        "sub r13, rax;\n"
        "sbb r14, rbx;\n"
        "sbb r15, rbp;\n"
        "sbb r8, 0x0;\n"
        "sbb r9, 0x0;\n"
        "sbb rdx, 0x0;\n"
        "add r10, rdx;\n"
        "adc r11, 0x0;\n"
        "mov rdx, [" $P2 "+ 0x28];\n"
        "xor r12d, r12d;\n"
        "mulx rbx, rax, [" $P1 "];\n"
        "adcx r13, rax;\n"
        "adox r14, rbx;\n"
        "mulx rbx, rax, [" $P1 "+ 0x8];\n"
        "adcx r14, rax;\n"
        "adox r15, rbx;\n"
        "mulx rbx, rax, [" $P1 "+ 0x10];\n"
        "adcx r15, rax;\n"
        "adox r8, rbx;\n"
        "mulx rbx, rax, [" $P1 "+ 0x18];\n"
        "adcx r8, rax;\n"
        "adox r9, rbx;\n"
        "mulx rbx, rax, [" $P1 "+ 0x20];\n"
        "adcx r9, rax;\n"
        "adox r10, rbx;\n"
        "adox r11, r12;\n"
        "mulx rbx, rax, [" $P1 "+ 0x28];\n"
        "adc r10, rax;\n"
        "adc r11, rbx;\n"
        "adc r12, r12;\n"
        "mov rdx, r13;\n"
        "shl rdx, 0x20;\n"
        "add rdx, r13;\n"
        "xor ebp, ebp;\n"
        "mov rax, 0xffffffff00000001;\n"
        "mulx rax, rbx, rax;\n"
        "mov ebx, 0xffffffff;\n"
        "mulx rbx, r13, rbx;\n"
        "adc rax, r13;\n"
        "adc rbx, rdx;\n"
        "adc ebp, ebp;\n"
        "sub r14, rax;\n"
        "sbb r15, rbx;\n"
        "sbb r8, rbp;\n"
        "sbb r9, 0x0;\n"
        "sbb r10, 0x0;\n"
        "sbb rdx, 0x0;\n"
        "add r11, rdx;\n"
        "adc r12, 0x0;\n"
        "xor edx, edx;\n"
        "xor ebp, ebp;\n"
        "xor r13d, r13d;\n"
        "mov rax, 0xffffffff00000001;\n"
        "add rax, r14;\n"
        "mov ebx, 0xffffffff;\n"
        "adc rbx, r15;\n"
        "mov ecx, 0x1;\n"
        "adc rcx, r8;\n"
        "adc rdx, r9;\n"
        "adc rbp, r10;\n"
        "adc r13, r11;\n"
        "adc r12, 0x0;\n"
        "cmovne r14, rax;\n"
        "cmovne r15, rbx;\n"
        "cmovne r8, rcx;\n"
        "cmovne r9, rdx;\n"
        "cmovne r10, rbp;\n"
        "cmovne r11, r13;\n"
        "mov [" $P0 "], r14;\n"
        "mov [" $P0 "+ 0x8], r15;\n"
        "mov [" $P0 "+ 0x10], r8;\n"
        "mov [" $P0 "+ 0x18], r9;\n"
        "mov [" $P0 "+ 0x20], r10;\n"
        "mov [" $P0 "+ 0x28], r11"
    )}
}

// Corresponds exactly to bignum_montsqr_p384

macro_rules! montsqr_p384 {
    ($P0:expr, $P1:expr) => { Q!(
        "mov rdx, [" $P1 "];\n"
        "mulx r10, r9, [" $P1 "+ 0x8];\n"
        "mulx r12, r11, [" $P1 "+ 0x18];\n"
        "mulx r14, r13, [" $P1 "+ 0x28];\n"
        "mov rdx, [" $P1 "+ 0x18];\n"
        "mulx rcx, r15, [" $P1 "+ 0x20];\n"
        "xor ebp, ebp;\n"
        "mov rdx, [" $P1 "+ 0x10];\n"
        "mulx rbx, rax, [" $P1 "];\n"
        "adcx r10, rax;\n"
        "adox r11, rbx;\n"
        "mulx rbx, rax, [" $P1 "+ 0x8];\n"
        "adcx r11, rax;\n"
        "adox r12, rbx;\n"
        "mov rdx, [" $P1 "+ 0x8];\n"
        "mulx rbx, rax, [" $P1 "+ 0x18];\n"
        "adcx r12, rax;\n"
        "adox r13, rbx;\n"
        "mulx rbx, rax, [" $P1 "+ 0x20];\n"
        "adcx r13, rax;\n"
        "adox r14, rbx;\n"
        "mulx rbx, rax, [" $P1 "+ 0x28];\n"
        "adcx r14, rax;\n"
        "adox r15, rbx;\n"
        "adcx r15, rbp;\n"
        "adox rcx, rbp;\n"
        "adc rcx, rbp;\n"
        "xor ebp, ebp;\n"
        "mov rdx, [" $P1 "+ 0x20];\n"
        "mulx rbx, rax, [" $P1 "];\n"
        "adcx r12, rax;\n"
        "adox r13, rbx;\n"
        "mov rdx, [" $P1 "+ 0x10];\n"
        "mulx rbx, rax, [" $P1 "+ 0x18];\n"
        "adcx r13, rax;\n"
        "adox r14, rbx;\n"
        "mulx rbx, rax, [" $P1 "+ 0x20];\n"
        "adcx r14, rax;\n"
        "adox r15, rbx;\n"
        "mulx rdx, rax, [" $P1 "+ 0x28];\n"
        "adcx r15, rax;\n"
        "adox rcx, rdx;\n"
        "mov rdx, [" $P1 "+ 0x28];\n"
        "mulx rbp, rbx, [" $P1 "+ 0x20];\n"
        "mulx rdx, rax, [" $P1 "+ 0x18];\n"
        "adcx rcx, rax;\n"
        "adox rbx, rdx;\n"
        "mov eax, 0x0;\n"
        "adcx rbx, rax;\n"
        "adox rbp, rax;\n"
        "adc rbp, rax;\n"
        "xor rax, rax;\n"
        "mov rdx, [" $P1 "];\n"
        "mulx rax, r8, [" $P1 "];\n"
        "adcx r9, r9;\n"
        "adox r9, rax;\n"
        "mov rdx, [" $P1 "+ 0x8];\n"
        "mulx rdx, rax, rdx;\n"
        "adcx r10, r10;\n"
        "adox r10, rax;\n"
        "adcx r11, r11;\n"
        "adox r11, rdx;\n"
        "mov rdx, [" $P1 "+ 0x10];\n"
        "mulx rdx, rax, rdx;\n"
        "adcx r12, r12;\n"
        "adox r12, rax;\n"
        "adcx r13, r13;\n"
        "adox r13, rdx;\n"
        "mov rdx, [" $P1 "+ 0x18];\n"
        "mulx rdx, rax, rdx;\n"
        "adcx r14, r14;\n"
        "adox r14, rax;\n"
        "adcx r15, r15;\n"
        "adox r15, rdx;\n"
        "mov rdx, [" $P1 "+ 0x20];\n"
        "mulx rdx, rax, rdx;\n"
        "adcx rcx, rcx;\n"
        "adox rcx, rax;\n"
        "adcx rbx, rbx;\n"
        "adox rbx, rdx;\n"
        "mov rdx, [" $P1 "+ 0x28];\n"
        "mulx rdi, rax, rdx;\n"
        "adcx rbp, rbp;\n"
        "adox rbp, rax;\n"
        "mov eax, 0x0;\n"
        "adcx rdi, rax;\n"
        "adox rdi, rax;\n"
        "mov [" $P0 "], rbx;\n"
        "mov rdx, r8;\n"
        "shl rdx, 0x20;\n"
        "add rdx, r8;\n"
        "mov rax, 0xffffffff00000001;\n"
        "mulx rax, r8, rax;\n"
        "mov ebx, 0xffffffff;\n"
        "mulx r8, rbx, rbx;\n"
        "add rax, rbx;\n"
        "adc r8, rdx;\n"
        "mov ebx, 0x0;\n"
        "adc rbx, rbx;\n"
        "sub r9, rax;\n"
        "sbb r10, r8;\n"
        "sbb r11, rbx;\n"
        "sbb r12, 0x0;\n"
        "sbb r13, 0x0;\n"
        "mov r8, rdx;\n"
        "sbb r8, 0x0;\n"
        "mov rdx, r9;\n"
        "shl rdx, 0x20;\n"
        "add rdx, r9;\n"
        "mov rax, 0xffffffff00000001;\n"
        "mulx rax, r9, rax;\n"
        "mov ebx, 0xffffffff;\n"
        "mulx r9, rbx, rbx;\n"
        "add rax, rbx;\n"
        "adc r9, rdx;\n"
        "mov ebx, 0x0;\n"
        "adc rbx, rbx;\n"
        "sub r10, rax;\n"
        "sbb r11, r9;\n"
        "sbb r12, rbx;\n"
        "sbb r13, 0x0;\n"
        "sbb r8, 0x0;\n"
        "mov r9, rdx;\n"
        "sbb r9, 0x0;\n"
        "mov rdx, r10;\n"
        "shl rdx, 0x20;\n"
        "add rdx, r10;\n"
        "mov rax, 0xffffffff00000001;\n"
        "mulx rax, r10, rax;\n"
        "mov ebx, 0xffffffff;\n"
        "mulx r10, rbx, rbx;\n"
        "add rax, rbx;\n"
        "adc r10, rdx;\n"
        "mov ebx, 0x0;\n"
        "adc rbx, rbx;\n"
        "sub r11, rax;\n"
        "sbb r12, r10;\n"
        "sbb r13, rbx;\n"
        "sbb r8, 0x0;\n"
        "sbb r9, 0x0;\n"
        "mov r10, rdx;\n"
        "sbb r10, 0x0;\n"
        "mov rdx, r11;\n"
        "shl rdx, 0x20;\n"
        "add rdx, r11;\n"
        "mov rax, 0xffffffff00000001;\n"
        "mulx rax, r11, rax;\n"
        "mov ebx, 0xffffffff;\n"
        "mulx r11, rbx, rbx;\n"
        "add rax, rbx;\n"
        "adc r11, rdx;\n"
        "mov ebx, 0x0;\n"
        "adc rbx, rbx;\n"
        "sub r12, rax;\n"
        "sbb r13, r11;\n"
        "sbb r8, rbx;\n"
        "sbb r9, 0x0;\n"
        "sbb r10, 0x0;\n"
        "mov r11, rdx;\n"
        "sbb r11, 0x0;\n"
        "mov rdx, r12;\n"
        "shl rdx, 0x20;\n"
        "add rdx, r12;\n"
        "mov rax, 0xffffffff00000001;\n"
        "mulx rax, r12, rax;\n"
        "mov ebx, 0xffffffff;\n"
        "mulx r12, rbx, rbx;\n"
        "add rax, rbx;\n"
        "adc r12, rdx;\n"
        "mov ebx, 0x0;\n"
        "adc rbx, rbx;\n"
        "sub r13, rax;\n"
        "sbb r8, r12;\n"
        "sbb r9, rbx;\n"
        "sbb r10, 0x0;\n"
        "sbb r11, 0x0;\n"
        "mov r12, rdx;\n"
        "sbb r12, 0x0;\n"
        "mov rdx, r13;\n"
        "shl rdx, 0x20;\n"
        "add rdx, r13;\n"
        "mov rax, 0xffffffff00000001;\n"
        "mulx rax, r13, rax;\n"
        "mov ebx, 0xffffffff;\n"
        "mulx r13, rbx, rbx;\n"
        "add rax, rbx;\n"
        "adc r13, rdx;\n"
        "mov ebx, 0x0;\n"
        "adc rbx, rbx;\n"
        "sub r8, rax;\n"
        "sbb r9, r13;\n"
        "sbb r10, rbx;\n"
        "sbb r11, 0x0;\n"
        "sbb r12, 0x0;\n"
        "mov r13, rdx;\n"
        "sbb r13, 0x0;\n"
        "mov rbx, [" $P0 "];\n"
        "add r14, r8;\n"
        "adc r15, r9;\n"
        "adc rcx, r10;\n"
        "adc rbx, r11;\n"
        "adc rbp, r12;\n"
        "adc rdi, r13;\n"
        "mov r8d, 0x0;\n"
        "adc r8, r8;\n"
        "xor r11, r11;\n"
        "xor r12, r12;\n"
        "xor r13, r13;\n"
        "mov rax, 0xffffffff00000001;\n"
        "add rax, r14;\n"
        "mov r9d, 0xffffffff;\n"
        "adc r9, r15;\n"
        "mov r10d, 0x1;\n"
        "adc r10, rcx;\n"
        "adc r11, rbx;\n"
        "adc r12, rbp;\n"
        "adc r13, rdi;\n"
        "adc r8, 0x0;\n"
        "cmovne r14, rax;\n"
        "cmovne r15, r9;\n"
        "cmovne rcx, r10;\n"
        "cmovne rbx, r11;\n"
        "cmovne rbp, r12;\n"
        "cmovne rdi, r13;\n"
        "mov [" $P0 "], r14;\n"
        "mov [" $P0 "+ 0x8], r15;\n"
        "mov [" $P0 "+ 0x10], rcx;\n"
        "mov [" $P0 "+ 0x18], rbx;\n"
        "mov [" $P0 "+ 0x20], rbp;\n"
        "mov [" $P0 "+ 0x28], rdi"
    )}
}

macro_rules! sub_p384 {
    ($P0:expr, $P1:expr, $P2:expr) => { Q!(
        "mov rax, [" $P1 "];\n"
        "sub rax, [" $P2 "];\n"
        "mov rdx, [" $P1 "+ 0x8];\n"
        "sbb rdx, [" $P2 "+ 0x8];\n"
        "mov r8, [" $P1 "+ 0x10];\n"
        "sbb r8, [" $P2 "+ 0x10];\n"
        "mov r9, [" $P1 "+ 0x18];\n"
        "sbb r9, [" $P2 "+ 0x18];\n"
        "mov r10, [" $P1 "+ 0x20];\n"
        "sbb r10, [" $P2 "+ 0x20];\n"
        "mov r11, [" $P1 "+ 0x28];\n"
        "sbb r11, [" $P2 "+ 0x28];\n"
        "sbb rcx, rcx;\n"
        "mov ebx, 0xffffffff;\n"
        "and rcx, rbx;\n"
        "xor rbx, rbx;\n"
        "sub rbx, rcx;\n"
        "sub rax, rbx;\n"
        "mov [" $P0 "], rax;\n"
        "sbb rdx, rcx;\n"
        "mov [" $P0 "+ 0x8], rdx;\n"
        "sbb rax, rax;\n"
        "and rcx, rbx;\n"
        "neg rax;\n"
        "sbb r8, rcx;\n"
        "mov [" $P0 "+ 0x10], r8;\n"
        "sbb r9, 0x0;\n"
        "mov [" $P0 "+ 0x18], r9;\n"
        "sbb r10, 0x0;\n"
        "mov [" $P0 "+ 0x20], r10;\n"
        "sbb r11, 0x0;\n"
        "mov [" $P0 "+ 0x28], r11"
    )}
}

// Simplified bignum_add_p384, without carry chain suspension

macro_rules! add_p384 {
    ($P0:expr, $P1:expr, $P2:expr) => { Q!(
        "mov rax, [" $P1 "];\n"
        "add rax, [" $P2 "];\n"
        "mov rcx, [" $P1 "+ 0x8];\n"
        "adc rcx, [" $P2 "+ 0x8];\n"
        "mov r8, [" $P1 "+ 0x10];\n"
        "adc r8, [" $P2 "+ 0x10];\n"
        "mov r9, [" $P1 "+ 0x18];\n"
        "adc r9, [" $P2 "+ 0x18];\n"
        "mov r10, [" $P1 "+ 0x20];\n"
        "adc r10, [" $P2 "+ 0x20];\n"
        "mov r11, [" $P1 "+ 0x28];\n"
        "adc r11, [" $P2 "+ 0x28];\n"
        "mov edx, 0x0;\n"
        "adc rdx, rdx;\n"
        "mov rbp, 0xffffffff00000001;\n"
        "add rax, rbp;\n"
        "mov ebp, 0xffffffff;\n"
        "adc rcx, rbp;\n"
        "adc r8, 0x1;\n"
        "adc r9, 0x0;\n"
        "adc r10, 0x0;\n"
        "adc r11, 0x0;\n"
        "adc rdx, 0xffffffffffffffff;\n"
        "mov ebx, 1;\n"
        "and rbx, rdx;\n"
        "and rdx, rbp;\n"
        "xor rbp, rbp;\n"
        "sub rbp, rdx;\n"
        "sub rax, rbp;\n"
        "mov [" $P0 "], rax;\n"
        "sbb rcx, rdx;\n"
        "mov [" $P0 "+ 0x8], rcx;\n"
        "sbb r8, rbx;\n"
        "mov [" $P0 "+ 0x10], r8;\n"
        "sbb r9, 0x0;\n"
        "mov [" $P0 "+ 0x18], r9;\n"
        "sbb r10, 0x0;\n"
        "mov [" $P0 "+ 0x20], r10;\n"
        "sbb r11, 0x0;\n"
        "mov [" $P0 "+ 0x28], r11"
    )}
}

// P0 = 4 * P1 - P2

macro_rules! cmsub41_p384 {
    ($P0:expr, $P1:expr, $P2:expr) => { Q!(
        "mov rdx, [" $P1 "+ 40];\n"
        "mov r13, rdx;\n"
        "shr rdx, 62;\n"
        "mov r12, [" $P1 "+ 32];\n"
        "shld r13, r12, 2;\n"
        "mov r11, [" $P1 "+ 24];\n"
        "shld r12, r11, 2;\n"
        "mov r10, [" $P1 "+ 16];\n"
        "shld r11, r10, 2;\n"
        "mov r9, [" $P1 "+ 8];\n"
        "shld r10, r9, 2;\n"
        "mov r8, [" $P1 "];\n"
        "shld r9, r8, 2;\n"
        "shl r8, 2;\n"
        "add rdx, 1;\n"
        "sub r8, [" $P2 "];\n"
        "sbb r9, [" $P2 "+ 0x8];\n"
        "sbb r10, [" $P2 "+ 0x10];\n"
        "sbb r11, [" $P2 "+ 0x18];\n"
        "sbb r12, [" $P2 "+ 0x20];\n"
        "sbb r13, [" $P2 "+ 0x28];\n"
        "sbb rdx, 0;\n"
        "xor rcx, rcx;\n"
        "mov rax, 0xffffffff00000001;\n"
        "mulx rcx, rax, rax;\n"
        "adcx r8, rax;\n"
        "adox r9, rcx;\n"
        "mov eax, 0xffffffff;\n"
        "mulx rcx, rax, rax;\n"
        "adcx r9, rax;\n"
        "adox r10, rcx;\n"
        "adcx r10, rdx;\n"
        "mov eax, 0x0;\n"
        "mov ecx, 0x0;\n"
        "adox rax, rax;\n"
        "adc r11, rax;\n"
        "adc r12, rcx;\n"
        "adc r13, rcx;\n"
        "adc rcx, rcx;\n"
        "sub rcx, 0x1;\n"
        "mov edx, 0xffffffff;\n"
        "xor rax, rax;\n"
        "and rdx, rcx;\n"
        "sub rax, rdx;\n"
        "and rcx, 0x1;\n"
        "sub r8, rax;\n"
        "mov [" $P0 "], r8;\n"
        "sbb r9, rdx;\n"
        "mov [" $P0 "+ 0x8], r9;\n"
        "sbb r10, rcx;\n"
        "mov [" $P0 "+ 0x10], r10;\n"
        "sbb r11, 0x0;\n"
        "mov [" $P0 "+ 0x18], r11;\n"
        "sbb r12, 0x0;\n"
        "mov [" $P0 "+ 0x20], r12;\n"
        "sbb r13, 0x0;\n"
        "mov [" $P0 "+ 0x28], r13"
    )}
}

// P0 = C * P1 - D * P2

macro_rules! cmsub_p384 {
    ($P0:expr, $C:expr, $P1:expr, $D:expr, $P2:expr) => { Q!(
        "mov r8, 0x00000000ffffffff;\n"
        "sub r8, [" $P2 "];\n"
        "mov r9, 0xffffffff00000000;\n"
        "sbb r9, [" $P2 "+ 8];\n"
        "mov r10, 0xfffffffffffffffe;\n"
        "sbb r10, [" $P2 "+ 16];\n"
        "mov r11, 0xffffffffffffffff;\n"
        "sbb r11, [" $P2 "+ 24];\n"
        "mov r12, 0xffffffffffffffff;\n"
        "sbb r12, [" $P2 "+ 32];\n"
        "mov r13, 0xffffffffffffffff;\n"
        "sbb r13, [" $P2 "+ 40];\n"
        "mov rdx, " $D ";\n"
        "mulx rax, r8, r8;\n"
        "mulx rcx, r9, r9;\n"
        "add r9, rax;\n"
        "mulx rax, r10, r10;\n"
        "adc r10, rcx;\n"
        "mulx rcx, r11, r11;\n"
        "adc r11, rax;\n"
        "mulx rax, r12, r12;\n"
        "adc r12, rcx;\n"
        "mulx r14, r13, r13;\n"
        "adc r13, rax;\n"
        "adc r14, 1;\n"
        "xor ecx, ecx;\n"
        "mov rdx, " $C ";\n"
        "mulx rbx, rax, [" $P1 "];\n"
        "adcx r8, rax;\n"
        "adox r9, rbx;\n"
        "mulx rbx, rax, [" $P1 "+ 8];\n"
        "adcx r9, rax;\n"
        "adox r10, rbx;\n"
        "mulx rbx, rax, [" $P1 "+ 16];\n"
        "adcx r10, rax;\n"
        "adox r11, rbx;\n"
        "mulx rbx, rax, [" $P1 "+ 24];\n"
        "adcx r11, rax;\n"
        "adox r12, rbx;\n"
        "mulx rbx, rax, [" $P1 "+ 32];\n"
        "adcx r12, rax;\n"
        "adox r13, rbx;\n"
        "mulx rdx, rax, [" $P1 "+ 40];\n"
        "adcx r13, rax;\n"
        "adox rdx, r14;\n"
        "adcx rdx, rcx;\n"
        "xor rcx, rcx;\n"
        "mov rax, 0xffffffff00000001;\n"
        "mulx rcx, rax, rax;\n"
        "adcx r8, rax;\n"
        "adox r9, rcx;\n"
        "mov eax, 0xffffffff;\n"
        "mulx rcx, rax, rax;\n"
        "adcx r9, rax;\n"
        "adox r10, rcx;\n"
        "adcx r10, rdx;\n"
        "mov eax, 0x0;\n"
        "mov ecx, 0x0;\n"
        "adox rax, rax;\n"
        "adc r11, rax;\n"
        "adc r12, rcx;\n"
        "adc r13, rcx;\n"
        "adc rcx, rcx;\n"
        "sub rcx, 0x1;\n"
        "mov edx, 0xffffffff;\n"
        "xor rax, rax;\n"
        "and rdx, rcx;\n"
        "sub rax, rdx;\n"
        "and rcx, 0x1;\n"
        "sub r8, rax;\n"
        "mov [" $P0 "], r8;\n"
        "sbb r9, rdx;\n"
        "mov [" $P0 "+ 0x8], r9;\n"
        "sbb r10, rcx;\n"
        "mov [" $P0 "+ 0x10], r10;\n"
        "sbb r11, 0x0;\n"
        "mov [" $P0 "+ 0x18], r11;\n"
        "sbb r12, 0x0;\n"
        "mov [" $P0 "+ 0x20], r12;\n"
        "sbb r13, 0x0;\n"
        "mov [" $P0 "+ 0x28], r13"
    )}
}

// A weak version of add that only guarantees sum in 6 digits

macro_rules! weakadd_p384 {
    ($P0:expr, $P1:expr, $P2:expr) => { Q!(
        "mov rax, [" $P1 "];\n"
        "add rax, [" $P2 "];\n"
        "mov rcx, [" $P1 "+ 0x8];\n"
        "adc rcx, [" $P2 "+ 0x8];\n"
        "mov r8, [" $P1 "+ 0x10];\n"
        "adc r8, [" $P2 "+ 0x10];\n"
        "mov r9, [" $P1 "+ 0x18];\n"
        "adc r9, [" $P2 "+ 0x18];\n"
        "mov r10, [" $P1 "+ 0x20];\n"
        "adc r10, [" $P2 "+ 0x20];\n"
        "mov r11, [" $P1 "+ 0x28];\n"
        "adc r11, [" $P2 "+ 0x28];\n"
        "sbb rdx, rdx;\n"
        "mov ebx, 1;\n"
        "and rbx, rdx;\n"
        "mov ebp, 0xffffffff;\n"
        "and rdx, rbp;\n"
        "xor rbp, rbp;\n"
        "sub rbp, rdx;\n"
        "add rax, rbp;\n"
        "mov [" $P0 "], rax;\n"
        "adc rcx, rdx;\n"
        "mov [" $P0 "+ 0x8], rcx;\n"
        "adc r8, rbx;\n"
        "mov [" $P0 "+ 0x10], r8;\n"
        "adc r9, 0x0;\n"
        "mov [" $P0 "+ 0x18], r9;\n"
        "adc r10, 0x0;\n"
        "mov [" $P0 "+ 0x20], r10;\n"
        "adc r11, 0x0;\n"
        "mov [" $P0 "+ 0x28], r11"
    )}
}

// P0 = 3 * P1 - 8 * P2

macro_rules! cmsub38_p384 {
    ($P0:expr, $P1:expr, $P2:expr) => { Q!(
        "mov r8, 0x00000000ffffffff;\n"
        "sub r8, [" $P2 "];\n"
        "mov r9, 0xffffffff00000000;\n"
        "sbb r9, [" $P2 "+ 8];\n"
        "mov r10, 0xfffffffffffffffe;\n"
        "sbb r10, [" $P2 "+ 16];\n"
        "mov r11, 0xffffffffffffffff;\n"
        "sbb r11, [" $P2 "+ 24];\n"
        "mov r12, 0xffffffffffffffff;\n"
        "sbb r12, [" $P2 "+ 32];\n"
        "mov r13, 0xffffffffffffffff;\n"
        "sbb r13, [" $P2 "+ 40];\n"
        "mov r14, r13;\n"
        "shr r14, 61;\n"
        "shld r13, r12, 3;\n"
        "shld r12, r11, 3;\n"
        "shld r11, r10, 3;\n"
        "shld r10, r9, 3;\n"
        "shld r9, r8, 3;\n"
        "shl r8, 3;\n"
        "add r14, 1;\n"
        "xor ecx, ecx;\n"
        "mov rdx, 3;\n"
        "mulx rbx, rax, [" $P1 "];\n"
        "adcx r8, rax;\n"
        "adox r9, rbx;\n"
        "mulx rbx, rax, [" $P1 "+ 8];\n"
        "adcx r9, rax;\n"
        "adox r10, rbx;\n"
        "mulx rbx, rax, [" $P1 "+ 16];\n"
        "adcx r10, rax;\n"
        "adox r11, rbx;\n"
        "mulx rbx, rax, [" $P1 "+ 24];\n"
        "adcx r11, rax;\n"
        "adox r12, rbx;\n"
        "mulx rbx, rax, [" $P1 "+ 32];\n"
        "adcx r12, rax;\n"
        "adox r13, rbx;\n"
        "mulx rdx, rax, [" $P1 "+ 40];\n"
        "adcx r13, rax;\n"
        "adox rdx, r14;\n"
        "adcx rdx, rcx;\n"
        "xor rcx, rcx;\n"
        "mov rax, 0xffffffff00000001;\n"
        "mulx rcx, rax, rax;\n"
        "adcx r8, rax;\n"
        "adox r9, rcx;\n"
        "mov eax, 0xffffffff;\n"
        "mulx rcx, rax, rax;\n"
        "adcx r9, rax;\n"
        "adox r10, rcx;\n"
        "adcx r10, rdx;\n"
        "mov eax, 0x0;\n"
        "mov ecx, 0x0;\n"
        "adox rax, rax;\n"
        "adc r11, rax;\n"
        "adc r12, rcx;\n"
        "adc r13, rcx;\n"
        "adc rcx, rcx;\n"
        "sub rcx, 0x1;\n"
        "mov edx, 0xffffffff;\n"
        "xor rax, rax;\n"
        "and rdx, rcx;\n"
        "sub rax, rdx;\n"
        "and rcx, 0x1;\n"
        "sub r8, rax;\n"
        "mov [" $P0 "], r8;\n"
        "sbb r9, rdx;\n"
        "mov [" $P0 "+ 0x8], r9;\n"
        "sbb r10, rcx;\n"
        "mov [" $P0 "+ 0x10], r10;\n"
        "sbb r11, 0x0;\n"
        "mov [" $P0 "+ 0x18], r11;\n"
        "sbb r12, 0x0;\n"
        "mov [" $P0 "+ 0x20], r12;\n"
        "sbb r13, 0x0;\n"
        "mov [" $P0 "+ 0x28], r13"
    )}
}

/// Point doubling on NIST curve P-384 in Montgomery-Jacobian coordinates
///
///
/// Does p3 := 2 * p1 where all points are regarded as Jacobian triples with
/// each coordinate in the Montgomery domain, i.e. x' = (2^384 * x) mod p_384.
/// A Jacobian triple (x',y',z') represents affine point (x/z^2,y/z^3).
pub(crate) fn p384_montjdouble(p3: &mut [u64; 18], p1: &[u64; 18]) {
    // SAFETY: inline assembly. see [crate::low::inline_assembly_safety] for safety info.
    unsafe {
        core::arch::asm!(

        Q!("    endbr64         " ),

        // Save registers and make room on stack for temporary variables
        // Save the output pointer rdi which gets overwritten in earlier
        // operations before it is used.

        Q!("    push            " "rbx"),
        Q!("    push            " "rbp"),
        Q!("    push            " "r12"),
        Q!("    push            " "r13"),
        Q!("    push            " "r14"),
        Q!("    push            " "r15"),

        Q!("    sub             " "rsp, " NSPACE!()),

        Q!("    mov             " input_z!() ", rdi"),

        // Main code, just a sequence of basic field operations

        // z2 = z^2
        // y2 = y^2

        montsqr_p384!(z2!(), z_1!()),
        montsqr_p384!(y2!(), y_1!()),

        // x2p = x^2 - z^4 = (x + z^2) * (x - z^2)

        weakadd_p384!(t1!(), x_1!(), z2!()),
        sub_p384!(t2!(), x_1!(), z2!()),
        montmul_p384!(x2p!(), t1!(), t2!()),

        // t1 = y + z
        // x4p = x2p^2
        // xy2 = x * y^2

        add_p384!(t1!(), y_1!(), z_1!()),
        montsqr_p384!(x4p!(), x2p!()),
        montmul_p384!(xy2!(), x_1!(), y2!()),

        // t2 = (y + z)^2

        montsqr_p384!(t2!(), t1!()),

        // d = 12 * xy2 - 9 * x4p
        // t1 = y^2 + 2 * y * z

        cmsub_p384!(d!(), "12", xy2!(), "9", x4p!()),
        sub_p384!(t1!(), t2!(), z2!()),

        // y4 = y^4

        montsqr_p384!(y4!(), y2!()),

        // Restore the output pointer to write to x_3, y_3 and z_3.

        Q!("    mov             " "rdi, " input_z!()),

        // z_3' = 2 * y * z
        // dx2 = d * x2p

        sub_p384!(z_3!(), t1!(), y2!()),
        montmul_p384!(dx2!(), d!(), x2p!()),

        // x' = 4 * xy2 - d

        cmsub41_p384!(x_3!(), xy2!(), d!()),

        // y' = 3 * dx2 - 8 * y4

        cmsub38_p384!(y_3!(), dx2!(), y4!()),

        // Restore stack and registers

        Q!("    add             " "rsp, " NSPACE!()),
        Q!("    pop             " "r15"),
        Q!("    pop             " "r14"),
        Q!("    pop             " "r13"),
        Q!("    pop             " "r12"),
        Q!("    pop             " "rbp"),
        Q!("    pop             " "rbx"),

        inout("rdi") p3.as_mut_ptr() => _,
        inout("rsi") p1.as_ptr() => _,
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
