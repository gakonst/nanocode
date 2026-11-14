// Written for Graviola by Joe Birr-Pixton, 2024.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

use core::arch::x86_64::*;

/// Viewing table as rows of 8 words width, copy the 8 words at
/// table[idx - 1] into z.  If `idx` is zero or larger than `height`,
/// `z` is set to zero (ie, the affine point at infinity).
///
/// This is useful to select an affine p256 point from a table of
/// precomputed points.
pub(crate) fn bignum_aff_point_select_p256(z: &mut [u64; 8], table: &[u64], index: u8) {
    // SAFETY: this crate requires the `avx` and `avx2` cpu features
    unsafe { _select_aff_p256(z, table, index) }
}

/// Viewing table as rows of 12 words width, copy the 12 words at
/// table[idx - 1] into z.  If `idx` is zero or larger than `height`,
/// `z` is set to zero (ie, a jacobian point at infinity).
pub(crate) fn bignum_jac_point_select_p256(z: &mut [u64; 12], table: &[u64], index: u8) {
    // SAFETY: this crate requires the `avx` and `avx2` cpu features
    unsafe { _select_jac_p256(z, table, index) }
}

#[target_feature(enable = "avx,avx2")]
fn _select_aff_p256(z: &mut [u64; 8], table: &[u64], index: u8) {
    super::cpu::prefetch(table, 16);

    let mut acc0 = _mm256_setzero_si256();
    let mut acc1 = _mm256_setzero_si256();

    let desired_index = _mm_set1_epi32(index as i32);
    let desired_index = _mm256_setr_m128i(desired_index, desired_index);

    let index = _mm_set1_epi32(1);
    let mut index = _mm256_setr_m128i(index, index);

    let ones = index;

    for point in table.chunks_exact(8) {
        // SAFETY: `point` is 8 words due to `chunks_exact` and readable
        let (row0, row1) = unsafe {
            (
                _mm256_loadu_si256(point.as_ptr().add(0).cast()),
                _mm256_loadu_si256(point.as_ptr().add(4).cast()),
            )
        };

        let mask = _mm256_cmpeq_epi32(index, desired_index);
        index = _mm256_add_epi32(index, ones);

        let row0 = _mm256_and_si256(row0, mask);
        let row1 = _mm256_and_si256(row1, mask);

        acc0 = _mm256_xor_si256(acc0, row0);
        acc1 = _mm256_xor_si256(acc1, row1);
    }

    // SAFETY: `z` is 8 words and writable
    unsafe {
        _mm256_storeu_si256(z.as_mut_ptr().add(0).cast(), acc0);
        _mm256_storeu_si256(z.as_mut_ptr().add(4).cast(), acc1);
    }
}

#[target_feature(enable = "avx,avx2")]
fn _select_jac_p256(z: &mut [u64; 12], table: &[u64], index: u8) {
    super::cpu::prefetch(table, 12);

    let mut acc0 = _mm256_setzero_si256();
    let mut acc1 = _mm256_setzero_si256();
    let mut acc2 = _mm256_setzero_si256();

    let desired_index = _mm_set1_epi32(index as i32);
    let desired_index = _mm256_setr_m128i(desired_index, desired_index);

    let index = _mm_set1_epi32(1);
    let mut index = _mm256_setr_m128i(index, index);

    let ones = index;

    for point in table.chunks_exact(12) {
        // SAFETY: `point` is 12 words due to `chunks_exact` and readable
        let (row0, row1, row2) = unsafe {
            (
                _mm256_loadu_si256(point.as_ptr().add(0).cast()),
                _mm256_loadu_si256(point.as_ptr().add(4).cast()),
                _mm256_loadu_si256(point.as_ptr().add(8).cast()),
            )
        };

        let mask = _mm256_cmpeq_epi32(index, desired_index);
        index = _mm256_add_epi32(index, ones);

        let row0 = _mm256_and_si256(row0, mask);
        let row1 = _mm256_and_si256(row1, mask);
        let row2 = _mm256_and_si256(row2, mask);

        acc0 = _mm256_xor_si256(acc0, row0);
        acc1 = _mm256_xor_si256(acc1, row1);
        acc2 = _mm256_xor_si256(acc2, row2);
    }

    // SAFETY: `z` is 12 words and writable
    unsafe {
        _mm256_storeu_si256(z.as_mut_ptr().add(0).cast(), acc0);
        _mm256_storeu_si256(z.as_mut_ptr().add(4).cast(), acc1);
        _mm256_storeu_si256(z.as_mut_ptr().add(8).cast(), acc2);
    }
}
