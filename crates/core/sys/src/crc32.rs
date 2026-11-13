#[derive(Clone, Copy, Debug)]
pub struct Crc32Instructions(Kind);

#[derive(Clone, Copy, Debug)]
enum Kind {
    #[cfg(target_arch = "x86_64")]
    Pclmulqdq,
    #[cfg(target_arch = "x86_64")]
    Avx2Vpclmulqdq,
    #[cfg(target_arch = "x86_64")]
    Avx512Vpclmulqdq,
    #[cfg(target_arch = "aarch64")]
    Crc,
    #[cfg(target_arch = "aarch64")]
    CrcPmull,
}

#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
impl Kind {
    #[inline]
    fn detect() -> Option<Self> {
        #[cfg(target_arch = "x86_64")]
        {
            if !(std::is_x86_feature_detected!("pclmulqdq")
                && std::is_x86_feature_detected!("sse4.1")
                && std::is_x86_feature_detected!("ssse3"))
            {
                return None;
            }
            if !(std::is_x86_feature_detected!("avx2")
                && std::is_x86_feature_detected!("vpclmulqdq"))
            {
                return Some(Self::Pclmulqdq);
            }
            if std::is_x86_feature_detected!("avx512f") {
                return Some(Self::Avx512Vpclmulqdq);
            }
            Some(Self::Avx2Vpclmulqdq)
        }
        #[cfg(target_arch = "aarch64")]
        {
            if !std::arch::is_aarch64_feature_detected!("crc") {
                return None;
            }
            if std::arch::is_aarch64_feature_detected!("aes") {
                return Some(Self::CrcPmull);
            }
            Some(Self::Crc)
        }
    }
}

#[cfg_attr(
    not(any(target_arch = "x86_64", target_arch = "aarch64")),
    expect(clippy::missing_const_for_fn)
)]
impl Crc32Instructions {
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
            (Kind::Pclmulqdq, start, data) => unsafe { x86_64::update(start, data) },
            #[cfg(target_arch = "x86_64")]
            (Kind::Avx2Vpclmulqdq, start, data) => unsafe { x86_64::update_avx2(start, data) },
            #[cfg(target_arch = "x86_64")]
            (Kind::Avx512Vpclmulqdq, start, data) => unsafe { x86_64::update_avx512(start, data) },
            #[cfg(target_arch = "aarch64")]
            (Kind::Crc, start, data) => unsafe { aarch64::update(start, data) },
            #[cfg(target_arch = "aarch64")]
            (Kind::CrcPmull, start, data) => unsafe { aarch64::update_folded(start, data) },
        }
    }
}

#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
mod folding {
    pub(super) const POLYNOMIAL: u32 = 0x04C1_1DB7;
    const POLYNOMIAL_REFLECTED: u32 = POLYNOMIAL.reverse_bits();
    pub(super) const REMAINDER_BITS: u32 = u32::BITS;
    const BLOCK_BITS: u64 = 128;
    pub(super) const BLOCK_BYTES: usize = 16;
    pub(super) const WORD_BYTES: usize = 4;
    pub(super) const FOLD_KEYS_128_BITS: [u64; 2] = fold_keys(BLOCK_BITS);
    pub(super) const FOLD_KEYS_256_BITS: [u64; 2] = fold_keys(2 * BLOCK_BITS);
    pub(super) const FOLD_KEYS_512_BITS: [u64; 2] = fold_keys(4 * BLOCK_BITS);
    pub(super) const FOLD_KEYS_1024_BITS: [u64; 2] = fold_keys(8 * BLOCK_BITS);
    pub(super) const FOLD_KEYS_2048_BITS: [u64; 2] = fold_keys(16 * BLOCK_BITS);

    const fn power_reflected(exponent: u64) -> u32 {
        let mut value = 1u32 << (REMAINDER_BITS - 1);
        let mut step = 0;
        while step < exponent {
            value = if value & 1 == 1 {
                (value >> 1) ^ POLYNOMIAL_REFLECTED
            } else {
                value >> 1
            };
            step += 1;
        }
        value
    }

    pub(super) const fn power_key(exponent: u64) -> u64 {
        (power_reflected(exponent) as u64) << 1
    }

    const fn fold_keys(bits: u64) -> [u64; 2] {
        [
            power_key(bits + REMAINDER_BITS as u64),
            power_key(bits - REMAINDER_BITS as u64),
        ]
    }
}

#[cfg(target_arch = "x86_64")]
mod x86_64 {
    use super::folding::{
        BLOCK_BYTES, FOLD_KEYS_128_BITS, FOLD_KEYS_256_BITS, FOLD_KEYS_512_BITS,
        FOLD_KEYS_1024_BITS, FOLD_KEYS_2048_BITS, POLYNOMIAL, REMAINDER_BITS, WORD_BYTES,
        power_key,
    };
    use std::arch::x86_64::{
        __m128i, __m256i, __m512i, _mm_and_si128, _mm_clmulepi64_si128, _mm_cvtsi32_si128,
        _mm_extract_epi32, _mm_loadu_si128, _mm_set_epi32, _mm_set_epi64x, _mm_set1_epi8,
        _mm_setr_epi8, _mm_shuffle_epi8, _mm_srli_si128, _mm_xor_si128, _mm256_castsi256_si128,
        _mm256_clmulepi64_epi128, _mm256_extracti128_si256, _mm256_loadu_si256, _mm256_set_epi64x,
        _mm256_xor_si256, _mm256_zextsi128_si256, _mm512_castsi512_si128, _mm512_clmulepi64_epi128,
        _mm512_extracti32x4_epi32, _mm512_loadu_si512, _mm512_set_epi64, _mm512_xor_si512,
        _mm512_zextsi128_si512,
    };

    const LANES: usize = 4;
    const LANES_BYTES: usize = LANES * BLOCK_BYTES;
    const MEDIUM_LANE_BYTES: usize = 2 * BLOCK_BYTES;
    const MEDIUM_GROUP_BYTES: usize = LANES * MEDIUM_LANE_BYTES;
    const WIDE_LANE_BYTES: usize = LANES * BLOCK_BYTES;
    const WIDE_GROUP_BYTES: usize = LANES * WIDE_LANE_BYTES;
    const LANES_BYTES_MIN: usize = 128;
    const MEDIUM_BYTES_MIN: usize = 256;
    const WIDE_BYTES_MIN: usize = WIDE_GROUP_BYTES;
    const _: () = assert!(WIDE_BYTES_MIN <= MEDIUM_BYTES_MIN);
    const BYTE_SIGN: i8 = i8::MIN;
    const REDUCE_KEY_64_BITS: u64 = power_key(2 * REMAINDER_BITS as u64);
    const BARRETT_KEYS: [u64; 2] = [polynomial_reflected_key(), barrett_quotient_reflected()];

    const fn polynomial_reflected_key() -> u64 {
        (((1u64 << REMAINDER_BITS) | POLYNOMIAL as u64).reverse_bits())
            >> (u64::BITS - REMAINDER_BITS - 1)
    }

    const fn barrett_quotient_reflected() -> u64 {
        let divisor = (1u128 << REMAINDER_BITS) | POLYNOMIAL as u128;
        let mut remainder = 1u128 << (2 * REMAINDER_BITS);
        let mut quotient = 0u64;
        let mut degree = 2 * REMAINDER_BITS;
        while degree >= REMAINDER_BITS {
            if (remainder >> degree) & 1 == 1 {
                quotient |= 1 << (degree - REMAINDER_BITS);
                remainder ^= divisor << (degree - REMAINDER_BITS);
            }
            degree -= 1;
        }
        quotient.reverse_bits() >> (u64::BITS - REMAINDER_BITS - 1)
    }

    #[inline(always)]
    fn load(block: &[u8; BLOCK_BYTES]) -> __m128i {
        unsafe { _mm_loadu_si128(block.as_ptr().cast()) }
    }

    #[target_feature(enable = "avx")]
    #[inline]
    fn load_medium(blocks: &[u8; MEDIUM_LANE_BYTES]) -> __m256i {
        unsafe { _mm256_loadu_si256(blocks.as_ptr().cast()) }
    }

    #[target_feature(enable = "avx512f")]
    #[inline]
    fn load_wide(blocks: &[u8; WIDE_LANE_BYTES]) -> __m512i {
        unsafe { _mm512_loadu_si512(blocks.as_ptr().cast()) }
    }

    #[target_feature(enable = "sse2")]
    #[inline]
    fn keys(keys: [u64; 2]) -> __m128i {
        _mm_set_epi64x(keys[1] as i64, keys[0] as i64)
    }

    #[target_feature(enable = "avx")]
    #[inline]
    fn keys_medium(keys: [u64; 2]) -> __m256i {
        let [low, high] = keys.map(|key| key as i64);
        _mm256_set_epi64x(high, low, high, low)
    }

    #[target_feature(enable = "avx512f")]
    #[inline]
    fn keys_wide(keys: [u64; 2]) -> __m512i {
        let [low, high] = keys.map(|key| key as i64);
        _mm512_set_epi64(high, low, high, low, high, low, high, low)
    }

    #[target_feature(enable = "pclmulqdq")]
    #[inline]
    fn fold(accumulator: __m128i, next: __m128i, keys: __m128i) -> __m128i {
        let low = _mm_clmulepi64_si128::<0x00>(accumulator, keys);
        let high = _mm_clmulepi64_si128::<0x11>(accumulator, keys);
        _mm_xor_si128(_mm_xor_si128(next, low), high)
    }

    #[target_feature(enable = "avx2,vpclmulqdq")]
    #[inline]
    fn fold_medium(accumulator: __m256i, next: __m256i, keys: __m256i) -> __m256i {
        let low = _mm256_clmulepi64_epi128::<0x00>(accumulator, keys);
        let high = _mm256_clmulepi64_epi128::<0x11>(accumulator, keys);
        _mm256_xor_si256(_mm256_xor_si256(next, low), high)
    }

    #[target_feature(enable = "avx512f,vpclmulqdq")]
    #[inline]
    fn fold_wide(accumulator: __m512i, next: __m512i, keys: __m512i) -> __m512i {
        let low = _mm512_clmulepi64_epi128::<0x00>(accumulator, keys);
        let high = _mm512_clmulepi64_epi128::<0x11>(accumulator, keys);
        _mm512_xor_si512(_mm512_xor_si512(next, low), high)
    }

    #[target_feature(enable = "pclmulqdq,sse4.1")]
    fn reduce(accumulator: __m128i) -> u32 {
        let low_word = _mm_set_epi32(0, 0, 0, -1);
        let folded = _mm_xor_si128(
            _mm_clmulepi64_si128::<0x10>(accumulator, keys(FOLD_KEYS_128_BITS)),
            _mm_srli_si128::<8>(accumulator),
        );
        let folded = _mm_xor_si128(
            _mm_clmulepi64_si128::<0x00>(
                _mm_and_si128(folded, low_word),
                _mm_set_epi64x(0, REDUCE_KEY_64_BITS as i64),
            ),
            _mm_srli_si128::<4>(folded),
        );
        barrett(folded)
    }

    #[target_feature(enable = "pclmulqdq,sse4.1")]
    fn barrett(value: __m128i) -> u32 {
        let low_word = _mm_set_epi32(0, 0, 0, -1);
        let barrett = keys(BARRETT_KEYS);
        let quotient = _mm_clmulepi64_si128::<0x10>(_mm_and_si128(value, low_word), barrett);
        let product = _mm_clmulepi64_si128::<0x00>(_mm_and_si128(quotient, low_word), barrett);
        _mm_extract_epi32::<1>(_mm_xor_si128(value, product)) as u32
    }

    fn partial_words(data: &[u8]) -> (u64, u64) {
        let length = data.len();
        assert!(length > 0);
        assert!(length < BLOCK_BYTES);
        let value = if length >= 2 * WORD_BYTES {
            let low = u64::from_le_bytes(*data.first_chunk::<8>().expect("eight bytes"));
            let high = u64::from_le_bytes(*data.last_chunk::<8>().expect("eight bytes"));
            u128::from(low) | (u128::from(high) << (u8::BITS as usize * (length - 8)))
        } else if length >= WORD_BYTES {
            let low = u32::from_le_bytes(*data.first_chunk::<4>().expect("four bytes"));
            let high = u32::from_le_bytes(*data.last_chunk::<4>().expect("four bytes"));
            u128::from(low) | (u128::from(high) << (u8::BITS as usize * (length - 4)))
        } else {
            u128::from(data[0])
                | (u128::from(data[length / 2]) << (u8::BITS as usize * (length / 2)))
                | (u128::from(data[length - 1]) << (u8::BITS as usize * (length - 1)))
        };
        (value as u64, (value >> u64::BITS) as u64)
    }

    #[target_feature(enable = "ssse3")]
    fn shift_up(length: usize) -> __m128i {
        let sequence = _mm_setr_epi8(0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15);
        let offset = i8::try_from(length).expect("a partial block") - BLOCK_BYTES as i8;
        std::arch::x86_64::_mm_add_epi8(sequence, _mm_set1_epi8(offset))
    }

    #[target_feature(enable = "pclmulqdq,sse4.1,ssse3")]
    fn short(state: u32, data: &[u8]) -> u32 {
        assert!(data.len() < WORD_BYTES);
        if data.is_empty() {
            return state;
        }
        let length = data.len();
        let bits = u8::BITS * length as u32;
        let word = u32::from(data[0])
            | (u32::from(data[length / 2]) << (u8::BITS as usize * (length / 2)))
            | (u32::from(data[length - 1]) << (u8::BITS as usize * (length - 1)));
        let mixed = (state ^ word) & ((1 << bits) - 1);
        let shifted = mixed << (u32::BITS - bits);
        (state >> bits) ^ barrett(_mm_cvtsi32_si128(shifted as i32))
    }

    #[target_feature(enable = "pclmulqdq,sse4.1,ssse3")]
    fn tiny(state: u32, data: &[u8]) -> u32 {
        assert!(data.len() >= WORD_BYTES);
        assert!(data.len() < BLOCK_BYTES);
        let (low, high) = partial_words(data);
        let part = _mm_xor_si128(
            _mm_set_epi64x(high as i64, low as i64),
            _mm_cvtsi32_si128(state as i32),
        );
        reduce(_mm_shuffle_epi8(part, shift_up(data.len())))
    }

    #[target_feature(enable = "pclmulqdq,sse4.1,ssse3")]
    fn finish(mut accumulator: __m128i, data: &[u8]) -> u32 {
        let keys = keys(FOLD_KEYS_128_BITS);
        let (blocks, tail) = data.as_chunks::<BLOCK_BYTES>();
        for block in blocks {
            accumulator = fold(accumulator, load(block), keys);
        }
        if !tail.is_empty() {
            let up = shift_up(tail.len());
            let down = _mm_xor_si128(up, _mm_set1_epi8(BYTE_SIGN));
            let overflow = _mm_shuffle_epi8(accumulator, up);
            let (low, high) = partial_words(tail);
            let part = _mm_shuffle_epi8(_mm_set_epi64x(high as i64, low as i64), up);
            accumulator = fold(
                overflow,
                _mm_xor_si128(_mm_shuffle_epi8(accumulator, down), part),
                keys,
            );
        }
        reduce(accumulator)
    }

    #[target_feature(enable = "pclmulqdq,sse4.1,ssse3")]
    pub(super) fn update(start: u32, data: &[u8]) -> u32 {
        let state = !start;
        if data.len() < WORD_BYTES {
            return !short(state, data);
        }
        if data.len() < BLOCK_BYTES {
            return !tiny(state, data);
        }
        if data.len() < LANES_BYTES_MIN {
            let (first, rest) = data.split_first_chunk::<BLOCK_BYTES>().expect("one block");
            let accumulator = _mm_xor_si128(load(first), _mm_cvtsi32_si128(state as i32));
            return !finish(accumulator, rest);
        }
        let (groups, rest) = data.as_chunks::<LANES_BYTES>();
        let first = groups[0].as_chunks::<BLOCK_BYTES>().0;
        let mut lanes = [
            load(&first[0]),
            load(&first[1]),
            load(&first[2]),
            load(&first[3]),
        ];
        lanes[0] = _mm_xor_si128(lanes[0], _mm_cvtsi32_si128(state as i32));
        let lane_keys = keys(FOLD_KEYS_512_BITS);
        for group in &groups[1..] {
            let blocks = group.as_chunks::<BLOCK_BYTES>().0;
            for (lane, block) in lanes.iter_mut().zip(blocks) {
                *lane = fold(*lane, load(block), lane_keys);
            }
        }
        let block_keys = keys(FOLD_KEYS_128_BITS);
        let accumulator = fold(
            fold(fold(lanes[0], lanes[1], block_keys), lanes[2], block_keys),
            lanes[3],
            block_keys,
        );
        !finish(accumulator, rest)
    }

    #[target_feature(enable = "pclmulqdq,sse4.1,ssse3,avx2,vpclmulqdq")]
    pub(super) fn update_avx2(start: u32, data: &[u8]) -> u32 {
        if data.len() < MEDIUM_BYTES_MIN {
            return update(start, data);
        }
        let state = !start;
        let (groups, rest) = data.as_chunks::<MEDIUM_GROUP_BYTES>();
        let first = groups[0].as_chunks::<MEDIUM_LANE_BYTES>().0;
        let mut lanes = [
            load_medium(&first[0]),
            load_medium(&first[1]),
            load_medium(&first[2]),
            load_medium(&first[3]),
        ];
        lanes[0] = _mm256_xor_si256(
            lanes[0],
            _mm256_zextsi128_si256(_mm_cvtsi32_si128(state as i32)),
        );
        let lane_keys = keys_medium(FOLD_KEYS_1024_BITS);
        for group in &groups[1..] {
            let blocks = group.as_chunks::<MEDIUM_LANE_BYTES>().0;
            for (lane, block) in lanes.iter_mut().zip(blocks) {
                *lane = fold_medium(*lane, load_medium(block), lane_keys);
            }
        }
        let pair_keys = keys_medium(FOLD_KEYS_256_BITS);
        let mut medium = fold_medium(
            fold_medium(
                fold_medium(lanes[0], lanes[1], pair_keys),
                lanes[2],
                pair_keys,
            ),
            lanes[3],
            pair_keys,
        );
        let (pairs, rest) = rest.as_chunks::<MEDIUM_LANE_BYTES>();
        for pair in pairs {
            medium = fold_medium(medium, load_medium(pair), pair_keys);
        }
        let accumulator = fold(
            _mm256_castsi256_si128(medium),
            _mm256_extracti128_si256::<1>(medium),
            keys(FOLD_KEYS_128_BITS),
        );
        !finish(accumulator, rest)
    }

    #[target_feature(enable = "pclmulqdq,sse4.1,ssse3,avx2,avx512f,vpclmulqdq")]
    pub(super) fn update_avx512(start: u32, data: &[u8]) -> u32 {
        if data.len() < WIDE_BYTES_MIN {
            return update(start, data);
        }
        let state = !start;
        let (groups, rest) = data.as_chunks::<WIDE_GROUP_BYTES>();
        let first = groups[0].as_chunks::<WIDE_LANE_BYTES>().0;
        let mut lanes = [
            load_wide(&first[0]),
            load_wide(&first[1]),
            load_wide(&first[2]),
            load_wide(&first[3]),
        ];
        lanes[0] = _mm512_xor_si512(
            lanes[0],
            _mm512_zextsi128_si512(_mm_cvtsi32_si128(state as i32)),
        );
        let lane_keys = keys_wide(FOLD_KEYS_2048_BITS);
        for group in &groups[1..] {
            let blocks = group.as_chunks::<WIDE_LANE_BYTES>().0;
            for (lane, block) in lanes.iter_mut().zip(blocks) {
                *lane = fold_wide(*lane, load_wide(block), lane_keys);
            }
        }
        let group_keys = keys_wide(FOLD_KEYS_512_BITS);
        let mut wide = fold_wide(
            fold_wide(
                fold_wide(lanes[0], lanes[1], group_keys),
                lanes[2],
                group_keys,
            ),
            lanes[3],
            group_keys,
        );
        let (wide_blocks, rest) = rest.as_chunks::<WIDE_LANE_BYTES>();
        for block in wide_blocks {
            wide = fold_wide(wide, load_wide(block), group_keys);
        }
        let block_keys = keys(FOLD_KEYS_128_BITS);
        let mut accumulator = _mm512_castsi512_si128(wide);
        accumulator = fold(
            accumulator,
            _mm512_extracti32x4_epi32::<1>(wide),
            block_keys,
        );
        accumulator = fold(
            accumulator,
            _mm512_extracti32x4_epi32::<2>(wide),
            block_keys,
        );
        accumulator = fold(
            accumulator,
            _mm512_extracti32x4_epi32::<3>(wide),
            block_keys,
        );
        !finish(accumulator, rest)
    }
}

#[cfg(target_arch = "aarch64")]
mod aarch64 {
    use super::folding::{
        BLOCK_BYTES, FOLD_KEYS_128_BITS, FOLD_KEYS_256_BITS, FOLD_KEYS_512_BITS,
        FOLD_KEYS_1024_BITS, FOLD_KEYS_2048_BITS, WORD_BYTES,
    };
    use std::arch::aarch64::{
        __crc32b, __crc32d, __crc32h, __crc32w, uint64x2_t, vcombine_u64, vcreate_u64, vdupq_n_u64,
        veorq_u64, vgetq_lane_u64, vld1q_u8, vmull_high_p64, vmull_p64, vreinterpretq_p64_u64,
        vreinterpretq_u64_p128, vreinterpretq_u64_u8, vsetq_lane_u64,
    };

    const DOUBLE_WORD_BYTES: usize = 2 * WORD_BYTES;
    const HALF_WORD_BYTES: usize = WORD_BYTES / 2;
    const FOLD_BYTES_MIN: usize = 64;
    const STREAMS_8_BYTES_MIN: usize = 256;
    const STREAMS_16_BYTES_MIN: usize = 4096;

    #[target_feature(enable = "neon")]
    #[inline]
    fn load(block: &[u8; BLOCK_BYTES]) -> uint64x2_t {
        unsafe { vreinterpretq_u64_u8(vld1q_u8(block.as_ptr())) }
    }

    #[target_feature(enable = "neon")]
    #[inline]
    fn keys(keys: [u64; 2]) -> uint64x2_t {
        vcombine_u64(vcreate_u64(keys[0]), vcreate_u64(keys[1]))
    }

    #[target_feature(enable = "aes")]
    #[inline]
    fn fold(accumulator: uint64x2_t, next: uint64x2_t, keys: uint64x2_t) -> uint64x2_t {
        let low = vmull_p64(vgetq_lane_u64::<0>(accumulator), vgetq_lane_u64::<0>(keys));
        let high = vmull_high_p64(
            vreinterpretq_p64_u64(accumulator),
            vreinterpretq_p64_u64(keys),
        );
        veorq_u64(
            next,
            veorq_u64(vreinterpretq_u64_p128(low), vreinterpretq_u64_p128(high)),
        )
    }

    #[target_feature(enable = "crc")]
    #[inline]
    fn tail(mut state: u32, data: &[u8]) -> u32 {
        assert!(data.len() < BLOCK_BYTES);
        if data.is_empty() {
            return state;
        }
        let mut rest = data;
        if let Some((word, after)) = rest.split_first_chunk::<DOUBLE_WORD_BYTES>() {
            state = __crc32d(state, u64::from_le_bytes(*word));
            rest = after;
        }
        if let Some((word, after)) = rest.split_first_chunk::<WORD_BYTES>() {
            state = __crc32w(state, u32::from_le_bytes(*word));
            rest = after;
        }
        if let Some((word, after)) = rest.split_first_chunk::<HALF_WORD_BYTES>() {
            state = __crc32h(state, u16::from_le_bytes(*word));
            rest = after;
        }
        if let Some(byte) = rest.first() {
            state = __crc32b(state, *byte);
        }
        state
    }

    #[target_feature(enable = "crc")]
    fn words(mut state: u32, data: &[u8]) -> u32 {
        let (blocks, rest) = data.as_chunks::<BLOCK_BYTES>();
        for block in blocks {
            let (low, high) = block.split_at(DOUBLE_WORD_BYTES);
            state = __crc32d(
                state,
                u64::from_le_bytes(low.try_into().expect("eight bytes")),
            );
            state = __crc32d(
                state,
                u64::from_le_bytes(high.try_into().expect("eight bytes")),
            );
        }
        tail(state, rest)
    }

    #[target_feature(enable = "crc")]
    pub(super) fn update(start: u32, data: &[u8]) -> u32 {
        !words(!start, data)
    }

    #[target_feature(enable = "aes")]
    #[inline]
    fn halve<const STREAMS: usize>(
        streams: &mut [uint64x2_t; STREAMS],
        half: usize,
        remaining: &mut &[[u8; BLOCK_BYTES]],
        keys_half: [u64; 2],
    ) {
        if STREAMS < 2 * half {
            return;
        }
        let keys = keys(keys_half);
        for index in 0..half {
            streams[index] = fold(streams[index], streams[index + half], keys);
        }
        if remaining.len() >= half {
            let (blocks, after) = remaining.split_at(half);
            for (stream, block) in streams.iter_mut().zip(blocks) {
                *stream = fold(*stream, load(block), keys);
            }
            *remaining = after;
        }
    }

    #[target_feature(enable = "crc,aes")]
    #[inline]
    fn fold_streams<const STREAMS: usize>(state: u32, data: &[u8], stream_keys: [u64; 2]) -> u32 {
        let (blocks, rest) = data.as_chunks::<BLOCK_BYTES>();
        assert!(blocks.len() >= STREAMS);
        let (first, after) = blocks.split_at(STREAMS);
        let mut streams = [vdupq_n_u64(0); STREAMS];
        for (stream, block) in streams.iter_mut().zip(first) {
            *stream = load(block);
        }
        streams[0] = veorq_u64(
            streams[0],
            vsetq_lane_u64::<0>(u64::from(state), vdupq_n_u64(0)),
        );
        let (groups, mut remaining) = after.as_chunks::<STREAMS>();
        let keys = keys(stream_keys);
        for group in groups {
            for (stream, block) in streams.iter_mut().zip(group) {
                *stream = fold(*stream, load(block), keys);
            }
        }
        halve(&mut streams, 8, &mut remaining, FOLD_KEYS_1024_BITS);
        halve(&mut streams, 4, &mut remaining, FOLD_KEYS_512_BITS);
        halve(&mut streams, 2, &mut remaining, FOLD_KEYS_256_BITS);
        halve(&mut streams, 1, &mut remaining, FOLD_KEYS_128_BITS);
        assert!(remaining.is_empty());
        let folded = __crc32d(0, vgetq_lane_u64::<0>(streams[0]));
        let folded = __crc32d(folded, vgetq_lane_u64::<1>(streams[0]));
        tail(folded, rest)
    }

    #[target_feature(enable = "crc,aes")]
    pub(super) fn update_folded(start: u32, data: &[u8]) -> u32 {
        let state = !start;
        let state = if data.len() < FOLD_BYTES_MIN {
            words(state, data)
        } else if data.len() < STREAMS_8_BYTES_MIN {
            fold_streams::<4>(state, data, FOLD_KEYS_512_BITS)
        } else if data.len() < STREAMS_16_BYTES_MIN {
            fold_streams::<8>(state, data, FOLD_KEYS_1024_BITS)
        } else {
            fold_streams::<16>(state, data, FOLD_KEYS_2048_BITS)
        };
        !state
    }
}
