const BLOCK_BYTES: usize = 64;
const CHARACTER_BYTES_MAX: usize = 4;
const CONTINUATION_MASK: u8 = 0xc0;
const CONTINUATION_TAG: u8 = 0x80;

pub fn valid_up_to(bytes: &[u8]) -> usize {
    let checked = valid_blocks(bytes);
    assert!(checked <= bytes.len(), "the block validator stays inside the input");
    assert_eq!(checked % BLOCK_BYTES, 0, "the block validator checks whole blocks");
    let start = last_character_start(bytes, checked);
    match std::str::from_utf8(&bytes[start..]) {
        Ok(_) => bytes.len(),
        Err(error) => start + error.valid_up_to(),
    }
}

fn last_character_start(bytes: &[u8], end: usize) -> usize {
    (end.saturating_sub(CHARACTER_BYTES_MAX)..end)
        .rev()
        .find(|&index| bytes[index] & CONTINUATION_MASK != CONTINUATION_TAG)
        .unwrap_or(end.saturating_sub(CHARACTER_BYTES_MAX))
}

#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
mod lookup {
    pub(super) const TOO_SHORT: u8 = 1 << 0;
    pub(super) const TOO_LONG: u8 = 1 << 1;
    pub(super) const OVERLONG_3: u8 = 1 << 2;
    pub(super) const TOO_LARGE: u8 = 1 << 3;
    pub(super) const SURROGATE: u8 = 1 << 4;
    pub(super) const OVERLONG_2: u8 = 1 << 5;
    pub(super) const TOO_LARGE_1000: u8 = 1 << 6;
    pub(super) const OVERLONG_4: u8 = 1 << 6;
    pub(super) const TWO_CONTINUATIONS: u8 = 1 << 7;
    pub(super) const CARRY: u8 = TOO_SHORT | TOO_LONG | TWO_CONTINUATIONS;
    pub(super) const THIRD_BYTE_BIAS: u8 = 0xe0 - 0x80;
    pub(super) const FOURTH_BYTE_BIAS: u8 = 0xf0 - 0x80;
    pub(super) const HIGH_BIT: u8 = 0x80;

    pub(super) const FIRST_HIGH: [u8; 16] = [
        TOO_LONG,
        TOO_LONG,
        TOO_LONG,
        TOO_LONG,
        TOO_LONG,
        TOO_LONG,
        TOO_LONG,
        TOO_LONG,
        TWO_CONTINUATIONS,
        TWO_CONTINUATIONS,
        TWO_CONTINUATIONS,
        TWO_CONTINUATIONS,
        TOO_SHORT | OVERLONG_2,
        TOO_SHORT,
        TOO_SHORT | OVERLONG_3 | SURROGATE,
        TOO_SHORT | TOO_LARGE | TOO_LARGE_1000 | OVERLONG_4,
    ];

    pub(super) const FIRST_LOW: [u8; 16] = [
        CARRY | OVERLONG_3 | OVERLONG_2 | OVERLONG_4,
        CARRY | OVERLONG_2,
        CARRY,
        CARRY,
        CARRY | TOO_LARGE,
        CARRY | TOO_LARGE | TOO_LARGE_1000,
        CARRY | TOO_LARGE | TOO_LARGE_1000,
        CARRY | TOO_LARGE | TOO_LARGE_1000,
        CARRY | TOO_LARGE | TOO_LARGE_1000,
        CARRY | TOO_LARGE | TOO_LARGE_1000,
        CARRY | TOO_LARGE | TOO_LARGE_1000,
        CARRY | TOO_LARGE | TOO_LARGE_1000,
        CARRY | TOO_LARGE | TOO_LARGE_1000,
        CARRY | TOO_LARGE | TOO_LARGE_1000 | SURROGATE,
        CARRY | TOO_LARGE | TOO_LARGE_1000,
        CARRY | TOO_LARGE | TOO_LARGE_1000,
    ];

    pub(super) const SECOND_HIGH: [u8; 16] = [
        TOO_SHORT,
        TOO_SHORT,
        TOO_SHORT,
        TOO_SHORT,
        TOO_SHORT,
        TOO_SHORT,
        TOO_SHORT,
        TOO_SHORT,
        TOO_LONG | OVERLONG_2 | TWO_CONTINUATIONS | OVERLONG_3 | TOO_LARGE_1000 | OVERLONG_4,
        TOO_LONG | OVERLONG_2 | TWO_CONTINUATIONS | OVERLONG_3 | TOO_LARGE,
        TOO_LONG | OVERLONG_2 | TWO_CONTINUATIONS | SURROGATE | TOO_LARGE,
        TOO_LONG | OVERLONG_2 | TWO_CONTINUATIONS | SURROGATE | TOO_LARGE,
        TOO_SHORT,
        TOO_SHORT,
        TOO_SHORT,
        TOO_SHORT,
    ];

    pub(super) const INCOMPLETE_MAX: [u8; 16] = [
        0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xf0 - 1, 0xe0 - 1, 0xc0 - 1,
    ];
}

#[cfg(target_arch = "x86_64")]
fn simd() -> bool {
    std::is_x86_feature_detected!("avx2")
}

#[cfg(target_arch = "aarch64")]
const fn simd() -> bool {
    true
}

#[cfg(target_arch = "x86_64")]
fn valid_blocks(bytes: &[u8]) -> usize {
    if simd() { x86::valid_blocks(bytes) } else { 0 }
}

#[cfg(target_arch = "aarch64")]
fn valid_blocks(bytes: &[u8]) -> usize {
    if simd() { arm::valid_blocks(bytes) } else { 0 }
}

#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
const fn valid_blocks(_bytes: &[u8]) -> usize {
    0
}

#[cfg(target_arch = "x86_64")]
mod x86 {
    use std::arch::x86_64::*;

    use super::BLOCK_BYTES;
    use super::lookup::{
        FIRST_HIGH, FIRST_LOW, FOURTH_BYTE_BIAS, HIGH_BIT, INCOMPLETE_MAX, SECOND_HIGH, THIRD_BYTE_BIAS,
    };

    const VECTOR_BYTES: usize = 32;
    const NIBBLE: i8 = 0x0f;

    pub(super) fn valid_blocks(bytes: &[u8]) -> usize {
        let blocks = bytes.len() / BLOCK_BYTES;
        if blocks == 0 {
            return 0;
        }
        assert!(blocks * BLOCK_BYTES <= bytes.len());
        let valid = unsafe { valid_blocks_avx2(bytes.as_ptr(), blocks) };
        assert!(valid <= blocks);
        valid * BLOCK_BYTES
    }

    #[target_feature(enable = "avx2")]
    fn table(values: &[u8; 16]) -> __m256i {
        let array: [u8; VECTOR_BYTES] = core::array::from_fn(|index| values[index % values.len()]);
        unsafe { _mm256_loadu_si256(array.as_ptr().cast()) }
    }

    #[target_feature(enable = "avx2")]
    fn previous<const SHIFT: i32>(input: __m256i, previous_input: __m256i) -> __m256i {
        _mm256_alignr_epi8::<SHIFT>(input, _mm256_permute2x128_si256::<0x21>(previous_input, input))
    }

    #[target_feature(enable = "avx2")]
    fn high_nibbles(input: __m256i) -> __m256i {
        _mm256_and_si256(_mm256_srli_epi16::<4>(input), _mm256_set1_epi8(NIBBLE))
    }

    #[target_feature(enable = "avx2")]
    fn errors(input: __m256i, previous_input: __m256i, tables: &[__m256i; 3]) -> __m256i {
        let previous1 = previous::<15>(input, previous_input);
        let first_high = _mm256_shuffle_epi8(tables[0], high_nibbles(previous1));
        let first_low = _mm256_shuffle_epi8(tables[1], _mm256_and_si256(previous1, _mm256_set1_epi8(NIBBLE)));
        let second_high = _mm256_shuffle_epi8(tables[2], high_nibbles(input));
        let special = _mm256_and_si256(_mm256_and_si256(first_high, first_low), second_high);
        let previous2 = previous::<14>(input, previous_input);
        let previous3 = previous::<13>(input, previous_input);
        let third = _mm256_subs_epu8(previous2, _mm256_set1_epi8(THIRD_BYTE_BIAS as i8));
        let fourth = _mm256_subs_epu8(previous3, _mm256_set1_epi8(FOURTH_BYTE_BIAS as i8));
        let continuation = _mm256_and_si256(_mm256_or_si256(third, fourth), _mm256_set1_epi8(HIGH_BIT as i8));
        _mm256_xor_si256(continuation, special)
    }

    #[target_feature(enable = "avx2")]
    unsafe fn valid_blocks_avx2(input: *const u8, blocks: usize) -> usize {
        let tables = [table(&FIRST_HIGH), table(&FIRST_LOW), table(&SECOND_HIGH)];
        let incomplete_max = {
            let mut array = [0xff_u8; VECTOR_BYTES];
            array[VECTOR_BYTES - INCOMPLETE_MAX.len()..].copy_from_slice(&INCOMPLETE_MAX);
            unsafe { _mm256_loadu_si256(array.as_ptr().cast()) }
        };
        let mut previous_input = _mm256_setzero_si256();
        let mut incomplete = _mm256_setzero_si256();
        for block in 0..blocks {
            let start = unsafe { input.add(block * BLOCK_BYTES) };
            let first = unsafe { _mm256_loadu_si256(start.cast()) };
            let second = unsafe { _mm256_loadu_si256(start.add(VECTOR_BYTES).cast()) };
            let error = if _mm256_movemask_epi8(_mm256_or_si256(first, second)) == 0 {
                incomplete
            } else {
                _mm256_or_si256(errors(first, previous_input, &tables), errors(second, first, &tables))
            };
            if _mm256_testz_si256(error, error) == 0 {
                return block;
            }
            incomplete = _mm256_subs_epu8(second, incomplete_max);
            previous_input = second;
        }
        blocks
    }
}

#[cfg(target_arch = "aarch64")]
mod arm {
    use std::arch::aarch64::*;

    use super::BLOCK_BYTES;
    use super::lookup::{
        FIRST_HIGH, FIRST_LOW, FOURTH_BYTE_BIAS, HIGH_BIT, INCOMPLETE_MAX, SECOND_HIGH, THIRD_BYTE_BIAS,
    };

    const NIBBLE: u8 = 0x0f;

    pub(super) fn valid_blocks(bytes: &[u8]) -> usize {
        let blocks = bytes.len() / BLOCK_BYTES;
        if blocks == 0 {
            return 0;
        }
        assert!(blocks * BLOCK_BYTES <= bytes.len());
        let valid = unsafe { valid_blocks_neon(bytes.as_ptr(), blocks) };
        assert!(valid <= blocks);
        valid * BLOCK_BYTES
    }

    #[target_feature(enable = "neon")]
    fn errors(input: uint8x16_t, previous_input: uint8x16_t, tables: &[uint8x16_t; 3]) -> uint8x16_t {
        let previous1 = vextq_u8::<15>(previous_input, input);
        let first_high = vqtbl1q_u8(tables[0], vshrq_n_u8::<4>(previous1));
        let first_low = vqtbl1q_u8(tables[1], vandq_u8(previous1, vdupq_n_u8(NIBBLE)));
        let second_high = vqtbl1q_u8(tables[2], vshrq_n_u8::<4>(input));
        let special = vandq_u8(vandq_u8(first_high, first_low), second_high);
        let previous2 = vextq_u8::<14>(previous_input, input);
        let previous3 = vextq_u8::<13>(previous_input, input);
        let third = vqsubq_u8(previous2, vdupq_n_u8(THIRD_BYTE_BIAS));
        let fourth = vqsubq_u8(previous3, vdupq_n_u8(FOURTH_BYTE_BIAS));
        let continuation = vandq_u8(vorrq_u8(third, fourth), vdupq_n_u8(HIGH_BIT));
        veorq_u8(continuation, special)
    }

    #[target_feature(enable = "neon")]
    unsafe fn valid_blocks_neon(input: *const u8, blocks: usize) -> usize {
        let tables = unsafe {
            [vld1q_u8(FIRST_HIGH.as_ptr()), vld1q_u8(FIRST_LOW.as_ptr()), vld1q_u8(SECOND_HIGH.as_ptr())]
        };
        let incomplete_max = unsafe { vld1q_u8(INCOMPLETE_MAX.as_ptr()) };
        let mut previous_input = vdupq_n_u8(0);
        let mut incomplete = vdupq_n_u8(0);
        for block in 0..blocks {
            let vectors = unsafe { vld1q_u8_x4(input.add(block * BLOCK_BYTES)) };
            let any = vorrq_u8(vorrq_u8(vectors.0, vectors.1), vorrq_u8(vectors.2, vectors.3));
            let error = if vmaxvq_u8(any) < HIGH_BIT {
                incomplete
            } else {
                vorrq_u8(
                    vorrq_u8(errors(vectors.0, previous_input, &tables), errors(vectors.1, vectors.0, &tables)),
                    vorrq_u8(errors(vectors.2, vectors.1, &tables), errors(vectors.3, vectors.2, &tables)),
                )
            };
            if vmaxvq_u8(error) != 0 {
                return block;
            }
            incomplete = vqsubq_u8(vectors.3, incomplete_max);
            previous_input = vectors.3;
        }
        blocks
    }
}
