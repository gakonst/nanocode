pub const BLOCK_BYTES: usize = 64;
pub const STATE_WORDS: usize = 8;
pub const ROUNDS: usize = 64;

pub const ROUND_CONSTANTS: [u32; ROUNDS] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    ShaExtensions,
    #[cfg(target_arch = "aarch64")]
    ArmSha2,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sha256Instructions(Kind);

#[cfg_attr(
    not(any(target_arch = "x86", target_arch = "x86_64", target_arch = "aarch64")),
    expect(clippy::missing_const_for_fn)
)]
impl Sha256Instructions {
    pub fn detect() -> Option<Self> {
        #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
        if std::is_x86_feature_detected!("sse2")
            && std::is_x86_feature_detected!("sha")
            && std::is_x86_feature_detected!("ssse3")
            && std::is_x86_feature_detected!("sse4.1")
        {
            return Some(Self(Kind::ShaExtensions));
        }
        #[cfg(target_arch = "aarch64")]
        if std::arch::is_aarch64_feature_detected!("sha2") {
            return Some(Self(Kind::ArmSha2));
        }
        None
    }

    pub fn compress(self, state: &mut [u32; STATE_WORDS], blocks: &[[u8; BLOCK_BYTES]]) {
        match (self.0, state, blocks) {
            #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
            (Kind::ShaExtensions, state, blocks) => unsafe { x86::compress(state, blocks) },
            #[cfg(target_arch = "aarch64")]
            (Kind::ArmSha2, state, blocks) => unsafe { arm::compress(state, blocks) },
        }
    }
}

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
mod x86 {
    #[cfg(target_arch = "x86")]
    use core::arch::x86::*;
    #[cfg(target_arch = "x86_64")]
    use core::arch::x86_64::*;

    use super::{BLOCK_BYTES, ROUND_CONSTANTS, STATE_WORDS};

    const BYTE_SWAP_WORDS: [u8; 16] = [3, 2, 1, 0, 7, 6, 5, 4, 11, 10, 9, 8, 15, 14, 13, 12];

    #[target_feature(enable = "sse2")]
    fn load(bytes: &[u8; 16]) -> __m128i {
        unsafe { _mm_loadu_si128(bytes.as_ptr().cast()) }
    }

    #[target_feature(enable = "sse2")]
    fn constants(group: usize) -> __m128i {
        let words = ROUND_CONSTANTS.as_chunks::<4>().0[group];
        _mm_set_epi32(
            words[3] as i32,
            words[2] as i32,
            words[1] as i32,
            words[0] as i32,
        )
    }

    #[target_feature(enable = "sha,sse2,ssse3,sse4.1")]
    fn scheduled(words: &mut [__m128i; 4], group: usize) -> __m128i {
        let partial = _mm_sha256msg1_epu32(words[group % 4], words[(group + 1) % 4]);
        let shifted = _mm_alignr_epi8(words[(group + 3) % 4], words[(group + 2) % 4], 4);
        let next = _mm_sha256msg2_epu32(_mm_add_epi32(partial, shifted), words[(group + 3) % 4]);
        words[group % 4] = next;
        next
    }

    #[target_feature(enable = "sha,sse2,ssse3,sse4.1")]
    fn rounds(abef: &mut __m128i, cdgh: &mut __m128i, words: __m128i, group: usize) {
        let sum = _mm_add_epi32(words, constants(group));
        *cdgh = _mm_sha256rnds2_epu32(*cdgh, *abef, sum);
        *abef = _mm_sha256rnds2_epu32(*abef, *cdgh, _mm_shuffle_epi32(sum, 0x0e));
    }

    #[target_feature(enable = "sha,sse2,ssse3,sse4.1")]
    pub(super) fn compress(state: &mut [u32; STATE_WORDS], blocks: &[[u8; BLOCK_BYTES]]) {
        let byte_swap = load(&BYTE_SWAP_WORDS);
        let dcba = _mm_set_epi32(
            state[3] as i32,
            state[2] as i32,
            state[1] as i32,
            state[0] as i32,
        );
        let hgfe = _mm_set_epi32(
            state[7] as i32,
            state[6] as i32,
            state[5] as i32,
            state[4] as i32,
        );
        let cdab = _mm_shuffle_epi32(dcba, 0xb1);
        let efgh = _mm_shuffle_epi32(hgfe, 0x1b);
        let mut abef = _mm_alignr_epi8(cdab, efgh, 8);
        let mut cdgh = _mm_blend_epi16(efgh, cdab, 0xf0);
        for block in blocks {
            let abef_saved = abef;
            let cdgh_saved = cdgh;
            let quarters = block.as_chunks::<16>().0;
            let mut words = [
                _mm_shuffle_epi8(load(&quarters[0]), byte_swap),
                _mm_shuffle_epi8(load(&quarters[1]), byte_swap),
                _mm_shuffle_epi8(load(&quarters[2]), byte_swap),
                _mm_shuffle_epi8(load(&quarters[3]), byte_swap),
            ];
            for (group, message) in words.into_iter().enumerate() {
                rounds(&mut abef, &mut cdgh, message, group);
            }
            for group in 4..8 {
                rounds(&mut abef, &mut cdgh, scheduled(&mut words, group), group);
            }
            for group in 8..12 {
                rounds(&mut abef, &mut cdgh, scheduled(&mut words, group), group);
            }
            for group in 12..16 {
                rounds(&mut abef, &mut cdgh, scheduled(&mut words, group), group);
            }
            abef = _mm_add_epi32(abef, abef_saved);
            cdgh = _mm_add_epi32(cdgh, cdgh_saved);
        }
        let feba = _mm_shuffle_epi32(abef, 0x1b);
        let dchg = _mm_shuffle_epi32(cdgh, 0xb1);
        let dcba = _mm_blend_epi16(feba, dchg, 0xf0);
        let hgef = _mm_alignr_epi8(dchg, feba, 8);
        *state = [
            _mm_extract_epi32(dcba, 0) as u32,
            _mm_extract_epi32(dcba, 1) as u32,
            _mm_extract_epi32(dcba, 2) as u32,
            _mm_extract_epi32(dcba, 3) as u32,
            _mm_extract_epi32(hgef, 0) as u32,
            _mm_extract_epi32(hgef, 1) as u32,
            _mm_extract_epi32(hgef, 2) as u32,
            _mm_extract_epi32(hgef, 3) as u32,
        ];
    }
}

#[cfg(target_arch = "aarch64")]
mod arm {
    use core::arch::aarch64::*;

    use super::{BLOCK_BYTES, ROUND_CONSTANTS, STATE_WORDS};

    #[target_feature(enable = "neon")]
    fn load_bytes(bytes: &[u8; 16]) -> uint32x4_t {
        vreinterpretq_u32_u8(vrev32q_u8(unsafe { vld1q_u8(bytes.as_ptr()) }))
    }

    #[target_feature(enable = "neon")]
    fn load_words(words: &[u32; 4]) -> uint32x4_t {
        unsafe { vld1q_u32(words.as_ptr()) }
    }

    #[target_feature(enable = "sha2")]
    fn scheduled(words: &mut [uint32x4_t; 4], group: usize) -> uint32x4_t {
        let next = vsha256su1q_u32(
            vsha256su0q_u32(words[group % 4], words[(group + 1) % 4]),
            words[(group + 2) % 4],
            words[(group + 3) % 4],
        );
        words[group % 4] = next;
        next
    }

    #[target_feature(enable = "sha2")]
    fn rounds(abcd: &mut uint32x4_t, efgh: &mut uint32x4_t, words: uint32x4_t, group: usize) {
        let sum = vaddq_u32(
            words,
            load_words(&ROUND_CONSTANTS.as_chunks::<4>().0[group]),
        );
        let abcd_before = *abcd;
        *abcd = vsha256hq_u32(abcd_before, *efgh, sum);
        *efgh = vsha256h2q_u32(*efgh, abcd_before, sum);
    }

    #[target_feature(enable = "sha2")]
    pub(super) fn compress(state: &mut [u32; STATE_WORDS], blocks: &[[u8; BLOCK_BYTES]]) {
        let halves = state.as_chunks::<4>().0;
        let mut abcd = load_words(&halves[0]);
        let mut efgh = load_words(&halves[1]);
        for block in blocks {
            let abcd_saved = abcd;
            let efgh_saved = efgh;
            let quarters = block.as_chunks::<16>().0;
            let mut words = [
                load_bytes(&quarters[0]),
                load_bytes(&quarters[1]),
                load_bytes(&quarters[2]),
                load_bytes(&quarters[3]),
            ];
            for (group, message) in words.into_iter().enumerate() {
                rounds(&mut abcd, &mut efgh, message, group);
            }
            for group in 4..8 {
                rounds(&mut abcd, &mut efgh, scheduled(&mut words, group), group);
            }
            for group in 8..12 {
                rounds(&mut abcd, &mut efgh, scheduled(&mut words, group), group);
            }
            for group in 12..16 {
                rounds(&mut abcd, &mut efgh, scheduled(&mut words, group), group);
            }
            abcd = vaddq_u32(abcd, abcd_saved);
            efgh = vaddq_u32(efgh, efgh_saved);
        }
        *state = [
            vgetq_lane_u32(abcd, 0),
            vgetq_lane_u32(abcd, 1),
            vgetq_lane_u32(abcd, 2),
            vgetq_lane_u32(abcd, 3),
            vgetq_lane_u32(efgh, 0),
            vgetq_lane_u32(efgh, 1),
            vgetq_lane_u32(efgh, 2),
            vgetq_lane_u32(efgh, 3),
        ];
    }
}
