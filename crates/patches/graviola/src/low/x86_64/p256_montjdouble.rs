// generated source. do not edit.
#![allow(non_upper_case_globals, unused_macros, unused_imports)]
use crate::low::macros::*;

// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

// ----------------------------------------------------------------------------
// Point doubling on NIST curve P-256 in Montgomery-Jacobian coordinates
//
//    extern void p256_montjdouble(uint64_t p3[static 12],
//                                 const uint64_t p1[static 12]);
//
// Does p3 := 2 * p1 where all points are regarded as Jacobian triples with
// each coordinate in the Montgomery domain, i.e. x' = (2^256 * x) mod p_256.
// A Jacobian triple (x',y',z') represents affine point (x/z^2,y/z^3).
//
// Standard x86-64 ABI: RDI = p3, RSI = p1
// Microsoft x64 ABI:   RCX = p3, RDX = p1
// ----------------------------------------------------------------------------

// Size of individual field elements

macro_rules! NUMSIZE {
    () => {
        "32"
    };
}

// Pointer-offset pairs for inputs and outputs
// These assume rdi = p3, rsi = p1, which is true when the
// arguments come in initially and is not disturbed throughout.

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
macro_rules! y4 { () => { Q!("rsp + (" NUMSIZE!() "* 0)") } }

macro_rules! y2 { () => { Q!("rsp + (" NUMSIZE!() "* 1)") } }

macro_rules! t1 { () => { Q!("rsp + (" NUMSIZE!() "* 2)") } }

macro_rules! t2 { () => { Q!("rsp + (" NUMSIZE!() "* 3)") } }
macro_rules! x2p { () => { Q!("rsp + (" NUMSIZE!() "* 3)") } }
macro_rules! dx2 { () => { Q!("rsp + (" NUMSIZE!() "* 3)") } }

macro_rules! xy2 { () => { Q!("rsp + (" NUMSIZE!() "* 4)") } }

macro_rules! x4p { () => { Q!("rsp + (" NUMSIZE!() "* 5)") } }
macro_rules! d { () => { Q!("rsp + (" NUMSIZE!() "* 5)") } }

macro_rules! NSPACE { () => { Q!(NUMSIZE!() "* 6") } }

// Corresponds exactly to bignum_montmul_p256

macro_rules! montmul_p256 {
    ($P0:expr, $P1:expr, $P2:expr) => { Q!(
        "xor r13d, r13d;\n"
        "mov rdx, [" $P2 "];\n"
        "mulx r9, r8, [" $P1 "];\n"
        "mulx r10, rbx, [" $P1 "+ 0x8];\n"
        "adc r9, rbx;\n"
        "mulx r11, rbx, [" $P1 "+ 0x10];\n"
        "adc r10, rbx;\n"
        "mulx r12, rbx, [" $P1 "+ 0x18];\n"
        "adc r11, rbx;\n"
        "adc r12, r13;\n"
        "mov rdx, [" $P2 "+ 0x8];\n"
        "xor r14d, r14d;\n"
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
        "adc r13, r14;\n"
        "xor r15d, r15d;\n"
        "movabs rdx, 0x100000000;\n"
        "mulx rbx, rax, r8;\n"
        "adcx r9, rax;\n"
        "adox r10, rbx;\n"
        "mulx rbx, rax, r9;\n"
        "adcx r10, rax;\n"
        "adox r11, rbx;\n"
        "not rdx;\n"
        "lea rdx, [rdx + 0x2];\n"
        "mulx rbx, rax, r8;\n"
        "adcx r11, rax;\n"
        "adox r12, rbx;\n"
        "mulx rbx, rax, r9;\n"
        "adcx r12, rax;\n"
        "adox r13, rbx;\n"
        "adcx r13, r15;\n"
        "adox r14, r15;\n"
        "adc r14, r15;\n"
        "mov rdx, [" $P2 "+ 0x10];\n"
        "xor r8d, r8d;\n"
        "mulx rbx, rax, [" $P1 "];\n"
        "adcx r10, rax;\n"
        "adox r11, rbx;\n"
        "mulx rbx, rax, [" $P1 "+ 0x8];\n"
        "adcx r11, rax;\n"
        "adox r12, rbx;\n"
        "mulx rbx, rax, [" $P1 "+ 0x10];\n"
        "adcx r12, rax;\n"
        "adox r13, rbx;\n"
        "adox r14, r8;\n"
        "mulx rbx, rax, [" $P1 "+ 0x18];\n"
        "adc r13, rax;\n"
        "adc r14, rbx;\n"
        "adc r15, r8;\n"
        "mov rdx, [" $P2 "+ 0x18];\n"
        "xor r9d, r9d;\n"
        "mulx rbx, rax, [" $P1 "];\n"
        "adcx r11, rax;\n"
        "adox r12, rbx;\n"
        "mulx rbx, rax, [" $P1 "+ 0x8];\n"
        "adcx r12, rax;\n"
        "adox r13, rbx;\n"
        "mulx rbx, rax, [" $P1 "+ 0x10];\n"
        "adcx r13, rax;\n"
        "adox r14, rbx;\n"
        "adox r15, r9;\n"
        "mulx rbx, rax, [" $P1 "+ 0x18];\n"
        "adc r14, rax;\n"
        "adc r15, rbx;\n"
        "adc r8, r9;\n"
        "xor r9d, r9d;\n"
        "movabs rdx, 0x100000000;\n"
        "mulx rbx, rax, r10;\n"
        "adcx r11, rax;\n"
        "adox r12, rbx;\n"
        "mulx rbx, rax, r11;\n"
        "adcx r12, rax;\n"
        "adox r13, rbx;\n"
        "not rdx;\n"
        "lea rdx, [rdx + 0x2];\n"
        "mulx rbx, rax, r10;\n"
        "adcx r13, rax;\n"
        "adox r14, rbx;\n"
        "mulx rbx, rax, r11;\n"
        "adcx r14, rax;\n"
        "adox r15, rbx;\n"
        "adcx r15, r9;\n"
        "adox r8, r9;\n"
        "adc r8, r9;\n"
        "mov ecx, 0x1;\n"
        "add rcx, r12;\n"
        "dec rdx;\n"
        "adc rdx, r13;\n"
        "dec r9;\n"
        "mov rax, r9;\n"
        "adc r9, r14;\n"
        "mov r11d, 0xfffffffe;\n"
        "adc r11, r15;\n"
        "adc rax, r8;\n"
        "cmovb r12, rcx;\n"
        "cmovb r13, rdx;\n"
        "cmovb r14, r9;\n"
        "cmovb r15, r11;\n"
        "mov [" $P0 "], r12;\n"
        "mov [" $P0 "+ 0x8], r13;\n"
        "mov [" $P0 "+ 0x10], r14;\n"
        "mov [" $P0 "+ 0x18], r15"
    )}
}

// Corresponds exactly to bignum_montsqr_p256

macro_rules! montsqr_p256 {
    ($P0:expr, $P1:expr) => { Q!(
        "mov rdx, [" $P1 "];\n"
        "mulx r15, r8, rdx;\n"
        "mulx r10, r9, [" $P1 "+ 0x8];\n"
        "mulx r12, r11, [" $P1 "+ 0x18];\n"
        "mov rdx, [" $P1 "+ 0x10];\n"
        "mulx r14, r13, [" $P1 "+ 0x18];\n"
        "xor ebp, ebp;\n"
        "mulx rbx, rax, [" $P1 "];\n"
        "adcx r10, rax;\n"
        "adox r11, rbx;\n"
        "mulx rbx, rax, [" $P1 "+ 0x8];\n"
        "adcx r11, rax;\n"
        "adox r12, rbx;\n"
        "mov rdx, [" $P1 "+ 0x18];\n"
        "mulx rbx, rax, [" $P1 "+ 0x8];\n"
        "adcx r12, rax;\n"
        "adox r13, rbx;\n"
        "adcx r13, rbp;\n"
        "adox r14, rbp;\n"
        "adc r14, rbp;\n"
        "xor ebp, ebp;\n"
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
        "adcx r15, rbp;\n"
        "adox r15, rbp;\n"
        "xor ebp, ebp;\n"
        "movabs rdx, 0x100000000;\n"
        "mulx rbx, rax, r8;\n"
        "adcx r9, rax;\n"
        "adox r10, rbx;\n"
        "mulx rbx, rax, r9;\n"
        "adcx r10, rax;\n"
        "adox r11, rbx;\n"
        "movabs rdx, 0xffffffff00000001;\n"
        "mulx rbx, rax, r8;\n"
        "adcx r11, rax;\n"
        "adox r12, rbx;\n"
        "mulx rbx, rax, r9;\n"
        "adcx r12, rax;\n"
        "adox r13, rbx;\n"
        "adcx r13, rbp;\n"
        "mov r9d, ebp;\n"
        "adox r9, rbp;\n"
        "adcx r9, rbp;\n"
        "add r14, r9;\n"
        "adc r15, rbp;\n"
        "mov r8d, ebp;\n"
        "adc r8, rbp;\n"
        "xor ebp, ebp;\n"
        "movabs rdx, 0x100000000;\n"
        "mulx rbx, rax, r10;\n"
        "adcx r11, rax;\n"
        "adox r12, rbx;\n"
        "mulx rbx, rax, r11;\n"
        "adcx r12, rax;\n"
        "adox r13, rbx;\n"
        "movabs rdx, 0xffffffff00000001;\n"
        "mulx rbx, rax, r10;\n"
        "adcx r13, rax;\n"
        "adox r14, rbx;\n"
        "mulx rbx, rax, r11;\n"
        "adcx r14, rax;\n"
        "adox r15, rbx;\n"
        "adcx r15, rbp;\n"
        "adox r8, rbp;\n"
        "adc r8, rbp;\n"
        "mov ecx, 0x1;\n"
        "add rcx, r12;\n"
        "lea rdx, [rdx -0x1];\n"
        "adc rdx, r13;\n"
        "lea rbp, [rbp -0x1];\n"
        "mov rax, rbp;\n"
        "adc rbp, r14;\n"
        "mov r11d, 0xfffffffe;\n"
        "adc r11, r15;\n"
        "adc rax, r8;\n"
        "cmovb r12, rcx;\n"
        "cmovb r13, rdx;\n"
        "cmovb r14, rbp;\n"
        "cmovb r15, r11;\n"
        "mov [" $P0 "], r12;\n"
        "mov [" $P0 "+ 0x8], r13;\n"
        "mov [" $P0 "+ 0x10], r14;\n"
        "mov [" $P0 "+ 0x18], r15"
    )}
}

// Corresponds exactly to bignum_sub_p256

macro_rules! sub_p256 {
    ($P0:expr, $P1:expr, $P2:expr) => { Q!(
        "mov rax, [" $P1 "];\n"
        "sub rax, [" $P2 "];\n"
        "mov rcx, [" $P1 "+ 0x8];\n"
        "sbb rcx, [" $P2 "+ 0x8];\n"
        "mov r8, [" $P1 "+ 0x10];\n"
        "sbb r8, [" $P2 "+ 0x10];\n"
        "mov r9, [" $P1 "+ 0x18];\n"
        "sbb r9, [" $P2 "+ 0x18];\n"
        "mov r10d, 0xffffffff;\n"
        "sbb r11, r11;\n"
        "xor rdx, rdx;\n"
        "and r10, r11;\n"
        "sub rdx, r10;\n"
        "add rax, r11;\n"
        "mov [" $P0 "], rax;\n"
        "adc rcx, r10;\n"
        "mov [" $P0 "+ 0x8], rcx;\n"
        "adc r8, 0x0;\n"
        "mov [" $P0 "+ 0x10], r8;\n"
        "adc r9, rdx;\n"
        "mov [" $P0 "+ 0x18], r9"
    )}
}

// Corresponds exactly to bignum_add_p256

macro_rules! add_p256 {
    ($P0:expr, $P1:expr, $P2:expr) => { Q!(
        "xor r11, r11;\n"
        "mov rax, [" $P1 "];\n"
        "add rax, [" $P2 "];\n"
        "mov rcx, [" $P1 "+ 0x8];\n"
        "adc rcx, [" $P2 "+ 0x8];\n"
        "mov r8, [" $P1 "+ 0x10];\n"
        "adc r8, [" $P2 "+ 0x10];\n"
        "mov r9, [" $P1 "+ 0x18];\n"
        "adc r9, [" $P2 "+ 0x18];\n"
        "adc r11, r11;\n"
        "sub rax, 0xffffffffffffffff;\n"
        "mov r10d, 0xffffffff;\n"
        "sbb rcx, r10;\n"
        "sbb r8, 0x0;\n"
        "mov rdx, 0xffffffff00000001;\n"
        "sbb r9, rdx;\n"
        "sbb r11, 0x0;\n"
        "and r10, r11;\n"
        "and rdx, r11;\n"
        "add rax, r11;\n"
        "mov [" $P0 "], rax;\n"
        "adc rcx, r10;\n"
        "mov [" $P0 "+ 0x8], rcx;\n"
        "adc r8, 0x0;\n"
        "mov [" $P0 "+ 0x10], r8;\n"
        "adc r9, rdx;\n"
        "mov [" $P0 "+ 0x18], r9"
    )}
}

// A weak version of add that only guarantees sum in 4 digits

macro_rules! weakadd_p256 {
    ($P0:expr, $P1:expr, $P2:expr) => { Q!(
        "mov rax, [" $P1 "];\n"
        "add rax, [" $P2 "];\n"
        "mov rcx, [" $P1 "+ 0x8];\n"
        "adc rcx, [" $P2 "+ 0x8];\n"
        "mov r8, [" $P1 "+ 0x10];\n"
        "adc r8, [" $P2 "+ 0x10];\n"
        "mov r9, [" $P1 "+ 0x18];\n"
        "adc r9, [" $P2 "+ 0x18];\n"
        "mov r10d, 0xffffffff;\n"
        "sbb r11, r11;\n"
        "xor rdx, rdx;\n"
        "and r10, r11;\n"
        "sub rdx, r10;\n"
        "sub rax, r11;\n"
        "mov [" $P0 "], rax;\n"
        "sbb rcx, r10;\n"
        "mov [" $P0 "+ 0x8], rcx;\n"
        "sbb r8, 0x0;\n"
        "mov [" $P0 "+ 0x10], r8;\n"
        "sbb r9, rdx;\n"
        "mov [" $P0 "+ 0x18], r9"
    )}
}

// P0 = C * P1 - D * P2  computed as d * (p_256 - P2) + c * P1
// Quotient estimation is done just as q = h + 1 as in bignum_triple_p256
// This also applies to the other functions following.

macro_rules! cmsub_p256 {
    ($P0:expr, $C:expr, $P1:expr, $D:expr, $P2:expr) => { Q!(
        /* First [r11;r10;r9;r8] = p_256 - P2 */
        "mov r8, 0xffffffffffffffff;\n"
        "xor r10d, r10d;\n"
        "sub r8, [" $P2 "];\n"
        "mov r9, 0x00000000ffffffff;\n"
        "sbb r9, [" $P2 "+ 0x8];\n"
        "sbb r10, [" $P2 "+ 0x10];\n"
        "mov r11, 0xffffffff00000001;\n"
        "sbb r11, [" $P2 "+ 0x18];\n"
        /* [r12;r11;r10;r9;r8] = D * (p_256 - P2) */
        "xor r12d, r12d;\n"
        "mov rdx, " $D ";\n"
        "mulx rax, r8, r8;\n"
        "mulx rcx, r9, r9;\n"
        "add r9, rax;\n"
        "mulx rax, r10, r10;\n"
        "adc r10, rcx;\n"
        "mulx rcx, r11, r11;\n"
        "adc r11, rax;\n"
        "adc r12, rcx;\n"
        /* [rdx;r11;r10;r9;r8] = 2^256 + C * P1 + D * (p_256 - P2) */
        "mov rdx, " $C ";\n"
        "xor eax, eax;\n"
        "mulx rcx, rax, [" $P1 "];\n"
        "adcx r8, rax;\n"
        "adox r9, rcx;\n"
        "mulx rcx, rax, [" $P1 "+ 0x8];\n"
        "adcx r9, rax;\n"
        "adox r10, rcx;\n"
        "mulx rcx, rax, [" $P1 "+ 0x10];\n"
        "adcx r10, rax;\n"
        "adox r11, rcx;\n"
        "mulx rdx, rax, [" $P1 "+ 0x18];\n"
        "adcx r11, rax;\n"
        "adox rdx, r12;\n"
        "adc rdx, 1;\n"
        /* Now the tail for modular reduction from tripling */
        "add r8, rdx;\n"
        "mov rax, 0x100000000;\n"
        "mulx rcx, rax, rax;\n"
        "sbb rax, 0x0;\n"
        "sbb rcx, 0x0;\n"
        "sub r9, rax;\n"
        "sbb r10, rcx;\n"
        "mov rax, 0xffffffff00000001;\n"
        "mulx rcx, rax, rax;\n"
        "sbb r11, rax;\n"
        "sbb rdx, rcx;\n"
        "dec rdx;\n"
        "mov eax, 0xffffffff;\n"
        "and rax, rdx;\n"
        "xor ecx, ecx;\n"
        "sub rcx, rax;\n"
        "add r8, rdx;\n"
        "mov [" $P0 "], r8;\n"
        "adc r9, rax;\n"
        "mov [" $P0 "+ 0x8], r9;\n"
        "adc r10, 0x0;\n"
        "mov [" $P0 "+ 0x10], r10;\n"
        "adc r11, rcx;\n"
        "mov [" $P0 "+ 0x18], r11"
    )}
}

// P0 = 3 * P1 - 8 * P2, computed as (p_256 - P2) << 3 + 3 * P1

macro_rules! cmsub38_p256 {
    ($P0:expr, $P1:expr, $P2:expr) => { Q!(
        /* First [r11;r10;r9;r8] = p_256 - P2 */
        "mov r8, 0xffffffffffffffff;\n"
        "xor r10d, r10d;\n"
        "sub r8, [" $P2 "];\n"
        "mov r9, 0x00000000ffffffff;\n"
        "sbb r9, [" $P2 "+ 0x8];\n"
        "sbb r10, [" $P2 "+ 0x10];\n"
        "mov r11, 0xffffffff00000001;\n"
        "sbb r11, [" $P2 "+ 0x18];\n"
        /* [r12;r11;r10;r9;r8] = (p_256 - P2) << 3 */
        "mov r12, r11;\n"
        "shld r11, r10, 3;\n"
        "shld r10, r9, 3;\n"
        "shld r9, r8, 3;\n"
        "shl r8, 3;\n"
        "shr r12, 61;\n"
        /* [rdx;r11;r10;r9;r8] = 2^256 + 3 * P1 + 8 * (p_256 - P2) */
        "mov rdx, 3;\n"
        "xor eax, eax;\n"
        "mulx rcx, rax, [" $P1 "];\n"
        "adcx r8, rax;\n"
        "adox r9, rcx;\n"
        "mulx rcx, rax, [" $P1 "+ 0x8];\n"
        "adcx r9, rax;\n"
        "adox r10, rcx;\n"
        "mulx rcx, rax, [" $P1 "+ 0x10];\n"
        "adcx r10, rax;\n"
        "adox r11, rcx;\n"
        "mulx rdx, rax, [" $P1 "+ 0x18];\n"
        "adcx r11, rax;\n"
        "adox rdx, r12;\n"
        "adc rdx, 1;\n"
        /* Now the tail for modular reduction from tripling */
        "add r8, rdx;\n"
        "mov rax, 0x100000000;\n"
        "mulx rcx, rax, rax;\n"
        "sbb rax, 0x0;\n"
        "sbb rcx, 0x0;\n"
        "sub r9, rax;\n"
        "sbb r10, rcx;\n"
        "mov rax, 0xffffffff00000001;\n"
        "mulx rcx, rax, rax;\n"
        "sbb r11, rax;\n"
        "sbb rdx, rcx;\n"
        "dec rdx;\n"
        "mov eax, 0xffffffff;\n"
        "and rax, rdx;\n"
        "xor ecx, ecx;\n"
        "sub rcx, rax;\n"
        "add r8, rdx;\n"
        "mov [" $P0 "], r8;\n"
        "adc r9, rax;\n"
        "mov [" $P0 "+ 0x8], r9;\n"
        "adc r10, 0x0;\n"
        "mov [" $P0 "+ 0x10], r10;\n"
        "adc r11, rcx;\n"
        "mov [" $P0 "+ 0x18], r11"
    )}
}

// P0 = 4 * P1 - P2, by direct subtraction of P2,
// since the quotient estimate still works safely
// for initial value > -p_256

macro_rules! cmsub41_p256 {
    ($P0:expr, $P1:expr, $P2:expr) => { Q!(
        "mov r11, [" $P1 "+ 0x18];\n"
        "mov rdx, r11;\n"
        "mov r10, [" $P1 "+ 0x10];\n"
        "shld r11, r10, 2;\n"
        "mov r9, [" $P1 "+ 0x8];\n"
        "shld r10, r9, 2;\n"
        "mov r8, [" $P1 "];\n"
        "shld r9, r8, 2;\n"
        "shl r8, 2;\n"
        "shr rdx, 62;\n"
        "add rdx, 1;\n"
        "sub r8, [" $P2 "];\n"
        "sbb r9, [" $P2 "+ 0x8];\n"
        "sbb r10, [" $P2 "+ 0x10];\n"
        "sbb r11, [" $P2 "+ 0x18];\n"
        "sbb rdx, 0;\n"
        /* Now the tail for modular reduction from tripling */
        "add r8, rdx;\n"
        "mov rax, 0x100000000;\n"
        "mulx rcx, rax, rax;\n"
        "sbb rax, 0x0;\n"
        "sbb rcx, 0x0;\n"
        "sub r9, rax;\n"
        "sbb r10, rcx;\n"
        "mov rax, 0xffffffff00000001;\n"
        "mulx rcx, rax, rax;\n"
        "sbb r11, rax;\n"
        "sbb rdx, rcx;\n"
        "dec rdx;\n"
        "mov eax, 0xffffffff;\n"
        "and rax, rdx;\n"
        "xor ecx, ecx;\n"
        "sub rcx, rax;\n"
        "add r8, rdx;\n"
        "mov [" $P0 "], r8;\n"
        "adc r9, rax;\n"
        "mov [" $P0 "+ 0x8], r9;\n"
        "adc r10, 0x0;\n"
        "mov [" $P0 "+ 0x10], r10;\n"
        "adc r11, rcx;\n"
        "mov [" $P0 "+ 0x18], r11"
    )}
}

/// Point doubling on NIST curve P-256 in Montgomery-Jacobian coordinates
///
///
/// Does p3 := 2 * p1 where all points are regarded as Jacobian triples with
/// each coordinate in the Montgomery domain, i.e. x' = (2^256 * x) mod p_256.
/// A Jacobian triple (x',y',z') represents affine point (x/z^2,y/z^3).
pub(crate) fn p256_montjdouble(p3: &mut [u64; 12], p1: &[u64; 12]) {
    // SAFETY: inline assembly. see [crate::low::inline_assembly_safety] for safety info.
    unsafe {
        core::arch::asm!(

        Q!("    endbr64         " ),

        // Save registers and make room on stack for temporary variables

        Q!("    push            " "rbx"),
        Q!("    push            " "rbp"),
        Q!("    push            " "r12"),
        Q!("    push            " "r13"),
        Q!("    push            " "r14"),
        Q!("    push            " "r15"),

        Q!("    sub             " "rsp, " NSPACE!()),

        // Main code, just a sequence of basic field operations

        // z2 = z^2
        // y2 = y^2

        montsqr_p256!(z2!(), z_1!()),
        montsqr_p256!(y2!(), y_1!()),

        // x2p = x^2 - z^4 = (x + z^2) * (x - z^2)

        sub_p256!(t2!(), x_1!(), z2!()),
        weakadd_p256!(t1!(), x_1!(), z2!()),
        montmul_p256!(x2p!(), t1!(), t2!()),

        // t1 = y + z
        // xy2 = x * y^2
        // x4p = x2p^2

        add_p256!(t1!(), y_1!(), z_1!()),
        montmul_p256!(xy2!(), x_1!(), y2!()),
        montsqr_p256!(x4p!(), x2p!()),

        // t1 = (y + z)^2

        montsqr_p256!(t1!(), t1!()),

        // d = 12 * xy2 - 9 * x4p
        // t1 = y^2 + 2 * y * z

        cmsub_p256!(d!(), "12", xy2!(), "9", x4p!()),
        sub_p256!(t1!(), t1!(), z2!()),

        // y4 = y^4

        montsqr_p256!(y4!(), y2!()),

        // dx2 = d * x2p

        montmul_p256!(dx2!(), d!(), x2p!()),

        // z_3' = 2 * y * z

        sub_p256!(z_3!(), t1!(), y2!()),

        // x' = 4 * xy2 - d

        cmsub41_p256!(x_3!(), xy2!(), d!()),

        // y' = 3 * dx2 - 8 * y4

        cmsub38_p256!(y_3!(), dx2!(), y4!()),

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
