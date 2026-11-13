pub const BLOCK_BYTES: usize = 64;
pub const STATE_WORDS: usize = 5;

pub const ROUND_CONSTANTS: [u32; 4] = [0x5a827999, 0x6ed9eba1, 0x8f1bbcdc, 0xca62c1d6];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    ShaExtensions,
    #[cfg(target_arch = "aarch64")]
    ArmSha1,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sha1Instructions(Kind);

#[cfg_attr(
    not(any(target_arch = "x86", target_arch = "x86_64", target_arch = "aarch64")),
    expect(clippy::missing_const_for_fn)
)]
impl Sha1Instructions {
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
            return Some(Self(Kind::ArmSha1));
        }
        None
    }

    pub fn compress(self, state: &mut [u32; STATE_WORDS], blocks: &[[u8; BLOCK_BYTES]]) {
        match (self.0, state, blocks) {
            #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
            (Kind::ShaExtensions, state, blocks) => unsafe { x86::compress(state, blocks) },
            #[cfg(target_arch = "aarch64")]
            (Kind::ArmSha1, state, blocks) => unsafe { arm::compress(state, blocks) },
        }
    }
}

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
mod x86 {
    #[cfg(target_arch = "x86")]
    use core::arch::x86::*;
    #[cfg(target_arch = "x86_64")]
    use core::arch::x86_64::*;

    use super::{BLOCK_BYTES, STATE_WORDS};

    const BYTE_REVERSE: [u8; 16] = [15, 14, 13, 12, 11, 10, 9, 8, 7, 6, 5, 4, 3, 2, 1, 0];

    #[target_feature(enable = "sse2")]
    fn load(bytes: &[u8; 16]) -> __m128i {
        unsafe { _mm_loadu_si128(bytes.as_ptr().cast()) }
    }

    #[target_feature(enable = "sha,sse2,ssse3,sse4.1")]
    fn scheduled(words: &mut [__m128i; 4], group: usize) -> __m128i {
        let partial = _mm_sha1msg1_epu32(words[group % 4], words[(group + 1) % 4]);
        let mixed = _mm_xor_si128(partial, words[(group + 2) % 4]);
        let next = _mm_sha1msg2_epu32(mixed, words[(group + 3) % 4]);
        words[group % 4] = next;
        next
    }

    #[target_feature(enable = "sha,sse2,ssse3,sse4.1")]
    fn rounds<const FUNCTION: i32>(current: &mut __m128i, previous: &mut __m128i, words: __m128i) {
        let next = _mm_sha1rnds4_epu32::<FUNCTION>(*current, _mm_sha1nexte_epu32(*previous, words));
        *previous = *current;
        *current = next;
    }

    #[target_feature(enable = "sha,sse2,ssse3,sse4.1")]
    pub(super) fn compress(state: &mut [u32; STATE_WORDS], blocks: &[[u8; BLOCK_BYTES]]) {
        let byte_reverse = load(&BYTE_REVERSE);
        let mut abcd = _mm_set_epi32(
            state[0] as i32,
            state[1] as i32,
            state[2] as i32,
            state[3] as i32,
        );
        let mut e = _mm_set_epi32(state[4] as i32, 0, 0, 0);
        for block in blocks {
            let quarters = block.as_chunks::<16>().0;
            let mut words = [
                _mm_shuffle_epi8(load(&quarters[0]), byte_reverse),
                _mm_shuffle_epi8(load(&quarters[1]), byte_reverse),
                _mm_shuffle_epi8(load(&quarters[2]), byte_reverse),
                _mm_shuffle_epi8(load(&quarters[3]), byte_reverse),
            ];
            let mut previous = abcd;
            let mut current = _mm_sha1rnds4_epu32::<0>(abcd, _mm_add_epi32(e, words[0]));
            for message in words.into_iter().skip(1) {
                rounds::<0>(&mut current, &mut previous, message);
            }
            rounds::<0>(&mut current, &mut previous, scheduled(&mut words, 4));
            for group in 5..10 {
                rounds::<1>(&mut current, &mut previous, scheduled(&mut words, group));
            }
            for group in 10..15 {
                rounds::<2>(&mut current, &mut previous, scheduled(&mut words, group));
            }
            for group in 15..20 {
                rounds::<3>(&mut current, &mut previous, scheduled(&mut words, group));
            }
            abcd = _mm_add_epi32(abcd, current);
            e = _mm_sha1nexte_epu32(previous, e);
        }
        *state = [
            _mm_extract_epi32(abcd, 3) as u32,
            _mm_extract_epi32(abcd, 2) as u32,
            _mm_extract_epi32(abcd, 1) as u32,
            _mm_extract_epi32(abcd, 0) as u32,
            _mm_extract_epi32(e, 3) as u32,
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
    fn message(words: &mut [uint32x4_t; 4], group: usize) -> uint32x4_t {
        if group < words.len() {
            return words[group];
        }
        let next = vsha1su1q_u32(
            vsha1su0q_u32(
                words[group % 4],
                words[(group + 1) % 4],
                words[(group + 2) % 4],
            ),
            words[(group + 3) % 4],
        );
        words[group % 4] = next;
        next
    }

    #[target_feature(enable = "sha2")]
    fn sum(words: &mut [uint32x4_t; 4], group: usize, constant: u32) -> uint32x4_t {
        vaddq_u32(message(words, group), vdupq_n_u32(constant))
    }

    #[target_feature(enable = "sha2")]
    pub(super) fn compress(state: &mut [u32; STATE_WORDS], blocks: &[[u8; BLOCK_BYTES]]) {
        let mut abcd = load_words(
            &state
                .first_chunk::<4>()
                .copied()
                .expect("five words hold four"),
        );
        let mut e = state[4];
        for block in blocks {
            let abcd_saved = abcd;
            let e_saved = e;
            let quarters = block.as_chunks::<16>().0;
            let mut words = [
                load_bytes(&quarters[0]),
                load_bytes(&quarters[1]),
                load_bytes(&quarters[2]),
                load_bytes(&quarters[3]),
            ];
            for group in 0..5 {
                let input = sum(&mut words, group, ROUND_CONSTANTS[0]);
                let e_next = vsha1h_u32(vgetq_lane_u32(abcd, 0));
                abcd = vsha1cq_u32(abcd, e, input);
                e = e_next;
            }
            for group in 5..10 {
                let input = sum(&mut words, group, ROUND_CONSTANTS[1]);
                let e_next = vsha1h_u32(vgetq_lane_u32(abcd, 0));
                abcd = vsha1pq_u32(abcd, e, input);
                e = e_next;
            }
            for group in 10..15 {
                let input = sum(&mut words, group, ROUND_CONSTANTS[2]);
                let e_next = vsha1h_u32(vgetq_lane_u32(abcd, 0));
                abcd = vsha1mq_u32(abcd, e, input);
                e = e_next;
            }
            for group in 15..20 {
                let input = sum(&mut words, group, ROUND_CONSTANTS[3]);
                let e_next = vsha1h_u32(vgetq_lane_u32(abcd, 0));
                abcd = vsha1pq_u32(abcd, e, input);
                e = e_next;
            }
            abcd = vaddq_u32(abcd, abcd_saved);
            e = e.wrapping_add(e_saved);
        }
        *state = [
            vgetq_lane_u32(abcd, 0),
            vgetq_lane_u32(abcd, 1),
            vgetq_lane_u32(abcd, 2),
            vgetq_lane_u32(abcd, 3),
            e,
        ];
    }
}
