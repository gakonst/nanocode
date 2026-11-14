// generated source. do not edit.
#![allow(non_upper_case_globals, unused_macros, unused_imports)]
use crate::low::macros::*;

// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

// ----------------------------------------------------------------------------
// The x25519 function for curve25519
// Inputs scalar[4], point[4]; output res[4]
//
// extern void curve25519_x25519
//   (uint64_t res[static 4],const uint64_t scalar[static 4],
//    const uint64_t point[static 4]);
//
// The function has a second prototype considering the arguments as arrays
// of bytes rather than 64-bit words. The underlying code is the same, since
// the x86 platform is little-endian.
//
// extern void curve25519_x25519_byte
//   (uint8_t res[static 32],const uint8_t scalar[static 32],
//    const uint8_t point[static 32]);
//
// Given a scalar n and the X coordinate of an input point P = (X,Y) on
// curve25519 (Y can live in any extension field of characteristic 2^255-19),
// this returns the X coordinate of n * P = (X, Y), or 0 when n * P is the
// point at infinity. Both n and X inputs are first slightly modified/mangled
// as specified in the relevant RFC (https://www.rfc-editor.org/rfc/rfc7748);
// in particular the lower three bits of n are set to zero. Does not implement
// the zero-check specified in Section 6.1.
//
// Standard x86-64 ABI: RDI = res, RSI = scalar, RDX = point
// Microsoft x64 ABI:   RCX = res, RDX = scalar, R8 = point
// ----------------------------------------------------------------------------

// Size of individual field elements

macro_rules! NUMSIZE {
    () => {
        "32"
    };
}

// Stable homes for the input result argument during the whole body
// and other variables that are only needed prior to the modular inverse.

macro_rules! res { () => { Q!("QWORD PTR [rsp + 12 * " NUMSIZE!() "]") } }
macro_rules! i { () => { Q!("QWORD PTR [rsp + 12 * " NUMSIZE!() "+ 8]") } }
macro_rules! swap { () => { Q!("QWORD PTR [rsp + 12 * " NUMSIZE!() "+ 16]") } }

// Pointers to result x coord to be written, assuming the base "res"
// has been loaded into rbp

macro_rules! resx {
    () => {
        "rbp + 0"
    };
}

// Pointer-offset pairs for temporaries on stack with some aliasing.
// Both dmsn and dnsm need space for >= 5 digits, and we allocate 8

macro_rules! scalar { () => { Q!("rsp + (0 * " NUMSIZE!() ")") } }

macro_rules! pointx { () => { Q!("rsp + (1 * " NUMSIZE!() ")") } }

macro_rules! dm { () => { Q!("rsp + (2 * " NUMSIZE!() ")") } }

macro_rules! zm { () => { Q!("rsp + (3 * " NUMSIZE!() ")") } }
macro_rules! sm { () => { Q!("rsp + (3 * " NUMSIZE!() ")") } }
macro_rules! dpro { () => { Q!("rsp + (3 * " NUMSIZE!() ")") } }

macro_rules! sn { () => { Q!("rsp + (4 * " NUMSIZE!() ")") } }

macro_rules! dn { () => { Q!("rsp + (5 * " NUMSIZE!() ")") } }
macro_rules! e { () => { Q!("rsp + (5 * " NUMSIZE!() ")") } }

macro_rules! dmsn { () => { Q!("rsp + (6 * " NUMSIZE!() ")") } }
macro_rules! p { () => { Q!("rsp + (6 * " NUMSIZE!() ")") } }
macro_rules! zn { () => { Q!("rsp + (7 * " NUMSIZE!() ")") } }

macro_rules! xm { () => { Q!("rsp + (8 * " NUMSIZE!() ")") } }
macro_rules! dnsm { () => { Q!("rsp + (8 * " NUMSIZE!() ")") } }
macro_rules! spro { () => { Q!("rsp + (8 * " NUMSIZE!() ")") } }

macro_rules! xn { () => { Q!("rsp + (10 * " NUMSIZE!() ")") } }
macro_rules! s { () => { Q!("rsp + (10 * " NUMSIZE!() ")") } }

macro_rules! d { () => { Q!("rsp + (11 * " NUMSIZE!() ")") } }

// Total size to reserve on the stack
// This includes space for the 3 other variables above
// and rounds up to a multiple of 32

macro_rules! NSPACE { () => { Q!("13 * " NUMSIZE!()) } }

// Macro wrapping up the basic field operation bignum_mul_p25519, only
// trivially different from a pure function call to that subroutine.

macro_rules! mul_p25519 {
    ($P0:expr, $P1:expr, $P2:expr) => { Q!(
        "xor edi, edi;\n"
        "mov rdx, [" $P2 "];\n"
        "mulx r9, r8, [" $P1 "];\n"
        "mulx r10, rax, [" $P1 "+ 0x8];\n"
        "add r9, rax;\n"
        "mulx r11, rax, [" $P1 "+ 0x10];\n"
        "adc r10, rax;\n"
        "mulx r12, rax, [" $P1 "+ 0x18];\n"
        "adc r11, rax;\n"
        "adc r12, rdi;\n"
        "xor edi, edi;\n"
        "mov rdx, [" $P2 "+ 0x8];\n"
        "mulx rbx, rax, [" $P1 "];\n"
        "adcx r9, rax;\n"
        "adox r10, rbx;\n"
        "mulx rbx, rax, [" $P1 "+ 0x8];\n"
        "adcx r10, rax;\n"
        "adox r11, rbx;\n"
        "mulx rbx, rax, [" $P1 "+ 0x10];\n"
        "adcx r11, rax;\n"
        "adox r12, rbx;\n"
        "mulx r13, rax, [" $P1 "+ 0x18];\n"
        "adcx r12, rax;\n"
        "adox r13, rdi;\n"
        "adcx r13, rdi;\n"
        "xor edi, edi;\n"
        "mov rdx, [" $P2 "+ 0x10];\n"
        "mulx rbx, rax, [" $P1 "];\n"
        "adcx r10, rax;\n"
        "adox r11, rbx;\n"
        "mulx rbx, rax, [" $P1 "+ 0x8];\n"
        "adcx r11, rax;\n"
        "adox r12, rbx;\n"
        "mulx rbx, rax, [" $P1 "+ 0x10];\n"
        "adcx r12, rax;\n"
        "adox r13, rbx;\n"
        "mulx r14, rax, [" $P1 "+ 0x18];\n"
        "adcx r13, rax;\n"
        "adox r14, rdi;\n"
        "adcx r14, rdi;\n"
        "xor edi, edi;\n"
        "mov rdx, [" $P2 "+ 0x18];\n"
        "mulx rbx, rax, [" $P1 "];\n"
        "adcx r11, rax;\n"
        "adox r12, rbx;\n"
        "mulx rbx, rax, [" $P1 "+ 0x8];\n"
        "adcx r12, rax;\n"
        "adox r13, rbx;\n"
        "mulx rbx, rax, [" $P1 "+ 0x10];\n"
        "adcx r13, rax;\n"
        "adox r14, rbx;\n"
        "mulx r15, rax, [" $P1 "+ 0x18];\n"
        "adcx r14, rax;\n"
        "adox r15, rdi;\n"
        "adcx r15, rdi;\n"
        "mov edx, 0x26;\n"
        "xor edi, edi;\n"
        "mulx rbx, rax, r12;\n"
        "adcx r8, rax;\n"
        "adox r9, rbx;\n"
        "mulx rbx, rax, r13;\n"
        "adcx r9, rax;\n"
        "adox r10, rbx;\n"
        "mulx rbx, rax, r14;\n"
        "adcx r10, rax;\n"
        "adox r11, rbx;\n"
        "mulx r12, rax, r15;\n"
        "adcx r11, rax;\n"
        "adox r12, rdi;\n"
        "adcx r12, rdi;\n"
        "shld r12, r11, 0x1;\n"
        "mov edx, 0x13;\n"
        "inc r12;\n"
        "bts r11, 63;\n"
        "mulx rbx, rax, r12;\n"
        "add r8, rax;\n"
        "adc r9, rbx;\n"
        "adc r10, rdi;\n"
        "adc r11, rdi;\n"
        "sbb rax, rax;\n"
        "not rax;\n"
        "and rax, rdx;\n"
        "sub r8, rax;\n"
        "sbb r9, rdi;\n"
        "sbb r10, rdi;\n"
        "sbb r11, rdi;\n"
        "btr r11, 63;\n"
        "mov [" $P0 "], r8;\n"
        "mov [" $P0 "+ 0x8], r9;\n"
        "mov [" $P0 "+ 0x10], r10;\n"
        "mov [" $P0 "+ 0x18], r11"
    )}
}

// A version of multiplication that only guarantees output < 2 * p_25519.
// This basically skips the +1 and final correction in quotient estimation.

macro_rules! mul_4 {
    ($P0:expr, $P1:expr, $P2:expr) => { Q!(
        "xor ecx, ecx;\n"
        "mov rdx, [" $P2 "];\n"
        "mulx r9, r8, [" $P1 "];\n"
        "mulx r10, rax, [" $P1 "+ 0x8];\n"
        "add r9, rax;\n"
        "mulx r11, rax, [" $P1 "+ 0x10];\n"
        "adc r10, rax;\n"
        "mulx r12, rax, [" $P1 "+ 0x18];\n"
        "adc r11, rax;\n"
        "adc r12, rcx;\n"
        "xor ecx, ecx;\n"
        "mov rdx, [" $P2 "+ 0x8];\n"
        "mulx rbx, rax, [" $P1 "];\n"
        "adcx r9, rax;\n"
        "adox r10, rbx;\n"
        "mulx rbx, rax, [" $P1 "+ 0x8];\n"
        "adcx r10, rax;\n"
        "adox r11, rbx;\n"
        "mulx rbx, rax, [" $P1 "+ 0x10];\n"
        "adcx r11, rax;\n"
        "adox r12, rbx;\n"
        "mulx r13, rax, [" $P1 "+ 0x18];\n"
        "adcx r12, rax;\n"
        "adox r13, rcx;\n"
        "adcx r13, rcx;\n"
        "xor ecx, ecx;\n"
        "mov rdx, [" $P2 "+ 0x10];\n"
        "mulx rbx, rax, [" $P1 "];\n"
        "adcx r10, rax;\n"
        "adox r11, rbx;\n"
        "mulx rbx, rax, [" $P1 "+ 0x8];\n"
        "adcx r11, rax;\n"
        "adox r12, rbx;\n"
        "mulx rbx, rax, [" $P1 "+ 0x10];\n"
        "adcx r12, rax;\n"
        "adox r13, rbx;\n"
        "mulx r14, rax, [" $P1 "+ 0x18];\n"
        "adcx r13, rax;\n"
        "adox r14, rcx;\n"
        "adcx r14, rcx;\n"
        "xor ecx, ecx;\n"
        "mov rdx, [" $P2 "+ 0x18];\n"
        "mulx rbx, rax, [" $P1 "];\n"
        "adcx r11, rax;\n"
        "adox r12, rbx;\n"
        "mulx rbx, rax, [" $P1 "+ 0x8];\n"
        "adcx r12, rax;\n"
        "adox r13, rbx;\n"
        "mulx rbx, rax, [" $P1 "+ 0x10];\n"
        "adcx r13, rax;\n"
        "adox r14, rbx;\n"
        "mulx r15, rax, [" $P1 "+ 0x18];\n"
        "adcx r14, rax;\n"
        "adox r15, rcx;\n"
        "adcx r15, rcx;\n"
        "mov edx, 0x26;\n"
        "xor ecx, ecx;\n"
        "mulx rbx, rax, r12;\n"
        "adcx r8, rax;\n"
        "adox r9, rbx;\n"
        "mulx rbx, rax, r13;\n"
        "adcx r9, rax;\n"
        "adox r10, rbx;\n"
        "mulx rbx, rax, r14;\n"
        "adcx r10, rax;\n"
        "adox r11, rbx;\n"
        "mulx r12, rax, r15;\n"
        "adcx r11, rax;\n"
        "adox r12, rcx;\n"
        "adcx r12, rcx;\n"
        "shld r12, r11, 0x1;\n"
        "btr r11, 0x3f;\n"
        "mov edx, 0x13;\n"
        "imul rdx, r12;\n"
        "add r8, rdx;\n"
        "adc r9, rcx;\n"
        "adc r10, rcx;\n"
        "adc r11, rcx;\n"
        "mov [" $P0 "], r8;\n"
        "mov [" $P0 "+ 0x8], r9;\n"
        "mov [" $P0 "+ 0x10], r10;\n"
        "mov [" $P0 "+ 0x18], r11"
    )}
}

// Multiplication just giving a 5-digit result (actually < 39 * p_25519)
// by not doing anything beyond the first stage of reduction

macro_rules! mul_5 {
    ($P0:expr, $P1:expr, $P2:expr) => { Q!(
        "xor edi, edi;\n"
        "mov rdx, [" $P2 "];\n"
        "mulx r9, r8, [" $P1 "];\n"
        "mulx r10, rax, [" $P1 "+ 0x8];\n"
        "add r9, rax;\n"
        "mulx r11, rax, [" $P1 "+ 0x10];\n"
        "adc r10, rax;\n"
        "mulx r12, rax, [" $P1 "+ 0x18];\n"
        "adc r11, rax;\n"
        "adc r12, rdi;\n"
        "xor edi, edi;\n"
        "mov rdx, [" $P2 "+ 0x8];\n"
        "mulx rbx, rax, [" $P1 "];\n"
        "adcx r9, rax;\n"
        "adox r10, rbx;\n"
        "mulx rbx, rax, [" $P1 "+ 0x8];\n"
        "adcx r10, rax;\n"
        "adox r11, rbx;\n"
        "mulx rbx, rax, [" $P1 "+ 0x10];\n"
        "adcx r11, rax;\n"
        "adox r12, rbx;\n"
        "mulx r13, rax, [" $P1 "+ 0x18];\n"
        "adcx r12, rax;\n"
        "adox r13, rdi;\n"
        "adcx r13, rdi;\n"
        "xor edi, edi;\n"
        "mov rdx, [" $P2 "+ 0x10];\n"
        "mulx rbx, rax, [" $P1 "];\n"
        "adcx r10, rax;\n"
        "adox r11, rbx;\n"
        "mulx rbx, rax, [" $P1 "+ 0x8];\n"
        "adcx r11, rax;\n"
        "adox r12, rbx;\n"
        "mulx rbx, rax, [" $P1 "+ 0x10];\n"
        "adcx r12, rax;\n"
        "adox r13, rbx;\n"
        "mulx r14, rax, [" $P1 "+ 0x18];\n"
        "adcx r13, rax;\n"
        "adox r14, rdi;\n"
        "adcx r14, rdi;\n"
        "xor edi, edi;\n"
        "mov rdx, [" $P2 "+ 0x18];\n"
        "mulx rbx, rax, [" $P1 "];\n"
        "adcx r11, rax;\n"
        "adox r12, rbx;\n"
        "mulx rbx, rax, [" $P1 "+ 0x8];\n"
        "adcx r12, rax;\n"
        "adox r13, rbx;\n"
        "mulx rbx, rax, [" $P1 "+ 0x10];\n"
        "adcx r13, rax;\n"
        "adox r14, rbx;\n"
        "mulx r15, rax, [" $P1 "+ 0x18];\n"
        "adcx r14, rax;\n"
        "adox r15, rdi;\n"
        "adcx r15, rdi;\n"
        "mov edx, 0x26;\n"
        "xor edi, edi;\n"
        "mulx rbx, rax, r12;\n"
        "adcx r8, rax;\n"
        "adox r9, rbx;\n"
        "mulx rbx, rax, r13;\n"
        "adcx r9, rax;\n"
        "adox r10, rbx;\n"
        "mulx rbx, rax, r14;\n"
        "adcx r10, rax;\n"
        "adox r11, rbx;\n"
        "mulx r12, rax, r15;\n"
        "adcx r11, rax;\n"
        "adox r12, rdi;\n"
        "adcx r12, rdi;\n"
        "mov [" $P0 "], r8;\n"
        "mov [" $P0 "+ 0x8], r9;\n"
        "mov [" $P0 "+ 0x10], r10;\n"
        "mov [" $P0 "+ 0x18], r11;\n"
        "mov [" $P0 "+ 0x20], r12"
    )}
}

// Squaring just giving a result < 2 * p_25519, which is done by
// basically skipping the +1 in the quotient estimate and the final
// optional correction.

macro_rules! sqr_4 {
    ($P0:expr, $P1:expr) => { Q!(
        "mov rdx, [" $P1 "];\n"
        "mulx r15, r8, rdx;\n"
        "mulx r10, r9, [" $P1 "+ 0x8];\n"
        "mulx r12, r11, [" $P1 "+ 0x18];\n"
        "mov rdx, [" $P1 "+ 0x10];\n"
        "mulx r14, r13, [" $P1 "+ 0x18];\n"
        "xor ebx, ebx;\n"
        "mulx rcx, rax, [" $P1 "];\n"
        "adcx r10, rax;\n"
        "adox r11, rcx;\n"
        "mulx rcx, rax, [" $P1 "+ 0x8];\n"
        "adcx r11, rax;\n"
        "adox r12, rcx;\n"
        "mov rdx, [" $P1 "+ 0x18];\n"
        "mulx rcx, rax, [" $P1 "+ 0x8];\n"
        "adcx r12, rax;\n"
        "adox r13, rcx;\n"
        "adcx r13, rbx;\n"
        "adox r14, rbx;\n"
        "adc r14, rbx;\n"
        "xor ebx, ebx;\n"
        "adcx r9, r9;\n"
        "adox r9, r15;\n"
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
        "mulx r15, rax, rdx;\n"
        "adcx r14, r14;\n"
        "adox r14, rax;\n"
        "adcx r15, rbx;\n"
        "adox r15, rbx;\n"
        "mov edx, 0x26;\n"
        "xor ebx, ebx;\n"
        "mulx rcx, rax, r12;\n"
        "adcx r8, rax;\n"
        "adox r9, rcx;\n"
        "mulx rcx, rax, r13;\n"
        "adcx r9, rax;\n"
        "adox r10, rcx;\n"
        "mulx rcx, rax, r14;\n"
        "adcx r10, rax;\n"
        "adox r11, rcx;\n"
        "mulx r12, rax, r15;\n"
        "adcx r11, rax;\n"
        "adox r12, rbx;\n"
        "adcx r12, rbx;\n"
        "shld r12, r11, 0x1;\n"
        "btr r11, 0x3f;\n"
        "mov edx, 0x13;\n"
        "imul rdx, r12;\n"
        "add r8, rdx;\n"
        "adc r9, rbx;\n"
        "adc r10, rbx;\n"
        "adc r11, rbx;\n"
        "mov [" $P0 "], r8;\n"
        "mov [" $P0 "+ 0x8], r9;\n"
        "mov [" $P0 "+ 0x10], r10;\n"
        "mov [" $P0 "+ 0x18], r11"
    )}
}

// Add 5-digit inputs and normalize to 4 digits

macro_rules! add5_4 {
    ($P0:expr, $P1:expr, $P2:expr) => { Q!(
        "mov r8, [" $P1 "];\n"
        "add r8, [" $P2 "];\n"
        "mov r9, [" $P1 "+ 8];\n"
        "adc r9, [" $P2 "+ 8];\n"
        "mov r10, [" $P1 "+ 16];\n"
        "adc r10, [" $P2 "+ 16];\n"
        "mov r11, [" $P1 "+ 24];\n"
        "adc r11, [" $P2 "+ 24];\n"
        "mov r12, [" $P1 "+ 32];\n"
        "adc r12, [" $P2 "+ 32];\n"
        "xor ebx, ebx;\n"
        "shld r12, r11, 0x1;\n"
        "btr r11, 0x3f;\n"
        "mov edx, 0x13;\n"
        "imul rdx, r12;\n"
        "add r8, rdx;\n"
        "adc r9, rbx;\n"
        "adc r10, rbx;\n"
        "adc r11, rbx;\n"
        "mov [" $P0 "], r8;\n"
        "mov [" $P0 "+ 0x8], r9;\n"
        "mov [" $P0 "+ 0x10], r10;\n"
        "mov [" $P0 "+ 0x18], r11"
    )}
}

// Modular addition with double modulus 2 * p_25519 = 2^256 - 38.
// This only ensures that the result fits in 4 digits, not that it is reduced
// even w.r.t. double modulus. The result is always correct modulo provided
// the sum of the inputs is < 2^256 + 2^256 - 38, so in particular provided
// at least one of them is reduced double modulo.

macro_rules! add_twice4 {
    ($P0:expr, $P1:expr, $P2:expr) => { Q!(
        "mov r8, [" $P1 "];\n"
        "xor ecx, ecx;\n"
        "add r8, [" $P2 "];\n"
        "mov r9, [" $P1 "+ 0x8];\n"
        "adc r9, [" $P2 "+ 0x8];\n"
        "mov r10, [" $P1 "+ 0x10];\n"
        "adc r10, [" $P2 "+ 0x10];\n"
        "mov r11, [" $P1 "+ 0x18];\n"
        "adc r11, [" $P2 "+ 0x18];\n"
        "mov eax, 38;\n"
        "cmovnc rax, rcx;\n"
        "add r8, rax;\n"
        "adc r9, rcx;\n"
        "adc r10, rcx;\n"
        "adc r11, rcx;\n"
        "mov [" $P0 "], r8;\n"
        "mov [" $P0 "+ 0x8], r9;\n"
        "mov [" $P0 "+ 0x10], r10;\n"
        "mov [" $P0 "+ 0x18], r11"
    )}
}

// Modular subtraction with double modulus 2 * p_25519 = 2^256 - 38

macro_rules! sub_twice4 {
    ($P0:expr, $P1:expr, $P2:expr) => { Q!(
        "mov r8, [" $P1 "];\n"
        "xor ebx, ebx;\n"
        "sub r8, [" $P2 "];\n"
        "mov r9, [" $P1 "+ 8];\n"
        "sbb r9, [" $P2 "+ 8];\n"
        "mov ecx, 38;\n"
        "mov r10, [" $P1 "+ 16];\n"
        "sbb r10, [" $P2 "+ 16];\n"
        "mov rax, [" $P1 "+ 24];\n"
        "sbb rax, [" $P2 "+ 24];\n"
        "cmovnc rcx, rbx;\n"
        "sub r8, rcx;\n"
        "sbb r9, rbx;\n"
        "sbb r10, rbx;\n"
        "sbb rax, rbx;\n"
        "mov [" $P0 "], r8;\n"
        "mov [" $P0 "+ 8], r9;\n"
        "mov [" $P0 "+ 16], r10;\n"
        "mov [" $P0 "+ 24], rax"
    )}
}

// 5-digit subtraction with upward bias to make it positive, adding
// 1000 * (2^255 - 19) = 2^256 * 500 - 19000, then normalizing to 4 digits

macro_rules! sub5_4 {
    ($P0:expr, $P1:expr, $P2:expr) => { Q!(
        "mov r8, [" $P1 "];\n"
        "sub r8, [" $P2 "];\n"
        "mov r9, [" $P1 "+ 8];\n"
        "sbb r9, [" $P2 "+ 8];\n"
        "mov r10, [" $P1 "+ 16];\n"
        "sbb r10, [" $P2 "+ 16];\n"
        "mov r11, [" $P1 "+ 24];\n"
        "sbb r11, [" $P2 "+ 24];\n"
        "mov r12, [" $P1 "+ 32];\n"
        "sbb r12, [" $P2 "+ 32];\n"
        "xor ebx, ebx;\n"
        "sub r8, 19000;\n"
        "sbb r9, rbx;\n"
        "sbb r10, rbx;\n"
        "sbb r11, rbx;\n"
        "sbb r12, rbx;\n"
        "add r12, 500;\n"
        "shld r12, r11, 0x1;\n"
        "btr r11, 0x3f;\n"
        "mov edx, 0x13;\n"
        "imul rdx, r12;\n"
        "add r8, rdx;\n"
        "adc r9, rbx;\n"
        "adc r10, rbx;\n"
        "adc r11, rbx;\n"
        "mov [" $P0 "], r8;\n"
        "mov [" $P0 "+ 0x8], r9;\n"
        "mov [" $P0 "+ 0x10], r10;\n"
        "mov [" $P0 "+ 0x18], r11"
    )}
}

// Combined z = c * x + y with reduction only < 2 * p_25519
// It is assumed that 19 * (c * x + y) < 2^60 * 2^256 so we
// don't need a high mul in the final part.

macro_rules! cmadd_4 {
    ($P0:expr, $C1:expr, $P2:expr, $P3:expr) => { Q!(
        "mov r8, [" $P3 "];\n"
        "mov r9, [" $P3 "+ 8];\n"
        "mov r10, [" $P3 "+ 16];\n"
        "mov r11, [" $P3 "+ 24];\n"
        "xor edi, edi;\n"
        "mov rdx, " $C1 ";\n"
        "mulx rbx, rax, [" $P2 "];\n"
        "adcx r8, rax;\n"
        "adox r9, rbx;\n"
        "mulx rbx, rax, [" $P2 "+ 8];\n"
        "adcx r9, rax;\n"
        "adox r10, rbx;\n"
        "mulx rbx, rax, [" $P2 "+ 16];\n"
        "adcx r10, rax;\n"
        "adox r11, rbx;\n"
        "mulx rbx, rax, [" $P2 "+ 24];\n"
        "adcx r11, rax;\n"
        "adox rbx, rdi;\n"
        "adcx rbx, rdi;\n"
        "shld rbx, r11, 0x1;\n"
        "btr r11, 63;\n"
        "mov edx, 0x13;\n"
        "imul rbx, rdx;\n"
        "add r8, rbx;\n"
        "adc r9, rdi;\n"
        "adc r10, rdi;\n"
        "adc r11, rdi;\n"
        "mov [" $P0 "], r8;\n"
        "mov [" $P0 "+ 0x8], r9;\n"
        "mov [" $P0 "+ 0x10], r10;\n"
        "mov [" $P0 "+ 0x18], r11"
    )}
}

// Multiplex: z := if NZ then x else y

macro_rules! mux_4 {
    ($P0:expr, $P1:expr, $P2:expr) => { Q!(
        "mov rax, [" $P1 "];\n"
        "mov rcx, [" $P2 "];\n"
        "cmovz rax, rcx;\n"
        "mov [" $P0 "], rax;\n"
        "mov rax, [" $P1 "+ 8];\n"
        "mov rcx, [" $P2 "+ 8];\n"
        "cmovz rax, rcx;\n"
        "mov [" $P0 "+ 8], rax;\n"
        "mov rax, [" $P1 "+ 16];\n"
        "mov rcx, [" $P2 "+ 16];\n"
        "cmovz rax, rcx;\n"
        "mov [" $P0 "+ 16], rax;\n"
        "mov rax, [" $P1 "+ 24];\n"
        "mov rcx, [" $P2 "+ 24];\n"
        "cmovz rax, rcx;\n"
        "mov [" $P0 "+ 24], rax"
    )}
}

/// The x25519 function for curve25519
///
/// Inputs scalar[4], point[4]; output res[4]
///
/// The function has a second prototype considering the arguments as arrays
/// of bytes rather than 64-bit words. The underlying code is the same, since
/// the x86 platform is little-endian.
///
/// Given a scalar n and the X coordinate of an input point P = (X,Y) on
/// curve25519 (Y can live in any extension field of characteristic 2^255-19),
/// this returns the X coordinate of n * P = (X, Y), or 0 when n * P is the
/// point at infinity. Both n and X inputs are first slightly modified/mangled
/// as specified in the relevant RFC (https://www.rfc-editor.org/rfc/rfc7748);
/// in particular the lower three bits of n are set to zero. Does not implement
/// the zero-check specified in Section 6.1.
pub(crate) fn curve25519_x25519(res: &mut [u64; 4], scalar: &[u64; 4], point: &[u64; 4]) {
    // SAFETY: inline assembly. see [crate::low::inline_assembly_safety] for safety info.
    unsafe {
        core::arch::asm!(
        Q!(Label!("curve25519_x25519_byte", 2) ":"),

        Q!("    endbr64         " ),

        // Save registers, make room for temps, preserve input arguments.

        Q!("    push            " "rbx"),
        Q!("    push            " "rbp"),
        Q!("    push            " "r12"),
        Q!("    push            " "r13"),
        Q!("    push            " "r14"),
        Q!("    push            " "r15"),
        Q!("    sub             " "rsp, " NSPACE!()),

        // Move the output pointer to a stable place

        Q!("    mov             " res!() ", rdi"),

        // Copy the inputs to the local variables with minimal mangling:
        //
        //  - The scalar is in principle turned into 01xxx...xxx000 but
        //    in the structure below the special handling of these bits is
        //    explicit in the main computation; the scalar is just copied.
        //
        //  - The point x coord is reduced mod 2^255 by masking off the
        //    top bit. In the main loop we only need reduction < 2 * p_25519.

        Q!("    mov             " "rax, [rsi]"),
        Q!("    mov             " "[rsp], rax"),
        Q!("    mov             " "rax, [rsi + 8]"),
        Q!("    mov             " "[rsp + 8], rax"),
        Q!("    mov             " "rax, [rsi + 16]"),
        Q!("    mov             " "[rsp + 16], rax"),
        Q!("    mov             " "rax, [rsi + 24]"),
        Q!("    mov             " "[rsp + 24], rax"),

        Q!("    mov             " "r8, [rdx]"),
        Q!("    mov             " "r9, [rdx + 8]"),
        Q!("    mov             " "r10, [rdx + 16]"),
        Q!("    mov             " "r11, [rdx + 24]"),
        Q!("    btr             " "r11, 63"),
        Q!("    mov             " "[rsp + 32], r8"),
        Q!("    mov             " "[rsp + 40], r9"),
        Q!("    mov             " "[rsp + 48], r10"),
        Q!("    mov             " "[rsp + 56], r11"),

        // Initialize with explicit doubling in order to handle set bit 254.
        // Set swap = 1 and (xm,zm) = (x,1) then double as (xn,zn) = 2 * (x,1).
        // We use the fact that the point x coordinate is still in registers.
        // Since zm = 1 we could do the doubling with an operation count of
        // 2 * S + M instead of 2 * S + 2 * M, but it doesn't seem worth
        // the slight complication arising from a different linear combination.

        Q!("    mov             " "eax, 1"),
        Q!("    mov             " swap!() ", rax"),
        Q!("    mov             " "[rsp + 256], r8"),
        Q!("    mov             " "[rsp + 96], rax"),
        Q!("    xor             " "eax, eax"),
        Q!("    mov             " "[rsp + 264], r9"),
        Q!("    mov             " "[rsp + 104], rax"),
        Q!("    mov             " "[rsp + 272], r10"),
        Q!("    mov             " "[rsp + 112], rax"),
        Q!("    mov             " "[rsp + 280], r11"),
        Q!("    mov             " "[rsp + 120], rax"),

        sub_twice4!(d!(), xm!(), zm!()),
        add_twice4!(s!(), xm!(), zm!()),
        sqr_4!(d!(), d!()),
        sqr_4!(s!(), s!()),
        sub_twice4!(p!(), s!(), d!()),
        cmadd_4!(e!(), "0x1db42", p!(), d!()),
        mul_4!(xn!(), s!(), d!()),
        mul_4!(zn!(), p!(), e!()),

        // The main loop over unmodified bits from i = 253, ..., i = 3 (inclusive).
        // This is a classic Montgomery ladder, with the main coordinates only
        // reduced mod 2 * p_25519, some intermediate results even more loosely.

        Q!("    mov             " "eax, 253"),
        Q!("    mov             " i!() ", rax"),

        Q!(Label!("Lcurve25519_x25519_scalarloop", 3) ":"),

        // sm = xm + zm; sn = xn + zn; dm = xm - zm; dn = xn - zn

        sub_twice4!(dm!(), xm!(), zm!()),
        add_twice4!(sn!(), xn!(), zn!()),
        sub_twice4!(dn!(), xn!(), zn!()),
        add_twice4!(sm!(), xm!(), zm!()),

        // DOUBLING: mux d = xt - zt and s = xt + zt for appropriate choice of (xt,zt)

        Q!("    mov             " "rdx, " i!()),
        Q!("    mov             " "rcx, rdx"),
        Q!("    shr             " "rdx, 6"),
        Q!("    mov             " "rdx, [rsp + 8 * rdx]"),
        Q!("    shr             " "rdx, cl"),
        Q!("    and             " "rdx, 1"),
        Q!("    cmp             " "rdx, " swap!()),
        Q!("    mov             " swap!() ", rdx"),
        mux_4!(d!(), dm!(), dn!()),
        mux_4!(s!(), sm!(), sn!()),

        // ADDING: dmsn = dm * sn; dnsm = sm * dn

        mul_5!(dnsm!(), sm!(), dn!()),
        mul_5!(dmsn!(), sn!(), dm!()),

        // DOUBLING: d = (xt - zt)^2

        sqr_4!(d!(), d!()),

        // ADDING: dpro = (dmsn - dnsm)^2, spro = (dmsn + dnsm)^2
        // DOUBLING: s = (xt + zt)^2

        sub5_4!(dpro!(), dmsn!(), dnsm!()),
        add5_4!(spro!(), dmsn!(), dnsm!()),
        sqr_4!(s!(), s!()),
        sqr_4!(dpro!(), dpro!()),

        // DOUBLING: p = 4 * xt * zt = s - d

        sub_twice4!(p!(), s!(), d!()),

        // ADDING: xm' = (dmsn + dnsm)^2

        sqr_4!(xm!(), spro!()),

        // DOUBLING: e = 121666 * p + d

        cmadd_4!(e!(), "0x1db42", p!(), d!()),

        // DOUBLING: xn' = (xt + zt)^2 * (xt - zt)^2 = s * d

        mul_4!(xn!(), s!(), d!()),

        // DOUBLING: zn' = (4 * xt * zt) * ((xt - zt)^2 + 121666 * (4 * xt * zt))
        //               = p * (d + 121666 * p)

        mul_4!(zn!(), p!(), e!()),

        // ADDING: zm' = x * (dmsn - dnsm)^2

        mul_4!(zm!(), dpro!(), pointx!()),

        // Loop down as far as 3 (inclusive)

        Q!("    mov             " "rax, " i!()),
        Q!("    sub             " "rax, 1"),
        Q!("    mov             " i!() ", rax"),
        Q!("    cmp             " "rax, 3"),
        Q!("    jnc             " Label!("Lcurve25519_x25519_scalarloop", 3, Before)),

        // Multiplex directly into (xn,zn) then do three pure doubling steps;
        // this accounts for the implicit zeroing of the three lowest bits
        // of the scalar.

        Q!("    mov             " "rdx, " swap!()),
        Q!("    test            " "rdx, rdx"),
        mux_4!(xn!(), xm!(), xn!()),
        mux_4!(zn!(), zm!(), zn!()),

        sub_twice4!(d!(), xn!(), zn!()),
        add_twice4!(s!(), xn!(), zn!()),
        sqr_4!(d!(), d!()),
        sqr_4!(s!(), s!()),
        sub_twice4!(p!(), s!(), d!()),
        cmadd_4!(e!(), "0x1db42", p!(), d!()),
        mul_4!(xn!(), s!(), d!()),
        mul_4!(zn!(), p!(), e!()),

        sub_twice4!(d!(), xn!(), zn!()),
        add_twice4!(s!(), xn!(), zn!()),
        sqr_4!(d!(), d!()),
        sqr_4!(s!(), s!()),
        sub_twice4!(p!(), s!(), d!()),
        cmadd_4!(e!(), "0x1db42", p!(), d!()),
        mul_4!(xn!(), s!(), d!()),
        mul_4!(zn!(), p!(), e!()),

        sub_twice4!(d!(), xn!(), zn!()),
        add_twice4!(s!(), xn!(), zn!()),
        sqr_4!(d!(), d!()),
        sqr_4!(s!(), s!()),
        sub_twice4!(p!(), s!(), d!()),
        cmadd_4!(e!(), "0x1db42", p!(), d!()),
        mul_4!(xn!(), s!(), d!()),
        mul_4!(zn!(), p!(), e!()),

        // The projective result of the scalar multiplication is now (xn,zn).
        // Prepare to call the modular inverse function to get zn' = 1/zn

        Q!("    lea             " "rdi, [rsp + 224]"),
        Q!("    lea             " "rsi, [rsp + 224]"),

        // Inline copy of bignum_inv_p25519, identical except for stripping out
        // the prologue and epilogue saving and restoring registers and making
        // and reclaiming room on the stack. For more details and explanations see
        // "x86/curve25519/bignum_inv_p25519.S". Note that the stack it uses for
        // its own temporaries is 208 bytes, so it has no effect on variables
        // that are needed in the rest of our computation here: res, xn and zn.

        Q!("    mov             " "[rsp + 0xc0], rdi"),
        Q!("    xor             " "eax, eax"),
        Q!("    lea             " "rcx, [rax -0x13]"),
        Q!("    not             " "rax"),
        Q!("    mov             " "[rsp], rcx"),
        Q!("    mov             " "[rsp + 0x8], rax"),
        Q!("    mov             " "[rsp + 0x10], rax"),
        Q!("    btr             " "rax, 0x3f"),
        Q!("    mov             " "[rsp + 0x18], rax"),
        Q!("    mov             " "rdx, [rsi]"),
        Q!("    mov             " "rcx, [rsi + 0x8]"),
        Q!("    mov             " "r8, [rsi + 0x10]"),
        Q!("    mov             " "r9, [rsi + 0x18]"),
        Q!("    mov             " "eax, 0x1"),
        Q!("    xor             " "r10d, r10d"),
        Q!("    bts             " "r9, 0x3f"),
        Q!("    adc             " "rax, r10"),
        Q!("    imul            " "rax, rax, 0x13"),
        Q!("    add             " "rdx, rax"),
        Q!("    adc             " "rcx, r10"),
        Q!("    adc             " "r8, r10"),
        Q!("    adc             " "r9, r10"),
        Q!("    mov             " "eax, 0x13"),
        Q!("    cmovb           " "rax, r10"),
        Q!("    sub             " "rdx, rax"),
        Q!("    sbb             " "rcx, r10"),
        Q!("    sbb             " "r8, r10"),
        Q!("    sbb             " "r9, r10"),
        Q!("    btr             " "r9, 0x3f"),
        Q!("    mov             " "[rsp + 0x20], rdx"),
        Q!("    mov             " "[rsp + 0x28], rcx"),
        Q!("    mov             " "[rsp + 0x30], r8"),
        Q!("    mov             " "[rsp + 0x38], r9"),
        Q!("    xor             " "eax, eax"),
        Q!("    mov             " "[rsp + 0x40], rax"),
        Q!("    mov             " "[rsp + 0x48], rax"),
        Q!("    mov             " "[rsp + 0x50], rax"),
        Q!("    mov             " "[rsp + 0x58], rax"),
        Q!("    movabs          " "rax, 0xa0f99e2375022099"),
        Q!("    mov             " "[rsp + 0x60], rax"),
        Q!("    movabs          " "rax, 0xa8c68f3f1d132595"),
        Q!("    mov             " "[rsp + 0x68], rax"),
        Q!("    movabs          " "rax, 0x6c6c893805ac5242"),
        Q!("    mov             " "[rsp + 0x70], rax"),
        Q!("    movabs          " "rax, 0x276508b241770615"),
        Q!("    mov             " "[rsp + 0x78], rax"),
        Q!("    mov             " "QWORD PTR [rsp + 0x90], 0xa"),
        Q!("    mov             " "QWORD PTR [rsp + 0x98], 0x1"),
        Q!("    jmp             " Label!("Lcurve25519_x25519_midloop", 4, After)),
        Q!(Label!("Lcurve25519_x25519_inverseloop", 5) ":"),
        Q!("    mov             " "r9, r8"),
        Q!("    sar             " "r9, 0x3f"),
        Q!("    xor             " "r8, r9"),
        Q!("    sub             " "r8, r9"),
        Q!("    mov             " "r11, r10"),
        Q!("    sar             " "r11, 0x3f"),
        Q!("    xor             " "r10, r11"),
        Q!("    sub             " "r10, r11"),
        Q!("    mov             " "r13, r12"),
        Q!("    sar             " "r13, 0x3f"),
        Q!("    xor             " "r12, r13"),
        Q!("    sub             " "r12, r13"),
        Q!("    mov             " "r15, r14"),
        Q!("    sar             " "r15, 0x3f"),
        Q!("    xor             " "r14, r15"),
        Q!("    sub             " "r14, r15"),
        Q!("    mov             " "rax, r8"),
        Q!("    and             " "rax, r9"),
        Q!("    mov             " "rdi, r10"),
        Q!("    and             " "rdi, r11"),
        Q!("    add             " "rdi, rax"),
        Q!("    mov             " "[rsp + 0x80], rdi"),
        Q!("    mov             " "rax, r12"),
        Q!("    and             " "rax, r13"),
        Q!("    mov             " "rsi, r14"),
        Q!("    and             " "rsi, r15"),
        Q!("    add             " "rsi, rax"),
        Q!("    mov             " "[rsp + 0x88], rsi"),
        Q!("    xor             " "ebx, ebx"),
        Q!("    mov             " "rax, [rsp]"),
        Q!("    xor             " "rax, r9"),
        Q!("    mul             " "r8"),
        Q!("    add             " "rdi, rax"),
        Q!("    adc             " "rbx, rdx"),
        Q!("    mov             " "rax, [rsp + 0x20]"),
        Q!("    xor             " "rax, r11"),
        Q!("    mul             " "r10"),
        Q!("    add             " "rdi, rax"),
        Q!("    adc             " "rbx, rdx"),
        Q!("    xor             " "ebp, ebp"),
        Q!("    mov             " "rax, [rsp]"),
        Q!("    xor             " "rax, r13"),
        Q!("    mul             " "r12"),
        Q!("    add             " "rsi, rax"),
        Q!("    adc             " "rbp, rdx"),
        Q!("    mov             " "rax, [rsp + 0x20]"),
        Q!("    xor             " "rax, r15"),
        Q!("    mul             " "r14"),
        Q!("    add             " "rsi, rax"),
        Q!("    adc             " "rbp, rdx"),
        Q!("    xor             " "ecx, ecx"),
        Q!("    mov             " "rax, [rsp + 0x8]"),
        Q!("    xor             " "rax, r9"),
        Q!("    mul             " "r8"),
        Q!("    add             " "rbx, rax"),
        Q!("    adc             " "rcx, rdx"),
        Q!("    mov             " "rax, [rsp + 0x28]"),
        Q!("    xor             " "rax, r11"),
        Q!("    mul             " "r10"),
        Q!("    add             " "rbx, rax"),
        Q!("    adc             " "rcx, rdx"),
        Q!("    shrd            " "rdi, rbx, 0x3b"),
        Q!("    mov             " "[rsp], rdi"),
        Q!("    xor             " "edi, edi"),
        Q!("    mov             " "rax, [rsp + 0x8]"),
        Q!("    xor             " "rax, r13"),
        Q!("    mul             " "r12"),
        Q!("    add             " "rbp, rax"),
        Q!("    adc             " "rdi, rdx"),
        Q!("    mov             " "rax, [rsp + 0x28]"),
        Q!("    xor             " "rax, r15"),
        Q!("    mul             " "r14"),
        Q!("    add             " "rbp, rax"),
        Q!("    adc             " "rdi, rdx"),
        Q!("    shrd            " "rsi, rbp, 0x3b"),
        Q!("    mov             " "[rsp + 0x20], rsi"),
        Q!("    xor             " "esi, esi"),
        Q!("    mov             " "rax, [rsp + 0x10]"),
        Q!("    xor             " "rax, r9"),
        Q!("    mul             " "r8"),
        Q!("    add             " "rcx, rax"),
        Q!("    adc             " "rsi, rdx"),
        Q!("    mov             " "rax, [rsp + 0x30]"),
        Q!("    xor             " "rax, r11"),
        Q!("    mul             " "r10"),
        Q!("    add             " "rcx, rax"),
        Q!("    adc             " "rsi, rdx"),
        Q!("    shrd            " "rbx, rcx, 0x3b"),
        Q!("    mov             " "[rsp + 0x8], rbx"),
        Q!("    xor             " "ebx, ebx"),
        Q!("    mov             " "rax, [rsp + 0x10]"),
        Q!("    xor             " "rax, r13"),
        Q!("    mul             " "r12"),
        Q!("    add             " "rdi, rax"),
        Q!("    adc             " "rbx, rdx"),
        Q!("    mov             " "rax, [rsp + 0x30]"),
        Q!("    xor             " "rax, r15"),
        Q!("    mul             " "r14"),
        Q!("    add             " "rdi, rax"),
        Q!("    adc             " "rbx, rdx"),
        Q!("    shrd            " "rbp, rdi, 0x3b"),
        Q!("    mov             " "[rsp + 0x28], rbp"),
        Q!("    mov             " "rax, [rsp + 0x18]"),
        Q!("    xor             " "rax, r9"),
        Q!("    mov             " "rbp, rax"),
        Q!("    sar             " "rbp, 0x3f"),
        Q!("    and             " "rbp, r8"),
        Q!("    neg             " "rbp"),
        Q!("    mul             " "r8"),
        Q!("    add             " "rsi, rax"),
        Q!("    adc             " "rbp, rdx"),
        Q!("    mov             " "rax, [rsp + 0x38]"),
        Q!("    xor             " "rax, r11"),
        Q!("    mov             " "rdx, rax"),
        Q!("    sar             " "rdx, 0x3f"),
        Q!("    and             " "rdx, r10"),
        Q!("    sub             " "rbp, rdx"),
        Q!("    mul             " "r10"),
        Q!("    add             " "rsi, rax"),
        Q!("    adc             " "rbp, rdx"),
        Q!("    shrd            " "rcx, rsi, 0x3b"),
        Q!("    mov             " "[rsp + 0x10], rcx"),
        Q!("    shrd            " "rsi, rbp, 0x3b"),
        Q!("    mov             " "rax, [rsp + 0x18]"),
        Q!("    mov             " "[rsp + 0x18], rsi"),
        Q!("    xor             " "rax, r13"),
        Q!("    mov             " "rsi, rax"),
        Q!("    sar             " "rsi, 0x3f"),
        Q!("    and             " "rsi, r12"),
        Q!("    neg             " "rsi"),
        Q!("    mul             " "r12"),
        Q!("    add             " "rbx, rax"),
        Q!("    adc             " "rsi, rdx"),
        Q!("    mov             " "rax, [rsp + 0x38]"),
        Q!("    xor             " "rax, r15"),
        Q!("    mov             " "rdx, rax"),
        Q!("    sar             " "rdx, 0x3f"),
        Q!("    and             " "rdx, r14"),
        Q!("    sub             " "rsi, rdx"),
        Q!("    mul             " "r14"),
        Q!("    add             " "rbx, rax"),
        Q!("    adc             " "rsi, rdx"),
        Q!("    shrd            " "rdi, rbx, 0x3b"),
        Q!("    mov             " "[rsp + 0x30], rdi"),
        Q!("    shrd            " "rbx, rsi, 0x3b"),
        Q!("    mov             " "[rsp + 0x38], rbx"),
        Q!("    mov             " "rbx, [rsp + 0x80]"),
        Q!("    mov             " "rbp, [rsp + 0x88]"),
        Q!("    xor             " "ecx, ecx"),
        Q!("    mov             " "rax, [rsp + 0x40]"),
        Q!("    xor             " "rax, r9"),
        Q!("    mul             " "r8"),
        Q!("    add             " "rbx, rax"),
        Q!("    adc             " "rcx, rdx"),
        Q!("    mov             " "rax, [rsp + 0x60]"),
        Q!("    xor             " "rax, r11"),
        Q!("    mul             " "r10"),
        Q!("    add             " "rbx, rax"),
        Q!("    adc             " "rcx, rdx"),
        Q!("    xor             " "esi, esi"),
        Q!("    mov             " "rax, [rsp + 0x40]"),
        Q!("    xor             " "rax, r13"),
        Q!("    mul             " "r12"),
        Q!("    mov             " "[rsp + 0x40], rbx"),
        Q!("    add             " "rbp, rax"),
        Q!("    adc             " "rsi, rdx"),
        Q!("    mov             " "rax, [rsp + 0x60]"),
        Q!("    xor             " "rax, r15"),
        Q!("    mul             " "r14"),
        Q!("    add             " "rbp, rax"),
        Q!("    adc             " "rsi, rdx"),
        Q!("    mov             " "[rsp + 0x60], rbp"),
        Q!("    xor             " "ebx, ebx"),
        Q!("    mov             " "rax, [rsp + 0x48]"),
        Q!("    xor             " "rax, r9"),
        Q!("    mul             " "r8"),
        Q!("    add             " "rcx, rax"),
        Q!("    adc             " "rbx, rdx"),
        Q!("    mov             " "rax, [rsp + 0x68]"),
        Q!("    xor             " "rax, r11"),
        Q!("    mul             " "r10"),
        Q!("    add             " "rcx, rax"),
        Q!("    adc             " "rbx, rdx"),
        Q!("    xor             " "ebp, ebp"),
        Q!("    mov             " "rax, [rsp + 0x48]"),
        Q!("    xor             " "rax, r13"),
        Q!("    mul             " "r12"),
        Q!("    mov             " "[rsp + 0x48], rcx"),
        Q!("    add             " "rsi, rax"),
        Q!("    adc             " "rbp, rdx"),
        Q!("    mov             " "rax, [rsp + 0x68]"),
        Q!("    xor             " "rax, r15"),
        Q!("    mul             " "r14"),
        Q!("    add             " "rsi, rax"),
        Q!("    adc             " "rbp, rdx"),
        Q!("    mov             " "[rsp + 0x68], rsi"),
        Q!("    xor             " "ecx, ecx"),
        Q!("    mov             " "rax, [rsp + 0x50]"),
        Q!("    xor             " "rax, r9"),
        Q!("    mul             " "r8"),
        Q!("    add             " "rbx, rax"),
        Q!("    adc             " "rcx, rdx"),
        Q!("    mov             " "rax, [rsp + 0x70]"),
        Q!("    xor             " "rax, r11"),
        Q!("    mul             " "r10"),
        Q!("    add             " "rbx, rax"),
        Q!("    adc             " "rcx, rdx"),
        Q!("    xor             " "esi, esi"),
        Q!("    mov             " "rax, [rsp + 0x50]"),
        Q!("    xor             " "rax, r13"),
        Q!("    mul             " "r12"),
        Q!("    mov             " "[rsp + 0x50], rbx"),
        Q!("    add             " "rbp, rax"),
        Q!("    adc             " "rsi, rdx"),
        Q!("    mov             " "rax, [rsp + 0x70]"),
        Q!("    xor             " "rax, r15"),
        Q!("    mul             " "r14"),
        Q!("    add             " "rbp, rax"),
        Q!("    adc             " "rsi, rdx"),
        Q!("    mov             " "[rsp + 0x70], rbp"),
        Q!("    mov             " "rax, [rsp + 0x58]"),
        Q!("    xor             " "rax, r9"),
        Q!("    mov             " "rbx, r9"),
        Q!("    and             " "rbx, r8"),
        Q!("    neg             " "rbx"),
        Q!("    mul             " "r8"),
        Q!("    add             " "rcx, rax"),
        Q!("    adc             " "rbx, rdx"),
        Q!("    mov             " "rax, [rsp + 0x78]"),
        Q!("    xor             " "rax, r11"),
        Q!("    mov             " "rdx, r11"),
        Q!("    and             " "rdx, r10"),
        Q!("    sub             " "rbx, rdx"),
        Q!("    mul             " "r10"),
        Q!("    add             " "rcx, rax"),
        Q!("    adc             " "rdx, rbx"),
        Q!("    mov             " "rbx, rdx"),
        Q!("    shld            " "rdx, rcx, 0x1"),
        Q!("    sar             " "rbx, 0x3f"),
        Q!("    add             " "rdx, rbx"),
        Q!("    mov             " "eax, 0x13"),
        Q!("    imul            " "rdx"),
        Q!("    mov             " "r8, [rsp + 0x40]"),
        Q!("    add             " "r8, rax"),
        Q!("    mov             " "[rsp + 0x40], r8"),
        Q!("    mov             " "r8, [rsp + 0x48]"),
        Q!("    adc             " "r8, rdx"),
        Q!("    mov             " "[rsp + 0x48], r8"),
        Q!("    mov             " "r8, [rsp + 0x50]"),
        Q!("    adc             " "r8, rbx"),
        Q!("    mov             " "[rsp + 0x50], r8"),
        Q!("    adc             " "rcx, rbx"),
        Q!("    shl             " "rax, 0x3f"),
        Q!("    add             " "rcx, rax"),
        Q!("    mov             " "rax, [rsp + 0x58]"),
        Q!("    mov             " "[rsp + 0x58], rcx"),
        Q!("    xor             " "rax, r13"),
        Q!("    mov             " "rcx, r13"),
        Q!("    and             " "rcx, r12"),
        Q!("    neg             " "rcx"),
        Q!("    mul             " "r12"),
        Q!("    add             " "rsi, rax"),
        Q!("    adc             " "rcx, rdx"),
        Q!("    mov             " "rax, [rsp + 0x78]"),
        Q!("    xor             " "rax, r15"),
        Q!("    mov             " "rdx, r15"),
        Q!("    and             " "rdx, r14"),
        Q!("    sub             " "rcx, rdx"),
        Q!("    mul             " "r14"),
        Q!("    add             " "rsi, rax"),
        Q!("    adc             " "rdx, rcx"),
        Q!("    mov             " "rcx, rdx"),
        Q!("    shld            " "rdx, rsi, 0x1"),
        Q!("    sar             " "rcx, 0x3f"),
        Q!("    mov             " "eax, 0x13"),
        Q!("    add             " "rdx, rcx"),
        Q!("    imul            " "rdx"),
        Q!("    mov             " "r8, [rsp + 0x60]"),
        Q!("    add             " "r8, rax"),
        Q!("    mov             " "[rsp + 0x60], r8"),
        Q!("    mov             " "r8, [rsp + 0x68]"),
        Q!("    adc             " "r8, rdx"),
        Q!("    mov             " "[rsp + 0x68], r8"),
        Q!("    mov             " "r8, [rsp + 0x70]"),
        Q!("    adc             " "r8, rcx"),
        Q!("    mov             " "[rsp + 0x70], r8"),
        Q!("    adc             " "rsi, rcx"),
        Q!("    shl             " "rax, 0x3f"),
        Q!("    add             " "rsi, rax"),
        Q!("    mov             " "[rsp + 0x78], rsi"),
        Q!(Label!("Lcurve25519_x25519_midloop", 4) ":"),
        Q!("    mov             " "rsi, [rsp + 0x98]"),
        Q!("    mov             " "rdx, [rsp]"),
        Q!("    mov             " "rcx, [rsp + 0x20]"),
        Q!("    mov             " "rbx, rdx"),
        Q!("    and             " "rbx, 0xfffff"),
        Q!("    movabs          " "rax, 0xfffffe0000000000"),
        Q!("    or              " "rbx, rax"),
        Q!("    and             " "rcx, 0xfffff"),
        Q!("    movabs          " "rax, 0xc000000000000000"),
        Q!("    or              " "rcx, rax"),
        Q!("    mov             " "rax, 0xfffffffffffffffe"),
        Q!("    xor             " "ebp, ebp"),
        Q!("    mov             " "edx, 0x2"),
        Q!("    mov             " "rdi, rbx"),
        Q!("    mov             " "r8, rax"),
        Q!("    test            " "rsi, rsi"),
        Q!("    cmovs           " "r8, rbp"),
        Q!("    test            " "rcx, 0x1"),
        Q!("    cmove           " "r8, rbp"),
        Q!("    cmove           " "rdi, rbp"),
        Q!("    xor             " "rdi, r8"),
        Q!("    xor             " "rsi, r8"),
        Q!("    bt              " "r8, 0x3f"),
        Q!("    cmovb           " "rbx, rcx"),
        Q!("    mov             " "r8, rax"),
        Q!("    sub             " "rsi, rax"),
        Q!("    lea             " "rcx, [rcx + rdi]"),
        Q!("    cmovs           " "r8, rbp"),
        Q!("    mov             " "rdi, rbx"),
        Q!("    test            " "rcx, rdx"),
        Q!("    cmove           " "r8, rbp"),
        Q!("    cmove           " "rdi, rbp"),
        Q!("    sar             " "rcx, 1"),
        Q!("    xor             " "rdi, r8"),
        Q!("    xor             " "rsi, r8"),
        Q!("    bt              " "r8, 0x3f"),
        Q!("    cmovb           " "rbx, rcx"),
        Q!("    mov             " "r8, rax"),
        Q!("    sub             " "rsi, rax"),
        Q!("    lea             " "rcx, [rcx + rdi]"),
        Q!("    cmovs           " "r8, rbp"),
        Q!("    mov             " "rdi, rbx"),
        Q!("    test            " "rcx, rdx"),
        Q!("    cmove           " "r8, rbp"),
        Q!("    cmove           " "rdi, rbp"),
        Q!("    sar             " "rcx, 1"),
        Q!("    xor             " "rdi, r8"),
        Q!("    xor             " "rsi, r8"),
        Q!("    bt              " "r8, 0x3f"),
        Q!("    cmovb           " "rbx, rcx"),
        Q!("    mov             " "r8, rax"),
        Q!("    sub             " "rsi, rax"),
        Q!("    lea             " "rcx, [rcx + rdi]"),
        Q!("    cmovs           " "r8, rbp"),
        Q!("    mov             " "rdi, rbx"),
        Q!("    test            " "rcx, rdx"),
        Q!("    cmove           " "r8, rbp"),
        Q!("    cmove           " "rdi, rbp"),
        Q!("    sar             " "rcx, 1"),
        Q!("    xor             " "rdi, r8"),
        Q!("    xor             " "rsi, r8"),
        Q!("    bt              " "r8, 0x3f"),
        Q!("    cmovb           " "rbx, rcx"),
        Q!("    mov             " "r8, rax"),
        Q!("    sub             " "rsi, rax"),
        Q!("    lea             " "rcx, [rcx + rdi]"),
        Q!("    cmovs           " "r8, rbp"),
        Q!("    mov             " "rdi, rbx"),
        Q!("    test            " "rcx, rdx"),
        Q!("    cmove           " "r8, rbp"),
        Q!("    cmove           " "rdi, rbp"),
        Q!("    sar             " "rcx, 1"),
        Q!("    xor             " "rdi, r8"),
        Q!("    xor             " "rsi, r8"),
        Q!("    bt              " "r8, 0x3f"),
        Q!("    cmovb           " "rbx, rcx"),
        Q!("    mov             " "r8, rax"),
        Q!("    sub             " "rsi, rax"),
        Q!("    lea             " "rcx, [rcx + rdi]"),
        Q!("    cmovs           " "r8, rbp"),
        Q!("    mov             " "rdi, rbx"),
        Q!("    test            " "rcx, rdx"),
        Q!("    cmove           " "r8, rbp"),
        Q!("    cmove           " "rdi, rbp"),
        Q!("    sar             " "rcx, 1"),
        Q!("    xor             " "rdi, r8"),
        Q!("    xor             " "rsi, r8"),
        Q!("    bt              " "r8, 0x3f"),
        Q!("    cmovb           " "rbx, rcx"),
        Q!("    mov             " "r8, rax"),
        Q!("    sub             " "rsi, rax"),
        Q!("    lea             " "rcx, [rcx + rdi]"),
        Q!("    cmovs           " "r8, rbp"),
        Q!("    mov             " "rdi, rbx"),
        Q!("    test            " "rcx, rdx"),
        Q!("    cmove           " "r8, rbp"),
        Q!("    cmove           " "rdi, rbp"),
        Q!("    sar             " "rcx, 1"),
        Q!("    xor             " "rdi, r8"),
        Q!("    xor             " "rsi, r8"),
        Q!("    bt              " "r8, 0x3f"),
        Q!("    cmovb           " "rbx, rcx"),
        Q!("    mov             " "r8, rax"),
        Q!("    sub             " "rsi, rax"),
        Q!("    lea             " "rcx, [rcx + rdi]"),
        Q!("    cmovs           " "r8, rbp"),
        Q!("    mov             " "rdi, rbx"),
        Q!("    test            " "rcx, rdx"),
        Q!("    cmove           " "r8, rbp"),
        Q!("    cmove           " "rdi, rbp"),
        Q!("    sar             " "rcx, 1"),
        Q!("    xor             " "rdi, r8"),
        Q!("    xor             " "rsi, r8"),
        Q!("    bt              " "r8, 0x3f"),
        Q!("    cmovb           " "rbx, rcx"),
        Q!("    mov             " "r8, rax"),
        Q!("    sub             " "rsi, rax"),
        Q!("    lea             " "rcx, [rcx + rdi]"),
        Q!("    cmovs           " "r8, rbp"),
        Q!("    mov             " "rdi, rbx"),
        Q!("    test            " "rcx, rdx"),
        Q!("    cmove           " "r8, rbp"),
        Q!("    cmove           " "rdi, rbp"),
        Q!("    sar             " "rcx, 1"),
        Q!("    xor             " "rdi, r8"),
        Q!("    xor             " "rsi, r8"),
        Q!("    bt              " "r8, 0x3f"),
        Q!("    cmovb           " "rbx, rcx"),
        Q!("    mov             " "r8, rax"),
        Q!("    sub             " "rsi, rax"),
        Q!("    lea             " "rcx, [rcx + rdi]"),
        Q!("    cmovs           " "r8, rbp"),
        Q!("    mov             " "rdi, rbx"),
        Q!("    test            " "rcx, rdx"),
        Q!("    cmove           " "r8, rbp"),
        Q!("    cmove           " "rdi, rbp"),
        Q!("    sar             " "rcx, 1"),
        Q!("    xor             " "rdi, r8"),
        Q!("    xor             " "rsi, r8"),
        Q!("    bt              " "r8, 0x3f"),
        Q!("    cmovb           " "rbx, rcx"),
        Q!("    mov             " "r8, rax"),
        Q!("    sub             " "rsi, rax"),
        Q!("    lea             " "rcx, [rcx + rdi]"),
        Q!("    cmovs           " "r8, rbp"),
        Q!("    mov             " "rdi, rbx"),
        Q!("    test            " "rcx, rdx"),
        Q!("    cmove           " "r8, rbp"),
        Q!("    cmove           " "rdi, rbp"),
        Q!("    sar             " "rcx, 1"),
        Q!("    xor             " "rdi, r8"),
        Q!("    xor             " "rsi, r8"),
        Q!("    bt              " "r8, 0x3f"),
        Q!("    cmovb           " "rbx, rcx"),
        Q!("    mov             " "r8, rax"),
        Q!("    sub             " "rsi, rax"),
        Q!("    lea             " "rcx, [rcx + rdi]"),
        Q!("    cmovs           " "r8, rbp"),
        Q!("    mov             " "rdi, rbx"),
        Q!("    test            " "rcx, rdx"),
        Q!("    cmove           " "r8, rbp"),
        Q!("    cmove           " "rdi, rbp"),
        Q!("    sar             " "rcx, 1"),
        Q!("    xor             " "rdi, r8"),
        Q!("    xor             " "rsi, r8"),
        Q!("    bt              " "r8, 0x3f"),
        Q!("    cmovb           " "rbx, rcx"),
        Q!("    mov             " "r8, rax"),
        Q!("    sub             " "rsi, rax"),
        Q!("    lea             " "rcx, [rcx + rdi]"),
        Q!("    cmovs           " "r8, rbp"),
        Q!("    mov             " "rdi, rbx"),
        Q!("    test            " "rcx, rdx"),
        Q!("    cmove           " "r8, rbp"),
        Q!("    cmove           " "rdi, rbp"),
        Q!("    sar             " "rcx, 1"),
        Q!("    xor             " "rdi, r8"),
        Q!("    xor             " "rsi, r8"),
        Q!("    bt              " "r8, 0x3f"),
        Q!("    cmovb           " "rbx, rcx"),
        Q!("    mov             " "r8, rax"),
        Q!("    sub             " "rsi, rax"),
        Q!("    lea             " "rcx, [rcx + rdi]"),
        Q!("    cmovs           " "r8, rbp"),
        Q!("    mov             " "rdi, rbx"),
        Q!("    test            " "rcx, rdx"),
        Q!("    cmove           " "r8, rbp"),
        Q!("    cmove           " "rdi, rbp"),
        Q!("    sar             " "rcx, 1"),
        Q!("    xor             " "rdi, r8"),
        Q!("    xor             " "rsi, r8"),
        Q!("    bt              " "r8, 0x3f"),
        Q!("    cmovb           " "rbx, rcx"),
        Q!("    mov             " "r8, rax"),
        Q!("    sub             " "rsi, rax"),
        Q!("    lea             " "rcx, [rcx + rdi]"),
        Q!("    cmovs           " "r8, rbp"),
        Q!("    mov             " "rdi, rbx"),
        Q!("    test            " "rcx, rdx"),
        Q!("    cmove           " "r8, rbp"),
        Q!("    cmove           " "rdi, rbp"),
        Q!("    sar             " "rcx, 1"),
        Q!("    xor             " "rdi, r8"),
        Q!("    xor             " "rsi, r8"),
        Q!("    bt              " "r8, 0x3f"),
        Q!("    cmovb           " "rbx, rcx"),
        Q!("    mov             " "r8, rax"),
        Q!("    sub             " "rsi, rax"),
        Q!("    lea             " "rcx, [rcx + rdi]"),
        Q!("    cmovs           " "r8, rbp"),
        Q!("    mov             " "rdi, rbx"),
        Q!("    test            " "rcx, rdx"),
        Q!("    cmove           " "r8, rbp"),
        Q!("    cmove           " "rdi, rbp"),
        Q!("    sar             " "rcx, 1"),
        Q!("    xor             " "rdi, r8"),
        Q!("    xor             " "rsi, r8"),
        Q!("    bt              " "r8, 0x3f"),
        Q!("    cmovb           " "rbx, rcx"),
        Q!("    mov             " "r8, rax"),
        Q!("    sub             " "rsi, rax"),
        Q!("    lea             " "rcx, [rcx + rdi]"),
        Q!("    cmovs           " "r8, rbp"),
        Q!("    mov             " "rdi, rbx"),
        Q!("    test            " "rcx, rdx"),
        Q!("    cmove           " "r8, rbp"),
        Q!("    cmove           " "rdi, rbp"),
        Q!("    sar             " "rcx, 1"),
        Q!("    xor             " "rdi, r8"),
        Q!("    xor             " "rsi, r8"),
        Q!("    bt              " "r8, 0x3f"),
        Q!("    cmovb           " "rbx, rcx"),
        Q!("    mov             " "r8, rax"),
        Q!("    sub             " "rsi, rax"),
        Q!("    lea             " "rcx, [rcx + rdi]"),
        Q!("    cmovs           " "r8, rbp"),
        Q!("    mov             " "rdi, rbx"),
        Q!("    test            " "rcx, rdx"),
        Q!("    cmove           " "r8, rbp"),
        Q!("    cmove           " "rdi, rbp"),
        Q!("    sar             " "rcx, 1"),
        Q!("    xor             " "rdi, r8"),
        Q!("    xor             " "rsi, r8"),
        Q!("    bt              " "r8, 0x3f"),
        Q!("    cmovb           " "rbx, rcx"),
        Q!("    mov             " "r8, rax"),
        Q!("    sub             " "rsi, rax"),
        Q!("    lea             " "rcx, [rcx + rdi]"),
        Q!("    cmovs           " "r8, rbp"),
        Q!("    mov             " "rdi, rbx"),
        Q!("    test            " "rcx, rdx"),
        Q!("    cmove           " "r8, rbp"),
        Q!("    cmove           " "rdi, rbp"),
        Q!("    sar             " "rcx, 1"),
        Q!("    xor             " "rdi, r8"),
        Q!("    xor             " "rsi, r8"),
        Q!("    bt              " "r8, 0x3f"),
        Q!("    cmovb           " "rbx, rcx"),
        Q!("    mov             " "r8, rax"),
        Q!("    sub             " "rsi, rax"),
        Q!("    lea             " "rcx, [rcx + rdi]"),
        Q!("    cmovs           " "r8, rbp"),
        Q!("    mov             " "rdi, rbx"),
        Q!("    test            " "rcx, rdx"),
        Q!("    cmove           " "r8, rbp"),
        Q!("    cmove           " "rdi, rbp"),
        Q!("    sar             " "rcx, 1"),
        Q!("    xor             " "rdi, r8"),
        Q!("    xor             " "rsi, r8"),
        Q!("    bt              " "r8, 0x3f"),
        Q!("    cmovb           " "rbx, rcx"),
        Q!("    mov             " "r8, rax"),
        Q!("    sub             " "rsi, rax"),
        Q!("    lea             " "rcx, [rcx + rdi]"),
        Q!("    sar             " "rcx, 1"),
        Q!("    mov             " "eax, 0x100000"),
        Q!("    lea             " "rdx, [rbx + rax]"),
        Q!("    lea             " "rdi, [rcx + rax]"),
        Q!("    shl             " "rdx, 0x16"),
        Q!("    shl             " "rdi, 0x16"),
        Q!("    sar             " "rdx, 0x2b"),
        Q!("    sar             " "rdi, 0x2b"),
        Q!("    movabs          " "rax, 0x20000100000"),
        Q!("    lea             " "rbx, [rbx + rax]"),
        Q!("    lea             " "rcx, [rcx + rax]"),
        Q!("    sar             " "rbx, 0x2a"),
        Q!("    sar             " "rcx, 0x2a"),
        Q!("    mov             " "[rsp + 0xa0], rdx"),
        Q!("    mov             " "[rsp + 0xa8], rbx"),
        Q!("    mov             " "[rsp + 0xb0], rdi"),
        Q!("    mov             " "[rsp + 0xb8], rcx"),
        Q!("    mov             " "r12, [rsp]"),
        Q!("    imul            " "rdi, r12"),
        Q!("    imul            " "r12, rdx"),
        Q!("    mov             " "r13, [rsp + 0x20]"),
        Q!("    imul            " "rbx, r13"),
        Q!("    imul            " "r13, rcx"),
        Q!("    add             " "r12, rbx"),
        Q!("    add             " "r13, rdi"),
        Q!("    sar             " "r12, 0x14"),
        Q!("    sar             " "r13, 0x14"),
        Q!("    mov             " "rbx, r12"),
        Q!("    and             " "rbx, 0xfffff"),
        Q!("    movabs          " "rax, 0xfffffe0000000000"),
        Q!("    or              " "rbx, rax"),
        Q!("    mov             " "rcx, r13"),
        Q!("    and             " "rcx, 0xfffff"),
        Q!("    movabs          " "rax, 0xc000000000000000"),
        Q!("    or              " "rcx, rax"),
        Q!("    mov             " "rax, 0xfffffffffffffffe"),
        Q!("    mov             " "edx, 0x2"),
        Q!("    mov             " "rdi, rbx"),
        Q!("    mov             " "r8, rax"),
        Q!("    test            " "rsi, rsi"),
        Q!("    cmovs           " "r8, rbp"),
        Q!("    test            " "rcx, 0x1"),
        Q!("    cmove           " "r8, rbp"),
        Q!("    cmove           " "rdi, rbp"),
        Q!("    xor             " "rdi, r8"),
        Q!("    xor             " "rsi, r8"),
        Q!("    bt              " "r8, 0x3f"),
        Q!("    cmovb           " "rbx, rcx"),
        Q!("    mov             " "r8, rax"),
        Q!("    sub             " "rsi, rax"),
        Q!("    lea             " "rcx, [rcx + rdi]"),
        Q!("    cmovs           " "r8, rbp"),
        Q!("    mov             " "rdi, rbx"),
        Q!("    test            " "rcx, rdx"),
        Q!("    cmove           " "r8, rbp"),
        Q!("    cmove           " "rdi, rbp"),
        Q!("    sar             " "rcx, 1"),
        Q!("    xor             " "rdi, r8"),
        Q!("    xor             " "rsi, r8"),
        Q!("    bt              " "r8, 0x3f"),
        Q!("    cmovb           " "rbx, rcx"),
        Q!("    mov             " "r8, rax"),
        Q!("    sub             " "rsi, rax"),
        Q!("    lea             " "rcx, [rcx + rdi]"),
        Q!("    cmovs           " "r8, rbp"),
        Q!("    mov             " "rdi, rbx"),
        Q!("    test            " "rcx, rdx"),
        Q!("    cmove           " "r8, rbp"),
        Q!("    cmove           " "rdi, rbp"),
        Q!("    sar             " "rcx, 1"),
        Q!("    xor             " "rdi, r8"),
        Q!("    xor             " "rsi, r8"),
        Q!("    bt              " "r8, 0x3f"),
        Q!("    cmovb           " "rbx, rcx"),
        Q!("    mov             " "r8, rax"),
        Q!("    sub             " "rsi, rax"),
        Q!("    lea             " "rcx, [rcx + rdi]"),
        Q!("    cmovs           " "r8, rbp"),
        Q!("    mov             " "rdi, rbx"),
        Q!("    test            " "rcx, rdx"),
        Q!("    cmove           " "r8, rbp"),
        Q!("    cmove           " "rdi, rbp"),
        Q!("    sar             " "rcx, 1"),
        Q!("    xor             " "rdi, r8"),
        Q!("    xor             " "rsi, r8"),
        Q!("    bt              " "r8, 0x3f"),
        Q!("    cmovb           " "rbx, rcx"),
        Q!("    mov             " "r8, rax"),
        Q!("    sub             " "rsi, rax"),
        Q!("    lea             " "rcx, [rcx + rdi]"),
        Q!("    cmovs           " "r8, rbp"),
        Q!("    mov             " "rdi, rbx"),
        Q!("    test            " "rcx, rdx"),
        Q!("    cmove           " "r8, rbp"),
        Q!("    cmove           " "rdi, rbp"),
        Q!("    sar             " "rcx, 1"),
        Q!("    xor             " "rdi, r8"),
        Q!("    xor             " "rsi, r8"),
        Q!("    bt              " "r8, 0x3f"),
        Q!("    cmovb           " "rbx, rcx"),
        Q!("    mov             " "r8, rax"),
        Q!("    sub             " "rsi, rax"),
        Q!("    lea             " "rcx, [rcx + rdi]"),
        Q!("    cmovs           " "r8, rbp"),
        Q!("    mov             " "rdi, rbx"),
        Q!("    test            " "rcx, rdx"),
        Q!("    cmove           " "r8, rbp"),
        Q!("    cmove           " "rdi, rbp"),
        Q!("    sar             " "rcx, 1"),
        Q!("    xor             " "rdi, r8"),
        Q!("    xor             " "rsi, r8"),
        Q!("    bt              " "r8, 0x3f"),
        Q!("    cmovb           " "rbx, rcx"),
        Q!("    mov             " "r8, rax"),
        Q!("    sub             " "rsi, rax"),
        Q!("    lea             " "rcx, [rcx + rdi]"),
        Q!("    cmovs           " "r8, rbp"),
        Q!("    mov             " "rdi, rbx"),
        Q!("    test            " "rcx, rdx"),
        Q!("    cmove           " "r8, rbp"),
        Q!("    cmove           " "rdi, rbp"),
        Q!("    sar             " "rcx, 1"),
        Q!("    xor             " "rdi, r8"),
        Q!("    xor             " "rsi, r8"),
        Q!("    bt              " "r8, 0x3f"),
        Q!("    cmovb           " "rbx, rcx"),
        Q!("    mov             " "r8, rax"),
        Q!("    sub             " "rsi, rax"),
        Q!("    lea             " "rcx, [rcx + rdi]"),
        Q!("    cmovs           " "r8, rbp"),
        Q!("    mov             " "rdi, rbx"),
        Q!("    test            " "rcx, rdx"),
        Q!("    cmove           " "r8, rbp"),
        Q!("    cmove           " "rdi, rbp"),
        Q!("    sar             " "rcx, 1"),
        Q!("    xor             " "rdi, r8"),
        Q!("    xor             " "rsi, r8"),
        Q!("    bt              " "r8, 0x3f"),
        Q!("    cmovb           " "rbx, rcx"),
        Q!("    mov             " "r8, rax"),
        Q!("    sub             " "rsi, rax"),
        Q!("    lea             " "rcx, [rcx + rdi]"),
        Q!("    cmovs           " "r8, rbp"),
        Q!("    mov             " "rdi, rbx"),
        Q!("    test            " "rcx, rdx"),
        Q!("    cmove           " "r8, rbp"),
        Q!("    cmove           " "rdi, rbp"),
        Q!("    sar             " "rcx, 1"),
        Q!("    xor             " "rdi, r8"),
        Q!("    xor             " "rsi, r8"),
        Q!("    bt              " "r8, 0x3f"),
        Q!("    cmovb           " "rbx, rcx"),
        Q!("    mov             " "r8, rax"),
        Q!("    sub             " "rsi, rax"),
        Q!("    lea             " "rcx, [rcx + rdi]"),
        Q!("    cmovs           " "r8, rbp"),
        Q!("    mov             " "rdi, rbx"),
        Q!("    test            " "rcx, rdx"),
        Q!("    cmove           " "r8, rbp"),
        Q!("    cmove           " "rdi, rbp"),
        Q!("    sar             " "rcx, 1"),
        Q!("    xor             " "rdi, r8"),
        Q!("    xor             " "rsi, r8"),
        Q!("    bt              " "r8, 0x3f"),
        Q!("    cmovb           " "rbx, rcx"),
        Q!("    mov             " "r8, rax"),
        Q!("    sub             " "rsi, rax"),
        Q!("    lea             " "rcx, [rcx + rdi]"),
        Q!("    cmovs           " "r8, rbp"),
        Q!("    mov             " "rdi, rbx"),
        Q!("    test            " "rcx, rdx"),
        Q!("    cmove           " "r8, rbp"),
        Q!("    cmove           " "rdi, rbp"),
        Q!("    sar             " "rcx, 1"),
        Q!("    xor             " "rdi, r8"),
        Q!("    xor             " "rsi, r8"),
        Q!("    bt              " "r8, 0x3f"),
        Q!("    cmovb           " "rbx, rcx"),
        Q!("    mov             " "r8, rax"),
        Q!("    sub             " "rsi, rax"),
        Q!("    lea             " "rcx, [rcx + rdi]"),
        Q!("    cmovs           " "r8, rbp"),
        Q!("    mov             " "rdi, rbx"),
        Q!("    test            " "rcx, rdx"),
        Q!("    cmove           " "r8, rbp"),
        Q!("    cmove           " "rdi, rbp"),
        Q!("    sar             " "rcx, 1"),
        Q!("    xor             " "rdi, r8"),
        Q!("    xor             " "rsi, r8"),
        Q!("    bt              " "r8, 0x3f"),
        Q!("    cmovb           " "rbx, rcx"),
        Q!("    mov             " "r8, rax"),
        Q!("    sub             " "rsi, rax"),
        Q!("    lea             " "rcx, [rcx + rdi]"),
        Q!("    cmovs           " "r8, rbp"),
        Q!("    mov             " "rdi, rbx"),
        Q!("    test            " "rcx, rdx"),
        Q!("    cmove           " "r8, rbp"),
        Q!("    cmove           " "rdi, rbp"),
        Q!("    sar             " "rcx, 1"),
        Q!("    xor             " "rdi, r8"),
        Q!("    xor             " "rsi, r8"),
        Q!("    bt              " "r8, 0x3f"),
        Q!("    cmovb           " "rbx, rcx"),
        Q!("    mov             " "r8, rax"),
        Q!("    sub             " "rsi, rax"),
        Q!("    lea             " "rcx, [rcx + rdi]"),
        Q!("    cmovs           " "r8, rbp"),
        Q!("    mov             " "rdi, rbx"),
        Q!("    test            " "rcx, rdx"),
        Q!("    cmove           " "r8, rbp"),
        Q!("    cmove           " "rdi, rbp"),
        Q!("    sar             " "rcx, 1"),
        Q!("    xor             " "rdi, r8"),
        Q!("    xor             " "rsi, r8"),
        Q!("    bt              " "r8, 0x3f"),
        Q!("    cmovb           " "rbx, rcx"),
        Q!("    mov             " "r8, rax"),
        Q!("    sub             " "rsi, rax"),
        Q!("    lea             " "rcx, [rcx + rdi]"),
        Q!("    cmovs           " "r8, rbp"),
        Q!("    mov             " "rdi, rbx"),
        Q!("    test            " "rcx, rdx"),
        Q!("    cmove           " "r8, rbp"),
        Q!("    cmove           " "rdi, rbp"),
        Q!("    sar             " "rcx, 1"),
        Q!("    xor             " "rdi, r8"),
        Q!("    xor             " "rsi, r8"),
        Q!("    bt              " "r8, 0x3f"),
        Q!("    cmovb           " "rbx, rcx"),
        Q!("    mov             " "r8, rax"),
        Q!("    sub             " "rsi, rax"),
        Q!("    lea             " "rcx, [rcx + rdi]"),
        Q!("    cmovs           " "r8, rbp"),
        Q!("    mov             " "rdi, rbx"),
        Q!("    test            " "rcx, rdx"),
        Q!("    cmove           " "r8, rbp"),
        Q!("    cmove           " "rdi, rbp"),
        Q!("    sar             " "rcx, 1"),
        Q!("    xor             " "rdi, r8"),
        Q!("    xor             " "rsi, r8"),
        Q!("    bt              " "r8, 0x3f"),
        Q!("    cmovb           " "rbx, rcx"),
        Q!("    mov             " "r8, rax"),
        Q!("    sub             " "rsi, rax"),
        Q!("    lea             " "rcx, [rcx + rdi]"),
        Q!("    cmovs           " "r8, rbp"),
        Q!("    mov             " "rdi, rbx"),
        Q!("    test            " "rcx, rdx"),
        Q!("    cmove           " "r8, rbp"),
        Q!("    cmove           " "rdi, rbp"),
        Q!("    sar             " "rcx, 1"),
        Q!("    xor             " "rdi, r8"),
        Q!("    xor             " "rsi, r8"),
        Q!("    bt              " "r8, 0x3f"),
        Q!("    cmovb           " "rbx, rcx"),
        Q!("    mov             " "r8, rax"),
        Q!("    sub             " "rsi, rax"),
        Q!("    lea             " "rcx, [rcx + rdi]"),
        Q!("    cmovs           " "r8, rbp"),
        Q!("    mov             " "rdi, rbx"),
        Q!("    test            " "rcx, rdx"),
        Q!("    cmove           " "r8, rbp"),
        Q!("    cmove           " "rdi, rbp"),
        Q!("    sar             " "rcx, 1"),
        Q!("    xor             " "rdi, r8"),
        Q!("    xor             " "rsi, r8"),
        Q!("    bt              " "r8, 0x3f"),
        Q!("    cmovb           " "rbx, rcx"),
        Q!("    mov             " "r8, rax"),
        Q!("    sub             " "rsi, rax"),
        Q!("    lea             " "rcx, [rcx + rdi]"),
        Q!("    cmovs           " "r8, rbp"),
        Q!("    mov             " "rdi, rbx"),
        Q!("    test            " "rcx, rdx"),
        Q!("    cmove           " "r8, rbp"),
        Q!("    cmove           " "rdi, rbp"),
        Q!("    sar             " "rcx, 1"),
        Q!("    xor             " "rdi, r8"),
        Q!("    xor             " "rsi, r8"),
        Q!("    bt              " "r8, 0x3f"),
        Q!("    cmovb           " "rbx, rcx"),
        Q!("    mov             " "r8, rax"),
        Q!("    sub             " "rsi, rax"),
        Q!("    lea             " "rcx, [rcx + rdi]"),
        Q!("    cmovs           " "r8, rbp"),
        Q!("    mov             " "rdi, rbx"),
        Q!("    test            " "rcx, rdx"),
        Q!("    cmove           " "r8, rbp"),
        Q!("    cmove           " "rdi, rbp"),
        Q!("    sar             " "rcx, 1"),
        Q!("    xor             " "rdi, r8"),
        Q!("    xor             " "rsi, r8"),
        Q!("    bt              " "r8, 0x3f"),
        Q!("    cmovb           " "rbx, rcx"),
        Q!("    mov             " "r8, rax"),
        Q!("    sub             " "rsi, rax"),
        Q!("    lea             " "rcx, [rcx + rdi]"),
        Q!("    sar             " "rcx, 1"),
        Q!("    mov             " "eax, 0x100000"),
        Q!("    lea             " "r8, [rbx + rax]"),
        Q!("    lea             " "r10, [rcx + rax]"),
        Q!("    shl             " "r8, 0x16"),
        Q!("    shl             " "r10, 0x16"),
        Q!("    sar             " "r8, 0x2b"),
        Q!("    sar             " "r10, 0x2b"),
        Q!("    movabs          " "rax, 0x20000100000"),
        Q!("    lea             " "r15, [rbx + rax]"),
        Q!("    lea             " "r11, [rcx + rax]"),
        Q!("    sar             " "r15, 0x2a"),
        Q!("    sar             " "r11, 0x2a"),
        Q!("    mov             " "rbx, r13"),
        Q!("    mov             " "rcx, r12"),
        Q!("    imul            " "r12, r8"),
        Q!("    imul            " "rbx, r15"),
        Q!("    add             " "r12, rbx"),
        Q!("    imul            " "r13, r11"),
        Q!("    imul            " "rcx, r10"),
        Q!("    add             " "r13, rcx"),
        Q!("    sar             " "r12, 0x14"),
        Q!("    sar             " "r13, 0x14"),
        Q!("    mov             " "rbx, r12"),
        Q!("    and             " "rbx, 0xfffff"),
        Q!("    movabs          " "rax, 0xfffffe0000000000"),
        Q!("    or              " "rbx, rax"),
        Q!("    mov             " "rcx, r13"),
        Q!("    and             " "rcx, 0xfffff"),
        Q!("    movabs          " "rax, 0xc000000000000000"),
        Q!("    or              " "rcx, rax"),
        Q!("    mov             " "rax, [rsp + 0xa0]"),
        Q!("    imul            " "rax, r8"),
        Q!("    mov             " "rdx, [rsp + 0xb0]"),
        Q!("    imul            " "rdx, r15"),
        Q!("    imul            " "r8, [rsp + 0xa8]"),
        Q!("    imul            " "r15, [rsp + 0xb8]"),
        Q!("    add             " "r15, r8"),
        Q!("    lea             " "r9, [rax + rdx]"),
        Q!("    mov             " "rax, [rsp + 0xa0]"),
        Q!("    imul            " "rax, r10"),
        Q!("    mov             " "rdx, [rsp + 0xb0]"),
        Q!("    imul            " "rdx, r11"),
        Q!("    imul            " "r10, [rsp + 0xa8]"),
        Q!("    imul            " "r11, [rsp + 0xb8]"),
        Q!("    add             " "r11, r10"),
        Q!("    lea             " "r13, [rax + rdx]"),
        Q!("    mov             " "rax, 0xfffffffffffffffe"),
        Q!("    mov             " "edx, 0x2"),
        Q!("    mov             " "rdi, rbx"),
        Q!("    mov             " "r8, rax"),
        Q!("    test            " "rsi, rsi"),
        Q!("    cmovs           " "r8, rbp"),
        Q!("    test            " "rcx, 0x1"),
        Q!("    cmove           " "r8, rbp"),
        Q!("    cmove           " "rdi, rbp"),
        Q!("    xor             " "rdi, r8"),
        Q!("    xor             " "rsi, r8"),
        Q!("    bt              " "r8, 0x3f"),
        Q!("    cmovb           " "rbx, rcx"),
        Q!("    mov             " "r8, rax"),
        Q!("    sub             " "rsi, rax"),
        Q!("    lea             " "rcx, [rcx + rdi]"),
        Q!("    cmovs           " "r8, rbp"),
        Q!("    mov             " "rdi, rbx"),
        Q!("    test            " "rcx, rdx"),
        Q!("    cmove           " "r8, rbp"),
        Q!("    cmove           " "rdi, rbp"),
        Q!("    sar             " "rcx, 1"),
        Q!("    xor             " "rdi, r8"),
        Q!("    xor             " "rsi, r8"),
        Q!("    bt              " "r8, 0x3f"),
        Q!("    cmovb           " "rbx, rcx"),
        Q!("    mov             " "r8, rax"),
        Q!("    sub             " "rsi, rax"),
        Q!("    lea             " "rcx, [rcx + rdi]"),
        Q!("    cmovs           " "r8, rbp"),
        Q!("    mov             " "rdi, rbx"),
        Q!("    test            " "rcx, rdx"),
        Q!("    cmove           " "r8, rbp"),
        Q!("    cmove           " "rdi, rbp"),
        Q!("    sar             " "rcx, 1"),
        Q!("    xor             " "rdi, r8"),
        Q!("    xor             " "rsi, r8"),
        Q!("    bt              " "r8, 0x3f"),
        Q!("    cmovb           " "rbx, rcx"),
        Q!("    mov             " "r8, rax"),
        Q!("    sub             " "rsi, rax"),
        Q!("    lea             " "rcx, [rcx + rdi]"),
        Q!("    cmovs           " "r8, rbp"),
        Q!("    mov             " "rdi, rbx"),
        Q!("    test            " "rcx, rdx"),
        Q!("    cmove           " "r8, rbp"),
        Q!("    cmove           " "rdi, rbp"),
        Q!("    sar             " "rcx, 1"),
        Q!("    xor             " "rdi, r8"),
        Q!("    xor             " "rsi, r8"),
        Q!("    bt              " "r8, 0x3f"),
        Q!("    cmovb           " "rbx, rcx"),
        Q!("    mov             " "r8, rax"),
        Q!("    sub             " "rsi, rax"),
        Q!("    lea             " "rcx, [rcx + rdi]"),
        Q!("    cmovs           " "r8, rbp"),
        Q!("    mov             " "rdi, rbx"),
        Q!("    test            " "rcx, rdx"),
        Q!("    cmove           " "r8, rbp"),
        Q!("    cmove           " "rdi, rbp"),
        Q!("    sar             " "rcx, 1"),
        Q!("    xor             " "rdi, r8"),
        Q!("    xor             " "rsi, r8"),
        Q!("    bt              " "r8, 0x3f"),
        Q!("    cmovb           " "rbx, rcx"),
        Q!("    mov             " "r8, rax"),
        Q!("    sub             " "rsi, rax"),
        Q!("    lea             " "rcx, [rcx + rdi]"),
        Q!("    cmovs           " "r8, rbp"),
        Q!("    mov             " "rdi, rbx"),
        Q!("    test            " "rcx, rdx"),
        Q!("    cmove           " "r8, rbp"),
        Q!("    cmove           " "rdi, rbp"),
        Q!("    sar             " "rcx, 1"),
        Q!("    xor             " "rdi, r8"),
        Q!("    xor             " "rsi, r8"),
        Q!("    bt              " "r8, 0x3f"),
        Q!("    cmovb           " "rbx, rcx"),
        Q!("    mov             " "r8, rax"),
        Q!("    sub             " "rsi, rax"),
        Q!("    lea             " "rcx, [rcx + rdi]"),
        Q!("    cmovs           " "r8, rbp"),
        Q!("    mov             " "rdi, rbx"),
        Q!("    test            " "rcx, rdx"),
        Q!("    cmove           " "r8, rbp"),
        Q!("    cmove           " "rdi, rbp"),
        Q!("    sar             " "rcx, 1"),
        Q!("    xor             " "rdi, r8"),
        Q!("    xor             " "rsi, r8"),
        Q!("    bt              " "r8, 0x3f"),
        Q!("    cmovb           " "rbx, rcx"),
        Q!("    mov             " "r8, rax"),
        Q!("    sub             " "rsi, rax"),
        Q!("    lea             " "rcx, [rcx + rdi]"),
        Q!("    cmovs           " "r8, rbp"),
        Q!("    mov             " "rdi, rbx"),
        Q!("    test            " "rcx, rdx"),
        Q!("    cmove           " "r8, rbp"),
        Q!("    cmove           " "rdi, rbp"),
        Q!("    sar             " "rcx, 1"),
        Q!("    xor             " "rdi, r8"),
        Q!("    xor             " "rsi, r8"),
        Q!("    bt              " "r8, 0x3f"),
        Q!("    cmovb           " "rbx, rcx"),
        Q!("    mov             " "r8, rax"),
        Q!("    sub             " "rsi, rax"),
        Q!("    lea             " "rcx, [rcx + rdi]"),
        Q!("    cmovs           " "r8, rbp"),
        Q!("    mov             " "rdi, rbx"),
        Q!("    test            " "rcx, rdx"),
        Q!("    cmove           " "r8, rbp"),
        Q!("    cmove           " "rdi, rbp"),
        Q!("    sar             " "rcx, 1"),
        Q!("    xor             " "rdi, r8"),
        Q!("    xor             " "rsi, r8"),
        Q!("    bt              " "r8, 0x3f"),
        Q!("    cmovb           " "rbx, rcx"),
        Q!("    mov             " "r8, rax"),
        Q!("    sub             " "rsi, rax"),
        Q!("    lea             " "rcx, [rcx + rdi]"),
        Q!("    cmovs           " "r8, rbp"),
        Q!("    mov             " "rdi, rbx"),
        Q!("    test            " "rcx, rdx"),
        Q!("    cmove           " "r8, rbp"),
        Q!("    cmove           " "rdi, rbp"),
        Q!("    sar             " "rcx, 1"),
        Q!("    xor             " "rdi, r8"),
        Q!("    xor             " "rsi, r8"),
        Q!("    bt              " "r8, 0x3f"),
        Q!("    cmovb           " "rbx, rcx"),
        Q!("    mov             " "r8, rax"),
        Q!("    sub             " "rsi, rax"),
        Q!("    lea             " "rcx, [rcx + rdi]"),
        Q!("    cmovs           " "r8, rbp"),
        Q!("    mov             " "rdi, rbx"),
        Q!("    test            " "rcx, rdx"),
        Q!("    cmove           " "r8, rbp"),
        Q!("    cmove           " "rdi, rbp"),
        Q!("    sar             " "rcx, 1"),
        Q!("    xor             " "rdi, r8"),
        Q!("    xor             " "rsi, r8"),
        Q!("    bt              " "r8, 0x3f"),
        Q!("    cmovb           " "rbx, rcx"),
        Q!("    mov             " "r8, rax"),
        Q!("    sub             " "rsi, rax"),
        Q!("    lea             " "rcx, [rcx + rdi]"),
        Q!("    cmovs           " "r8, rbp"),
        Q!("    mov             " "rdi, rbx"),
        Q!("    test            " "rcx, rdx"),
        Q!("    cmove           " "r8, rbp"),
        Q!("    cmove           " "rdi, rbp"),
        Q!("    sar             " "rcx, 1"),
        Q!("    xor             " "rdi, r8"),
        Q!("    xor             " "rsi, r8"),
        Q!("    bt              " "r8, 0x3f"),
        Q!("    cmovb           " "rbx, rcx"),
        Q!("    mov             " "r8, rax"),
        Q!("    sub             " "rsi, rax"),
        Q!("    lea             " "rcx, [rcx + rdi]"),
        Q!("    cmovs           " "r8, rbp"),
        Q!("    mov             " "rdi, rbx"),
        Q!("    test            " "rcx, rdx"),
        Q!("    cmove           " "r8, rbp"),
        Q!("    cmove           " "rdi, rbp"),
        Q!("    sar             " "rcx, 1"),
        Q!("    xor             " "rdi, r8"),
        Q!("    xor             " "rsi, r8"),
        Q!("    bt              " "r8, 0x3f"),
        Q!("    cmovb           " "rbx, rcx"),
        Q!("    mov             " "r8, rax"),
        Q!("    sub             " "rsi, rax"),
        Q!("    lea             " "rcx, [rcx + rdi]"),
        Q!("    cmovs           " "r8, rbp"),
        Q!("    mov             " "rdi, rbx"),
        Q!("    test            " "rcx, rdx"),
        Q!("    cmove           " "r8, rbp"),
        Q!("    cmove           " "rdi, rbp"),
        Q!("    sar             " "rcx, 1"),
        Q!("    xor             " "rdi, r8"),
        Q!("    xor             " "rsi, r8"),
        Q!("    bt              " "r8, 0x3f"),
        Q!("    cmovb           " "rbx, rcx"),
        Q!("    mov             " "r8, rax"),
        Q!("    sub             " "rsi, rax"),
        Q!("    lea             " "rcx, [rcx + rdi]"),
        Q!("    cmovs           " "r8, rbp"),
        Q!("    mov             " "rdi, rbx"),
        Q!("    test            " "rcx, rdx"),
        Q!("    cmove           " "r8, rbp"),
        Q!("    cmove           " "rdi, rbp"),
        Q!("    sar             " "rcx, 1"),
        Q!("    xor             " "rdi, r8"),
        Q!("    xor             " "rsi, r8"),
        Q!("    bt              " "r8, 0x3f"),
        Q!("    cmovb           " "rbx, rcx"),
        Q!("    mov             " "r8, rax"),
        Q!("    sub             " "rsi, rax"),
        Q!("    lea             " "rcx, [rcx + rdi]"),
        Q!("    cmovs           " "r8, rbp"),
        Q!("    mov             " "rdi, rbx"),
        Q!("    test            " "rcx, rdx"),
        Q!("    cmove           " "r8, rbp"),
        Q!("    cmove           " "rdi, rbp"),
        Q!("    sar             " "rcx, 1"),
        Q!("    xor             " "rdi, r8"),
        Q!("    xor             " "rsi, r8"),
        Q!("    bt              " "r8, 0x3f"),
        Q!("    cmovb           " "rbx, rcx"),
        Q!("    mov             " "r8, rax"),
        Q!("    sub             " "rsi, rax"),
        Q!("    lea             " "rcx, [rcx + rdi]"),
        Q!("    cmovs           " "r8, rbp"),
        Q!("    mov             " "rdi, rbx"),
        Q!("    test            " "rcx, rdx"),
        Q!("    cmove           " "r8, rbp"),
        Q!("    cmove           " "rdi, rbp"),
        Q!("    sar             " "rcx, 1"),
        Q!("    xor             " "rdi, r8"),
        Q!("    xor             " "rsi, r8"),
        Q!("    bt              " "r8, 0x3f"),
        Q!("    cmovb           " "rbx, rcx"),
        Q!("    mov             " "r8, rax"),
        Q!("    sub             " "rsi, rax"),
        Q!("    lea             " "rcx, [rcx + rdi]"),
        Q!("    cmovs           " "r8, rbp"),
        Q!("    mov             " "rdi, rbx"),
        Q!("    test            " "rcx, rdx"),
        Q!("    cmove           " "r8, rbp"),
        Q!("    cmove           " "rdi, rbp"),
        Q!("    sar             " "rcx, 1"),
        Q!("    xor             " "rdi, r8"),
        Q!("    xor             " "rsi, r8"),
        Q!("    bt              " "r8, 0x3f"),
        Q!("    cmovb           " "rbx, rcx"),
        Q!("    mov             " "r8, rax"),
        Q!("    sub             " "rsi, rax"),
        Q!("    lea             " "rcx, [rcx + rdi]"),
        Q!("    cmovs           " "r8, rbp"),
        Q!("    mov             " "rdi, rbx"),
        Q!("    test            " "rcx, rdx"),
        Q!("    cmove           " "r8, rbp"),
        Q!("    cmove           " "rdi, rbp"),
        Q!("    sar             " "rcx, 1"),
        Q!("    xor             " "rdi, r8"),
        Q!("    xor             " "rsi, r8"),
        Q!("    bt              " "r8, 0x3f"),
        Q!("    cmovb           " "rbx, rcx"),
        Q!("    mov             " "r8, rax"),
        Q!("    sub             " "rsi, rax"),
        Q!("    lea             " "rcx, [rcx + rdi]"),
        Q!("    sar             " "rcx, 1"),
        Q!("    mov             " "eax, 0x100000"),
        Q!("    lea             " "r8, [rbx + rax]"),
        Q!("    lea             " "r12, [rcx + rax]"),
        Q!("    shl             " "r8, 0x15"),
        Q!("    shl             " "r12, 0x15"),
        Q!("    sar             " "r8, 0x2b"),
        Q!("    sar             " "r12, 0x2b"),
        Q!("    movabs          " "rax, 0x20000100000"),
        Q!("    lea             " "r10, [rbx + rax]"),
        Q!("    lea             " "r14, [rcx + rax]"),
        Q!("    sar             " "r10, 0x2b"),
        Q!("    sar             " "r14, 0x2b"),
        Q!("    mov             " "rax, r9"),
        Q!("    imul            " "rax, r8"),
        Q!("    mov             " "rdx, r13"),
        Q!("    imul            " "rdx, r10"),
        Q!("    imul            " "r8, r15"),
        Q!("    imul            " "r10, r11"),
        Q!("    add             " "r10, r8"),
        Q!("    lea             " "r8, [rax + rdx]"),
        Q!("    mov             " "rax, r9"),
        Q!("    imul            " "rax, r12"),
        Q!("    mov             " "rdx, r13"),
        Q!("    imul            " "rdx, r14"),
        Q!("    imul            " "r12, r15"),
        Q!("    imul            " "r14, r11"),
        Q!("    add             " "r14, r12"),
        Q!("    lea             " "r12, [rax + rdx]"),
        Q!("    mov             " "[rsp + 0x98], rsi"),
        Q!("    dec             " "QWORD PTR [rsp + 0x90]"),
        Q!("    jne             " Label!("Lcurve25519_x25519_inverseloop", 5, Before)),
        Q!("    mov             " "rax, [rsp]"),
        Q!("    mov             " "rcx, [rsp + 0x20]"),
        Q!("    imul            " "rax, r8"),
        Q!("    imul            " "rcx, r10"),
        Q!("    add             " "rax, rcx"),
        Q!("    sar             " "rax, 0x3f"),
        Q!("    mov             " "r9, r8"),
        Q!("    sar             " "r9, 0x3f"),
        Q!("    xor             " "r8, r9"),
        Q!("    sub             " "r8, r9"),
        Q!("    xor             " "r9, rax"),
        Q!("    mov             " "r11, r10"),
        Q!("    sar             " "r11, 0x3f"),
        Q!("    xor             " "r10, r11"),
        Q!("    sub             " "r10, r11"),
        Q!("    xor             " "r11, rax"),
        Q!("    mov             " "r13, r12"),
        Q!("    sar             " "r13, 0x3f"),
        Q!("    xor             " "r12, r13"),
        Q!("    sub             " "r12, r13"),
        Q!("    xor             " "r13, rax"),
        Q!("    mov             " "r15, r14"),
        Q!("    sar             " "r15, 0x3f"),
        Q!("    xor             " "r14, r15"),
        Q!("    sub             " "r14, r15"),
        Q!("    xor             " "r15, rax"),
        Q!("    mov             " "rax, r8"),
        Q!("    and             " "rax, r9"),
        Q!("    mov             " "r12, r10"),
        Q!("    and             " "r12, r11"),
        Q!("    add             " "r12, rax"),
        Q!("    xor             " "r13d, r13d"),
        Q!("    mov             " "rax, [rsp + 0x40]"),
        Q!("    xor             " "rax, r9"),
        Q!("    mul             " "r8"),
        Q!("    add             " "r12, rax"),
        Q!("    adc             " "r13, rdx"),
        Q!("    mov             " "rax, [rsp + 0x60]"),
        Q!("    xor             " "rax, r11"),
        Q!("    mul             " "r10"),
        Q!("    add             " "r12, rax"),
        Q!("    adc             " "r13, rdx"),
        Q!("    xor             " "r14d, r14d"),
        Q!("    mov             " "rax, [rsp + 0x48]"),
        Q!("    xor             " "rax, r9"),
        Q!("    mul             " "r8"),
        Q!("    add             " "r13, rax"),
        Q!("    adc             " "r14, rdx"),
        Q!("    mov             " "rax, [rsp + 0x68]"),
        Q!("    xor             " "rax, r11"),
        Q!("    mul             " "r10"),
        Q!("    add             " "r13, rax"),
        Q!("    adc             " "r14, rdx"),
        Q!("    xor             " "r15d, r15d"),
        Q!("    mov             " "rax, [rsp + 0x50]"),
        Q!("    xor             " "rax, r9"),
        Q!("    mul             " "r8"),
        Q!("    add             " "r14, rax"),
        Q!("    adc             " "r15, rdx"),
        Q!("    mov             " "rax, [rsp + 0x70]"),
        Q!("    xor             " "rax, r11"),
        Q!("    mul             " "r10"),
        Q!("    add             " "r14, rax"),
        Q!("    adc             " "r15, rdx"),
        Q!("    mov             " "rax, [rsp + 0x58]"),
        Q!("    xor             " "rax, r9"),
        Q!("    and             " "r9, r8"),
        Q!("    neg             " "r9"),
        Q!("    mul             " "r8"),
        Q!("    add             " "r15, rax"),
        Q!("    adc             " "r9, rdx"),
        Q!("    mov             " "rax, [rsp + 0x78]"),
        Q!("    xor             " "rax, r11"),
        Q!("    mov             " "rdx, r11"),
        Q!("    and             " "rdx, r10"),
        Q!("    sub             " "r9, rdx"),
        Q!("    mul             " "r10"),
        Q!("    add             " "r15, rax"),
        Q!("    adc             " "r9, rdx"),
        Q!("    mov             " "rax, r9"),
        Q!("    shld            " "rax, r15, 0x1"),
        Q!("    sar             " "r9, 0x3f"),
        Q!("    mov             " "ebx, 0x13"),
        Q!("    lea             " "rax, [rax + r9 + 0x1]"),
        Q!("    imul            " "rbx"),
        Q!("    xor             " "ebp, ebp"),
        Q!("    add             " "r12, rax"),
        Q!("    adc             " "r13, rdx"),
        Q!("    adc             " "r14, r9"),
        Q!("    adc             " "r15, r9"),
        Q!("    shl             " "rax, 0x3f"),
        Q!("    add             " "r15, rax"),
        Q!("    cmovns          " "rbx, rbp"),
        Q!("    sub             " "r12, rbx"),
        Q!("    sbb             " "r13, rbp"),
        Q!("    sbb             " "r14, rbp"),
        Q!("    sbb             " "r15, rbp"),
        Q!("    btr             " "r15, 0x3f"),
        Q!("    mov             " "rdi, [rsp + 0xc0]"),
        Q!("    mov             " "[rdi], r12"),
        Q!("    mov             " "[rdi + 0x8], r13"),
        Q!("    mov             " "[rdi + 0x10], r14"),
        Q!("    mov             " "[rdi + 0x18], r15"),

        // Now the result is xn * (1/zn), fully reduced modulo p.
        // Note that in the degenerate case zn = 0 (mod p_25519), the
        // modular inverse code above will produce 1/zn = 0, giving
        // the correct overall X25519 result of zero for the point at
        // infinity.

        Q!("    mov             " "rbp, " res!()),
        mul_p25519!(resx!(), xn!(), zn!()),

        // Restore stack and registers

        Q!("    add             " "rsp, " NSPACE!()),

        Q!("    pop             " "r15"),
        Q!("    pop             " "r14"),
        Q!("    pop             " "r13"),
        Q!("    pop             " "r12"),
        Q!("    pop             " "rbp"),
        Q!("    pop             " "rbx"),

        inout("rdi") res.as_mut_ptr() => _,
        inout("rsi") scalar.as_ptr() => _,
        inout("rdx") point.as_ptr() => _,
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
            )
    };
}
