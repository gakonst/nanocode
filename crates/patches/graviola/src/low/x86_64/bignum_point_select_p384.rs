// Written for Graviola by Joe Birr-Pixton, 2024.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

use core::arch::x86_64::*;

/// Viewing table as rows of 18 words width, copy the 18 words at
/// table[idx - 1] into z.  If `idx` is zero or larger than `height`,
/// `z` is set to zero (ie, a jacobian point at infinity).
pub(crate) fn bignum_jac_point_select_p384(z: &mut [u64; 18], table: &[u64], index: u8) {
    // SAFETY: this crate requires the `avx` and `avx2` cpu features
    unsafe { _select_jac_p384(z, table, index) }
}

#[target_feature(enable = "avx,avx2")]
fn _select_jac_p384(z: &mut [u64; 18], table: &[u64], index: u8) {
    super::cpu::prefetch(table, 18);

    let mut acc0 = _mm256_setzero_si256();
    let mut acc1 = _mm256_setzero_si256();
    let mut acc2 = _mm256_setzero_si256();
    let mut acc3 = _mm256_setzero_si256();
    let mut acc4 = _mm256_setzero_si256();

    let desired_index = _mm_set1_epi32(index as i32);
    let desired_index = _mm256_setr_m128i(desired_index, desired_index);

    let index = _mm_set1_epi32(1);
    let mut index = _mm256_setr_m128i(index, index);

    let ones = index;

    for point in table.chunks_exact(18) {
        // SAFETY: `point` is 18 words due to `chunks_exact` and readable.
        let (row0, row1, row2, row3) = unsafe {
            (
                _mm256_loadu_si256(point.as_ptr().add(0).cast()),
                _mm256_loadu_si256(point.as_ptr().add(4).cast()),
                _mm256_loadu_si256(point.as_ptr().add(8).cast()),
                _mm256_loadu_si256(point.as_ptr().add(12).cast()),
            )
        };

        let mut tmp = [0u64; 4];
        tmp[0..2].copy_from_slice(&point[16..18]);
        // SAFETY: `tmp` is 4 x 64 bit words and readable
        let row4 = unsafe { _mm256_loadu_si256(tmp.as_ptr().cast()) };

        let mask = _mm256_cmpeq_epi32(index, desired_index);
        index = _mm256_add_epi32(index, ones);

        let row0 = _mm256_and_si256(row0, mask);
        let row1 = _mm256_and_si256(row1, mask);
        let row2 = _mm256_and_si256(row2, mask);
        let row3 = _mm256_and_si256(row3, mask);
        let row4 = _mm256_and_si256(row4, mask);

        acc0 = _mm256_xor_si256(acc0, row0);
        acc1 = _mm256_xor_si256(acc1, row1);
        acc2 = _mm256_xor_si256(acc2, row2);
        acc3 = _mm256_xor_si256(acc3, row3);
        acc4 = _mm256_xor_si256(acc4, row4);
    }

    // SAFETY: `z` is 18-words/1152-bits, requiring a partial write of the final 256-bit term
    unsafe {
        _mm256_storeu_si256(z.as_mut_ptr().add(0).cast(), acc0);
        _mm256_storeu_si256(z.as_mut_ptr().add(4).cast(), acc1);
        _mm256_storeu_si256(z.as_mut_ptr().add(8).cast(), acc2);
        _mm256_storeu_si256(z.as_mut_ptr().add(12).cast(), acc3);
    }

    let mut tmp = [0u64; 4];
    // SAFETY: `tmp` is 256-bits and writable
    unsafe { _mm256_storeu_si256(tmp.as_mut_ptr().cast(), acc4) };
    z[16..18].copy_from_slice(&tmp[0..2]);
}
