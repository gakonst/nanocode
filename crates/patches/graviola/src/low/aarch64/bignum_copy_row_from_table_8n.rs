// generated source. do not edit.
#![allow(non_upper_case_globals, unused_macros, unused_imports)]
use crate::low::macros::*;

// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

// ----------------------------------------------------------------------------
// Given table: uint64_t[height*width], copy table[idx*width...(idx+1)*width-1]
// into z[0..width-1]. width must be a multiple of 8.
// This function is constant-time with respect to the value of `idx`. This is
// achieved by reading the whole table and using the bit-masking to get the
// `idx`-th row.
//
//    extern void bignum_copy_row_from_table_8n
//     (uint64_t *z, const uint64_t *table, uint64_t height, uint64_t width, uint64_t idx);
//
// Standard ARM ABI: X0 = z, X1 = table, X2 = height, X3 = width, X4 = idx
// ----------------------------------------------------------------------------

macro_rules! z {
    () => {
        "x0"
    };
}
macro_rules! table {
    () => {
        "x1"
    };
}
macro_rules! height {
    () => {
        "x2"
    };
}
macro_rules! width {
    () => {
        "x3"
    };
}
macro_rules! idx {
    () => {
        "x4"
    };
}

macro_rules! i {
    () => {
        "x5"
    };
}
macro_rules! mask {
    () => {
        "x6"
    };
}
macro_rules! j {
    () => {
        "x7"
    };
}

macro_rules! vmask {
    () => {
        "v16"
    };
}

/// Given table: uint64_t[height*width], copy table[idx*width...(idx+1)*width-1]
///
/// into z[0..width-1]. width must be a multiple of 8.
/// This function is constant-time with respect to the value of `idx`. This is
/// achieved by reading the whole table and using the bit-masking to get the
/// `idx`-th row.
pub(crate) fn bignum_copy_row_from_table_8n(
    z: &mut [u64],
    table: &[u64],
    height: u64,
    width: u64,
    index: u64,
) {
    debug_assert!(z.len() as u64 == width);
    debug_assert!(width.is_multiple_of(8));
    // SAFETY: inline assembly. see [crate::low::inline_assembly_safety] for safety info.
    unsafe {
        core::arch::asm!(

        Q!("    cbz             " height!() ", " Label!("Lbignum_copy_row_from_table_8n_end", 2, After)),
        Q!("    cbz             " width!() ", " Label!("Lbignum_copy_row_from_table_8n_end", 2, After)),
        Q!("    mov             " i!() ", " width!()),
        Q!("    mov             " "x6, " z!()),
        Q!("    dup             " "v16.2d, xzr"),

        Q!(Label!("Lbignum_copy_row_from_table_8n_initzero", 3) ":"),
        Q!("    str             " "q16, [x6]"),
        Q!("    str             " "q16, [x6, #16]"),
        Q!("    str             " "q16, [x6, #32]"),
        Q!("    str             " "q16, [x6, #48]"),
        Q!("    add             " "x6, x6, #64"),
        Q!("    subs            " i!() ", " i!() ", #8"),
        Q!("    bne             " Label!("Lbignum_copy_row_from_table_8n_initzero", 3, Before)),

        Q!("    mov             " i!() ", xzr"),
        Q!("    mov             " "x8, " table!()),

        Q!(Label!("Lbignum_copy_row_from_table_8n_outerloop", 4) ":"),

        Q!("    cmp             " i!() ", " idx!()),
        Q!("    csetm           " mask!() ", eq"),
        Q!("    dup             " vmask!() ".2d, " mask!()),

        Q!("    mov             " j!() ", " width!()),
        Q!("    mov             " "x9, " z!()),

        Q!(Label!("Lbignum_copy_row_from_table_8n_innerloop", 5) ":"),

        Q!("    ldr             " "q17, [x8]"),
        Q!("    ldr             " "q18, [x9]"),
        Q!("    bit             " "v18.16b, v17.16b, " vmask!() ".16b"),
        Q!("    str             " "q18, [x9]"),

        Q!("    ldr             " "q17, [x8, #16]"),
        Q!("    ldr             " "q18, [x9, #16]"),
        Q!("    bit             " "v18.16b, v17.16b, " vmask!() ".16b"),
        Q!("    str             " "q18, [x9, #16]"),

        Q!("    ldr             " "q17, [x8, #32]"),
        Q!("    ldr             " "q18, [x9, #32]"),
        Q!("    bit             " "v18.16b, v17.16b, " vmask!() ".16b"),
        Q!("    str             " "q18, [x9, #32]"),

        Q!("    ldr             " "q17, [x8, #48]"),
        Q!("    ldr             " "q18, [x9, #48]"),
        Q!("    bit             " "v18.16b, v17.16b, " vmask!() ".16b"),
        Q!("    str             " "q18, [x9, #48]"),

        Q!("    add             " "x8, x8, #64"),
        Q!("    add             " "x9, x9, #64"),
        Q!("    subs            " j!() ", " j!() ", #8"),
        Q!("    bne             " Label!("Lbignum_copy_row_from_table_8n_innerloop", 5, Before)),

        Q!(Label!("Lbignum_copy_row_from_table_8n_innerloop_done", 6) ":"),
        Q!("    add             " i!() ", " i!() ", #1"),
        Q!("    cmp             " i!() ", " height!()),
        Q!("    bne             " Label!("Lbignum_copy_row_from_table_8n_outerloop", 4, Before)),

        Q!(Label!("Lbignum_copy_row_from_table_8n_end", 2) ":"),
        inout("x0") z.as_mut_ptr() => _,
        inout("x1") table.as_ptr() => _,
        inout("x2") height => _,
        inout("x3") width => _,
        inout("x4") index => _,
        // clobbers
        out("v16") _,
        out("v17") _,
        out("v18") _,
        out("x5") _,
        out("x6") _,
        out("x7") _,
        out("x8") _,
        out("x9") _,
            )
    };
}
