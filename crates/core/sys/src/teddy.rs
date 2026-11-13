pub const TEDDY_BUCKETS: usize = 8;
pub const TEDDY_PREFIX_BYTES_MAX: usize = 3;

const NIBBLES: usize = 16;
const NIBBLE_MASK: u8 = 0x0F;
const NIBBLE_BITS: u32 = 4;
#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
const NIBBLE_SHIFT: i32 = 4;

type Masks = [[u8; NIBBLES]; TEDDY_PREFIX_BYTES_MAX];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Kernel {
    #[cfg(target_arch = "x86_64")]
    Avx2,
    #[cfg(target_arch = "x86_64")]
    Ssse3,
    #[cfg(target_arch = "aarch64")]
    Neon,
}

impl Kernel {
    #[cfg_attr(
        not(any(target_arch = "x86_64", target_arch = "aarch64")),
        expect(clippy::missing_const_for_fn)
    )]
    fn detect() -> Option<Self> {
        #[cfg(target_arch = "x86_64")]
        {
            if std::is_x86_feature_detected!("avx2") {
                return Some(Self::Avx2);
            }
            if std::is_x86_feature_detected!("ssse3") {
                return Some(Self::Ssse3);
            }
        }
        #[cfg(target_arch = "aarch64")]
        {
            if std::arch::is_aarch64_feature_detected!("neon") {
                return Some(Self::Neon);
            }
        }
        None
    }
}

#[derive(Clone, Debug)]
#[cfg_attr(
    not(any(target_arch = "x86_64", target_arch = "aarch64")),
    expect(dead_code)
)]
pub struct Teddy {
    low: Masks,
    high: Masks,
    prefix_len: usize,
    kernel: Kernel,
}

impl Teddy {
    pub fn new(prefixes: &[(usize, &[u8])], prefix_len: usize) -> Option<Self> {
        assert!(
            (1..=TEDDY_PREFIX_BYTES_MAX).contains(&prefix_len),
            "a Teddy prefix holds one to three bytes"
        );
        let kernel = Kernel::detect()?;
        let mut low = [[0; NIBBLES]; TEDDY_PREFIX_BYTES_MAX];
        let mut high = [[0; NIBBLES]; TEDDY_PREFIX_BYTES_MAX];
        for &(bucket, prefix) in prefixes {
            assert!(bucket < TEDDY_BUCKETS, "a Teddy bucket index is below eight");
            assert_eq!(prefix.len(), prefix_len, "every Teddy prefix has the prefix length");
            for (offset, &byte) in prefix.iter().enumerate() {
                low[offset][usize::from(byte & NIBBLE_MASK)] |= 1 << bucket;
                high[offset][usize::from(byte >> NIBBLE_BITS)] |= 1 << bucket;
            }
        }
        Some(Self {
            low,
            high,
            prefix_len,
            kernel,
        })
    }

    pub const fn prefix_len(&self) -> usize {
        self.prefix_len
    }

    #[cfg_attr(
        not(any(target_arch = "x86_64", target_arch = "aarch64")),
        expect(clippy::missing_const_for_fn)
    )]
    pub fn find(&self, haystack: &[u8], start: usize) -> Option<(usize, u8)> {
        if start + self.prefix_len > haystack.len() {
            return None;
        }
        match self.kernel {
            #[cfg(target_arch = "x86_64")]
            Kernel::Avx2 => match self.prefix_len {
                1 => unsafe { x86::find_avx2::<1>(self, haystack, start) },
                2 => unsafe { x86::find_avx2::<2>(self, haystack, start) },
                _ => unsafe { x86::find_avx2::<3>(self, haystack, start) },
            },
            #[cfg(target_arch = "x86_64")]
            Kernel::Ssse3 => match self.prefix_len {
                1 => unsafe { x86::find_ssse3::<1>(self, haystack, start) },
                2 => unsafe { x86::find_ssse3::<2>(self, haystack, start) },
                _ => unsafe { x86::find_ssse3::<3>(self, haystack, start) },
            },
            #[cfg(target_arch = "aarch64")]
            Kernel::Neon => match self.prefix_len {
                1 => unsafe { arm::find_neon::<1>(self, haystack, start) },
                2 => unsafe { arm::find_neon::<2>(self, haystack, start) },
                _ => unsafe { arm::find_neon::<3>(self, haystack, start) },
            },
        }
    }

    #[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
    fn buckets_at<const N: usize>(&self, haystack: &[u8], at: usize) -> u8 {
        let mut buckets = u8::MAX;
        for offset in 0..N {
            let byte = haystack[at + offset];
            buckets &= self.low[offset][usize::from(byte & NIBBLE_MASK)]
                & self.high[offset][usize::from(byte >> NIBBLE_BITS)];
        }
        buckets
    }

    #[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
    fn find_tail<const N: usize>(&self, haystack: &[u8], start: usize) -> Option<(usize, u8)> {
        (start..=haystack.len() - N).find_map(|at| {
            let buckets = self.buckets_at::<N>(haystack, at);
            (buckets != 0).then_some((at, buckets))
        })
    }
}

#[cfg(target_arch = "x86_64")]
mod x86 {
    use core::arch::x86_64::{
        __m128i, __m256i, _mm_and_si128, _mm_cmpeq_epi8, _mm_loadu_si128, _mm_movemask_epi8,
        _mm_set1_epi8, _mm_setzero_si128, _mm_shuffle_epi8, _mm_srli_epi16, _mm_storeu_si128,
        _mm256_and_si256, _mm256_broadcastsi128_si256, _mm256_cmpeq_epi8, _mm256_loadu_si256,
        _mm256_movemask_epi8, _mm256_set1_epi8, _mm256_setzero_si256, _mm256_shuffle_epi8,
        _mm256_srli_epi16, _mm256_storeu_si256,
    };

    use super::{NIBBLE_MASK, NIBBLE_SHIFT, NIBBLES, Teddy};

    const SSE_LANES: usize = 16;
    const AVX_LANES: usize = 32;

    #[inline(always)]
    fn load128(bytes: &[u8; SSE_LANES]) -> __m128i {
        unsafe { _mm_loadu_si128(bytes.as_ptr().cast()) }
    }

    #[inline(always)]
    fn store128(value: __m128i) -> [u8; SSE_LANES] {
        let mut bytes = [0; SSE_LANES];
        unsafe { _mm_storeu_si128(bytes.as_mut_ptr().cast(), value) };
        bytes
    }

    #[target_feature(enable = "avx2")]
    fn load256(bytes: &[u8; AVX_LANES]) -> __m256i {
        unsafe { _mm256_loadu_si256(bytes.as_ptr().cast()) }
    }

    #[target_feature(enable = "avx2")]
    fn store256(value: __m256i) -> [u8; AVX_LANES] {
        let mut bytes = [0; AVX_LANES];
        unsafe { _mm256_storeu_si256(bytes.as_mut_ptr().cast(), value) };
        bytes
    }

    #[target_feature(enable = "ssse3")]
    pub(super) fn find_ssse3<const N: usize>(
        teddy: &Teddy,
        haystack: &[u8],
        start: usize,
    ) -> Option<(usize, u8)> {
        let low: [__m128i; N] = core::array::from_fn(|offset| load128(&teddy.low[offset]));
        let high: [__m128i; N] = core::array::from_fn(|offset| load128(&teddy.high[offset]));
        let nibble = _mm_set1_epi8(NIBBLE_MASK as i8);
        let mut at = start;
        while at + SSE_LANES + N - 1 <= haystack.len() {
            let mut hits = _mm_set1_epi8(-1);
            for offset in 0..N {
                let chunk = load128(
                    haystack[at + offset..at + offset + SSE_LANES]
                        .try_into()
                        .expect("the window holds a block past every offset"),
                );
                let low_nibbles = _mm_and_si128(chunk, nibble);
                let high_nibbles = _mm_and_si128(_mm_srli_epi16::<NIBBLE_SHIFT>(chunk), nibble);
                hits = _mm_and_si128(
                    hits,
                    _mm_and_si128(
                        _mm_shuffle_epi8(low[offset], low_nibbles),
                        _mm_shuffle_epi8(high[offset], high_nibbles),
                    ),
                );
            }
            let empty = _mm_movemask_epi8(_mm_cmpeq_epi8(hits, _mm_setzero_si128())) as u32;
            let lanes = !empty & ((1 << SSE_LANES) - 1);
            if lanes != 0 {
                let lane = lanes.trailing_zeros() as usize;
                return Some((at + lane, store128(hits)[lane]));
            }
            at += SSE_LANES;
        }
        teddy.find_tail::<N>(haystack, at)
    }

    #[target_feature(enable = "avx2")]
    pub(super) fn find_avx2<const N: usize>(
        teddy: &Teddy,
        haystack: &[u8],
        start: usize,
    ) -> Option<(usize, u8)> {
        let table = |masks: &[u8; NIBBLES]| _mm256_broadcastsi128_si256(load128(masks));
        let low: [__m256i; N] = core::array::from_fn(|offset| table(&teddy.low[offset]));
        let high: [__m256i; N] = core::array::from_fn(|offset| table(&teddy.high[offset]));
        let nibble = _mm256_set1_epi8(NIBBLE_MASK as i8);
        let mut at = start;
        while at + AVX_LANES + N - 1 <= haystack.len() {
            let mut hits = _mm256_set1_epi8(-1);
            for offset in 0..N {
                let chunk = load256(
                    haystack[at + offset..at + offset + AVX_LANES]
                        .try_into()
                        .expect("the window holds a block past every offset"),
                );
                let low_nibbles = _mm256_and_si256(chunk, nibble);
                let high_nibbles = _mm256_and_si256(_mm256_srli_epi16::<NIBBLE_SHIFT>(chunk), nibble);
                hits = _mm256_and_si256(
                    hits,
                    _mm256_and_si256(
                        _mm256_shuffle_epi8(low[offset], low_nibbles),
                        _mm256_shuffle_epi8(high[offset], high_nibbles),
                    ),
                );
            }
            let empty = _mm256_movemask_epi8(_mm256_cmpeq_epi8(hits, _mm256_setzero_si256())) as u32;
            let lanes = !empty;
            if lanes != 0 {
                let lane = lanes.trailing_zeros() as usize;
                return Some((at + lane, store256(hits)[lane]));
            }
            at += AVX_LANES;
        }
        teddy.find_tail::<N>(haystack, at)
    }
}

#[cfg(target_arch = "aarch64")]
mod arm {
    use core::arch::aarch64::{
        uint8x16_t, vandq_u8, vdupq_n_u8, vget_lane_u64, vld1q_u8, vmaxvq_u8, vqtbl1q_u8,
        vreinterpret_u64_u8, vreinterpretq_u16_u8, vshrn_n_u16, vshrq_n_u8, vst1q_u8, vtstq_u8,
    };

    use super::{NIBBLE_MASK, NIBBLE_SHIFT, Teddy};

    const LANES: usize = 16;
    const MASK_BITS_PER_LANE: u32 = 4;

    #[target_feature(enable = "neon")]
    fn load(bytes: &[u8; LANES]) -> uint8x16_t {
        unsafe { vld1q_u8(bytes.as_ptr()) }
    }

    #[target_feature(enable = "neon")]
    fn store(value: uint8x16_t) -> [u8; LANES] {
        let mut bytes = [0; LANES];
        unsafe { vst1q_u8(bytes.as_mut_ptr(), value) };
        bytes
    }

    #[target_feature(enable = "neon")]
    pub(super) fn find_neon<const N: usize>(
        teddy: &Teddy,
        haystack: &[u8],
        start: usize,
    ) -> Option<(usize, u8)> {
        let low: [uint8x16_t; N] = core::array::from_fn(|offset| load(&teddy.low[offset]));
        let high: [uint8x16_t; N] = core::array::from_fn(|offset| load(&teddy.high[offset]));
        let nibble = vdupq_n_u8(NIBBLE_MASK);
        let mut at = start;
        while at + LANES + N - 1 <= haystack.len() {
            let mut hits = vdupq_n_u8(u8::MAX);
            for offset in 0..N {
                let chunk = load(
                    haystack[at + offset..at + offset + LANES]
                        .try_into()
                        .expect("the window holds a block past every offset"),
                );
                hits = vandq_u8(
                    hits,
                    vandq_u8(
                        vqtbl1q_u8(low[offset], vandq_u8(chunk, nibble)),
                        vqtbl1q_u8(high[offset], vshrq_n_u8::<NIBBLE_SHIFT>(chunk)),
                    ),
                );
            }
            if vmaxvq_u8(hits) != 0 {
                let nonzero = vtstq_u8(hits, hits);
                let packed = vshrn_n_u16::<NIBBLE_SHIFT>(vreinterpretq_u16_u8(nonzero));
                let lanes = vget_lane_u64::<0>(vreinterpret_u64_u8(packed));
                let lane = (lanes.trailing_zeros() / MASK_BITS_PER_LANE) as usize;
                return Some((at + lane, store(hits)[lane]));
            }
            at += LANES;
        }
        teddy.find_tail::<N>(haystack, at)
    }
}
