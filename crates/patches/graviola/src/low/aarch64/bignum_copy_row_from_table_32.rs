// generated source. do not edit.
#![allow(non_upper_case_globals, unused_macros, unused_imports)]
use crate::low::macros::*;

// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

// ----------------------------------------------------------------------------
// Given table: uint64_t[height*32], copy table[idx*32...(idx+1)*32-1]
// into z[0..row-1].
// This function is constant-time with respect to the value of `idx`. This is
// achieved by reading the whole table and using the bit-masking to get the
// `idx`-th row.
//
//    extern void bignum_copy_row_from_table_32
//     (uint64_t *z, const uint64_t *table, uint64_t height, uint64_t idx);
//
// Initial version written by Hanno Becker
// Standard ARM ABI: X0 = z, X1 = table, X2 = height, X3 = idx
// ----------------------------------------------------------------------------

// *****************************************************
// Main code
// *****************************************************

macro_rules! z {
    () => {
        "x0"
    };
}
macro_rules! tbl {
    () => {
        "x1"
    };
}
macro_rules! height {
    () => {
        "x2"
    };
}
macro_rules! idx {
    () => {
        "x3"
    };
}

macro_rules! mask {
    () => {
        "x5"
    };
}
macro_rules! cnt {
    () => {
        "x6"
    };
}

macro_rules! ventry0 {
    () => {
        "v20"
    };
}
macro_rules! qentry0 {
    () => {
        "q20"
    };
}
macro_rules! ventry1 {
    () => {
        "v21"
    };
}
macro_rules! qentry1 {
    () => {
        "q21"
    };
}
macro_rules! ventry2 {
    () => {
        "v22"
    };
}
macro_rules! qentry2 {
    () => {
        "q22"
    };
}
macro_rules! ventry3 {
    () => {
        "v23"
    };
}
macro_rules! qentry3 {
    () => {
        "q23"
    };
}
macro_rules! ventry4 {
    () => {
        "v24"
    };
}
macro_rules! qentry4 {
    () => {
        "q24"
    };
}
macro_rules! ventry5 {
    () => {
        "v25"
    };
}
macro_rules! qentry5 {
    () => {
        "q25"
    };
}
macro_rules! ventry6 {
    () => {
        "v26"
    };
}
macro_rules! qentry6 {
    () => {
        "q26"
    };
}
macro_rules! ventry7 {
    () => {
        "v27"
    };
}
macro_rules! qentry7 {
    () => {
        "q27"
    };
}
macro_rules! ventry8 {
    () => {
        "v28"
    };
}
macro_rules! qentry8 {
    () => {
        "q28"
    };
}
macro_rules! ventry9 {
    () => {
        "v29"
    };
}
macro_rules! qentry9 {
    () => {
        "q29"
    };
}
macro_rules! ventry10 {
    () => {
        "v30"
    };
}
macro_rules! qentry10 {
    () => {
        "q30"
    };
}
macro_rules! ventry11 {
    () => {
        "v31"
    };
}
macro_rules! qentry11 {
    () => {
        "q31"
    };
}
macro_rules! ventry12 {
    () => {
        "v0"
    };
}
macro_rules! qentry12 {
    () => {
        "q0"
    };
}
macro_rules! ventry13 {
    () => {
        "v1"
    };
}
macro_rules! qentry13 {
    () => {
        "q1"
    };
}
macro_rules! ventry14 {
    () => {
        "v2"
    };
}
macro_rules! qentry14 {
    () => {
        "q2"
    };
}
macro_rules! ventry15 {
    () => {
        "v3"
    };
}
macro_rules! qentry15 {
    () => {
        "q3"
    };
}

macro_rules! vtmp {
    () => {
        "v16"
    };
}
macro_rules! qtmp {
    () => {
        "q16"
    };
}

macro_rules! vmask {
    () => {
        "v17"
    };
}

/// Given table: uint64_t[height*32], copy table[idx*32...(idx+1)*32-1]
///
/// into z[0..row-1].
/// This function is constant-time with respect to the value of `idx`. This is
/// achieved by reading the whole table and using the bit-masking to get the
/// `idx`-th row.
pub(crate) fn bignum_copy_row_from_table_32(z: &mut [u64], table: &[u64], height: u64, index: u64) {
    debug_assert!(z.len() == 32);
    debug_assert!(crate::low::ct::into_public(index) < height);
    // SAFETY: inline assembly. see [crate::low::inline_assembly_safety] for safety info.
    unsafe {
        core::arch::asm!(

        // Clear accumulator
        // Zeroing can be done via xor, but xor isn't formalized yet.
        Q!("    dup             " ventry0!() ".2d, xzr"),
        Q!("    mov             " ventry1!() ".16b, " ventry0!() ".16b"),
        Q!("    mov             " ventry2!() ".16b, " ventry0!() ".16b"),
        Q!("    mov             " ventry3!() ".16b, " ventry0!() ".16b"),
        Q!("    mov             " ventry4!() ".16b, " ventry0!() ".16b"),
        Q!("    mov             " ventry5!() ".16b, " ventry0!() ".16b"),
        Q!("    mov             " ventry6!() ".16b, " ventry0!() ".16b"),
        Q!("    mov             " ventry7!() ".16b, " ventry0!() ".16b"),
        Q!("    mov             " ventry8!() ".16b, " ventry0!() ".16b"),
        Q!("    mov             " ventry9!() ".16b, " ventry0!() ".16b"),
        Q!("    mov             " ventry10!() ".16b, " ventry0!() ".16b"),
        Q!("    mov             " ventry11!() ".16b, " ventry0!() ".16b"),
        Q!("    mov             " ventry12!() ".16b, " ventry0!() ".16b"),
        Q!("    mov             " ventry13!() ".16b, " ventry0!() ".16b"),
        Q!("    mov             " ventry14!() ".16b, " ventry0!() ".16b"),
        Q!("    mov             " ventry15!() ".16b, " ventry0!() ".16b"),

        Q!("    mov             " cnt!() ", #0"),
        Q!(Label!("Lbignum_copy_row_from_table_32_loop", 2) ":"),

        // Compute mask: Check if current index matches target index
        Q!("    subs            " "xzr, " cnt!() ", " idx!()),
        Q!("    cinv            " mask!() ", xzr, eq"),
        Q!("    dup             " vmask!() ".2d, " mask!()),

        Q!("    ldr             " qtmp!() ", [" tbl!() ", #16 * 0]"),
        Q!("    bit             " ventry0!() ".16b, " vtmp!() ".16b, " vmask!() ".16b"),

        Q!("    ldr             " qtmp!() ", [" tbl!() ", #16 * 1]"),
        Q!("    bit             " ventry1!() ".16b, " vtmp!() ".16b, " vmask!() ".16b"),

        Q!("    ldr             " qtmp!() ", [" tbl!() ", #16 * 2]"),
        Q!("    bit             " ventry2!() ".16b, " vtmp!() ".16b, " vmask!() ".16b"),

        Q!("    ldr             " qtmp!() ", [" tbl!() ", #16 * 3]"),
        Q!("    bit             " ventry3!() ".16b, " vtmp!() ".16b, " vmask!() ".16b"),

        Q!("    ldr             " qtmp!() ", [" tbl!() ", #16 * 4]"),
        Q!("    bit             " ventry4!() ".16b, " vtmp!() ".16b, " vmask!() ".16b"),

        Q!("    ldr             " qtmp!() ", [" tbl!() ", #16 * 5]"),
        Q!("    bit             " ventry5!() ".16b, " vtmp!() ".16b, " vmask!() ".16b"),

        Q!("    ldr             " qtmp!() ", [" tbl!() ", #16 * 6]"),
        Q!("    bit             " ventry6!() ".16b, " vtmp!() ".16b, " vmask!() ".16b"),

        Q!("    ldr             " qtmp!() ", [" tbl!() ", #16 * 7]"),
        Q!("    bit             " ventry7!() ".16b, " vtmp!() ".16b, " vmask!() ".16b"),

        Q!("    ldr             " qtmp!() ", [" tbl!() ", #16 * 8]"),
        Q!("    bit             " ventry8!() ".16b, " vtmp!() ".16b, " vmask!() ".16b"),

        Q!("    ldr             " qtmp!() ", [" tbl!() ", #16 * 9]"),
        Q!("    bit             " ventry9!() ".16b, " vtmp!() ".16b, " vmask!() ".16b"),

        Q!("    ldr             " qtmp!() ", [" tbl!() ", #16 * 10]"),
        Q!("    bit             " ventry10!() ".16b, " vtmp!() ".16b, " vmask!() ".16b"),

        Q!("    ldr             " qtmp!() ", [" tbl!() ", #16 * 11]"),
        Q!("    bit             " ventry11!() ".16b, " vtmp!() ".16b, " vmask!() ".16b"),

        Q!("    ldr             " qtmp!() ", [" tbl!() ", #16 * 12]"),
        Q!("    bit             " ventry12!() ".16b, " vtmp!() ".16b, " vmask!() ".16b"),

        Q!("    ldr             " qtmp!() ", [" tbl!() ", #16 * 13]"),
        Q!("    bit             " ventry13!() ".16b, " vtmp!() ".16b, " vmask!() ".16b"),

        Q!("    ldr             " qtmp!() ", [" tbl!() ", #16 * 14]"),
        Q!("    bit             " ventry14!() ".16b, " vtmp!() ".16b, " vmask!() ".16b"),

        Q!("    ldr             " qtmp!() ", [" tbl!() ", #16 * 15]"),
        Q!("    bit             " ventry15!() ".16b, " vtmp!() ".16b, " vmask!() ".16b"),

        Q!("    add             " tbl!() ", " tbl!() ", #32 * 8"),

        Q!("    add             " cnt!() ", " cnt!() ", #1"),
        Q!("    subs            " "xzr, " height!() ", " cnt!()),
        Q!("    b.ne            " Label!("Lbignum_copy_row_from_table_32_loop", 2, Before)),

        Q!(Label!("Lbignum_copy_row_from_table_32_end", 3) ":"),

        Q!("    str             " qentry0!() ", [" z!() ", #16 * 0]"),
        Q!("    str             " qentry1!() ", [" z!() ", #16 * 1]"),
        Q!("    str             " qentry2!() ", [" z!() ", #16 * 2]"),
        Q!("    str             " qentry3!() ", [" z!() ", #16 * 3]"),
        Q!("    str             " qentry4!() ", [" z!() ", #16 * 4]"),
        Q!("    str             " qentry5!() ", [" z!() ", #16 * 5]"),
        Q!("    str             " qentry6!() ", [" z!() ", #16 * 6]"),
        Q!("    str             " qentry7!() ", [" z!() ", #16 * 7]"),
        Q!("    str             " qentry8!() ", [" z!() ", #16 * 8]"),
        Q!("    str             " qentry9!() ", [" z!() ", #16 * 9]"),
        Q!("    str             " qentry10!() ", [" z!() ", #16 * 10]"),
        Q!("    str             " qentry11!() ", [" z!() ", #16 * 11]"),
        Q!("    str             " qentry12!() ", [" z!() ", #16 * 12]"),
        Q!("    str             " qentry13!() ", [" z!() ", #16 * 13]"),
        Q!("    str             " qentry14!() ", [" z!() ", #16 * 14]"),
        Q!("    str             " qentry15!() ", [" z!() ", #16 * 15]"),

        inout("x0") z.as_mut_ptr() => _,
        inout("x1") table.as_ptr() => _,
        inout("x2") height => _,
        inout("x3") index => _,
        // clobbers
        out("v0") _,
        out("v1") _,
        out("v16") _,
        out("v17") _,
        out("v2") _,
        out("v20") _,
        out("v21") _,
        out("v22") _,
        out("v23") _,
        out("v24") _,
        out("v25") _,
        out("v26") _,
        out("v27") _,
        out("v28") _,
        out("v29") _,
        out("v3") _,
        out("v30") _,
        out("v31") _,
        out("x5") _,
        out("x6") _,
            )
    };
}
