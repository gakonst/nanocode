// generated source. do not edit.
#![allow(non_upper_case_globals, unused_macros, unused_imports)]
use crate::low::macros::*;

// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

// ----------------------------------------------------------------------------
// Double scalar multiplication for edwards25519, fresh and base point
// Input scalar[4], point[8], bscalar[4]; output res[8]
//
// extern void edwards25519_scalarmuldouble
//   (uint64_t res[static 8],const uint64_t scalar[static 4],
//    const uint64_t point[static 8],const uint64_t bscalar[static 4]);
//
// Given scalar = n, point = P and bscalar = m, returns in res
// the point (X,Y) = n * P + m * B where B = (...,4/5) is
// the standard basepoint for the edwards25519 (Ed25519) curve.
//
// Both 256-bit coordinates of the input point P are implicitly
// reduced modulo 2^255-19 if they are not already in reduced form,
// but the conventional usage is that they *are* already reduced.
// The scalars can be arbitrary 256-bit numbers but may also be
// considered as implicitly reduced modulo the group order.
//
// Standard x86-64 ABI: RDI = res, RSI = scalar, RDX = point, RCX = bscalar
// Microsoft x64 ABI:   RCX = res, RDX = scalar, R8 = point, R9 = bscalar
// ----------------------------------------------------------------------------

// Size of individual field elements

macro_rules! NUMSIZE {
    () => {
        "32"
    };
}

// Pointer-offset pairs for result and temporaries on stack with some aliasing.
// Both "resx" and "resy" assume the "res" pointer has been preloaded into rbp.

macro_rules! resx { () => { Q!("rbp + (0 * " NUMSIZE!() ")") } }
macro_rules! resy { () => { Q!("rbp + (1 * " NUMSIZE!() ")") } }

macro_rules! scalar { () => { Q!("rsp + (0 * " NUMSIZE!() ")") } }
macro_rules! bscalar { () => { Q!("rsp + (1 * " NUMSIZE!() ")") } }

macro_rules! tabent { () => { Q!("rsp + (2 * " NUMSIZE!() ")") } }
macro_rules! btabent { () => { Q!("rsp + (6 * " NUMSIZE!() ")") } }

macro_rules! acc { () => { Q!("rsp + (9 * " NUMSIZE!() ")") } }

macro_rules! tab { () => { Q!("rsp + (13 * " NUMSIZE!() ")") } }

// Additional variables kept on the stack

macro_rules! bf { () => { Q!("QWORD PTR [rsp + 45 * " NUMSIZE!() "]") } }
macro_rules! cf { () => { Q!("QWORD PTR [rsp + 45 * " NUMSIZE!() "+ 8]") } }
macro_rules! i { () => { Q!("QWORD PTR [rsp + 45 * " NUMSIZE!() "+ 16]") } }
macro_rules! res { () => { Q!("QWORD PTR [rsp + 45 * " NUMSIZE!() "+ 24]") } }

// Total size to reserve on the stack (excluding local subroutines)

macro_rules! NSPACE { () => { Q!("46 * " NUMSIZE!()) } }

// Syntactic variants to make x86_att forms easier to generate

macro_rules! SCALAR { () => { Q!("(0 * " NUMSIZE!() ")") } }
macro_rules! BSCALAR { () => { Q!("(1 * " NUMSIZE!() ")") } }
macro_rules! TABENT { () => { Q!("(2 * " NUMSIZE!() ")") } }
macro_rules! BTABENT { () => { Q!("(6 * " NUMSIZE!() ")") } }
macro_rules! ACC { () => { Q!("(9 * " NUMSIZE!() ")") } }
macro_rules! TAB { () => { Q!("(13 * " NUMSIZE!() ")") } }

// Sub-references used in local subroutines with local stack

macro_rules! x_0 {
    () => {
        "rdi + 0"
    };
}
macro_rules! y_0 { () => { Q!("rdi + " NUMSIZE!()) } }
macro_rules! z_0 { () => { Q!("rdi + (2 * " NUMSIZE!() ")") } }
macro_rules! w_0 { () => { Q!("rdi + (3 * " NUMSIZE!() ")") } }

macro_rules! x_1 {
    () => {
        "rsi + 0"
    };
}
macro_rules! y_1 { () => { Q!("rsi + " NUMSIZE!()) } }
macro_rules! z_1 { () => { Q!("rsi + (2 * " NUMSIZE!() ")") } }
macro_rules! w_1 { () => { Q!("rsi + (3 * " NUMSIZE!() ")") } }

macro_rules! x_2 {
    () => {
        "rbp + 0"
    };
}
macro_rules! y_2 { () => { Q!("rbp + " NUMSIZE!()) } }
macro_rules! z_2 { () => { Q!("rbp + (2 * " NUMSIZE!() ")") } }
macro_rules! w_2 { () => { Q!("rbp + (3 * " NUMSIZE!() ")") } }

macro_rules! t0 { () => { Q!("rsp + (0 * " NUMSIZE!() ")") } }
macro_rules! t1 { () => { Q!("rsp + (1 * " NUMSIZE!() ")") } }
macro_rules! t2 { () => { Q!("rsp + (2 * " NUMSIZE!() ")") } }
macro_rules! t3 { () => { Q!("rsp + (3 * " NUMSIZE!() ")") } }
macro_rules! t4 { () => { Q!("rsp + (4 * " NUMSIZE!() ")") } }
macro_rules! t5 { () => { Q!("rsp + (5 * " NUMSIZE!() ")") } }

// Macro wrapping up the basic field multiplication, only trivially
// different from a pure function call to bignum_mul_p25519.

macro_rules! mul_p25519 {
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
        "mov edx, 0x13;\n"
        "inc r12;\n"
        "bts r11, 63;\n"
        "mulx rbx, rax, r12;\n"
        "add r8, rax;\n"
        "adc r9, rbx;\n"
        "adc r10, rcx;\n"
        "adc r11, rcx;\n"
        "sbb rax, rax;\n"
        "not rax;\n"
        "and rax, rdx;\n"
        "sub r8, rax;\n"
        "sbb r9, rcx;\n"
        "sbb r10, rcx;\n"
        "sbb r11, rcx;\n"
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

// Modular addition and doubling with double modulus 2 * p_25519 = 2^256 - 38.
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

macro_rules! double_twice4 {
    ($P0:expr, $P1:expr) => { Q!(
        "mov r8, [" $P1 "];\n"
        "xor ecx, ecx;\n"
        "add r8, r8;\n"
        "mov r9, [" $P1 "+ 0x8];\n"
        "adc r9, r9;\n"
        "mov r10, [" $P1 "+ 0x10];\n"
        "adc r10, r10;\n"
        "mov r11, [" $P1 "+ 0x18];\n"
        "adc r11, r11;\n"
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

// Load the constant k_25519 = 2 * d_25519 using immediate operations

macro_rules! load_k25519 {
    ($P0:expr) => { Q!(
        "mov rax, 0xebd69b9426b2f159;\n"
        "mov [" $P0 "], rax;\n"
        "mov rax, 0x00e0149a8283b156;\n"
        "mov [" $P0 "+ 8], rax;\n"
        "mov rax, 0x198e80f2eef3d130;\n"
        "mov [" $P0 "+ 16], rax;\n"
        "mov rax, 0x2406d9dc56dffce7;\n"
        "mov [" $P0 "+ 24], rax"
    )}
}

/// Double scalar multiplication for edwards25519, fresh and base point
///
/// Input scalar[4], point[8], bscalar[4]; output res[8]
///
/// Given scalar = n, point = P and bscalar = m, returns in res
/// the point (X,Y) = n * P + m * B where B = (...,4/5) is
/// the standard basepoint for the edwards25519 (Ed25519) curve.
///
/// Both 256-bit coordinates of the input point P are implicitly
/// reduced modulo 2^255-19 if they are not already in reduced form,
/// but the conventional usage is that they *are* already reduced.
/// The scalars can be arbitrary 256-bit numbers but may also be
/// considered as implicitly reduced modulo the group order.
pub(crate) fn edwards25519_scalarmuldouble(
    res: &mut [u64; 8],
    scalar: &[u64; 4],
    point: &[u64; 8],
    bscalar: &[u64; 4],
) {
    // SAFETY: inline assembly. see [crate::low::inline_assembly_safety] for safety info.
    unsafe {
        core::arch::asm!(

        Q!("    endbr64         " ),

        // In this case the Windows form literally makes a subroutine call.
        // This avoids hassle arising from keeping code and data together.

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

        // Copy scalars while recoding all 4-bit nybbles except the top
        // one (bits 252..255) into signed 4-bit digits. This is essentially
        // done just by adding the recoding constant 0x0888..888, after
        // which all digits except the first have an implicit bias of -8,
        // so 0 -> -8, 1 -> -7, ... 7 -> -1, 8 -> 0, 9 -> 1, ... 15 -> 7.
        // (We could literally create 2s complement signed nybbles by
        // XORing with the same constant 0x0888..888 afterwards, but it
        // doesn't seem to make the end usage any simpler.)
        //
        // In order to ensure that the unrecoded top nybble (bits 252..255)
        // does not become > 8 as a result of carries lower down from the
        // recoding, we first (conceptually) subtract the group order iff
        // the top digit of the scalar is > 2^63. In the implementation the
        // reduction and recoding are combined by optionally using the
        // modified recoding constant 0x0888...888 + (2^256 - group_order).

        Q!("    mov             " "r8, [rcx]"),
        Q!("    mov             " "r9, [rcx + 8]"),
        Q!("    mov             " "r10, [rcx + 16]"),
        Q!("    mov             " "r11, [rcx + 24]"),
        Q!("    mov             " "r12, 0xc7f56fb5a0d9e920"),
        Q!("    mov             " "r13, 0xe190b99370cba1d5"),
        Q!("    mov             " "r14, 0x8888888888888887"),
        Q!("    mov             " "r15, 0x8888888888888888"),
        Q!("    mov             " "rax, 0x8000000000000000"),
        Q!("    mov             " "rbx, 0x0888888888888888"),
        Q!("    cmp             " "rax, r11"),
        Q!("    cmovnc          " "r12, r15"),
        Q!("    cmovnc          " "r13, r15"),
        Q!("    cmovnc          " "r14, r15"),
        Q!("    cmovnc          " "r15, rbx"),
        Q!("    add             " "r8, r12"),
        Q!("    adc             " "r9, r13"),
        Q!("    adc             " "r10, r14"),
        Q!("    adc             " "r11, r15"),
        Q!("    mov             " "[rsp + " BSCALAR!() "], r8"),
        Q!("    mov             " "[rsp + " BSCALAR!() "+ 8], r9"),
        Q!("    mov             " "[rsp + " BSCALAR!() "+ 16], r10"),
        Q!("    mov             " "[rsp + " BSCALAR!() "+ 24], r11"),

        Q!("    mov             " "r8, [rsi]"),
        Q!("    mov             " "r9, [rsi + 8]"),
        Q!("    mov             " "r10, [rsi + 16]"),
        Q!("    mov             " "r11, [rsi + 24]"),
        Q!("    mov             " "r12, 0xc7f56fb5a0d9e920"),
        Q!("    mov             " "r13, 0xe190b99370cba1d5"),
        Q!("    mov             " "r14, 0x8888888888888887"),
        Q!("    mov             " "r15, 0x8888888888888888"),
        Q!("    mov             " "rax, 0x8000000000000000"),
        Q!("    mov             " "rbx, 0x0888888888888888"),
        Q!("    cmp             " "rax, r11"),
        Q!("    cmovnc          " "r12, r15"),
        Q!("    cmovnc          " "r13, r15"),
        Q!("    cmovnc          " "r14, r15"),
        Q!("    cmovnc          " "r15, rbx"),
        Q!("    add             " "r8, r12"),
        Q!("    adc             " "r9, r13"),
        Q!("    adc             " "r10, r14"),
        Q!("    adc             " "r11, r15"),
        Q!("    mov             " "[rsp + " SCALAR!() "], r8"),
        Q!("    mov             " "[rsp + " SCALAR!() "+ 8], r9"),
        Q!("    mov             " "[rsp + " SCALAR!() "+ 16], r10"),
        Q!("    mov             " "[rsp + " SCALAR!() "+ 24], r11"),

        // Create table of multiples 1..8 of the general input point at "tab".
        // Reduce the input coordinates x and y modulo 2^256 - 38 first, for the
        // sake of definiteness; this is the reduction that will be maintained.
        // We could slightly optimize the additions because we know the input
        // point is affine (so Z = 1), but it doesn't seem worth the complication.

        Q!("    mov             " "eax, 38"),
        Q!("    mov             " "r8, [rdx]"),
        Q!("    xor             " "ebx, ebx"),
        Q!("    mov             " "r9, [rdx + 8]"),
        Q!("    xor             " "ecx, ecx"),
        Q!("    mov             " "r10, [rdx + 16]"),
        Q!("    xor             " "esi, esi"),
        Q!("    mov             " "r11, [rdx + 24]"),
        Q!("    add             " "rax, r8"),
        Q!("    adc             " "rbx, r9"),
        Q!("    adc             " "rcx, r10"),
        Q!("    adc             " "rsi, r11"),
        Q!("    cmovnc          " "rax, r8"),
        Q!("    mov             " "[rsp + " TAB!() "], rax"),
        Q!("    cmovnc          " "rbx, r9"),
        Q!("    mov             " "[rsp + " TAB!() "+ 8], rbx"),
        Q!("    cmovnc          " "rcx, r10"),
        Q!("    mov             " "[rsp + " TAB!() "+ 16], rcx"),
        Q!("    cmovnc          " "rsi, r11"),
        Q!("    mov             " "[rsp + " TAB!() "+ 24], rsi"),

        Q!("    mov             " "eax, 38"),
        Q!("    mov             " "r8, [rdx + 32]"),
        Q!("    xor             " "ebx, ebx"),
        Q!("    mov             " "r9, [rdx + 40]"),
        Q!("    xor             " "ecx, ecx"),
        Q!("    mov             " "r10, [rdx + 48]"),
        Q!("    xor             " "esi, esi"),
        Q!("    mov             " "r11, [rdx + 56]"),
        Q!("    add             " "rax, r8"),
        Q!("    adc             " "rbx, r9"),
        Q!("    adc             " "rcx, r10"),
        Q!("    adc             " "rsi, r11"),
        Q!("    cmovnc          " "rax, r8"),
        Q!("    mov             " "[rsp + " TAB!() "+ 32], rax"),
        Q!("    cmovnc          " "rbx, r9"),
        Q!("    mov             " "[rsp + " TAB!() "+ 40], rbx"),
        Q!("    cmovnc          " "rcx, r10"),
        Q!("    mov             " "[rsp + " TAB!() "+ 48], rcx"),
        Q!("    cmovnc          " "rsi, r11"),
        Q!("    mov             " "[rsp + " TAB!() "+ 56], rsi"),

        Q!("    mov             " "eax, 1"),
        Q!("    mov             " "[rsp + " TAB!() "+ 64], rax"),
        Q!("    xor             " "eax, eax"),
        Q!("    mov             " "[rsp + " TAB!() "+ 72], rax"),
        Q!("    mov             " "[rsp + " TAB!() "+ 80], rax"),
        Q!("    mov             " "[rsp + " TAB!() "+ 88], rax"),

        Q!("    lea             " "rdi, [rsp + " TAB!() "+ 96]"),
        Q!("    lea             " "rsi, [rsp + " TAB!() "]"),
        Q!("    lea             " "rbp, [rsp + " TAB!() "+ 32]"),
        mul_4!(x_0!(), x_1!(), x_2!()),

        // Multiple 2

        Q!("    lea             " "rdi, [rsp + " TAB!() "+ 1 * 128]"),
        Q!("    lea             " "rsi, [rsp + " TAB!() "]"),
        Q!("    call            " Label!("Ledwards25519_scalarmuldouble_epdouble", 2, After)),

        // Multiple 3

        Q!("    lea             " "rdi, [rsp + " TAB!() "+ 2 * 128]"),
        Q!("    lea             " "rsi, [rsp + " TAB!() "]"),
        Q!("    lea             " "rbp, [rsp + " TAB!() "+ 1 * 128]"),
        Q!("    call            " Label!("Ledwards25519_scalarmuldouble_epadd", 3, After)),

        // Multiple 4

        Q!("    lea             " "rdi, [rsp + " TAB!() "+ 3 * 128]"),
        Q!("    lea             " "rsi, [rsp + " TAB!() "+ 1 * 128]"),
        Q!("    call            " Label!("Ledwards25519_scalarmuldouble_epdouble", 2, After)),

        // Multiple 5

        Q!("    lea             " "rdi, [rsp + " TAB!() "+ 4 * 128]"),
        Q!("    lea             " "rsi, [rsp + " TAB!() "]"),
        Q!("    lea             " "rbp, [rsp + " TAB!() "+ 3 * 128]"),
        Q!("    call            " Label!("Ledwards25519_scalarmuldouble_epadd", 3, After)),

        // Multiple 6

        Q!("    lea             " "rdi, [rsp + " TAB!() "+ 5 * 128]"),
        Q!("    lea             " "rsi, [rsp + " TAB!() "+ 2 * 128]"),
        Q!("    call            " Label!("Ledwards25519_scalarmuldouble_epdouble", 2, After)),

        // Multiple 7

        Q!("    lea             " "rdi, [rsp + " TAB!() "+ 6 * 128]"),
        Q!("    lea             " "rsi, [rsp + " TAB!() "]"),
        Q!("    lea             " "rbp, [rsp + " TAB!() "+ 5 * 128]"),
        Q!("    call            " Label!("Ledwards25519_scalarmuldouble_epadd", 3, After)),

        // Multiple 8

        Q!("    lea             " "rdi, [rsp + " TAB!() "+ 7 * 128]"),
        Q!("    lea             " "rsi, [rsp + " TAB!() "+ 3 * 128]"),
        Q!("    call            " Label!("Ledwards25519_scalarmuldouble_epdouble", 2, After)),

        // Handle the initialization, starting the loop counter at i = 252
        // and initializing acc to the sum of the table entries for the
        // top nybbles of the scalars (the ones with no implicit -8 bias).

        Q!("    mov             " "rax, 252"),
        Q!("    mov             " i!() ", rax"),

        // Index for btable entry...

        Q!("    mov             " "rax, [rsp + " BSCALAR!() "+ 24]"),
        Q!("    shr             " "rax, 60"),
        Q!("    mov             " bf!() ", rax"),

        // ...and constant-time indexing based on that index

        Q!("    mov             " "eax, 1"),
        Q!("    xor             " "ebx, ebx"),
        Q!("    xor             " "ecx, ecx"),
        Q!("    xor             " "edx, edx"),
        Q!("    mov             " "r8d, 1"),
        Q!("    xor             " "r9d, r9d"),
        Q!("    xor             " "r10d, r10d"),
        Q!("    xor             " "r11d, r11d"),
        Q!("    xor             " "r12d, r12d"),
        Q!("    xor             " "r13d, r13d"),
        Q!("    xor             " "r14d, r14d"),
        Q!("    xor             " "r15d, r15d"),

        Q!("    lea             " "rbp, [rip + {Ledwards25519_scalarmuldouble_table}]"),

        Q!("    cmp             " bf!() ", 1"),
        Q!("    mov             " "rsi, [rbp]"),
        Q!("    cmovz           " "rax, rsi"),
        Q!("    mov             " "rsi, [rbp + 8]"),
        Q!("    cmovz           " "rbx, rsi"),
        Q!("    mov             " "rsi, [rbp + 16]"),
        Q!("    cmovz           " "rcx, rsi"),
        Q!("    mov             " "rsi, [rbp + 24]"),
        Q!("    cmovz           " "rdx, rsi"),
        Q!("    mov             " "rsi, [rbp + 32]"),
        Q!("    cmovz           " "r8, rsi"),
        Q!("    mov             " "rsi, [rbp + 40]"),
        Q!("    cmovz           " "r9, rsi"),
        Q!("    mov             " "rsi, [rbp + 48]"),
        Q!("    cmovz           " "r10, rsi"),
        Q!("    mov             " "rsi, [rbp + 56]"),
        Q!("    cmovz           " "r11, rsi"),
        Q!("    mov             " "rsi, [rbp + 64]"),
        Q!("    cmovz           " "r12, rsi"),
        Q!("    mov             " "rsi, [rbp + 72]"),
        Q!("    cmovz           " "r13, rsi"),
        Q!("    mov             " "rsi, [rbp + 80]"),
        Q!("    cmovz           " "r14, rsi"),
        Q!("    mov             " "rsi, [rbp + 88]"),
        Q!("    cmovz           " "r15, rsi"),
        Q!("    add             " "rbp, 96"),

        Q!("    cmp             " bf!() ", 2"),
        Q!("    mov             " "rsi, [rbp]"),
        Q!("    cmovz           " "rax, rsi"),
        Q!("    mov             " "rsi, [rbp + 8]"),
        Q!("    cmovz           " "rbx, rsi"),
        Q!("    mov             " "rsi, [rbp + 16]"),
        Q!("    cmovz           " "rcx, rsi"),
        Q!("    mov             " "rsi, [rbp + 24]"),
        Q!("    cmovz           " "rdx, rsi"),
        Q!("    mov             " "rsi, [rbp + 32]"),
        Q!("    cmovz           " "r8, rsi"),
        Q!("    mov             " "rsi, [rbp + 40]"),
        Q!("    cmovz           " "r9, rsi"),
        Q!("    mov             " "rsi, [rbp + 48]"),
        Q!("    cmovz           " "r10, rsi"),
        Q!("    mov             " "rsi, [rbp + 56]"),
        Q!("    cmovz           " "r11, rsi"),
        Q!("    mov             " "rsi, [rbp + 64]"),
        Q!("    cmovz           " "r12, rsi"),
        Q!("    mov             " "rsi, [rbp + 72]"),
        Q!("    cmovz           " "r13, rsi"),
        Q!("    mov             " "rsi, [rbp + 80]"),
        Q!("    cmovz           " "r14, rsi"),
        Q!("    mov             " "rsi, [rbp + 88]"),
        Q!("    cmovz           " "r15, rsi"),
        Q!("    add             " "rbp, 96"),

        Q!("    cmp             " bf!() ", 3"),
        Q!("    mov             " "rsi, [rbp]"),
        Q!("    cmovz           " "rax, rsi"),
        Q!("    mov             " "rsi, [rbp + 8]"),
        Q!("    cmovz           " "rbx, rsi"),
        Q!("    mov             " "rsi, [rbp + 16]"),
        Q!("    cmovz           " "rcx, rsi"),
        Q!("    mov             " "rsi, [rbp + 24]"),
        Q!("    cmovz           " "rdx, rsi"),
        Q!("    mov             " "rsi, [rbp + 32]"),
        Q!("    cmovz           " "r8, rsi"),
        Q!("    mov             " "rsi, [rbp + 40]"),
        Q!("    cmovz           " "r9, rsi"),
        Q!("    mov             " "rsi, [rbp + 48]"),
        Q!("    cmovz           " "r10, rsi"),
        Q!("    mov             " "rsi, [rbp + 56]"),
        Q!("    cmovz           " "r11, rsi"),
        Q!("    mov             " "rsi, [rbp + 64]"),
        Q!("    cmovz           " "r12, rsi"),
        Q!("    mov             " "rsi, [rbp + 72]"),
        Q!("    cmovz           " "r13, rsi"),
        Q!("    mov             " "rsi, [rbp + 80]"),
        Q!("    cmovz           " "r14, rsi"),
        Q!("    mov             " "rsi, [rbp + 88]"),
        Q!("    cmovz           " "r15, rsi"),
        Q!("    add             " "rbp, 96"),

        Q!("    cmp             " bf!() ", 4"),
        Q!("    mov             " "rsi, [rbp]"),
        Q!("    cmovz           " "rax, rsi"),
        Q!("    mov             " "rsi, [rbp + 8]"),
        Q!("    cmovz           " "rbx, rsi"),
        Q!("    mov             " "rsi, [rbp + 16]"),
        Q!("    cmovz           " "rcx, rsi"),
        Q!("    mov             " "rsi, [rbp + 24]"),
        Q!("    cmovz           " "rdx, rsi"),
        Q!("    mov             " "rsi, [rbp + 32]"),
        Q!("    cmovz           " "r8, rsi"),
        Q!("    mov             " "rsi, [rbp + 40]"),
        Q!("    cmovz           " "r9, rsi"),
        Q!("    mov             " "rsi, [rbp + 48]"),
        Q!("    cmovz           " "r10, rsi"),
        Q!("    mov             " "rsi, [rbp + 56]"),
        Q!("    cmovz           " "r11, rsi"),
        Q!("    mov             " "rsi, [rbp + 64]"),
        Q!("    cmovz           " "r12, rsi"),
        Q!("    mov             " "rsi, [rbp + 72]"),
        Q!("    cmovz           " "r13, rsi"),
        Q!("    mov             " "rsi, [rbp + 80]"),
        Q!("    cmovz           " "r14, rsi"),
        Q!("    mov             " "rsi, [rbp + 88]"),
        Q!("    cmovz           " "r15, rsi"),
        Q!("    add             " "rbp, 96"),

        Q!("    cmp             " bf!() ", 5"),
        Q!("    mov             " "rsi, [rbp]"),
        Q!("    cmovz           " "rax, rsi"),
        Q!("    mov             " "rsi, [rbp + 8]"),
        Q!("    cmovz           " "rbx, rsi"),
        Q!("    mov             " "rsi, [rbp + 16]"),
        Q!("    cmovz           " "rcx, rsi"),
        Q!("    mov             " "rsi, [rbp + 24]"),
        Q!("    cmovz           " "rdx, rsi"),
        Q!("    mov             " "rsi, [rbp + 32]"),
        Q!("    cmovz           " "r8, rsi"),
        Q!("    mov             " "rsi, [rbp + 40]"),
        Q!("    cmovz           " "r9, rsi"),
        Q!("    mov             " "rsi, [rbp + 48]"),
        Q!("    cmovz           " "r10, rsi"),
        Q!("    mov             " "rsi, [rbp + 56]"),
        Q!("    cmovz           " "r11, rsi"),
        Q!("    mov             " "rsi, [rbp + 64]"),
        Q!("    cmovz           " "r12, rsi"),
        Q!("    mov             " "rsi, [rbp + 72]"),
        Q!("    cmovz           " "r13, rsi"),
        Q!("    mov             " "rsi, [rbp + 80]"),
        Q!("    cmovz           " "r14, rsi"),
        Q!("    mov             " "rsi, [rbp + 88]"),
        Q!("    cmovz           " "r15, rsi"),
        Q!("    add             " "rbp, 96"),

        Q!("    cmp             " bf!() ", 6"),
        Q!("    mov             " "rsi, [rbp]"),
        Q!("    cmovz           " "rax, rsi"),
        Q!("    mov             " "rsi, [rbp + 8]"),
        Q!("    cmovz           " "rbx, rsi"),
        Q!("    mov             " "rsi, [rbp + 16]"),
        Q!("    cmovz           " "rcx, rsi"),
        Q!("    mov             " "rsi, [rbp + 24]"),
        Q!("    cmovz           " "rdx, rsi"),
        Q!("    mov             " "rsi, [rbp + 32]"),
        Q!("    cmovz           " "r8, rsi"),
        Q!("    mov             " "rsi, [rbp + 40]"),
        Q!("    cmovz           " "r9, rsi"),
        Q!("    mov             " "rsi, [rbp + 48]"),
        Q!("    cmovz           " "r10, rsi"),
        Q!("    mov             " "rsi, [rbp + 56]"),
        Q!("    cmovz           " "r11, rsi"),
        Q!("    mov             " "rsi, [rbp + 64]"),
        Q!("    cmovz           " "r12, rsi"),
        Q!("    mov             " "rsi, [rbp + 72]"),
        Q!("    cmovz           " "r13, rsi"),
        Q!("    mov             " "rsi, [rbp + 80]"),
        Q!("    cmovz           " "r14, rsi"),
        Q!("    mov             " "rsi, [rbp + 88]"),
        Q!("    cmovz           " "r15, rsi"),
        Q!("    add             " "rbp, 96"),

        Q!("    cmp             " bf!() ", 7"),
        Q!("    mov             " "rsi, [rbp]"),
        Q!("    cmovz           " "rax, rsi"),
        Q!("    mov             " "rsi, [rbp + 8]"),
        Q!("    cmovz           " "rbx, rsi"),
        Q!("    mov             " "rsi, [rbp + 16]"),
        Q!("    cmovz           " "rcx, rsi"),
        Q!("    mov             " "rsi, [rbp + 24]"),
        Q!("    cmovz           " "rdx, rsi"),
        Q!("    mov             " "rsi, [rbp + 32]"),
        Q!("    cmovz           " "r8, rsi"),
        Q!("    mov             " "rsi, [rbp + 40]"),
        Q!("    cmovz           " "r9, rsi"),
        Q!("    mov             " "rsi, [rbp + 48]"),
        Q!("    cmovz           " "r10, rsi"),
        Q!("    mov             " "rsi, [rbp + 56]"),
        Q!("    cmovz           " "r11, rsi"),
        Q!("    mov             " "rsi, [rbp + 64]"),
        Q!("    cmovz           " "r12, rsi"),
        Q!("    mov             " "rsi, [rbp + 72]"),
        Q!("    cmovz           " "r13, rsi"),
        Q!("    mov             " "rsi, [rbp + 80]"),
        Q!("    cmovz           " "r14, rsi"),
        Q!("    mov             " "rsi, [rbp + 88]"),
        Q!("    cmovz           " "r15, rsi"),
        Q!("    add             " "rbp, 96"),

        Q!("    cmp             " bf!() ", 8"),
        Q!("    mov             " "rsi, [rbp]"),
        Q!("    cmovz           " "rax, rsi"),
        Q!("    mov             " "rsi, [rbp + 8]"),
        Q!("    cmovz           " "rbx, rsi"),
        Q!("    mov             " "rsi, [rbp + 16]"),
        Q!("    cmovz           " "rcx, rsi"),
        Q!("    mov             " "rsi, [rbp + 24]"),
        Q!("    cmovz           " "rdx, rsi"),
        Q!("    mov             " "rsi, [rbp + 32]"),
        Q!("    cmovz           " "r8, rsi"),
        Q!("    mov             " "rsi, [rbp + 40]"),
        Q!("    cmovz           " "r9, rsi"),
        Q!("    mov             " "rsi, [rbp + 48]"),
        Q!("    cmovz           " "r10, rsi"),
        Q!("    mov             " "rsi, [rbp + 56]"),
        Q!("    cmovz           " "r11, rsi"),
        Q!("    mov             " "rsi, [rbp + 64]"),
        Q!("    cmovz           " "r12, rsi"),
        Q!("    mov             " "rsi, [rbp + 72]"),
        Q!("    cmovz           " "r13, rsi"),
        Q!("    mov             " "rsi, [rbp + 80]"),
        Q!("    cmovz           " "r14, rsi"),
        Q!("    mov             " "rsi, [rbp + 88]"),
        Q!("    cmovz           " "r15, rsi"),

        Q!("    mov             " "[rsp + " BTABENT!() "], rax"),
        Q!("    mov             " "[rsp + " BTABENT!() "+ 8], rbx"),
        Q!("    mov             " "[rsp + " BTABENT!() "+ 16], rcx"),
        Q!("    mov             " "[rsp + " BTABENT!() "+ 24], rdx"),
        Q!("    mov             " "[rsp + " BTABENT!() "+ 32], r8"),
        Q!("    mov             " "[rsp + " BTABENT!() "+ 40], r9"),
        Q!("    mov             " "[rsp + " BTABENT!() "+ 48], r10"),
        Q!("    mov             " "[rsp + " BTABENT!() "+ 56], r11"),
        Q!("    mov             " "[rsp + " BTABENT!() "+ 64], r12"),
        Q!("    mov             " "[rsp + " BTABENT!() "+ 72], r13"),
        Q!("    mov             " "[rsp + " BTABENT!() "+ 80], r14"),
        Q!("    mov             " "[rsp + " BTABENT!() "+ 88], r15"),

        // Index for table entry...

        Q!("    mov             " "rax, [rsp + " SCALAR!() "+ 24]"),
        Q!("    shr             " "rax, 60"),
        Q!("    mov             " bf!() ", rax"),

        // ...and constant-time indexing based on that index.
        // Do the Y and Z fields first, to save on registers...

        Q!("    mov             " "eax, 1"),
        Q!("    xor             " "ebx, ebx"),
        Q!("    xor             " "ecx, ecx"),
        Q!("    xor             " "edx, edx"),
        Q!("    mov             " "r8d, 1"),
        Q!("    xor             " "r9d, r9d"),
        Q!("    xor             " "r10d, r10d"),
        Q!("    xor             " "r11d, r11d"),

        Q!("    lea             " "rbp, [rsp + " TAB!() "+ 32]"),

        Q!("    cmp             " bf!() ", 1"),
        Q!("    mov             " "rsi, [rbp]"),
        Q!("    cmovz           " "rax, rsi"),
        Q!("    mov             " "rsi, [rbp + 8]"),
        Q!("    cmovz           " "rbx, rsi"),
        Q!("    mov             " "rsi, [rbp + 16]"),
        Q!("    cmovz           " "rcx, rsi"),
        Q!("    mov             " "rsi, [rbp + 24]"),
        Q!("    cmovz           " "rdx, rsi"),
        Q!("    mov             " "rsi, [rbp + 32]"),
        Q!("    cmovz           " "r8, rsi"),
        Q!("    mov             " "rsi, [rbp + 40]"),
        Q!("    cmovz           " "r9, rsi"),
        Q!("    mov             " "rsi, [rbp + 48]"),
        Q!("    cmovz           " "r10, rsi"),
        Q!("    mov             " "rsi, [rbp + 56]"),
        Q!("    cmovz           " "r11, rsi"),
        Q!("    add             " "rbp, 128"),

        Q!("    cmp             " bf!() ", 2"),
        Q!("    mov             " "rsi, [rbp]"),
        Q!("    cmovz           " "rax, rsi"),
        Q!("    mov             " "rsi, [rbp + 8]"),
        Q!("    cmovz           " "rbx, rsi"),
        Q!("    mov             " "rsi, [rbp + 16]"),
        Q!("    cmovz           " "rcx, rsi"),
        Q!("    mov             " "rsi, [rbp + 24]"),
        Q!("    cmovz           " "rdx, rsi"),
        Q!("    mov             " "rsi, [rbp + 32]"),
        Q!("    cmovz           " "r8, rsi"),
        Q!("    mov             " "rsi, [rbp + 40]"),
        Q!("    cmovz           " "r9, rsi"),
        Q!("    mov             " "rsi, [rbp + 48]"),
        Q!("    cmovz           " "r10, rsi"),
        Q!("    mov             " "rsi, [rbp + 56]"),
        Q!("    cmovz           " "r11, rsi"),
        Q!("    add             " "rbp, 128"),

        Q!("    cmp             " bf!() ", 3"),
        Q!("    mov             " "rsi, [rbp]"),
        Q!("    cmovz           " "rax, rsi"),
        Q!("    mov             " "rsi, [rbp + 8]"),
        Q!("    cmovz           " "rbx, rsi"),
        Q!("    mov             " "rsi, [rbp + 16]"),
        Q!("    cmovz           " "rcx, rsi"),
        Q!("    mov             " "rsi, [rbp + 24]"),
        Q!("    cmovz           " "rdx, rsi"),
        Q!("    mov             " "rsi, [rbp + 32]"),
        Q!("    cmovz           " "r8, rsi"),
        Q!("    mov             " "rsi, [rbp + 40]"),
        Q!("    cmovz           " "r9, rsi"),
        Q!("    mov             " "rsi, [rbp + 48]"),
        Q!("    cmovz           " "r10, rsi"),
        Q!("    mov             " "rsi, [rbp + 56]"),
        Q!("    cmovz           " "r11, rsi"),
        Q!("    add             " "rbp, 128"),

        Q!("    cmp             " bf!() ", 4"),
        Q!("    mov             " "rsi, [rbp]"),
        Q!("    cmovz           " "rax, rsi"),
        Q!("    mov             " "rsi, [rbp + 8]"),
        Q!("    cmovz           " "rbx, rsi"),
        Q!("    mov             " "rsi, [rbp + 16]"),
        Q!("    cmovz           " "rcx, rsi"),
        Q!("    mov             " "rsi, [rbp + 24]"),
        Q!("    cmovz           " "rdx, rsi"),
        Q!("    mov             " "rsi, [rbp + 32]"),
        Q!("    cmovz           " "r8, rsi"),
        Q!("    mov             " "rsi, [rbp + 40]"),
        Q!("    cmovz           " "r9, rsi"),
        Q!("    mov             " "rsi, [rbp + 48]"),
        Q!("    cmovz           " "r10, rsi"),
        Q!("    mov             " "rsi, [rbp + 56]"),
        Q!("    cmovz           " "r11, rsi"),
        Q!("    add             " "rbp, 128"),

        Q!("    cmp             " bf!() ", 5"),
        Q!("    mov             " "rsi, [rbp]"),
        Q!("    cmovz           " "rax, rsi"),
        Q!("    mov             " "rsi, [rbp + 8]"),
        Q!("    cmovz           " "rbx, rsi"),
        Q!("    mov             " "rsi, [rbp + 16]"),
        Q!("    cmovz           " "rcx, rsi"),
        Q!("    mov             " "rsi, [rbp + 24]"),
        Q!("    cmovz           " "rdx, rsi"),
        Q!("    mov             " "rsi, [rbp + 32]"),
        Q!("    cmovz           " "r8, rsi"),
        Q!("    mov             " "rsi, [rbp + 40]"),
        Q!("    cmovz           " "r9, rsi"),
        Q!("    mov             " "rsi, [rbp + 48]"),
        Q!("    cmovz           " "r10, rsi"),
        Q!("    mov             " "rsi, [rbp + 56]"),
        Q!("    cmovz           " "r11, rsi"),
        Q!("    add             " "rbp, 128"),

        Q!("    cmp             " bf!() ", 6"),
        Q!("    mov             " "rsi, [rbp]"),
        Q!("    cmovz           " "rax, rsi"),
        Q!("    mov             " "rsi, [rbp + 8]"),
        Q!("    cmovz           " "rbx, rsi"),
        Q!("    mov             " "rsi, [rbp + 16]"),
        Q!("    cmovz           " "rcx, rsi"),
        Q!("    mov             " "rsi, [rbp + 24]"),
        Q!("    cmovz           " "rdx, rsi"),
        Q!("    mov             " "rsi, [rbp + 32]"),
        Q!("    cmovz           " "r8, rsi"),
        Q!("    mov             " "rsi, [rbp + 40]"),
        Q!("    cmovz           " "r9, rsi"),
        Q!("    mov             " "rsi, [rbp + 48]"),
        Q!("    cmovz           " "r10, rsi"),
        Q!("    mov             " "rsi, [rbp + 56]"),
        Q!("    cmovz           " "r11, rsi"),
        Q!("    add             " "rbp, 128"),

        Q!("    cmp             " bf!() ", 7"),
        Q!("    mov             " "rsi, [rbp]"),
        Q!("    cmovz           " "rax, rsi"),
        Q!("    mov             " "rsi, [rbp + 8]"),
        Q!("    cmovz           " "rbx, rsi"),
        Q!("    mov             " "rsi, [rbp + 16]"),
        Q!("    cmovz           " "rcx, rsi"),
        Q!("    mov             " "rsi, [rbp + 24]"),
        Q!("    cmovz           " "rdx, rsi"),
        Q!("    mov             " "rsi, [rbp + 32]"),
        Q!("    cmovz           " "r8, rsi"),
        Q!("    mov             " "rsi, [rbp + 40]"),
        Q!("    cmovz           " "r9, rsi"),
        Q!("    mov             " "rsi, [rbp + 48]"),
        Q!("    cmovz           " "r10, rsi"),
        Q!("    mov             " "rsi, [rbp + 56]"),
        Q!("    cmovz           " "r11, rsi"),
        Q!("    add             " "rbp, 128"),

        Q!("    cmp             " bf!() ", 8"),
        Q!("    mov             " "rsi, [rbp]"),
        Q!("    cmovz           " "rax, rsi"),
        Q!("    mov             " "rsi, [rbp + 8]"),
        Q!("    cmovz           " "rbx, rsi"),
        Q!("    mov             " "rsi, [rbp + 16]"),
        Q!("    cmovz           " "rcx, rsi"),
        Q!("    mov             " "rsi, [rbp + 24]"),
        Q!("    cmovz           " "rdx, rsi"),
        Q!("    mov             " "rsi, [rbp + 32]"),
        Q!("    cmovz           " "r8, rsi"),
        Q!("    mov             " "rsi, [rbp + 40]"),
        Q!("    cmovz           " "r9, rsi"),
        Q!("    mov             " "rsi, [rbp + 48]"),
        Q!("    cmovz           " "r10, rsi"),
        Q!("    mov             " "rsi, [rbp + 56]"),
        Q!("    cmovz           " "r11, rsi"),

        Q!("    mov             " "[rsp + " TABENT!() "+ 32], rax"),
        Q!("    mov             " "[rsp + " TABENT!() "+ 40], rbx"),
        Q!("    mov             " "[rsp + " TABENT!() "+ 48], rcx"),
        Q!("    mov             " "[rsp + " TABENT!() "+ 56], rdx"),
        Q!("    mov             " "[rsp + " TABENT!() "+ 64], r8"),
        Q!("    mov             " "[rsp + " TABENT!() "+ 72], r9"),
        Q!("    mov             " "[rsp + " TABENT!() "+ 80], r10"),
        Q!("    mov             " "[rsp + " TABENT!() "+ 88], r11"),

        // ...followed by the X and W fields

        Q!("    lea             " "rbp, [rsp + " TAB!() "]"),

        Q!("    xor             " "eax, eax"),
        Q!("    xor             " "ebx, ebx"),
        Q!("    xor             " "ecx, ecx"),
        Q!("    xor             " "edx, edx"),
        Q!("    xor             " "r8d, r8d"),
        Q!("    xor             " "r9d, r9d"),
        Q!("    xor             " "r10d, r10d"),
        Q!("    xor             " "r11d, r11d"),

        Q!("    cmp             " bf!() ", 1"),
        Q!("    mov             " "rsi, [rbp]"),
        Q!("    cmovz           " "rax, rsi"),
        Q!("    mov             " "rsi, [rbp + 8]"),
        Q!("    cmovz           " "rbx, rsi"),
        Q!("    mov             " "rsi, [rbp + 16]"),
        Q!("    cmovz           " "rcx, rsi"),
        Q!("    mov             " "rsi, [rbp + 24]"),
        Q!("    cmovz           " "rdx, rsi"),
        Q!("    mov             " "rsi, [rbp + 96]"),
        Q!("    cmovz           " "r8, rsi"),
        Q!("    mov             " "rsi, [rbp + 104]"),
        Q!("    cmovz           " "r9, rsi"),
        Q!("    mov             " "rsi, [rbp + 112]"),
        Q!("    cmovz           " "r10, rsi"),
        Q!("    mov             " "rsi, [rbp + 120]"),
        Q!("    cmovz           " "r11, rsi"),
        Q!("    add             " "rbp, 128"),

        Q!("    cmp             " bf!() ", 2"),
        Q!("    mov             " "rsi, [rbp]"),
        Q!("    cmovz           " "rax, rsi"),
        Q!("    mov             " "rsi, [rbp + 8]"),
        Q!("    cmovz           " "rbx, rsi"),
        Q!("    mov             " "rsi, [rbp + 16]"),
        Q!("    cmovz           " "rcx, rsi"),
        Q!("    mov             " "rsi, [rbp + 24]"),
        Q!("    cmovz           " "rdx, rsi"),
        Q!("    mov             " "rsi, [rbp + 96]"),
        Q!("    cmovz           " "r8, rsi"),
        Q!("    mov             " "rsi, [rbp + 104]"),
        Q!("    cmovz           " "r9, rsi"),
        Q!("    mov             " "rsi, [rbp + 112]"),
        Q!("    cmovz           " "r10, rsi"),
        Q!("    mov             " "rsi, [rbp + 120]"),
        Q!("    cmovz           " "r11, rsi"),
        Q!("    add             " "rbp, 128"),

        Q!("    cmp             " bf!() ", 3"),
        Q!("    mov             " "rsi, [rbp]"),
        Q!("    cmovz           " "rax, rsi"),
        Q!("    mov             " "rsi, [rbp + 8]"),
        Q!("    cmovz           " "rbx, rsi"),
        Q!("    mov             " "rsi, [rbp + 16]"),
        Q!("    cmovz           " "rcx, rsi"),
        Q!("    mov             " "rsi, [rbp + 24]"),
        Q!("    cmovz           " "rdx, rsi"),
        Q!("    mov             " "rsi, [rbp + 96]"),
        Q!("    cmovz           " "r8, rsi"),
        Q!("    mov             " "rsi, [rbp + 104]"),
        Q!("    cmovz           " "r9, rsi"),
        Q!("    mov             " "rsi, [rbp + 112]"),
        Q!("    cmovz           " "r10, rsi"),
        Q!("    mov             " "rsi, [rbp + 120]"),
        Q!("    cmovz           " "r11, rsi"),
        Q!("    add             " "rbp, 128"),

        Q!("    cmp             " bf!() ", 4"),
        Q!("    mov             " "rsi, [rbp]"),
        Q!("    cmovz           " "rax, rsi"),
        Q!("    mov             " "rsi, [rbp + 8]"),
        Q!("    cmovz           " "rbx, rsi"),
        Q!("    mov             " "rsi, [rbp + 16]"),
        Q!("    cmovz           " "rcx, rsi"),
        Q!("    mov             " "rsi, [rbp + 24]"),
        Q!("    cmovz           " "rdx, rsi"),
        Q!("    mov             " "rsi, [rbp + 96]"),
        Q!("    cmovz           " "r8, rsi"),
        Q!("    mov             " "rsi, [rbp + 104]"),
        Q!("    cmovz           " "r9, rsi"),
        Q!("    mov             " "rsi, [rbp + 112]"),
        Q!("    cmovz           " "r10, rsi"),
        Q!("    mov             " "rsi, [rbp + 120]"),
        Q!("    cmovz           " "r11, rsi"),
        Q!("    add             " "rbp, 128"),

        Q!("    cmp             " bf!() ", 5"),
        Q!("    mov             " "rsi, [rbp]"),
        Q!("    cmovz           " "rax, rsi"),
        Q!("    mov             " "rsi, [rbp + 8]"),
        Q!("    cmovz           " "rbx, rsi"),
        Q!("    mov             " "rsi, [rbp + 16]"),
        Q!("    cmovz           " "rcx, rsi"),
        Q!("    mov             " "rsi, [rbp + 24]"),
        Q!("    cmovz           " "rdx, rsi"),
        Q!("    mov             " "rsi, [rbp + 96]"),
        Q!("    cmovz           " "r8, rsi"),
        Q!("    mov             " "rsi, [rbp + 104]"),
        Q!("    cmovz           " "r9, rsi"),
        Q!("    mov             " "rsi, [rbp + 112]"),
        Q!("    cmovz           " "r10, rsi"),
        Q!("    mov             " "rsi, [rbp + 120]"),
        Q!("    cmovz           " "r11, rsi"),
        Q!("    add             " "rbp, 128"),

        Q!("    cmp             " bf!() ", 6"),
        Q!("    mov             " "rsi, [rbp]"),
        Q!("    cmovz           " "rax, rsi"),
        Q!("    mov             " "rsi, [rbp + 8]"),
        Q!("    cmovz           " "rbx, rsi"),
        Q!("    mov             " "rsi, [rbp + 16]"),
        Q!("    cmovz           " "rcx, rsi"),
        Q!("    mov             " "rsi, [rbp + 24]"),
        Q!("    cmovz           " "rdx, rsi"),
        Q!("    mov             " "rsi, [rbp + 96]"),
        Q!("    cmovz           " "r8, rsi"),
        Q!("    mov             " "rsi, [rbp + 104]"),
        Q!("    cmovz           " "r9, rsi"),
        Q!("    mov             " "rsi, [rbp + 112]"),
        Q!("    cmovz           " "r10, rsi"),
        Q!("    mov             " "rsi, [rbp + 120]"),
        Q!("    cmovz           " "r11, rsi"),
        Q!("    add             " "rbp, 128"),

        Q!("    cmp             " bf!() ", 7"),
        Q!("    mov             " "rsi, [rbp]"),
        Q!("    cmovz           " "rax, rsi"),
        Q!("    mov             " "rsi, [rbp + 8]"),
        Q!("    cmovz           " "rbx, rsi"),
        Q!("    mov             " "rsi, [rbp + 16]"),
        Q!("    cmovz           " "rcx, rsi"),
        Q!("    mov             " "rsi, [rbp + 24]"),
        Q!("    cmovz           " "rdx, rsi"),
        Q!("    mov             " "rsi, [rbp + 96]"),
        Q!("    cmovz           " "r8, rsi"),
        Q!("    mov             " "rsi, [rbp + 104]"),
        Q!("    cmovz           " "r9, rsi"),
        Q!("    mov             " "rsi, [rbp + 112]"),
        Q!("    cmovz           " "r10, rsi"),
        Q!("    mov             " "rsi, [rbp + 120]"),
        Q!("    cmovz           " "r11, rsi"),
        Q!("    add             " "rbp, 128"),

        Q!("    cmp             " bf!() ", 8"),
        Q!("    mov             " "rsi, [rbp]"),
        Q!("    cmovz           " "rax, rsi"),
        Q!("    mov             " "rsi, [rbp + 8]"),
        Q!("    cmovz           " "rbx, rsi"),
        Q!("    mov             " "rsi, [rbp + 16]"),
        Q!("    cmovz           " "rcx, rsi"),
        Q!("    mov             " "rsi, [rbp + 24]"),
        Q!("    cmovz           " "rdx, rsi"),
        Q!("    mov             " "rsi, [rbp + 96]"),
        Q!("    cmovz           " "r8, rsi"),
        Q!("    mov             " "rsi, [rbp + 104]"),
        Q!("    cmovz           " "r9, rsi"),
        Q!("    mov             " "rsi, [rbp + 112]"),
        Q!("    cmovz           " "r10, rsi"),
        Q!("    mov             " "rsi, [rbp + 120]"),
        Q!("    cmovz           " "r11, rsi"),

        Q!("    mov             " "[rsp + " TABENT!() "], rax"),
        Q!("    mov             " "[rsp + " TABENT!() "+ 8], rbx"),
        Q!("    mov             " "[rsp + " TABENT!() "+ 16], rcx"),
        Q!("    mov             " "[rsp + " TABENT!() "+ 24], rdx"),
        Q!("    mov             " "[rsp + " TABENT!() "+ 96], r8"),
        Q!("    mov             " "[rsp + " TABENT!() "+ 104], r9"),
        Q!("    mov             " "[rsp + " TABENT!() "+ 112], r10"),
        Q!("    mov             " "[rsp + " TABENT!() "+ 120], r11"),

        // Add those elements to initialize the accumulator for bit position 252

        Q!("    lea             " "rdi, [rsp + " ACC!() "]"),
        Q!("    lea             " "rsi, [rsp + " TABENT!() "]"),
        Q!("    lea             " "rbp, [rsp + " BTABENT!() "]"),
        Q!("    call            " Label!("Ledwards25519_scalarmuldouble_pepadd", 4, After)),

        // Main loop with acc = [scalar/2^i] * point + [bscalar/2^i] * basepoint
        // Start with i = 252 for bits 248..251 and go down four at a time to 3..0

        Q!(Label!("Ledwards25519_scalarmuldouble_loop", 5) ":"),

        Q!("    mov             " "rax, " i!()),
        Q!("    sub             " "rax, 4"),
        Q!("    mov             " i!() ", rax"),

        // Double to acc' = 2 * acc

        Q!("    lea             " "rdi, [rsp + " ACC!() "]"),
        Q!("    lea             " "rsi, [rsp + " ACC!() "]"),
        Q!("    call            " Label!("Ledwards25519_scalarmuldouble_pdouble", 6, After)),

        // Get btable entry, first getting the adjusted bitfield...

        Q!("    mov             " "rax, " i!()),
        Q!("    mov             " "rcx, rax"),
        Q!("    shr             " "rax, 6"),
        Q!("    mov             " "rax, [rsp + 8 * rax + 32]"),
        Q!("    shr             " "rax, cl"),
        Q!("    and             " "rax, 15"),

        Q!("    sub             " "rax, 8"),
        Q!("    sbb             " "rcx, rcx"),
        Q!("    xor             " "rax, rcx"),
        Q!("    sub             " "rax, rcx"),
        Q!("    mov             " cf!() ", rcx"),
        Q!("    mov             " bf!() ", rax"),

        // ... then doing constant-time lookup with the appropriate index...

        Q!("    mov             " "eax, 1"),
        Q!("    xor             " "ebx, ebx"),
        Q!("    xor             " "ecx, ecx"),
        Q!("    xor             " "edx, edx"),
        Q!("    mov             " "r8d, 1"),
        Q!("    xor             " "r9d, r9d"),
        Q!("    xor             " "r10d, r10d"),
        Q!("    xor             " "r11d, r11d"),
        Q!("    xor             " "r12d, r12d"),
        Q!("    xor             " "r13d, r13d"),
        Q!("    xor             " "r14d, r14d"),
        Q!("    xor             " "r15d, r15d"),

        Q!("    lea             " "rbp, [rip + {Ledwards25519_scalarmuldouble_table}]"),

        Q!("    cmp             " bf!() ", 1"),
        Q!("    mov             " "rsi, [rbp]"),
        Q!("    cmovz           " "rax, rsi"),
        Q!("    mov             " "rsi, [rbp + 8]"),
        Q!("    cmovz           " "rbx, rsi"),
        Q!("    mov             " "rsi, [rbp + 16]"),
        Q!("    cmovz           " "rcx, rsi"),
        Q!("    mov             " "rsi, [rbp + 24]"),
        Q!("    cmovz           " "rdx, rsi"),
        Q!("    mov             " "rsi, [rbp + 32]"),
        Q!("    cmovz           " "r8, rsi"),
        Q!("    mov             " "rsi, [rbp + 40]"),
        Q!("    cmovz           " "r9, rsi"),
        Q!("    mov             " "rsi, [rbp + 48]"),
        Q!("    cmovz           " "r10, rsi"),
        Q!("    mov             " "rsi, [rbp + 56]"),
        Q!("    cmovz           " "r11, rsi"),
        Q!("    mov             " "rsi, [rbp + 64]"),
        Q!("    cmovz           " "r12, rsi"),
        Q!("    mov             " "rsi, [rbp + 72]"),
        Q!("    cmovz           " "r13, rsi"),
        Q!("    mov             " "rsi, [rbp + 80]"),
        Q!("    cmovz           " "r14, rsi"),
        Q!("    mov             " "rsi, [rbp + 88]"),
        Q!("    cmovz           " "r15, rsi"),
        Q!("    add             " "rbp, 96"),

        Q!("    cmp             " bf!() ", 2"),
        Q!("    mov             " "rsi, [rbp]"),
        Q!("    cmovz           " "rax, rsi"),
        Q!("    mov             " "rsi, [rbp + 8]"),
        Q!("    cmovz           " "rbx, rsi"),
        Q!("    mov             " "rsi, [rbp + 16]"),
        Q!("    cmovz           " "rcx, rsi"),
        Q!("    mov             " "rsi, [rbp + 24]"),
        Q!("    cmovz           " "rdx, rsi"),
        Q!("    mov             " "rsi, [rbp + 32]"),
        Q!("    cmovz           " "r8, rsi"),
        Q!("    mov             " "rsi, [rbp + 40]"),
        Q!("    cmovz           " "r9, rsi"),
        Q!("    mov             " "rsi, [rbp + 48]"),
        Q!("    cmovz           " "r10, rsi"),
        Q!("    mov             " "rsi, [rbp + 56]"),
        Q!("    cmovz           " "r11, rsi"),
        Q!("    mov             " "rsi, [rbp + 64]"),
        Q!("    cmovz           " "r12, rsi"),
        Q!("    mov             " "rsi, [rbp + 72]"),
        Q!("    cmovz           " "r13, rsi"),
        Q!("    mov             " "rsi, [rbp + 80]"),
        Q!("    cmovz           " "r14, rsi"),
        Q!("    mov             " "rsi, [rbp + 88]"),
        Q!("    cmovz           " "r15, rsi"),
        Q!("    add             " "rbp, 96"),

        Q!("    cmp             " bf!() ", 3"),
        Q!("    mov             " "rsi, [rbp]"),
        Q!("    cmovz           " "rax, rsi"),
        Q!("    mov             " "rsi, [rbp + 8]"),
        Q!("    cmovz           " "rbx, rsi"),
        Q!("    mov             " "rsi, [rbp + 16]"),
        Q!("    cmovz           " "rcx, rsi"),
        Q!("    mov             " "rsi, [rbp + 24]"),
        Q!("    cmovz           " "rdx, rsi"),
        Q!("    mov             " "rsi, [rbp + 32]"),
        Q!("    cmovz           " "r8, rsi"),
        Q!("    mov             " "rsi, [rbp + 40]"),
        Q!("    cmovz           " "r9, rsi"),
        Q!("    mov             " "rsi, [rbp + 48]"),
        Q!("    cmovz           " "r10, rsi"),
        Q!("    mov             " "rsi, [rbp + 56]"),
        Q!("    cmovz           " "r11, rsi"),
        Q!("    mov             " "rsi, [rbp + 64]"),
        Q!("    cmovz           " "r12, rsi"),
        Q!("    mov             " "rsi, [rbp + 72]"),
        Q!("    cmovz           " "r13, rsi"),
        Q!("    mov             " "rsi, [rbp + 80]"),
        Q!("    cmovz           " "r14, rsi"),
        Q!("    mov             " "rsi, [rbp + 88]"),
        Q!("    cmovz           " "r15, rsi"),
        Q!("    add             " "rbp, 96"),

        Q!("    cmp             " bf!() ", 4"),
        Q!("    mov             " "rsi, [rbp]"),
        Q!("    cmovz           " "rax, rsi"),
        Q!("    mov             " "rsi, [rbp + 8]"),
        Q!("    cmovz           " "rbx, rsi"),
        Q!("    mov             " "rsi, [rbp + 16]"),
        Q!("    cmovz           " "rcx, rsi"),
        Q!("    mov             " "rsi, [rbp + 24]"),
        Q!("    cmovz           " "rdx, rsi"),
        Q!("    mov             " "rsi, [rbp + 32]"),
        Q!("    cmovz           " "r8, rsi"),
        Q!("    mov             " "rsi, [rbp + 40]"),
        Q!("    cmovz           " "r9, rsi"),
        Q!("    mov             " "rsi, [rbp + 48]"),
        Q!("    cmovz           " "r10, rsi"),
        Q!("    mov             " "rsi, [rbp + 56]"),
        Q!("    cmovz           " "r11, rsi"),
        Q!("    mov             " "rsi, [rbp + 64]"),
        Q!("    cmovz           " "r12, rsi"),
        Q!("    mov             " "rsi, [rbp + 72]"),
        Q!("    cmovz           " "r13, rsi"),
        Q!("    mov             " "rsi, [rbp + 80]"),
        Q!("    cmovz           " "r14, rsi"),
        Q!("    mov             " "rsi, [rbp + 88]"),
        Q!("    cmovz           " "r15, rsi"),
        Q!("    add             " "rbp, 96"),

        Q!("    cmp             " bf!() ", 5"),
        Q!("    mov             " "rsi, [rbp]"),
        Q!("    cmovz           " "rax, rsi"),
        Q!("    mov             " "rsi, [rbp + 8]"),
        Q!("    cmovz           " "rbx, rsi"),
        Q!("    mov             " "rsi, [rbp + 16]"),
        Q!("    cmovz           " "rcx, rsi"),
        Q!("    mov             " "rsi, [rbp + 24]"),
        Q!("    cmovz           " "rdx, rsi"),
        Q!("    mov             " "rsi, [rbp + 32]"),
        Q!("    cmovz           " "r8, rsi"),
        Q!("    mov             " "rsi, [rbp + 40]"),
        Q!("    cmovz           " "r9, rsi"),
        Q!("    mov             " "rsi, [rbp + 48]"),
        Q!("    cmovz           " "r10, rsi"),
        Q!("    mov             " "rsi, [rbp + 56]"),
        Q!("    cmovz           " "r11, rsi"),
        Q!("    mov             " "rsi, [rbp + 64]"),
        Q!("    cmovz           " "r12, rsi"),
        Q!("    mov             " "rsi, [rbp + 72]"),
        Q!("    cmovz           " "r13, rsi"),
        Q!("    mov             " "rsi, [rbp + 80]"),
        Q!("    cmovz           " "r14, rsi"),
        Q!("    mov             " "rsi, [rbp + 88]"),
        Q!("    cmovz           " "r15, rsi"),
        Q!("    add             " "rbp, 96"),

        Q!("    cmp             " bf!() ", 6"),
        Q!("    mov             " "rsi, [rbp]"),
        Q!("    cmovz           " "rax, rsi"),
        Q!("    mov             " "rsi, [rbp + 8]"),
        Q!("    cmovz           " "rbx, rsi"),
        Q!("    mov             " "rsi, [rbp + 16]"),
        Q!("    cmovz           " "rcx, rsi"),
        Q!("    mov             " "rsi, [rbp + 24]"),
        Q!("    cmovz           " "rdx, rsi"),
        Q!("    mov             " "rsi, [rbp + 32]"),
        Q!("    cmovz           " "r8, rsi"),
        Q!("    mov             " "rsi, [rbp + 40]"),
        Q!("    cmovz           " "r9, rsi"),
        Q!("    mov             " "rsi, [rbp + 48]"),
        Q!("    cmovz           " "r10, rsi"),
        Q!("    mov             " "rsi, [rbp + 56]"),
        Q!("    cmovz           " "r11, rsi"),
        Q!("    mov             " "rsi, [rbp + 64]"),
        Q!("    cmovz           " "r12, rsi"),
        Q!("    mov             " "rsi, [rbp + 72]"),
        Q!("    cmovz           " "r13, rsi"),
        Q!("    mov             " "rsi, [rbp + 80]"),
        Q!("    cmovz           " "r14, rsi"),
        Q!("    mov             " "rsi, [rbp + 88]"),
        Q!("    cmovz           " "r15, rsi"),
        Q!("    add             " "rbp, 96"),

        Q!("    cmp             " bf!() ", 7"),
        Q!("    mov             " "rsi, [rbp]"),
        Q!("    cmovz           " "rax, rsi"),
        Q!("    mov             " "rsi, [rbp + 8]"),
        Q!("    cmovz           " "rbx, rsi"),
        Q!("    mov             " "rsi, [rbp + 16]"),
        Q!("    cmovz           " "rcx, rsi"),
        Q!("    mov             " "rsi, [rbp + 24]"),
        Q!("    cmovz           " "rdx, rsi"),
        Q!("    mov             " "rsi, [rbp + 32]"),
        Q!("    cmovz           " "r8, rsi"),
        Q!("    mov             " "rsi, [rbp + 40]"),
        Q!("    cmovz           " "r9, rsi"),
        Q!("    mov             " "rsi, [rbp + 48]"),
        Q!("    cmovz           " "r10, rsi"),
        Q!("    mov             " "rsi, [rbp + 56]"),
        Q!("    cmovz           " "r11, rsi"),
        Q!("    mov             " "rsi, [rbp + 64]"),
        Q!("    cmovz           " "r12, rsi"),
        Q!("    mov             " "rsi, [rbp + 72]"),
        Q!("    cmovz           " "r13, rsi"),
        Q!("    mov             " "rsi, [rbp + 80]"),
        Q!("    cmovz           " "r14, rsi"),
        Q!("    mov             " "rsi, [rbp + 88]"),
        Q!("    cmovz           " "r15, rsi"),
        Q!("    add             " "rbp, 96"),

        Q!("    cmp             " bf!() ", 8"),
        Q!("    mov             " "rsi, [rbp]"),
        Q!("    cmovz           " "rax, rsi"),
        Q!("    mov             " "rsi, [rbp + 8]"),
        Q!("    cmovz           " "rbx, rsi"),
        Q!("    mov             " "rsi, [rbp + 16]"),
        Q!("    cmovz           " "rcx, rsi"),
        Q!("    mov             " "rsi, [rbp + 24]"),
        Q!("    cmovz           " "rdx, rsi"),
        Q!("    mov             " "rsi, [rbp + 32]"),
        Q!("    cmovz           " "r8, rsi"),
        Q!("    mov             " "rsi, [rbp + 40]"),
        Q!("    cmovz           " "r9, rsi"),
        Q!("    mov             " "rsi, [rbp + 48]"),
        Q!("    cmovz           " "r10, rsi"),
        Q!("    mov             " "rsi, [rbp + 56]"),
        Q!("    cmovz           " "r11, rsi"),
        Q!("    mov             " "rsi, [rbp + 64]"),
        Q!("    cmovz           " "r12, rsi"),
        Q!("    mov             " "rsi, [rbp + 72]"),
        Q!("    cmovz           " "r13, rsi"),
        Q!("    mov             " "rsi, [rbp + 80]"),
        Q!("    cmovz           " "r14, rsi"),
        Q!("    mov             " "rsi, [rbp + 88]"),
        Q!("    cmovz           " "r15, rsi"),

        // ... then optionally negating before storing. The table entry
        // is in precomputed form and we currently have
        //
        //      [rdx;rcx;rbx;rax] = y - x
        //      [r11;r10;r9;r8] = x + y
        //      [r15;r14;r13;r12] = 2 * d * x * y
        //
        // Negation for Edwards curves is -(x,y) = (-x,y), which in this modified
        // form amounts to swapping the first two fields and negating the third.
        // The negation does not always fully reduce even mod 2^256-38 in the zero
        // case, instead giving -0 = 2^256-38. But that is fine since the result is
        // always fed to a multiplication inside the "pepadd" function below that
        // handles any 256-bit input.

        Q!("    mov             " "rdi, " cf!()),
        Q!("    test            " "rdi, rdi"),

        Q!("    mov             " "rsi, rax"),
        Q!("    cmovnz          " "rsi, r8"),
        Q!("    cmovnz          " "r8, rax"),
        Q!("    mov             " "[rsp + " BTABENT!() "], rsi"),
        Q!("    mov             " "[rsp + " BTABENT!() "+ 32], r8"),

        Q!("    mov             " "rsi, rbx"),
        Q!("    cmovnz          " "rsi, r9"),
        Q!("    cmovnz          " "r9, rbx"),
        Q!("    mov             " "[rsp + " BTABENT!() "+ 8], rsi"),
        Q!("    mov             " "[rsp + " BTABENT!() "+ 40], r9"),

        Q!("    mov             " "rsi, rcx"),
        Q!("    cmovnz          " "rsi, r10"),
        Q!("    cmovnz          " "r10, rcx"),
        Q!("    mov             " "[rsp + " BTABENT!() "+ 16], rsi"),
        Q!("    mov             " "[rsp + " BTABENT!() "+ 48], r10"),

        Q!("    mov             " "rsi, rdx"),
        Q!("    cmovnz          " "rsi, r11"),
        Q!("    cmovnz          " "r11, rdx"),
        Q!("    mov             " "[rsp + " BTABENT!() "+ 24], rsi"),
        Q!("    mov             " "[rsp + " BTABENT!() "+ 56], r11"),

        Q!("    xor             " "r12, rdi"),
        Q!("    xor             " "r13, rdi"),
        Q!("    xor             " "r14, rdi"),
        Q!("    xor             " "r15, rdi"),
        Q!("    and             " "rdi, 37"),
        Q!("    sub             " "r12, rdi"),
        Q!("    sbb             " "r13, 0"),
        Q!("    sbb             " "r14, 0"),
        Q!("    sbb             " "r15, 0"),
        Q!("    mov             " "[rsp + " BTABENT!() "+ 64], r12"),
        Q!("    mov             " "[rsp + " BTABENT!() "+ 72], r13"),
        Q!("    mov             " "[rsp + " BTABENT!() "+ 80], r14"),
        Q!("    mov             " "[rsp + " BTABENT!() "+ 88], r15"),

        // Get table entry, first getting the adjusted bitfield...

        Q!("    mov             " "rax, " i!()),
        Q!("    mov             " "rcx, rax"),
        Q!("    shr             " "rax, 6"),
        Q!("    mov             " "rax, [rsp + 8 * rax]"),
        Q!("    shr             " "rax, cl"),
        Q!("    and             " "rax, 15"),

        Q!("    sub             " "rax, 8"),
        Q!("    sbb             " "rcx, rcx"),
        Q!("    xor             " "rax, rcx"),
        Q!("    sub             " "rax, rcx"),
        Q!("    mov             " cf!() ", rcx"),
        Q!("    mov             " bf!() ", rax"),

        // ...and constant-time indexing based on that index
        // Do the Y and Z fields first, to save on registers
        // and store them back (they don't need any modification)

        Q!("    mov             " "eax, 1"),
        Q!("    xor             " "ebx, ebx"),
        Q!("    xor             " "ecx, ecx"),
        Q!("    xor             " "edx, edx"),
        Q!("    mov             " "r8d, 1"),
        Q!("    xor             " "r9d, r9d"),
        Q!("    xor             " "r10d, r10d"),
        Q!("    xor             " "r11d, r11d"),

        Q!("    lea             " "rbp, [rsp + " TAB!() "+ 32]"),

        Q!("    cmp             " bf!() ", 1"),
        Q!("    mov             " "rsi, [rbp]"),
        Q!("    cmovz           " "rax, rsi"),
        Q!("    mov             " "rsi, [rbp + 8]"),
        Q!("    cmovz           " "rbx, rsi"),
        Q!("    mov             " "rsi, [rbp + 16]"),
        Q!("    cmovz           " "rcx, rsi"),
        Q!("    mov             " "rsi, [rbp + 24]"),
        Q!("    cmovz           " "rdx, rsi"),
        Q!("    mov             " "rsi, [rbp + 32]"),
        Q!("    cmovz           " "r8, rsi"),
        Q!("    mov             " "rsi, [rbp + 40]"),
        Q!("    cmovz           " "r9, rsi"),
        Q!("    mov             " "rsi, [rbp + 48]"),
        Q!("    cmovz           " "r10, rsi"),
        Q!("    mov             " "rsi, [rbp + 56]"),
        Q!("    cmovz           " "r11, rsi"),
        Q!("    add             " "rbp, 128"),

        Q!("    cmp             " bf!() ", 2"),
        Q!("    mov             " "rsi, [rbp]"),
        Q!("    cmovz           " "rax, rsi"),
        Q!("    mov             " "rsi, [rbp + 8]"),
        Q!("    cmovz           " "rbx, rsi"),
        Q!("    mov             " "rsi, [rbp + 16]"),
        Q!("    cmovz           " "rcx, rsi"),
        Q!("    mov             " "rsi, [rbp + 24]"),
        Q!("    cmovz           " "rdx, rsi"),
        Q!("    mov             " "rsi, [rbp + 32]"),
        Q!("    cmovz           " "r8, rsi"),
        Q!("    mov             " "rsi, [rbp + 40]"),
        Q!("    cmovz           " "r9, rsi"),
        Q!("    mov             " "rsi, [rbp + 48]"),
        Q!("    cmovz           " "r10, rsi"),
        Q!("    mov             " "rsi, [rbp + 56]"),
        Q!("    cmovz           " "r11, rsi"),
        Q!("    add             " "rbp, 128"),

        Q!("    cmp             " bf!() ", 3"),
        Q!("    mov             " "rsi, [rbp]"),
        Q!("    cmovz           " "rax, rsi"),
        Q!("    mov             " "rsi, [rbp + 8]"),
        Q!("    cmovz           " "rbx, rsi"),
        Q!("    mov             " "rsi, [rbp + 16]"),
        Q!("    cmovz           " "rcx, rsi"),
        Q!("    mov             " "rsi, [rbp + 24]"),
        Q!("    cmovz           " "rdx, rsi"),
        Q!("    mov             " "rsi, [rbp + 32]"),
        Q!("    cmovz           " "r8, rsi"),
        Q!("    mov             " "rsi, [rbp + 40]"),
        Q!("    cmovz           " "r9, rsi"),
        Q!("    mov             " "rsi, [rbp + 48]"),
        Q!("    cmovz           " "r10, rsi"),
        Q!("    mov             " "rsi, [rbp + 56]"),
        Q!("    cmovz           " "r11, rsi"),
        Q!("    add             " "rbp, 128"),

        Q!("    cmp             " bf!() ", 4"),
        Q!("    mov             " "rsi, [rbp]"),
        Q!("    cmovz           " "rax, rsi"),
        Q!("    mov             " "rsi, [rbp + 8]"),
        Q!("    cmovz           " "rbx, rsi"),
        Q!("    mov             " "rsi, [rbp + 16]"),
        Q!("    cmovz           " "rcx, rsi"),
        Q!("    mov             " "rsi, [rbp + 24]"),
        Q!("    cmovz           " "rdx, rsi"),
        Q!("    mov             " "rsi, [rbp + 32]"),
        Q!("    cmovz           " "r8, rsi"),
        Q!("    mov             " "rsi, [rbp + 40]"),
        Q!("    cmovz           " "r9, rsi"),
        Q!("    mov             " "rsi, [rbp + 48]"),
        Q!("    cmovz           " "r10, rsi"),
        Q!("    mov             " "rsi, [rbp + 56]"),
        Q!("    cmovz           " "r11, rsi"),
        Q!("    add             " "rbp, 128"),

        Q!("    cmp             " bf!() ", 5"),
        Q!("    mov             " "rsi, [rbp]"),
        Q!("    cmovz           " "rax, rsi"),
        Q!("    mov             " "rsi, [rbp + 8]"),
        Q!("    cmovz           " "rbx, rsi"),
        Q!("    mov             " "rsi, [rbp + 16]"),
        Q!("    cmovz           " "rcx, rsi"),
        Q!("    mov             " "rsi, [rbp + 24]"),
        Q!("    cmovz           " "rdx, rsi"),
        Q!("    mov             " "rsi, [rbp + 32]"),
        Q!("    cmovz           " "r8, rsi"),
        Q!("    mov             " "rsi, [rbp + 40]"),
        Q!("    cmovz           " "r9, rsi"),
        Q!("    mov             " "rsi, [rbp + 48]"),
        Q!("    cmovz           " "r10, rsi"),
        Q!("    mov             " "rsi, [rbp + 56]"),
        Q!("    cmovz           " "r11, rsi"),
        Q!("    add             " "rbp, 128"),

        Q!("    cmp             " bf!() ", 6"),
        Q!("    mov             " "rsi, [rbp]"),
        Q!("    cmovz           " "rax, rsi"),
        Q!("    mov             " "rsi, [rbp + 8]"),
        Q!("    cmovz           " "rbx, rsi"),
        Q!("    mov             " "rsi, [rbp + 16]"),
        Q!("    cmovz           " "rcx, rsi"),
        Q!("    mov             " "rsi, [rbp + 24]"),
        Q!("    cmovz           " "rdx, rsi"),
        Q!("    mov             " "rsi, [rbp + 32]"),
        Q!("    cmovz           " "r8, rsi"),
        Q!("    mov             " "rsi, [rbp + 40]"),
        Q!("    cmovz           " "r9, rsi"),
        Q!("    mov             " "rsi, [rbp + 48]"),
        Q!("    cmovz           " "r10, rsi"),
        Q!("    mov             " "rsi, [rbp + 56]"),
        Q!("    cmovz           " "r11, rsi"),
        Q!("    add             " "rbp, 128"),

        Q!("    cmp             " bf!() ", 7"),
        Q!("    mov             " "rsi, [rbp]"),
        Q!("    cmovz           " "rax, rsi"),
        Q!("    mov             " "rsi, [rbp + 8]"),
        Q!("    cmovz           " "rbx, rsi"),
        Q!("    mov             " "rsi, [rbp + 16]"),
        Q!("    cmovz           " "rcx, rsi"),
        Q!("    mov             " "rsi, [rbp + 24]"),
        Q!("    cmovz           " "rdx, rsi"),
        Q!("    mov             " "rsi, [rbp + 32]"),
        Q!("    cmovz           " "r8, rsi"),
        Q!("    mov             " "rsi, [rbp + 40]"),
        Q!("    cmovz           " "r9, rsi"),
        Q!("    mov             " "rsi, [rbp + 48]"),
        Q!("    cmovz           " "r10, rsi"),
        Q!("    mov             " "rsi, [rbp + 56]"),
        Q!("    cmovz           " "r11, rsi"),
        Q!("    add             " "rbp, 128"),

        Q!("    cmp             " bf!() ", 8"),
        Q!("    mov             " "rsi, [rbp]"),
        Q!("    cmovz           " "rax, rsi"),
        Q!("    mov             " "rsi, [rbp + 8]"),
        Q!("    cmovz           " "rbx, rsi"),
        Q!("    mov             " "rsi, [rbp + 16]"),
        Q!("    cmovz           " "rcx, rsi"),
        Q!("    mov             " "rsi, [rbp + 24]"),
        Q!("    cmovz           " "rdx, rsi"),
        Q!("    mov             " "rsi, [rbp + 32]"),
        Q!("    cmovz           " "r8, rsi"),
        Q!("    mov             " "rsi, [rbp + 40]"),
        Q!("    cmovz           " "r9, rsi"),
        Q!("    mov             " "rsi, [rbp + 48]"),
        Q!("    cmovz           " "r10, rsi"),
        Q!("    mov             " "rsi, [rbp + 56]"),
        Q!("    cmovz           " "r11, rsi"),

        Q!("    mov             " "[rsp + " TABENT!() "+ 32], rax"),
        Q!("    mov             " "[rsp + " TABENT!() "+ 40], rbx"),
        Q!("    mov             " "[rsp + " TABENT!() "+ 48], rcx"),
        Q!("    mov             " "[rsp + " TABENT!() "+ 56], rdx"),
        Q!("    mov             " "[rsp + " TABENT!() "+ 64], r8"),
        Q!("    mov             " "[rsp + " TABENT!() "+ 72], r9"),
        Q!("    mov             " "[rsp + " TABENT!() "+ 80], r10"),
        Q!("    mov             " "[rsp + " TABENT!() "+ 88], r11"),

        // Now do the X and W fields...

        Q!("    lea             " "rbp, [rsp + " TAB!() "]"),

        Q!("    xor             " "eax, eax"),
        Q!("    xor             " "ebx, ebx"),
        Q!("    xor             " "ecx, ecx"),
        Q!("    xor             " "edx, edx"),
        Q!("    xor             " "r8d, r8d"),
        Q!("    xor             " "r9d, r9d"),
        Q!("    xor             " "r10d, r10d"),
        Q!("    xor             " "r11d, r11d"),

        Q!("    cmp             " bf!() ", 1"),
        Q!("    mov             " "rsi, [rbp]"),
        Q!("    cmovz           " "rax, rsi"),
        Q!("    mov             " "rsi, [rbp + 8]"),
        Q!("    cmovz           " "rbx, rsi"),
        Q!("    mov             " "rsi, [rbp + 16]"),
        Q!("    cmovz           " "rcx, rsi"),
        Q!("    mov             " "rsi, [rbp + 24]"),
        Q!("    cmovz           " "rdx, rsi"),
        Q!("    mov             " "rsi, [rbp + 96]"),
        Q!("    cmovz           " "r8, rsi"),
        Q!("    mov             " "rsi, [rbp + 104]"),
        Q!("    cmovz           " "r9, rsi"),
        Q!("    mov             " "rsi, [rbp + 112]"),
        Q!("    cmovz           " "r10, rsi"),
        Q!("    mov             " "rsi, [rbp + 120]"),
        Q!("    cmovz           " "r11, rsi"),
        Q!("    add             " "rbp, 128"),

        Q!("    cmp             " bf!() ", 2"),
        Q!("    mov             " "rsi, [rbp]"),
        Q!("    cmovz           " "rax, rsi"),
        Q!("    mov             " "rsi, [rbp + 8]"),
        Q!("    cmovz           " "rbx, rsi"),
        Q!("    mov             " "rsi, [rbp + 16]"),
        Q!("    cmovz           " "rcx, rsi"),
        Q!("    mov             " "rsi, [rbp + 24]"),
        Q!("    cmovz           " "rdx, rsi"),
        Q!("    mov             " "rsi, [rbp + 96]"),
        Q!("    cmovz           " "r8, rsi"),
        Q!("    mov             " "rsi, [rbp + 104]"),
        Q!("    cmovz           " "r9, rsi"),
        Q!("    mov             " "rsi, [rbp + 112]"),
        Q!("    cmovz           " "r10, rsi"),
        Q!("    mov             " "rsi, [rbp + 120]"),
        Q!("    cmovz           " "r11, rsi"),
        Q!("    add             " "rbp, 128"),

        Q!("    cmp             " bf!() ", 3"),
        Q!("    mov             " "rsi, [rbp]"),
        Q!("    cmovz           " "rax, rsi"),
        Q!("    mov             " "rsi, [rbp + 8]"),
        Q!("    cmovz           " "rbx, rsi"),
        Q!("    mov             " "rsi, [rbp + 16]"),
        Q!("    cmovz           " "rcx, rsi"),
        Q!("    mov             " "rsi, [rbp + 24]"),
        Q!("    cmovz           " "rdx, rsi"),
        Q!("    mov             " "rsi, [rbp + 96]"),
        Q!("    cmovz           " "r8, rsi"),
        Q!("    mov             " "rsi, [rbp + 104]"),
        Q!("    cmovz           " "r9, rsi"),
        Q!("    mov             " "rsi, [rbp + 112]"),
        Q!("    cmovz           " "r10, rsi"),
        Q!("    mov             " "rsi, [rbp + 120]"),
        Q!("    cmovz           " "r11, rsi"),
        Q!("    add             " "rbp, 128"),

        Q!("    cmp             " bf!() ", 4"),
        Q!("    mov             " "rsi, [rbp]"),
        Q!("    cmovz           " "rax, rsi"),
        Q!("    mov             " "rsi, [rbp + 8]"),
        Q!("    cmovz           " "rbx, rsi"),
        Q!("    mov             " "rsi, [rbp + 16]"),
        Q!("    cmovz           " "rcx, rsi"),
        Q!("    mov             " "rsi, [rbp + 24]"),
        Q!("    cmovz           " "rdx, rsi"),
        Q!("    mov             " "rsi, [rbp + 96]"),
        Q!("    cmovz           " "r8, rsi"),
        Q!("    mov             " "rsi, [rbp + 104]"),
        Q!("    cmovz           " "r9, rsi"),
        Q!("    mov             " "rsi, [rbp + 112]"),
        Q!("    cmovz           " "r10, rsi"),
        Q!("    mov             " "rsi, [rbp + 120]"),
        Q!("    cmovz           " "r11, rsi"),
        Q!("    add             " "rbp, 128"),

        Q!("    cmp             " bf!() ", 5"),
        Q!("    mov             " "rsi, [rbp]"),
        Q!("    cmovz           " "rax, rsi"),
        Q!("    mov             " "rsi, [rbp + 8]"),
        Q!("    cmovz           " "rbx, rsi"),
        Q!("    mov             " "rsi, [rbp + 16]"),
        Q!("    cmovz           " "rcx, rsi"),
        Q!("    mov             " "rsi, [rbp + 24]"),
        Q!("    cmovz           " "rdx, rsi"),
        Q!("    mov             " "rsi, [rbp + 96]"),
        Q!("    cmovz           " "r8, rsi"),
        Q!("    mov             " "rsi, [rbp + 104]"),
        Q!("    cmovz           " "r9, rsi"),
        Q!("    mov             " "rsi, [rbp + 112]"),
        Q!("    cmovz           " "r10, rsi"),
        Q!("    mov             " "rsi, [rbp + 120]"),
        Q!("    cmovz           " "r11, rsi"),
        Q!("    add             " "rbp, 128"),

        Q!("    cmp             " bf!() ", 6"),
        Q!("    mov             " "rsi, [rbp]"),
        Q!("    cmovz           " "rax, rsi"),
        Q!("    mov             " "rsi, [rbp + 8]"),
        Q!("    cmovz           " "rbx, rsi"),
        Q!("    mov             " "rsi, [rbp + 16]"),
        Q!("    cmovz           " "rcx, rsi"),
        Q!("    mov             " "rsi, [rbp + 24]"),
        Q!("    cmovz           " "rdx, rsi"),
        Q!("    mov             " "rsi, [rbp + 96]"),
        Q!("    cmovz           " "r8, rsi"),
        Q!("    mov             " "rsi, [rbp + 104]"),
        Q!("    cmovz           " "r9, rsi"),
        Q!("    mov             " "rsi, [rbp + 112]"),
        Q!("    cmovz           " "r10, rsi"),
        Q!("    mov             " "rsi, [rbp + 120]"),
        Q!("    cmovz           " "r11, rsi"),
        Q!("    add             " "rbp, 128"),

        Q!("    cmp             " bf!() ", 7"),
        Q!("    mov             " "rsi, [rbp]"),
        Q!("    cmovz           " "rax, rsi"),
        Q!("    mov             " "rsi, [rbp + 8]"),
        Q!("    cmovz           " "rbx, rsi"),
        Q!("    mov             " "rsi, [rbp + 16]"),
        Q!("    cmovz           " "rcx, rsi"),
        Q!("    mov             " "rsi, [rbp + 24]"),
        Q!("    cmovz           " "rdx, rsi"),
        Q!("    mov             " "rsi, [rbp + 96]"),
        Q!("    cmovz           " "r8, rsi"),
        Q!("    mov             " "rsi, [rbp + 104]"),
        Q!("    cmovz           " "r9, rsi"),
        Q!("    mov             " "rsi, [rbp + 112]"),
        Q!("    cmovz           " "r10, rsi"),
        Q!("    mov             " "rsi, [rbp + 120]"),
        Q!("    cmovz           " "r11, rsi"),
        Q!("    add             " "rbp, 128"),

        Q!("    cmp             " bf!() ", 8"),
        Q!("    mov             " "rsi, [rbp]"),
        Q!("    cmovz           " "rax, rsi"),
        Q!("    mov             " "rsi, [rbp + 8]"),
        Q!("    cmovz           " "rbx, rsi"),
        Q!("    mov             " "rsi, [rbp + 16]"),
        Q!("    cmovz           " "rcx, rsi"),
        Q!("    mov             " "rsi, [rbp + 24]"),
        Q!("    cmovz           " "rdx, rsi"),
        Q!("    mov             " "rsi, [rbp + 96]"),
        Q!("    cmovz           " "r8, rsi"),
        Q!("    mov             " "rsi, [rbp + 104]"),
        Q!("    cmovz           " "r9, rsi"),
        Q!("    mov             " "rsi, [rbp + 112]"),
        Q!("    cmovz           " "r10, rsi"),
        Q!("    mov             " "rsi, [rbp + 120]"),
        Q!("    cmovz           " "r11, rsi"),

        // ... then optionally negate before storing the X and W fields. This
        // time the table entry is extended-projective, and is here:
        //
        //      [rdx;rcx;rbx;rax] = X
        //      [tabent+32] = Y
        //      [tabent+64] = Z
        //      [r11;r10;r9;r8] = W
        //
        // This time we just need to negate the X and the W fields.
        // The crude way negation is done can result in values of X or W
        // (when initially zero before negation) being exactly equal to
        // 2^256-38, but the "pepadd" function handles that correctly.

        Q!("    mov             " "rdi, " cf!()),

        Q!("    xor             " "rax, rdi"),
        Q!("    xor             " "rbx, rdi"),
        Q!("    xor             " "rcx, rdi"),
        Q!("    xor             " "rdx, rdi"),

        Q!("    xor             " "r8, rdi"),
        Q!("    xor             " "r9, rdi"),
        Q!("    xor             " "r10, rdi"),
        Q!("    xor             " "r11, rdi"),

        Q!("    and             " "rdi, 37"),

        Q!("    sub             " "rax, rdi"),
        Q!("    sbb             " "rbx, 0"),
        Q!("    sbb             " "rcx, 0"),
        Q!("    sbb             " "rdx, 0"),

        Q!("    mov             " "[rsp + " TABENT!() "], rax"),
        Q!("    mov             " "[rsp + " TABENT!() "+ 8], rbx"),
        Q!("    mov             " "[rsp + " TABENT!() "+ 16], rcx"),
        Q!("    mov             " "[rsp + " TABENT!() "+ 24], rdx"),

        Q!("    sub             " "r8, rdi"),
        Q!("    sbb             " "r9, 0"),
        Q!("    sbb             " "r10, 0"),
        Q!("    sbb             " "r11, 0"),

        Q!("    mov             " "[rsp + " TABENT!() "+ 96], r8"),
        Q!("    mov             " "[rsp + " TABENT!() "+ 104], r9"),
        Q!("    mov             " "[rsp + " TABENT!() "+ 112], r10"),
        Q!("    mov             " "[rsp + " TABENT!() "+ 120], r11"),

        // Double to acc' = 4 * acc

        Q!("    lea             " "rdi, [rsp + " ACC!() "]"),
        Q!("    lea             " "rsi, [rsp + " ACC!() "]"),
        Q!("    call            " Label!("Ledwards25519_scalarmuldouble_pdouble", 6, After)),

        // Add tabent := tabent + btabent

        Q!("    lea             " "rdi, [rsp + " TABENT!() "]"),
        Q!("    lea             " "rsi, [rsp + " TABENT!() "]"),
        Q!("    lea             " "rbp, [rsp + " BTABENT!() "]"),
        Q!("    call            " Label!("Ledwards25519_scalarmuldouble_pepadd", 4, After)),

        // Double to acc' = 8 * acc

        Q!("    lea             " "rdi, [rsp + " ACC!() "]"),
        Q!("    lea             " "rsi, [rsp + " ACC!() "]"),
        Q!("    call            " Label!("Ledwards25519_scalarmuldouble_pdouble", 6, After)),

        // Double to acc' = 16 * acc

        Q!("    lea             " "rdi, [rsp + " ACC!() "]"),
        Q!("    lea             " "rsi, [rsp + " ACC!() "]"),
        Q!("    call            " Label!("Ledwards25519_scalarmuldouble_epdouble", 2, After)),

        // Add table entry, acc := acc + tabent

        Q!("    lea             " "rdi, [rsp + " ACC!() "]"),
        Q!("    lea             " "rsi, [rsp + " ACC!() "]"),
        Q!("    lea             " "rbp, [rsp + " TABENT!() "]"),
        Q!("    call            " Label!("Ledwards25519_scalarmuldouble_epadd", 3, After)),

        // Loop down

        Q!("    mov             " "rax, " i!()),
        Q!("    test            " "rax, rax"),
        Q!("    jnz             " Label!("Ledwards25519_scalarmuldouble_loop", 5, Before)),

        // Prepare to call the modular inverse function to get tab = 1/z

        Q!("    lea             " "rdi, [rsp + " TAB!() "]"),
        Q!("    lea             " "rsi, [rsp + " ACC!() "+ 64]"),

        // Inline copy of bignum_inv_p25519, identical except for stripping out
        // the prologue and epilogue saving and restoring registers and making
        // and reclaiming room on the stack. For more details and explanations see
        // "x86/curve25519/bignum_inv_p25519.S". Note that the stack it uses for
        // its own temporaries is 208 bytes, so it has no effect on variables
        // that are needed in the rest of our computation here: res, tab and acc.

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
        Q!("    jmp             " Label!("Ledwards25519_scalarmuldouble_midloop", 7, After)),
        Q!(Label!("Ledwards25519_scalarmuldouble_inverseloop", 8) ":"),
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
        Q!(Label!("Ledwards25519_scalarmuldouble_midloop", 7) ":"),
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
        Q!("    jne             " Label!("Ledwards25519_scalarmuldouble_inverseloop", 8, Before)),
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

        // Store result

        Q!("    mov             " "rdi, " res!()),
        Q!("    lea             " "rsi, [rsp + " ACC!() "]"),
        Q!("    lea             " "rbp, [rsp + " TAB!() "]"),
        mul_p25519!(x_0!(), x_1!(), x_2!()),

        Q!("    mov             " "rdi, " res!()),
        Q!("    add             " "rdi, 32"),
        Q!("    lea             " "rsi, [rsp + " ACC!() "+ 32]"),
        Q!("    lea             " "rbp, [rsp + " TAB!() "]"),
        mul_p25519!(x_0!(), x_1!(), x_2!()),

        // Restore stack and registers

        Q!("    add             " "rsp, " NSPACE!()),

        Q!("    pop             " "r15"),
        Q!("    pop             " "r14"),
        Q!("    pop             " "r13"),
        Q!("    pop             " "r12"),
        Q!("    pop             " "rbp"),
        Q!("    pop             " "rbx"),
        // proc hoisting in -> ret after Ledwards25519_scalarmuldouble_pepadd
        Q!("    jmp             " Label!("hoist_finish", 9, After)),

        // ****************************************************************************
        // Localized versions of subroutines.
        // These are close to the standalone functions "edwards25519_epdouble" etc.,
        // but are only maintaining reduction modulo 2^256 - 38, not 2^255 - 19.
        // ****************************************************************************

        Q!(Label!("Ledwards25519_scalarmuldouble_epdouble", 2) ":"),

        Q!("    sub             " "rsp, 5 * " NUMSIZE!()),
        add_twice4!(t0!(), x_1!(), y_1!()),
        sqr_4!(t1!(), z_1!()),
        sqr_4!(t2!(), x_1!()),
        sqr_4!(t3!(), y_1!()),
        double_twice4!(t1!(), t1!()),
        sqr_4!(t0!(), t0!()),
        add_twice4!(t4!(), t2!(), t3!()),
        sub_twice4!(t2!(), t2!(), t3!()),
        add_twice4!(t3!(), t1!(), t2!()),
        sub_twice4!(t1!(), t4!(), t0!()),
        mul_4!(y_0!(), t2!(), t4!()),
        mul_4!(z_0!(), t3!(), t2!()),
        mul_4!(w_0!(), t1!(), t4!()),
        mul_4!(x_0!(), t1!(), t3!()),
        Q!("    add             " "rsp, 5 * " NUMSIZE!()),
        Q!("    ret             " ),

        Q!(Label!("Ledwards25519_scalarmuldouble_pdouble", 6) ":"),

        Q!("    sub             " "rsp, 5 * " NUMSIZE!()),
        add_twice4!(t0!(), x_1!(), y_1!()),
        sqr_4!(t1!(), z_1!()),
        sqr_4!(t2!(), x_1!()),
        sqr_4!(t3!(), y_1!()),
        double_twice4!(t1!(), t1!()),
        sqr_4!(t0!(), t0!()),
        add_twice4!(t4!(), t2!(), t3!()),
        sub_twice4!(t2!(), t2!(), t3!()),
        add_twice4!(t3!(), t1!(), t2!()),
        sub_twice4!(t1!(), t4!(), t0!()),
        mul_4!(y_0!(), t2!(), t4!()),
        mul_4!(z_0!(), t3!(), t2!()),
        mul_4!(x_0!(), t1!(), t3!()),
        Q!("    add             " "rsp, 5 * " NUMSIZE!()),
        Q!("    ret             " ),

        Q!(Label!("Ledwards25519_scalarmuldouble_epadd", 3) ":"),

        Q!("    sub             " "rsp, 6 * " NUMSIZE!()),
        mul_4!(t0!(), w_1!(), w_2!()),
        sub_twice4!(t1!(), y_1!(), x_1!()),
        sub_twice4!(t2!(), y_2!(), x_2!()),
        add_twice4!(t3!(), y_1!(), x_1!()),
        add_twice4!(t4!(), y_2!(), x_2!()),
        double_twice4!(t5!(), z_2!()),
        mul_4!(t1!(), t1!(), t2!()),
        mul_4!(t3!(), t3!(), t4!()),
        load_k25519!(t2!()),
        mul_4!(t2!(), t2!(), t0!()),
        mul_4!(t4!(), z_1!(), t5!()),
        sub_twice4!(t0!(), t3!(), t1!()),
        add_twice4!(t5!(), t3!(), t1!()),
        sub_twice4!(t1!(), t4!(), t2!()),
        add_twice4!(t3!(), t4!(), t2!()),
        mul_4!(w_0!(), t0!(), t5!()),
        mul_4!(x_0!(), t0!(), t1!()),
        mul_4!(y_0!(), t3!(), t5!()),
        mul_4!(z_0!(), t1!(), t3!()),
        Q!("    add             " "rsp, 6 * " NUMSIZE!()),
        Q!("    ret             " ),

        Q!(Label!("Ledwards25519_scalarmuldouble_pepadd", 4) ":"),

        Q!("    sub             " "rsp, 6 * " NUMSIZE!()),
        double_twice4!(t0!(), z_1!()),
        sub_twice4!(t1!(), y_1!(), x_1!()),
        add_twice4!(t2!(), y_1!(), x_1!()),
        mul_4!(t3!(), w_1!(), z_2!()),
        mul_4!(t1!(), t1!(), x_2!()),
        mul_4!(t2!(), t2!(), y_2!()),
        sub_twice4!(t4!(), t0!(), t3!()),
        add_twice4!(t0!(), t0!(), t3!()),
        sub_twice4!(t5!(), t2!(), t1!()),
        add_twice4!(t1!(), t2!(), t1!()),
        mul_4!(z_0!(), t4!(), t0!()),
        mul_4!(x_0!(), t5!(), t4!()),
        mul_4!(y_0!(), t0!(), t1!()),
        mul_4!(w_0!(), t5!(), t1!()),
        Q!("    add             " "rsp, 6 * " NUMSIZE!()),
        Q!("    ret             " ),
        Q!(Label!("hoist_finish", 9) ":"),
        inout("rdi") res.as_mut_ptr() => _,
        inout("rsi") scalar.as_ptr() => _,
        inout("rdx") point.as_ptr() => _,
        inout("rcx") bscalar.as_ptr() => _,
        Ledwards25519_scalarmuldouble_table = sym Ledwards25519_scalarmuldouble_table,
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
            )
    };
}

// ****************************************************************************
// The precomputed data (all read-only). This is currently part of the same
// text section, which gives position-independent code with simple PC-relative
// addressing. However it could be put in a separate section via something like
//
// .section .rodata
// ****************************************************************************

// Precomputed table of multiples of generator for edwards25519
// all in precomputed extended-projective (y-x,x+y,2*d*x*y) triples.

static Ledwards25519_scalarmuldouble_table: [u64; 96] = [
    // 1 * G
    0x9d103905d740913e,
    0xfd399f05d140beb3,
    0xa5c18434688f8a09,
    0x44fd2f9298f81267,
    0x2fbc93c6f58c3b85,
    0xcf932dc6fb8c0e19,
    0x270b4898643d42c2,
    0x07cf9d3a33d4ba65,
    0xabc91205877aaa68,
    0x26d9e823ccaac49e,
    0x5a1b7dcbdd43598c,
    0x6f117b689f0c65a8,
    // 2 * G
    0x8a99a56042b4d5a8,
    0x8f2b810c4e60acf6,
    0xe09e236bb16e37aa,
    0x6bb595a669c92555,
    0x9224e7fc933c71d7,
    0x9f469d967a0ff5b5,
    0x5aa69a65e1d60702,
    0x590c063fa87d2e2e,
    0x43faa8b3a59b7a5f,
    0x36c16bdd5d9acf78,
    0x500fa0840b3d6a31,
    0x701af5b13ea50b73,
    // 3 * G
    0x56611fe8a4fcd265,
    0x3bd353fde5c1ba7d,
    0x8131f31a214bd6bd,
    0x2ab91587555bda62,
    0xaf25b0a84cee9730,
    0x025a8430e8864b8a,
    0xc11b50029f016732,
    0x7a164e1b9a80f8f4,
    0x14ae933f0dd0d889,
    0x589423221c35da62,
    0xd170e5458cf2db4c,
    0x5a2826af12b9b4c6,
    // 4 * G
    0x95fe050a056818bf,
    0x327e89715660faa9,
    0xc3e8e3cd06a05073,
    0x27933f4c7445a49a,
    0x287351b98efc099f,
    0x6765c6f47dfd2538,
    0xca348d3dfb0a9265,
    0x680e910321e58727,
    0x5a13fbe9c476ff09,
    0x6e9e39457b5cc172,
    0x5ddbdcf9102b4494,
    0x7f9d0cbf63553e2b,
    // 5 * G
    0x7f9182c3a447d6ba,
    0xd50014d14b2729b7,
    0xe33cf11cb864a087,
    0x154a7e73eb1b55f3,
    0xa212bc4408a5bb33,
    0x8d5048c3c75eed02,
    0xdd1beb0c5abfec44,
    0x2945ccf146e206eb,
    0xbcbbdbf1812a8285,
    0x270e0807d0bdd1fc,
    0xb41b670b1bbda72d,
    0x43aabe696b3bb69a,
    // 6 * G
    0x499806b67b7d8ca4,
    0x575be28427d22739,
    0xbb085ce7204553b9,
    0x38b64c41ae417884,
    0x3a0ceeeb77157131,
    0x9b27158900c8af88,
    0x8065b668da59a736,
    0x51e57bb6a2cc38bd,
    0x85ac326702ea4b71,
    0xbe70e00341a1bb01,
    0x53e4a24b083bc144,
    0x10b8e91a9f0d61e3,
    // 7 * G
    0xba6f2c9aaa3221b1,
    0x6ca021533bba23a7,
    0x9dea764f92192c3a,
    0x1d6edd5d2e5317e0,
    0x6b1a5cd0944ea3bf,
    0x7470353ab39dc0d2,
    0x71b2528228542e49,
    0x461bea69283c927e,
    0xf1836dc801b8b3a2,
    0xb3035f47053ea49a,
    0x529c41ba5877adf3,
    0x7a9fbb1c6a0f90a7,
    // 8 * G
    0xe2a75dedf39234d9,
    0x963d7680e1b558f9,
    0x2c2741ac6e3c23fb,
    0x3a9024a1320e01c3,
    0x59b7596604dd3e8f,
    0x6cb30377e288702c,
    0xb1339c665ed9c323,
    0x0915e76061bce52f,
    0xe7c1f5d9c9a2911a,
    0xb8a371788bcca7d7,
    0x636412190eb62a32,
    0x26907c5c2ecc4e95,
];
