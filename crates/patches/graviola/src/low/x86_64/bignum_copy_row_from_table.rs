// generated source. do not edit.
#![allow(non_upper_case_globals, unused_macros, unused_imports)]
use crate::low::macros::*;

// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

// ----------------------------------------------------------------------------
// Given table: uint64_t[height*width], copy table[idx*width...(idx+1)*width-1]
// into z[0..width-1].
//
//    extern void bignum_copy_row_from_table
//     (uint64_t *z, const uint64_t *table, uint64_t height, uint64_t width,
//      uint64_t idx);
//
// Standard x86-64 ABI: RDI = z, RSI = table, RDX = height, RCX = width,
//                      R8 = idx
// Microsoft x64 ABI:   RCX = z, RDX = table, R8 = height, R9 = width,
//                      [RSP+40] = idx
// ----------------------------------------------------------------------------

macro_rules! z {
    () => {
        "rdi"
    };
}
macro_rules! table {
    () => {
        "rsi"
    };
}
macro_rules! height {
    () => {
        "rdx"
    };
}
macro_rules! width {
    () => {
        "rcx"
    };
}
macro_rules! idx {
    () => {
        "r8"
    };
}

macro_rules! i {
    () => {
        "r9"
    };
}
macro_rules! j {
    () => {
        "r10"
    };
}

/// Given table: uint64_t[height*width], copy table[idx*width...(idx+1)*width-1]
///
/// into z[0..width-1].
pub(crate) fn bignum_copy_row_from_table(
    z: &mut [u64],
    table: &[u64],
    height: u64,
    width: u64,
    index: u64,
) {
    debug_assert!(z.len() as u64 == width);
    // SAFETY: inline assembly. see [crate::low::inline_assembly_safety] for safety info.
    unsafe {
        core::arch::asm!(

        Q!("    endbr64         " ),

        Q!("    test            " height!() ", " height!()),
        Q!("    jz              " Label!("Lbignum_copy_row_from_table_end", 2, After)),
        Q!("    test            " width!() ", " width!()),
        Q!("    jz              " Label!("Lbignum_copy_row_from_table_end", 2, After)),
        Q!("    mov             " "rax, " z!()),
        Q!("    mov             " i!() ", " width!()),

        Q!(Label!("Lbignum_copy_row_from_table_initzero", 3) ":"),
        Q!("    mov             " "QWORD PTR [rax], 0"),
        Q!("    add             " "rax, 8"),
        Q!("    dec             " i!()),
        Q!("    jnz             " Label!("Lbignum_copy_row_from_table_initzero", 3, Before)),

        Q!("    mov             " i!() ", 0"),
        Q!("    mov             " "rax, " table!()),

        Q!(Label!("Lbignum_copy_row_from_table_outerloop", 4) ":"),
        Q!("    mov             " j!() ", 0"),

        Q!(Label!("Lbignum_copy_row_from_table_innerloop", 5) ":"),
        Q!("    xor             " "r11, r11"),
        Q!("    cmp             " i!() ", " idx!()),
        // cmov always read the memory address
        // https://stackoverflow.com/a/54050427
        Q!("    cmove           " "r11, [rax + 8 * " j!() "]"),
        Q!("    or              " "[" z!() "+ 8 * " j!() "], r11"),

        Q!("    inc             " j!()),
        Q!("    cmp             " j!() ", " width!()),
        Q!("    jne             " Label!("Lbignum_copy_row_from_table_innerloop", 5, Before)),

        Q!(Label!("Lbignum_copy_row_from_table_innerloop_done", 6) ":"),
        Q!("    lea             " j!() ", [" width!() "* 8]"),
        Q!("    add             " "rax, " j!()),
        Q!("    inc             " i!()),
        Q!("    cmp             " i!() ", " height!()),
        Q!("    jne             " Label!("Lbignum_copy_row_from_table_outerloop", 4, Before)),

        Q!(Label!("Lbignum_copy_row_from_table_end", 2) ":"),
        inout("rdi") z.as_mut_ptr() => _,
        inout("rsi") table.as_ptr() => _,
        inout("rdx") height => _,
        inout("rcx") width => _,
        inout("r8") index => _,
        // clobbers
        out("r10") _,
        out("r11") _,
        out("r9") _,
        out("rax") _,
            )
    };
}
