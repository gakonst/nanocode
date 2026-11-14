// Written for Graviola by Joe Birr-Pixton, 2024.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

use core::arch::x86_64::*;

pub(crate) fn bignum_copy_row_from_table_16_avx2(
    z: &mut [u64],
    table: &[u64],
    _height: u64,
    index: u64,
) {
    debug_assert!(z.len() == 16);
    debug_assert!(table.len() == (_height as usize) * z.len());

    // SAFETY: this crate requires the `avx` and `avx2` cpu features
    unsafe { _bignum_copy_row_from_table_16_avx2(z, table, index) }
}

#[target_feature(enable = "avx,avx2")]
fn _bignum_copy_row_from_table_16_avx2(z: &mut [u64], table: &[u64], index: u64) {
    super::cpu::prefetch(table, 16);

    let mut acc0 = _mm256_setzero_si256();
    let mut acc1 = _mm256_setzero_si256();
    let mut acc2 = _mm256_setzero_si256();
    let mut acc3 = _mm256_setzero_si256();

    let desired_index = _mm_set1_epi64x(index as i64);
    let desired_index = _mm256_setr_m128i(desired_index, desired_index);

    let index = _mm_set1_epi64x(0);
    let mut index = _mm256_setr_m128i(index, index);

    let ones = _mm_set1_epi64x(1);
    let ones = _mm256_setr_m128i(ones, ones);

    for row in table.chunks_exact(16) {
        let mask = _mm256_cmpeq_epi64(index, desired_index);
        index = _mm256_add_epi64(index, ones);

        // SAFETY: `row` is exactly 16 words; `loadu` relaxes 256-bit alignment req.
        let (row0, row1, row2, row3) = unsafe {
            (
                _mm256_loadu_si256(row.as_ptr().add(0).cast()),
                _mm256_loadu_si256(row.as_ptr().add(4).cast()),
                _mm256_loadu_si256(row.as_ptr().add(8).cast()),
                _mm256_loadu_si256(row.as_ptr().add(12).cast()),
            )
        };

        let row0 = _mm256_and_si256(row0, mask);
        let row1 = _mm256_and_si256(row1, mask);
        let row2 = _mm256_and_si256(row2, mask);
        let row3 = _mm256_and_si256(row3, mask);

        acc0 = _mm256_xor_si256(row0, acc0);
        acc1 = _mm256_xor_si256(row1, acc1);
        acc2 = _mm256_xor_si256(row2, acc2);
        acc3 = _mm256_xor_si256(row3, acc3);
    }

    // SAFETY: `z` is exactly 16 words
    unsafe {
        _mm256_storeu_si256(z.as_mut_ptr().add(0).cast(), acc0);
        _mm256_storeu_si256(z.as_mut_ptr().add(4).cast(), acc1);
        _mm256_storeu_si256(z.as_mut_ptr().add(8).cast(), acc2);
        _mm256_storeu_si256(z.as_mut_ptr().add(12).cast(), acc3);
    }
}
