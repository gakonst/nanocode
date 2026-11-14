// generated source. do not edit.
#![allow(non_upper_case_globals, unused_macros, unused_imports)]
use crate::low::macros::*;

// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

// ----------------------------------------------------------------------------
// Add modulo p_384, z := (x + y) mod p_384, assuming x and y reduced
// Inputs x[6], y[6]; output z[6]
//
//    extern void bignum_add_p384(uint64_t z[static 6], const uint64_t x[static 6],
//                                const uint64_t y[static 6]);
//
// Standard x86-64 ABI: RDI = z, RSI = x, RDX = y
// Microsoft x64 ABI:   RCX = z, RDX = x, R8 = y
// ----------------------------------------------------------------------------

macro_rules! z {
    () => {
        "rdi"
    };
}
macro_rules! x {
    () => {
        "rsi"
    };
}
macro_rules! y {
    () => {
        "rdx"
    };
}

macro_rules! d0 {
    () => {
        "rax"
    };
}
macro_rules! d1 {
    () => {
        "rcx"
    };
}
macro_rules! d2 {
    () => {
        "r8"
    };
}
macro_rules! d3 {
    () => {
        "r9"
    };
}
macro_rules! d4 {
    () => {
        "r10"
    };
}
macro_rules! d5 {
    () => {
        "r11"
    };
}

// Re-use the input pointers as temporaries once we're done

macro_rules! a {
    () => {
        "rsi"
    };
}
macro_rules! c {
    () => {
        "rdx"
    };
}

macro_rules! ashort {
    () => {
        "esi"
    };
}
macro_rules! cshort {
    () => {
        "edx"
    };
}

/// Add modulo p_384, z := (x + y) mod p_384, assuming x and y reduced
///
/// Inputs x[6], y[6]; output z[6]
pub(crate) fn bignum_add_p384(z: &mut [u64; 6], x: &[u64; 6], y: &[u64; 6]) {
    // SAFETY: inline assembly. see [crate::low::inline_assembly_safety] for safety info.
    unsafe {
        core::arch::asm!(

        Q!("    endbr64         " ),

        // Add the inputs as 2^384 * c + [d5;d4;d3;d2;d1;d0] = x + y
        // This could be combined with the next block using ADCX and ADOX.

        Q!("    mov             " d0!() ", [" x!() "]"),
        Q!("    add             " d0!() ", [" y!() "]"),
        Q!("    mov             " d1!() ", [" x!() "+ 8]"),
        Q!("    adc             " d1!() ", [" y!() "+ 8]"),
        Q!("    mov             " d2!() ", [" x!() "+ 16]"),
        Q!("    adc             " d2!() ", [" y!() "+ 16]"),
        Q!("    mov             " d3!() ", [" x!() "+ 24]"),
        Q!("    adc             " d3!() ", [" y!() "+ 24]"),
        Q!("    mov             " d4!() ", [" x!() "+ 32]"),
        Q!("    adc             " d4!() ", [" y!() "+ 32]"),
        Q!("    mov             " d5!() ", [" x!() "+ 40]"),
        Q!("    adc             " d5!() ", [" y!() "+ 40]"),
        Q!("    mov             " cshort!() ", 0"),
        Q!("    adc             " c!() ", " c!()),

        // Now subtract p_384 from 2^384 * c + [d5;d4;d3;d2;d1;d0] to get x + y - p_384
        // This is actually done by *adding* the 7-word negation r_384 = 2^448 - p_384
        // where r_384 = [-1; 0; 0; 0; 1; 0x00000000ffffffff; 0xffffffff00000001]

        Q!("    mov             " a!() ", 0xffffffff00000001"),
        Q!("    add             " d0!() ", " a!()),
        Q!("    mov             " ashort!() ", 0x00000000ffffffff"),
        Q!("    adc             " d1!() ", " a!()),
        Q!("    adc             " d2!() ", 1"),
        Q!("    adc             " d3!() ", 0"),
        Q!("    adc             " d4!() ", 0"),
        Q!("    adc             " d5!() ", 0"),
        Q!("    adc             " c!() ", -1"),

        // Since by hypothesis x < p_384 we know x + y - p_384 < 2^384, so the top
        // carry c actually gives us a bitmask for x + y - p_384 < 0, which we
        // now use to make r' = mask * (2^384 - p_384) for a compensating subtraction.
        // We don't quite have enough ABI-modifiable registers to create all three
        // nonzero digits of r while maintaining d0..d5, but make the first two now.

        Q!("    and             " c!() ", " a!()),
        Q!("    xor             " a!() ", " a!()),
        Q!("    sub             " a!() ", " c!()),

        // Do the first two digits of addition and writeback

        Q!("    sub             " d0!() ", " a!()),
        Q!("    mov             " "[" z!() "], " d0!()),
        Q!("    sbb             " d1!() ", " c!()),
        Q!("    mov             " "[" z!() "+ 8], " d1!()),

        // Preserve the carry chain while creating the extra masked digit since
        // the logical operation will clear CF

        Q!("    sbb             " d0!() ", " d0!()),
        Q!("    and             " c!() ", " a!()),
        Q!("    neg             " d0!()),

        // Do the rest of the addition and writeback

        Q!("    sbb             " d2!() ", " c!()),
        Q!("    mov             " "[" z!() "+ 16], " d2!()),
        Q!("    sbb             " d3!() ", 0"),
        Q!("    mov             " "[" z!() "+ 24], " d3!()),
        Q!("    sbb             " d4!() ", 0"),
        Q!("    mov             " "[" z!() "+ 32], " d4!()),
        Q!("    sbb             " d5!() ", 0"),
        Q!("    mov             " "[" z!() "+ 40], " d5!()),

        inout("rdi") z.as_mut_ptr() => _,
        inout("rsi") x.as_ptr() => _,
        inout("rdx") y.as_ptr() => _,
        // clobbers
        out("r10") _,
        out("r11") _,
        out("r8") _,
        out("r9") _,
        out("rax") _,
        out("rcx") _,
            )
    };
}
