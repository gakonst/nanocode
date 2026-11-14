// generated source. do not edit.
#![allow(non_upper_case_globals, unused_macros, unused_imports)]
use crate::low::macros::*;

// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

// ----------------------------------------------------------------------------
// Multiply-add modulo the order of the curve25519/edwards25519 basepoint
// Inputs x[4], y[4], c[4]; output z[4]
//
//    extern void bignum_madd_n25519(uint64_t z[static 4], const uint64_t x[static 4],
//                                   const uint64_t y[static 4],
//                                   const uint64_t c[static 4]);
//
// Performs z := (x * y + c) mod n_25519, where the modulus is
// n_25519 = 2^252 + 27742317777372353535851937790883648493, the
// order of the curve25519/edwards25519 basepoint. The result z
// and the inputs x, y and c are all 4 digits (256 bits).
//
// Standard x86-64 ABI: RDI = z, RSI = x, RDX = y, RCX = c
// Microsoft x64 ABI:   RCX = z, RDX = x, R8 = y, R9 = c
// ----------------------------------------------------------------------------

// Single round of modular reduction mod_n25519, mapping
// [m4;m3;m2;m1;m0] = m to [m3;m2;m1;m0] = m mod n_25519,
// *assuming* the input m < 2^64 * n_25519. This is very
// close to the loop body of the bignum_mod_n25519 function.

macro_rules! reduce {
    ($m4:expr, $m3:expr, $m2:expr, $m1:expr, $m0:expr) => { Q!(
        "mov rbx, " $m4 ";\n"
        "shld rbx, " $m3 ", 0x4;\n"
        "shr " $m4 ", 0x3c;\n"
        "sub rbx, " $m4 ";\n"
        "shl " $m3 ", 0x4;\n"
        "shrd " $m3 ", " $m4 ", 0x4;\n"
        "movabs rax, 0x5812631a5cf5d3ed;\n"
        "mul rbx;\n"
        "mov rbp, rax;\n"
        "mov rcx, rdx;\n"
        "movabs rax, 0x14def9dea2f79cd6;\n"
        "mul rbx;\n"
        "add rcx, rax;\n"
        "adc rdx, 0x0;\n"
        "sub " $m0 ", rbp;\n"
        "sbb " $m1 ", rcx;\n"
        "sbb " $m2 ", rdx;\n"
        "sbb " $m3 ", 0x0;\n"
        "sbb rbx, rbx;\n"
        "movabs rax, 0x5812631a5cf5d3ed;\n"
        "and rax, rbx;\n"
        "movabs rdx, 0x14def9dea2f79cd6;\n"
        "and rdx, rbx;\n"
        "movabs rbx, 0x1000000000000000;\n"
        "and rbx, rax;\n"
        "add " $m0 ", rax;\n"
        "adc " $m1 ", rdx;\n"
        "adc " $m2 ", 0x0;\n"
        "adc " $m3 ", rbx"
    )}
}

// Special case of "reduce" with m4 = 0. As well as not using m4,
// the quotient selection is slightly simpler, just floor(m/2^252)
// versus min (floor(m/2^252)) (2^63-1).

macro_rules! reduce0 {
    ($m3:expr, $m2:expr, $m1:expr, $m0:expr) => { Q!(
        "mov rbx, " $m3 ";\n"
        "shr rbx, 60;\n"
        "shl " $m3 ", 4;\n"
        "shr " $m3 ", 4;\n"
        "movabs rax, 0x5812631a5cf5d3ed;\n"
        "mul rbx;\n"
        "mov rbp, rax;\n"
        "mov rcx, rdx;\n"
        "movabs rax, 0x14def9dea2f79cd6;\n"
        "mul rbx;\n"
        "add rcx, rax;\n"
        "adc rdx, 0x0;\n"
        "sub " $m0 ", rbp;\n"
        "sbb " $m1 ", rcx;\n"
        "sbb " $m2 ", rdx;\n"
        "sbb " $m3 ", 0x0;\n"
        "sbb rbx, rbx;\n"
        "movabs rax, 0x5812631a5cf5d3ed;\n"
        "and rax, rbx;\n"
        "movabs rdx, 0x14def9dea2f79cd6;\n"
        "and rdx, rbx;\n"
        "movabs rbx, 0x1000000000000000;\n"
        "and rbx, rax;\n"
        "add " $m0 ", rax;\n"
        "adc " $m1 ", rdx;\n"
        "adc " $m2 ", 0x0;\n"
        "adc " $m3 ", rbx"
    )}
}

/// Multiply-add modulo the order of the curve25519/edwards25519 basepoint
///
/// Inputs x[4], y[4], c[4]; output z[4]
///
/// Performs z := (x * y + c) mod n_25519, where the modulus is
/// n_25519 = 2^252 + 27742317777372353535851937790883648493, the
/// order of the curve25519/edwards25519 basepoint. The result z
/// and the inputs x, y and c are all 4 digits (256 bits).
pub(crate) fn bignum_madd_n25519(z: &mut [u64; 4], x: &[u64; 4], y: &[u64; 4], c: &[u64; 4]) {
    // SAFETY: inline assembly. see [crate::low::inline_assembly_safety] for safety info.
    unsafe {
        core::arch::asm!(

        Q!("    endbr64         " ),

        // Save some additional registers for use

        Q!("    push            " "rbx"),
        Q!("    push            " "rbp"),
        Q!("    push            " "r12"),
        Q!("    push            " "r13"),
        Q!("    push            " "r14"),
        Q!("    push            " "r15"),

        // First compute [r15;r14;r13;r12;r11;r10;r9;r8] = x * y + c. This is
        // a multiply-add variant of an ADCX/ADOX-based schoolbook multiplier,
        // starting the accumulation with the c term and doing the zeroth row
        // in the same uniform fashion, otherwise similar to the start of
        // bignum_mul_p256k1.

        Q!("    mov             " "r8, [rcx]"),
        Q!("    mov             " "r9, [rcx + 8]"),
        Q!("    mov             " "r10, [rcx + 16]"),
        Q!("    mov             " "r11, [rcx + 24]"),
        Q!("    mov             " "rcx, rdx"),
        Q!("    xor             " "ebp, ebp"),
        Q!("    mov             " "rdx, [rcx]"),
        Q!("    mulx            " "rbx, rax, [rsi]"),
        Q!("    adcx            " "r8, rax"),
        Q!("    adox            " "r9, rbx"),
        Q!("    mulx            " "rbx, rax, [rsi + 0x8]"),
        Q!("    adcx            " "r9, rax"),
        Q!("    adox            " "r10, rbx"),
        Q!("    mulx            " "rbx, rax, [rsi + 0x10]"),
        Q!("    adcx            " "r10, rax"),
        Q!("    adox            " "r11, rbx"),
        Q!("    mulx            " "r12, rax, [rsi + 0x18]"),
        Q!("    adcx            " "r11, rax"),
        Q!("    adox            " "r12, rbp"),
        Q!("    adcx            " "r12, rbp"),
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
        Q!("    adcx            " "r13, rbp"),
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
        Q!("    adcx            " "r14, rbp"),
        Q!("    xor             " "ebp, ebp"),
        Q!("    mov             " "rdx, [rcx + 0x18]"),
        Q!("    mulx            " "rbx, rax, [rsi]"),
        Q!("    adcx            " "r11, rax"),
        Q!("    adox            " "r12, rbx"),
        Q!("    mulx            " "rbx, rax, [rsi + 0x8]"),
        Q!("    adcx            " "r12, rax"),
        Q!("    adox            " "r13, rbx"),
        Q!("    mulx            " "rbx, rax, [rsi + 0x10]"),
        Q!("    adcx            " "r13, rax"),
        Q!("    adox            " "r14, rbx"),
        Q!("    mulx            " "r15, rax, [rsi + 0x18]"),
        Q!("    adcx            " "r14, rax"),
        Q!("    adox            " "r15, rbp"),
        Q!("    adcx            " "r15, rbp"),

        // Now do the modular reduction and write back

        reduce0!("r15", "r14", "r13", "r12"),
        reduce!("r15", "r14", "r13", "r12", "r11"),
        reduce!("r14", "r13", "r12", "r11", "r10"),
        reduce!("r13", "r12", "r11", "r10", "r9"),
        reduce!("r12", "r11", "r10", "r9", "r8"),

        Q!("    mov             " "[rdi], r8"),
        Q!("    mov             " "[rdi + 8], r9"),
        Q!("    mov             " "[rdi + 16], r10"),
        Q!("    mov             " "[rdi + 24], r11"),

        // Restore registers and return

        Q!("    pop             " "r15"),
        Q!("    pop             " "r14"),
        Q!("    pop             " "r13"),
        Q!("    pop             " "r12"),
        Q!("    pop             " "rbp"),
        Q!("    pop             " "rbx"),

        inout("rdi") z.as_mut_ptr() => _,
        inout("rsi") x.as_ptr() => _,
        inout("rdx") y.as_ptr() => _,
        inout("rcx") c.as_ptr() => _,
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
