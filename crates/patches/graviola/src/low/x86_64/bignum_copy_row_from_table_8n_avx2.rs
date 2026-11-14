// Written for Graviola by Joe Birr-Pixton, 2024.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

use core::arch::x86_64::*;

pub(crate) fn bignum_copy_row_from_table_8n_avx2(
    z: &mut [u64],
    table: &[u64],
    _height: u64,
    width: u64,
    index: u64,
) {
    debug_assert!(z.len() as u64 == width);
    debug_assert!(width.is_multiple_of(8));
    debug_assert!(table.len() as u64 == _height * width);

    // SAFETY: this crate requires the `avx` and `avx2` cpu features
    unsafe { _bignum_copy_row_from_table_8n_avx2(z, table, width, index) }
}

#[target_feature(enable = "avx,avx2")]
fn _bignum_copy_row_from_table_8n_avx2(z: &mut [u64], table: &[u64], width: u64, index: u64) {
    super::cpu::prefetch(table, 16);

    z.fill(0);

    let desired_index = _mm_set1_epi64x(index as i64);
    let desired_index = _mm256_setr_m128i(desired_index, desired_index);

    let index = _mm_set1_epi64x(0);
    let mut index = _mm256_setr_m128i(index, index);

    let ones = _mm_set1_epi64x(1);
    let ones = _mm256_setr_m128i(ones, ones);

    for row in table.chunks_exact(width as usize) {
        let mask = _mm256_cmpeq_epi64(index, desired_index);
        index = _mm256_add_epi64(index, ones);

        for (i, zz) in z.chunks_exact_mut(8).enumerate() {
            // SAFETY: `row` is a multiple of 8 words in length
            let (row0, row1) = unsafe {
                (
                    _mm256_loadu_si256(row.as_ptr().add(i * 8).cast()),
                    _mm256_loadu_si256(row.as_ptr().add(i * 8 + 4).cast()),
                )
            };

            let row0 = _mm256_and_si256(row0, mask);
            let row1 = _mm256_and_si256(row1, mask);

            // SAFETY: `zz` is exactly 8 words
            let (store0, store1) = unsafe {
                (
                    _mm256_loadu_si256(zz.as_ptr().add(0).cast()),
                    _mm256_loadu_si256(zz.as_ptr().add(4).cast()),
                )
            };
            let store0 = _mm256_xor_si256(store0, row0);
            let store1 = _mm256_xor_si256(store1, row1);

            // SAFETY: `zz` is exactly 8 words and writable
            unsafe {
                _mm256_storeu_si256(zz.as_mut_ptr().add(0).cast(), store0);
                _mm256_storeu_si256(zz.as_mut_ptr().add(4).cast(), store1);
            }
        }
    }
}
