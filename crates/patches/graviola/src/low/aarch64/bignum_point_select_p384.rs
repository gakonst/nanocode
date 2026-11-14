// Written for Graviola by Joe Birr-Pixton, 2024.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

use core::arch::aarch64::*;
use core::mem;

/// Viewing table as rows of 18 words width, copy the 18 words at
/// table[idx - 1] into z.  If `idx` is zero or larger than `height`,
/// `z` is set to zero (ie, a jacobian point at infinity).
pub(crate) fn bignum_jac_point_select_p384(z: &mut [u64; 18], table: &[u64], index: u8) {
    // SAFETY: crate requires `neon` cpu feature
    unsafe { _select_jac_p384(z, table, index) }
}

#[target_feature(enable = "neon")]
fn _select_jac_p384(z: &mut [u64; 18], table: &[u64], index: u8) {
    // SAFETY: uint32x4_t and u128 have the same number and meaning of bits
    let mut acc0: uint32x4_t = unsafe { mem::transmute(0u128) };
    let mut acc1 = acc0;
    let mut acc2 = acc0;
    let mut acc3 = acc0;
    let mut acc4 = acc0;
    let mut acc5 = acc0;
    let mut acc6 = acc0;
    let mut acc7 = acc0;
    let mut acc8 = acc0;

    let desired_index = vdupq_n_u32(index as u32);
    let mut index = vdupq_n_u32(1);
    let ones = index;

    for point in table.chunks_exact(18) {
        // SAFETY: `point` has 18 words and is readable, via `chunks_exact`
        let (row0, row1, row2, row3, row4, row5, row6, row7, row8) = unsafe {
            (
                vld1q_u32(point.as_ptr().add(0).cast()),
                vld1q_u32(point.as_ptr().add(2).cast()),
                vld1q_u32(point.as_ptr().add(4).cast()),
                vld1q_u32(point.as_ptr().add(6).cast()),
                vld1q_u32(point.as_ptr().add(8).cast()),
                vld1q_u32(point.as_ptr().add(10).cast()),
                vld1q_u32(point.as_ptr().add(12).cast()),
                vld1q_u32(point.as_ptr().add(14).cast()),
                vld1q_u32(point.as_ptr().add(16).cast()),
            )
        };

        let mask = vceqq_u32(index, desired_index);
        index = vaddq_u32(index, ones);

        let row0 = vandq_u32(row0, mask);
        let row1 = vandq_u32(row1, mask);
        let row2 = vandq_u32(row2, mask);
        let row3 = vandq_u32(row3, mask);
        let row4 = vandq_u32(row4, mask);
        let row5 = vandq_u32(row5, mask);
        let row6 = vandq_u32(row6, mask);
        let row7 = vandq_u32(row7, mask);
        let row8 = vandq_u32(row8, mask);

        acc0 = veorq_u32(acc0, row0);
        acc1 = veorq_u32(acc1, row1);
        acc2 = veorq_u32(acc2, row2);
        acc3 = veorq_u32(acc3, row3);
        acc4 = veorq_u32(acc4, row4);
        acc5 = veorq_u32(acc5, row5);
        acc6 = veorq_u32(acc6, row6);
        acc7 = veorq_u32(acc7, row7);
        acc8 = veorq_u32(acc8, row8);
    }

    // SAFETY: `z` can have 18 64-bit words written to it
    unsafe {
        vst1q_u32(z.as_mut_ptr().add(0).cast(), acc0);
        vst1q_u32(z.as_mut_ptr().add(2).cast(), acc1);
        vst1q_u32(z.as_mut_ptr().add(4).cast(), acc2);
        vst1q_u32(z.as_mut_ptr().add(6).cast(), acc3);
        vst1q_u32(z.as_mut_ptr().add(8).cast(), acc4);
        vst1q_u32(z.as_mut_ptr().add(10).cast(), acc5);
        vst1q_u32(z.as_mut_ptr().add(12).cast(), acc6);
        vst1q_u32(z.as_mut_ptr().add(14).cast(), acc7);
        vst1q_u32(z.as_mut_ptr().add(16).cast(), acc8);
    }
}
