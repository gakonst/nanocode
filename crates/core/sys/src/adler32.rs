#[derive(Clone, Copy, Debug)]
pub struct Adler32Instructions(Kind);

#[derive(Clone, Copy, Debug)]
enum Kind {
    #[cfg(target_arch = "x86_64")]
    Avx2,
    #[cfg(target_arch = "x86_64")]
    Avx512,
    #[cfg(target_arch = "aarch64")]
    Neon,
}

#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
impl Kind {
    #[inline]
    fn detect() -> Option<Self> {
        #[cfg(target_arch = "x86_64")]
        {
            if !std::is_x86_feature_detected!("avx2") {
                return None;
            }
            if std::is_x86_feature_detected!("avx512f") && std::is_x86_feature_detected!("avx512bw")
            {
                return Some(Self::Avx512);
            }
            Some(Self::Avx2)
        }
        #[cfg(target_arch = "aarch64")]
        {
            if std::arch::is_aarch64_feature_detected!("neon") {
                return Some(Self::Neon);
            }
            None
        }
    }
}

#[cfg_attr(
    not(any(target_arch = "x86_64", target_arch = "aarch64")),
    expect(clippy::missing_const_for_fn)
)]
impl Adler32Instructions {
    #[inline]
    pub fn detect() -> Option<Self> {
        #[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
        {
            Kind::detect().map(Self)
        }
        #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
        {
            None
        }
    }

    #[inline]
    pub fn update(self, start: u32, data: &[u8]) -> u32 {
        match (self.0, start, data) {
            #[cfg(target_arch = "x86_64")]
            (Kind::Avx2, start, data) => unsafe { x86_64::update_avx2(start, data) },
            #[cfg(target_arch = "x86_64")]
            (Kind::Avx512, start, data) => unsafe { x86_64::update_avx512(start, data) },
            #[cfg(target_arch = "aarch64")]
            (Kind::Neon, start, data) => unsafe { aarch64::update(start, data) },
        }
    }
}

#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
mod sums {
    const MODULUS: u32 = 65_521;
    pub(super) const REDUCE_BYTES: usize = 5_552;
    pub(super) const HALF_BITS: u32 = 16;
    pub(super) const HALF_MASK: u32 = 0xFFFF;

    pub(super) struct BlockSums {
        pub(super) bytes: usize,
        pub(super) chunk_bytes: usize,
        pub(super) total: u64,
        pub(super) prefixes: u64,
        pub(super) weighted: u64,
    }

    pub(super) fn combine(low: u32, high: u32, sums: &BlockSums) -> (u32, u32) {
        assert!(sums.bytes <= REDUCE_BYTES);
        assert!(sums.bytes.is_multiple_of(sums.chunk_bytes));
        let modulus = u64::from(MODULUS);
        let high = (u64::from(high)
            + u64::from(low) * sums.bytes as u64
            + sums.chunk_bytes as u64 * sums.prefixes
            + sums.weighted)
            % modulus;
        let low = (u64::from(low) + sums.total) % modulus;
        (low as u32, high as u32)
    }

    pub(super) fn finish(mut low: u32, mut high: u32, rest: &[u8], chunk_bytes: usize) -> u32 {
        assert!(rest.len() < chunk_bytes);
        for byte in rest {
            low += u32::from(*byte);
            high += low;
        }
        ((high % MODULUS) << HALF_BITS) | (low % MODULUS)
    }
}

#[cfg(target_arch = "x86_64")]
mod x86_64 {
    use super::sums::{BlockSums, HALF_BITS, HALF_MASK, REDUCE_BYTES, combine, finish};
    use std::arch::x86_64::{
        __m256i, __m512i, _mm_add_epi64, _mm_cvtsi128_si64, _mm_extract_epi64, _mm256_add_epi32,
        _mm256_add_epi64, _mm256_castsi256_si128, _mm256_cvtepu32_epi64, _mm256_extracti128_si256,
        _mm256_loadu_si256, _mm256_madd_epi16, _mm256_maddubs_epi16, _mm256_sad_epu8,
        _mm256_set_epi64x, _mm256_set1_epi16, _mm256_setzero_si256, _mm512_add_epi32,
        _mm512_add_epi64, _mm512_castsi512_si256, _mm512_cvtepu32_epi64, _mm512_extracti64x4_epi64,
        _mm512_loadu_si512, _mm512_madd_epi16, _mm512_maddubs_epi16, _mm512_reduce_add_epi64,
        _mm512_sad_epu8, _mm512_set_epi64, _mm512_set1_epi16, _mm512_setzero_si512,
    };

    const CHUNK_BYTES_AVX2: usize = 32;
    const CHUNK_BYTES_AVX512: usize = 64;
    const WORD_BYTES: usize = 8;

    const fn weight_word(first: usize) -> u64 {
        let mut word = 0u64;
        let mut index = 0;
        while index < WORD_BYTES {
            word |= ((first - index) as u64) << (u8::BITS as usize * index);
            index += 1;
        }
        word
    }

    #[target_feature(enable = "avx")]
    #[inline]
    fn load_avx2(chunk: &[u8; CHUNK_BYTES_AVX2]) -> __m256i {
        unsafe { _mm256_loadu_si256(chunk.as_ptr().cast()) }
    }

    #[target_feature(enable = "avx512f")]
    #[inline]
    fn load_avx512(chunk: &[u8; CHUNK_BYTES_AVX512]) -> __m512i {
        unsafe { _mm512_loadu_si512(chunk.as_ptr().cast()) }
    }

    #[target_feature(enable = "avx2")]
    #[inline]
    fn sum_avx2(lanes: __m256i) -> u64 {
        let wide = _mm256_add_epi64(
            _mm256_cvtepu32_epi64(_mm256_castsi256_si128(lanes)),
            _mm256_cvtepu32_epi64(_mm256_extracti128_si256::<1>(lanes)),
        );
        let pair = _mm_add_epi64(
            _mm256_castsi256_si128(wide),
            _mm256_extracti128_si256::<1>(wide),
        );
        _mm_cvtsi128_si64(pair) as u64 + _mm_extract_epi64::<1>(pair) as u64
    }

    #[target_feature(enable = "avx512f")]
    #[inline]
    fn sum_avx512(lanes: __m512i) -> u64 {
        let wide = _mm512_add_epi64(
            _mm512_cvtepu32_epi64(_mm512_castsi512_si256(lanes)),
            _mm512_cvtepu32_epi64(_mm512_extracti64x4_epi64::<1>(lanes)),
        );
        _mm512_reduce_add_epi64(wide) as u64
    }

    #[target_feature(enable = "avx2")]
    fn block_avx2(low: u32, high: u32, block: &[[u8; CHUNK_BYTES_AVX2]]) -> (u32, u32) {
        let zero = _mm256_setzero_si256();
        let weights = _mm256_set_epi64x(
            weight_word(WORD_BYTES) as i64,
            weight_word(2 * WORD_BYTES) as i64,
            weight_word(3 * WORD_BYTES) as i64,
            weight_word(4 * WORD_BYTES) as i64,
        );
        let ones = _mm256_set1_epi16(1);
        let mut total = zero;
        let mut prefixes = zero;
        let mut weighted = zero;
        for chunk in block {
            let bytes = load_avx2(chunk);
            prefixes = _mm256_add_epi32(prefixes, total);
            total = _mm256_add_epi32(total, _mm256_sad_epu8(bytes, zero));
            weighted = _mm256_add_epi32(
                weighted,
                _mm256_madd_epi16(_mm256_maddubs_epi16(bytes, weights), ones),
            );
        }
        let sums = BlockSums {
            bytes: block.len() * CHUNK_BYTES_AVX2,
            chunk_bytes: CHUNK_BYTES_AVX2,
            total: sum_avx2(total),
            prefixes: sum_avx2(prefixes),
            weighted: sum_avx2(weighted),
        };
        combine(low, high, &sums)
    }

    #[target_feature(enable = "avx512f,avx512bw")]
    fn block_avx512(low: u32, high: u32, block: &[[u8; CHUNK_BYTES_AVX512]]) -> (u32, u32) {
        let zero = _mm512_setzero_si512();
        let weights = _mm512_set_epi64(
            weight_word(WORD_BYTES) as i64,
            weight_word(2 * WORD_BYTES) as i64,
            weight_word(3 * WORD_BYTES) as i64,
            weight_word(4 * WORD_BYTES) as i64,
            weight_word(5 * WORD_BYTES) as i64,
            weight_word(6 * WORD_BYTES) as i64,
            weight_word(7 * WORD_BYTES) as i64,
            weight_word(8 * WORD_BYTES) as i64,
        );
        let ones = _mm512_set1_epi16(1);
        let mut total = zero;
        let mut prefixes = zero;
        let mut weighted = zero;
        for chunk in block {
            let bytes = load_avx512(chunk);
            prefixes = _mm512_add_epi32(prefixes, total);
            total = _mm512_add_epi32(total, _mm512_sad_epu8(bytes, zero));
            weighted = _mm512_add_epi32(
                weighted,
                _mm512_madd_epi16(_mm512_maddubs_epi16(bytes, weights), ones),
            );
        }
        let sums = BlockSums {
            bytes: block.len() * CHUNK_BYTES_AVX512,
            chunk_bytes: CHUNK_BYTES_AVX512,
            total: sum_avx512(total),
            prefixes: sum_avx512(prefixes),
            weighted: sum_avx512(weighted),
        };
        combine(low, high, &sums)
    }

    #[target_feature(enable = "avx2")]
    pub(super) fn update_avx2(start: u32, data: &[u8]) -> u32 {
        let mut low = start & HALF_MASK;
        let mut high = start >> HALF_BITS;
        let (chunks, rest) = data.as_chunks::<CHUNK_BYTES_AVX2>();
        for block in chunks.chunks(REDUCE_BYTES / CHUNK_BYTES_AVX2) {
            (low, high) = block_avx2(low, high, block);
        }
        finish(low, high, rest, CHUNK_BYTES_AVX2)
    }

    #[target_feature(enable = "avx2,avx512f,avx512bw")]
    pub(super) fn update_avx512(start: u32, data: &[u8]) -> u32 {
        let mut low = start & HALF_MASK;
        let mut high = start >> HALF_BITS;
        let (chunks, rest) = data.as_chunks::<CHUNK_BYTES_AVX512>();
        for block in chunks.chunks(REDUCE_BYTES / CHUNK_BYTES_AVX512) {
            (low, high) = block_avx512(low, high, block);
        }
        if rest.len() >= CHUNK_BYTES_AVX2 {
            return update_avx2(finish(low, high, &[], CHUNK_BYTES_AVX512), rest);
        }
        finish(low, high, rest, CHUNK_BYTES_AVX512)
    }
}

#[cfg(target_arch = "aarch64")]
mod aarch64 {
    use super::sums::{BlockSums, HALF_BITS, HALF_MASK, REDUCE_BYTES, combine, finish};
    use std::arch::aarch64::{
        uint8x16x4_t, uint16x8_t, vaddlvq_u32, vaddq_u32, vaddw_high_u8, vaddw_u8, vcombine_u16,
        vcreate_u16, vdupq_n_u16, vdupq_n_u32, vget_low_u8, vget_low_u16, vld1q_u8_x4,
        vmlal_high_u16, vmlal_u16, vpadalq_u8, vpadalq_u16, vpaddlq_u8,
    };

    const CHUNK_BYTES: usize = 64;
    const COLUMN_LANES: usize = 8;
    const COLUMNS: usize = CHUNK_BYTES / COLUMN_LANES;
    const HALF_COLUMN_LANES: usize = COLUMN_LANES / 2;

    #[target_feature(enable = "neon")]
    #[inline]
    fn load(chunk: &[u8; CHUNK_BYTES]) -> uint8x16x4_t {
        unsafe { vld1q_u8_x4(chunk.as_ptr()) }
    }

    const fn half_weights(first: usize) -> u64 {
        let mut word = 0u64;
        let mut index = 0;
        while index < HALF_COLUMN_LANES {
            word |= ((first - index) as u64) << (u16::BITS as usize * index);
            index += 1;
        }
        word
    }

    #[target_feature(enable = "neon")]
    #[inline]
    fn weights(column: usize) -> uint16x8_t {
        let first = CHUNK_BYTES - COLUMN_LANES * column;
        vcombine_u16(
            vcreate_u16(half_weights(first)),
            vcreate_u16(half_weights(first - HALF_COLUMN_LANES)),
        )
    }

    #[target_feature(enable = "neon")]
    fn block(low: u32, high: u32, block: &[[u8; CHUNK_BYTES]]) -> (u32, u32) {
        let mut total = vdupq_n_u32(0);
        let mut prefixes = vdupq_n_u32(0);
        let mut columns = [vdupq_n_u16(0); COLUMNS];
        for chunk in block {
            let bytes = load(chunk);
            prefixes = vaddq_u32(prefixes, total);
            let pairs = vpadalq_u8(
                vpadalq_u8(vpadalq_u8(vpaddlq_u8(bytes.0), bytes.1), bytes.2),
                bytes.3,
            );
            total = vpadalq_u16(total, pairs);
            columns[0] = vaddw_u8(columns[0], vget_low_u8(bytes.0));
            columns[1] = vaddw_high_u8(columns[1], bytes.0);
            columns[2] = vaddw_u8(columns[2], vget_low_u8(bytes.1));
            columns[3] = vaddw_high_u8(columns[3], bytes.1);
            columns[4] = vaddw_u8(columns[4], vget_low_u8(bytes.2));
            columns[5] = vaddw_high_u8(columns[5], bytes.2);
            columns[6] = vaddw_u8(columns[6], vget_low_u8(bytes.3));
            columns[7] = vaddw_high_u8(columns[7], bytes.3);
        }
        let mut weighted = vdupq_n_u32(0);
        for (index, column) in columns.iter().enumerate() {
            let weights = weights(index);
            weighted = vmlal_u16(weighted, vget_low_u16(*column), vget_low_u16(weights));
            weighted = vmlal_high_u16(weighted, *column, weights);
        }
        let sums = BlockSums {
            bytes: block.len() * CHUNK_BYTES,
            chunk_bytes: CHUNK_BYTES,
            total: vaddlvq_u32(total),
            prefixes: vaddlvq_u32(prefixes),
            weighted: vaddlvq_u32(weighted),
        };
        combine(low, high, &sums)
    }

    #[target_feature(enable = "neon")]
    pub(super) fn update(start: u32, data: &[u8]) -> u32 {
        let mut low = start & HALF_MASK;
        let mut high = start >> HALF_BITS;
        let (chunks, rest) = data.as_chunks::<CHUNK_BYTES>();
        for chunks in chunks.chunks(REDUCE_BYTES / CHUNK_BYTES) {
            (low, high) = block(low, high, chunks);
        }
        finish(low, high, rest, CHUNK_BYTES)
    }
}
