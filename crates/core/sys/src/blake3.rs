const BLOCK_BYTES: usize = 64;
const CHUNK_BYTES: usize = 1024;
const CHAINING_BYTES: usize = 32;
const CHAINING_WORDS: usize = 8;

#[derive(Clone, Copy, Debug)]
pub struct Blake3Instructions(Kind);

#[cfg(not(any(
    target_arch = "x86_64",
    all(target_arch = "aarch64", target_endian = "little")
)))]
#[derive(Clone, Copy, Debug)]
enum Kind {}

#[cfg(not(any(
    target_arch = "x86_64",
    all(target_arch = "aarch64", target_endian = "little")
)))]
#[expect(clippy::missing_const_for_fn)]
impl Blake3Instructions {
    pub fn detect() -> Option<Self> {
        None
    }

    pub fn hash_chunks(
        self,
        _: &[[u8; CHUNK_BYTES]],
        _: &[u32; CHAINING_WORDS],
        _: u64,
        _: u32,
        _: &mut [[u8; CHAINING_BYTES]],
    ) {
        match self.0 {}
    }

    pub fn hash_parents(
        self,
        _: &[[u8; BLOCK_BYTES]],
        _: &[u32; CHAINING_WORDS],
        _: u32,
        _: &mut [[u8; CHAINING_BYTES]],
    ) {
        match self.0 {}
    }
}

#[cfg(any(
    target_arch = "x86_64",
    all(target_arch = "aarch64", target_endian = "little")
))]
use lanes::Kind;

#[cfg(any(
    target_arch = "x86_64",
    all(target_arch = "aarch64", target_endian = "little")
))]
mod lanes {
    use super::{BLOCK_BYTES, Blake3Instructions, CHAINING_BYTES, CHAINING_WORDS, CHUNK_BYTES};
    use core::array;

    const CHUNK_BLOCKS: usize = CHUNK_BYTES / BLOCK_BYTES;
    const BLOCK_WORDS: usize = 16;
    const ROUNDS: usize = 7;
    const LANES_MIN: usize = 4;

    const CHUNK_START: u32 = 1 << 0;
    const CHUNK_END: u32 = 1 << 1;
    const PARENT: u32 = 1 << 2;

    const IV: [u32; CHAINING_WORDS] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];

    const MESSAGE_SCHEDULE: [[usize; BLOCK_WORDS]; ROUNDS] = [
        [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15],
        [2, 6, 3, 10, 7, 0, 4, 13, 1, 11, 12, 5, 9, 14, 15, 8],
        [3, 4, 10, 12, 13, 2, 7, 14, 6, 5, 9, 0, 11, 15, 8, 1],
        [10, 7, 12, 9, 14, 3, 13, 15, 4, 0, 11, 2, 5, 8, 1, 6],
        [12, 13, 9, 11, 15, 10, 14, 8, 7, 2, 5, 3, 0, 1, 6, 4],
        [9, 14, 11, 5, 8, 12, 15, 1, 13, 3, 0, 10, 2, 6, 4, 7],
        [11, 15, 5, 0, 1, 9, 8, 6, 14, 10, 2, 12, 3, 4, 7, 13],
    ];

    #[derive(Clone, Copy, Debug)]
    pub(super) enum Kind {
        #[cfg(target_arch = "x86_64")]
        Sse41,
        #[cfg(target_arch = "x86_64")]
        Avx2,
        #[cfg(target_arch = "x86_64")]
        Avx512,
        #[cfg(target_arch = "aarch64")]
        Neon,
    }

    impl Blake3Instructions {
        pub fn detect() -> Option<Self> {
            #[cfg(target_arch = "x86_64")]
            {
                if !std::arch::is_x86_feature_detected!("sse4.1") {
                    return None;
                }
                if !std::arch::is_x86_feature_detected!("avx2") {
                    return Some(Self(Kind::Sse41));
                }
                if !std::arch::is_x86_feature_detected!("avx512f") {
                    return Some(Self(Kind::Avx2));
                }
                Some(Self(Kind::Avx512))
            }
            #[cfg(target_arch = "aarch64")]
            {
                if !std::arch::is_aarch64_feature_detected!("neon") {
                    return None;
                }
                Some(Self(Kind::Neon))
            }
        }

        pub fn hash_chunks(
            self,
            chunks: &[[u8; CHUNK_BYTES]],
            key: &[u32; CHAINING_WORDS],
            counter: u64,
            flags: u32,
            chaining_values: &mut [[u8; CHAINING_BYTES]],
        ) {
            assert_eq!(chunks.len(), chaining_values.len());
            let chunks_count = u64::try_from(chunks.len()).expect("a slice length fits in u64");
            assert!(counter.checked_add(chunks_count).is_some());
            let (blocks, rest) = chunks.as_flattened().as_chunks::<BLOCK_BYTES>();
            assert!(rest.is_empty());
            let (inputs, rest) = blocks.as_chunks::<CHUNK_BLOCKS>();
            assert!(rest.is_empty());
            let lanes = Lanes {
                key: *key,
                counter,
                counter_step: 1,
                flags,
                flags_start: CHUNK_START,
                flags_end: CHUNK_END,
            };
            hash_many(self.0, inputs, lanes, chaining_values);
        }

        pub fn hash_parents(
            self,
            parents: &[[u8; BLOCK_BYTES]],
            key: &[u32; CHAINING_WORDS],
            flags: u32,
            chaining_values: &mut [[u8; CHAINING_BYTES]],
        ) {
            assert_eq!(parents.len(), chaining_values.len());
            let (inputs, rest) = parents.as_chunks::<1>();
            assert!(rest.is_empty());
            let lanes = Lanes {
                key: *key,
                counter: 0,
                counter_step: 0,
                flags: flags | PARENT,
                flags_start: 0,
                flags_end: 0,
            };
            hash_many(self.0, inputs, lanes, chaining_values);
        }
    }

    #[derive(Clone, Copy)]
    struct Lanes {
        key: [u32; CHAINING_WORDS],
        counter: u64,
        counter_step: u64,
        flags: u32,
        flags_start: u32,
        flags_end: u32,
    }

    impl Lanes {
        const fn counter(&self, lane: usize) -> u64 {
            self.counter
                .wrapping_add(self.counter_step.wrapping_mul(lane as u64))
        }

        const fn advanced(self, lanes: usize) -> Self {
            Self {
                counter: self.counter(lanes),
                ..self
            }
        }

        const fn block_flags(&self, block: usize, blocks: usize) -> u32 {
            let mut flags = self.flags;
            if block == 0 {
                flags |= self.flags_start;
            }
            if block + 1 == blocks {
                flags |= self.flags_end;
            }
            flags
        }

        fn counter_words<const LANES: usize>(&self) -> ([u32; LANES], [u32; LANES]) {
            let mut low = [0; LANES];
            let mut high = [0; LANES];
            for (lane, (low, high)) in low.iter_mut().zip(&mut high).enumerate() {
                let counter = self.counter(lane);
                *low = counter as u32;
                *high = (counter >> u32::BITS) as u32;
            }
            (low, high)
        }
    }

    fn blocks_at<'a, const LANES: usize, const BLOCKS: usize>(
        inputs: &[&'a [[u8; BLOCK_BYTES]; BLOCKS]; LANES],
        block: usize,
    ) -> [&'a [u8; BLOCK_BYTES]; LANES] {
        array::from_fn(|lane| &inputs[lane][block])
    }

    fn hash_many<const BLOCKS: usize>(
        kind: Kind,
        inputs: &[[[u8; BLOCK_BYTES]; BLOCKS]],
        lanes: Lanes,
        outputs: &mut [[u8; CHAINING_BYTES]],
    ) {
        assert_eq!(inputs.len(), outputs.len());
        match kind {
            #[cfg(target_arch = "x86_64")]
            Kind::Sse41 => unsafe { x86::hash_many_sse41(inputs, lanes, outputs) },
            #[cfg(target_arch = "x86_64")]
            Kind::Avx2 => unsafe { x86::hash_many_avx2(inputs, lanes, outputs) },
            #[cfg(target_arch = "x86_64")]
            Kind::Avx512 => unsafe { x86::hash_many_avx512(inputs, lanes, outputs) },
            #[cfg(target_arch = "aarch64")]
            Kind::Neon => unsafe { neon::hash_many(inputs, lanes, outputs) },
        }
    }

    macro_rules! lanes_round {
        ($v:ident, $m:ident, $round:literal) => {{
            let schedule = &MESSAGE_SCHEDULE[$round];
            $v[0] = add($v[0], $m[schedule[0]]);
            $v[1] = add($v[1], $m[schedule[2]]);
            $v[2] = add($v[2], $m[schedule[4]]);
            $v[3] = add($v[3], $m[schedule[6]]);
            $v[0] = add($v[0], $v[4]);
            $v[1] = add($v[1], $v[5]);
            $v[2] = add($v[2], $v[6]);
            $v[3] = add($v[3], $v[7]);
            $v[12] = rot16(xor($v[12], $v[0]));
            $v[13] = rot16(xor($v[13], $v[1]));
            $v[14] = rot16(xor($v[14], $v[2]));
            $v[15] = rot16(xor($v[15], $v[3]));
            $v[8] = add($v[8], $v[12]);
            $v[9] = add($v[9], $v[13]);
            $v[10] = add($v[10], $v[14]);
            $v[11] = add($v[11], $v[15]);
            $v[4] = rot12(xor($v[4], $v[8]));
            $v[5] = rot12(xor($v[5], $v[9]));
            $v[6] = rot12(xor($v[6], $v[10]));
            $v[7] = rot12(xor($v[7], $v[11]));
            $v[0] = add($v[0], $m[schedule[1]]);
            $v[1] = add($v[1], $m[schedule[3]]);
            $v[2] = add($v[2], $m[schedule[5]]);
            $v[3] = add($v[3], $m[schedule[7]]);
            $v[0] = add($v[0], $v[4]);
            $v[1] = add($v[1], $v[5]);
            $v[2] = add($v[2], $v[6]);
            $v[3] = add($v[3], $v[7]);
            $v[12] = rot8(xor($v[12], $v[0]));
            $v[13] = rot8(xor($v[13], $v[1]));
            $v[14] = rot8(xor($v[14], $v[2]));
            $v[15] = rot8(xor($v[15], $v[3]));
            $v[8] = add($v[8], $v[12]);
            $v[9] = add($v[9], $v[13]);
            $v[10] = add($v[10], $v[14]);
            $v[11] = add($v[11], $v[15]);
            $v[4] = rot7(xor($v[4], $v[8]));
            $v[5] = rot7(xor($v[5], $v[9]));
            $v[6] = rot7(xor($v[6], $v[10]));
            $v[7] = rot7(xor($v[7], $v[11]));
            $v[0] = add($v[0], $m[schedule[8]]);
            $v[1] = add($v[1], $m[schedule[10]]);
            $v[2] = add($v[2], $m[schedule[12]]);
            $v[3] = add($v[3], $m[schedule[14]]);
            $v[0] = add($v[0], $v[5]);
            $v[1] = add($v[1], $v[6]);
            $v[2] = add($v[2], $v[7]);
            $v[3] = add($v[3], $v[4]);
            $v[15] = rot16(xor($v[15], $v[0]));
            $v[12] = rot16(xor($v[12], $v[1]));
            $v[13] = rot16(xor($v[13], $v[2]));
            $v[14] = rot16(xor($v[14], $v[3]));
            $v[10] = add($v[10], $v[15]);
            $v[11] = add($v[11], $v[12]);
            $v[8] = add($v[8], $v[13]);
            $v[9] = add($v[9], $v[14]);
            $v[5] = rot12(xor($v[5], $v[10]));
            $v[6] = rot12(xor($v[6], $v[11]));
            $v[7] = rot12(xor($v[7], $v[8]));
            $v[4] = rot12(xor($v[4], $v[9]));
            $v[0] = add($v[0], $m[schedule[9]]);
            $v[1] = add($v[1], $m[schedule[11]]);
            $v[2] = add($v[2], $m[schedule[13]]);
            $v[3] = add($v[3], $m[schedule[15]]);
            $v[0] = add($v[0], $v[5]);
            $v[1] = add($v[1], $v[6]);
            $v[2] = add($v[2], $v[7]);
            $v[3] = add($v[3], $v[4]);
            $v[15] = rot8(xor($v[15], $v[0]));
            $v[12] = rot8(xor($v[12], $v[1]));
            $v[13] = rot8(xor($v[13], $v[2]));
            $v[14] = rot8(xor($v[14], $v[3]));
            $v[10] = add($v[10], $v[15]);
            $v[11] = add($v[11], $v[12]);
            $v[8] = add($v[8], $v[13]);
            $v[9] = add($v[9], $v[14]);
            $v[5] = rot7(xor($v[5], $v[10]));
            $v[6] = rot7(xor($v[6], $v[11]));
            $v[7] = rot7(xor($v[7], $v[8]));
            $v[4] = rot7(xor($v[4], $v[9]));
        }};
    }

    macro_rules! lanes_compress {
        ($h:ident, $m:ident, $counter_low:expr, $counter_high:expr, $flags:expr) => {{
            let mut v = [
                $h[0],
                $h[1],
                $h[2],
                $h[3],
                $h[4],
                $h[5],
                $h[6],
                $h[7],
                set1(IV[0]),
                set1(IV[1]),
                set1(IV[2]),
                set1(IV[3]),
                $counter_low,
                $counter_high,
                set1(BLOCK_BYTES as u32),
                set1($flags),
            ];
            lanes_round!(v, $m, 0);
            lanes_round!(v, $m, 1);
            lanes_round!(v, $m, 2);
            lanes_round!(v, $m, 3);
            lanes_round!(v, $m, 4);
            lanes_round!(v, $m, 5);
            lanes_round!(v, $m, 6);
            $h = [
                xor(v[0], v[8]),
                xor(v[1], v[9]),
                xor(v[2], v[10]),
                xor(v[3], v[11]),
                xor(v[4], v[12]),
                xor(v[5], v[13]),
                xor(v[6], v[14]),
                xor(v[7], v[15]),
            ];
        }};
    }

    #[cfg(target_arch = "x86_64")]
    mod x86 {
        use super::{BLOCK_BYTES, CHAINING_BYTES, LANES_MIN, Lanes, array};

        #[target_feature(enable = "avx512f")]
        pub(super) fn hash_many_avx512<const BLOCKS: usize>(
            inputs: &[[[u8; BLOCK_BYTES]; BLOCKS]],
            mut lanes: Lanes,
            outputs: &mut [[u8; CHAINING_BYTES]],
        ) {
            let (groups, inputs_rest) = inputs.as_chunks::<{ avx512::LANES }>();
            let (output_groups, outputs_rest) = outputs.as_chunks_mut::<{ avx512::LANES }>();
            for (group, output) in groups.iter().zip(output_groups) {
                avx512::hash16(group.each_ref(), lanes, output);
                lanes = lanes.advanced(avx512::LANES);
            }
            hash_many_avx2(inputs_rest, lanes, outputs_rest);
        }

        #[target_feature(enable = "avx2")]
        pub(super) fn hash_many_avx2<const BLOCKS: usize>(
            inputs: &[[[u8; BLOCK_BYTES]; BLOCKS]],
            mut lanes: Lanes,
            outputs: &mut [[u8; CHAINING_BYTES]],
        ) {
            let (groups, inputs_rest) = inputs.as_chunks::<{ avx2::LANES }>();
            let (output_groups, outputs_rest) = outputs.as_chunks_mut::<{ avx2::LANES }>();
            for (group, output) in groups.iter().zip(output_groups) {
                avx2::hash8(group.each_ref(), lanes, output);
                lanes = lanes.advanced(avx2::LANES);
            }
            hash_many_sse41(inputs_rest, lanes, outputs_rest);
        }

        #[target_feature(enable = "sse4.1")]
        pub(super) fn hash_many_sse41<const BLOCKS: usize>(
            inputs: &[[[u8; BLOCK_BYTES]; BLOCKS]],
            mut lanes: Lanes,
            outputs: &mut [[u8; CHAINING_BYTES]],
        ) {
            let (groups, inputs_rest) = inputs.as_chunks::<LANES_MIN>();
            let (output_groups, outputs_rest) = outputs.as_chunks_mut::<LANES_MIN>();
            for (group, output) in groups.iter().zip(output_groups) {
                sse41::hash4(group.each_ref(), lanes, output);
                lanes = lanes.advanced(LANES_MIN);
            }
            if let Some(last) = inputs_rest.len().checked_sub(1) {
                let mut padded = [[0u8; CHAINING_BYTES]; LANES_MIN];
                sse41::hash4(
                    array::from_fn(|lane| &inputs_rest[lane.min(last)]),
                    lanes,
                    &mut padded,
                );
                outputs_rest.copy_from_slice(&padded[..inputs_rest.len()]);
            }
        }

        mod sse41 {
            use super::super::{
                BLOCK_BYTES, CHAINING_BYTES, CHAINING_WORDS, IV, LANES_MIN, Lanes,
                MESSAGE_SCHEDULE, blocks_at,
            };
            use core::arch::x86_64::*;

            const LOAD_BYTES: usize = 16;

            #[target_feature(enable = "sse4.1")]
            #[inline]
            fn load(bytes: &[u8; LOAD_BYTES]) -> __m128i {
                unsafe { _mm_loadu_si128(bytes.as_ptr().cast()) }
            }

            #[target_feature(enable = "sse4.1")]
            #[inline]
            fn store(vector: __m128i, bytes: &mut [u8; LOAD_BYTES]) {
                unsafe { _mm_storeu_si128(bytes.as_mut_ptr().cast(), vector) }
            }

            #[target_feature(enable = "sse4.1")]
            #[inline]
            fn add(left: __m128i, right: __m128i) -> __m128i {
                _mm_add_epi32(left, right)
            }

            #[target_feature(enable = "sse4.1")]
            #[inline]
            fn xor(left: __m128i, right: __m128i) -> __m128i {
                _mm_xor_si128(left, right)
            }

            #[target_feature(enable = "sse4.1")]
            #[inline]
            fn set1(word: u32) -> __m128i {
                _mm_set1_epi32(word as i32)
            }

            #[target_feature(enable = "sse4.1")]
            #[inline]
            fn set4(words: [u32; LANES_MIN]) -> __m128i {
                _mm_setr_epi32(
                    words[0] as i32,
                    words[1] as i32,
                    words[2] as i32,
                    words[3] as i32,
                )
            }

            #[target_feature(enable = "sse4.1")]
            #[inline]
            fn rot16(vector: __m128i) -> __m128i {
                _mm_shuffle_epi8(
                    vector,
                    _mm_setr_epi8(2, 3, 0, 1, 6, 7, 4, 5, 10, 11, 8, 9, 14, 15, 12, 13),
                )
            }

            #[target_feature(enable = "sse4.1")]
            #[inline]
            fn rot12(vector: __m128i) -> __m128i {
                _mm_or_si128(_mm_srli_epi32::<12>(vector), _mm_slli_epi32::<20>(vector))
            }

            #[target_feature(enable = "sse4.1")]
            #[inline]
            fn rot8(vector: __m128i) -> __m128i {
                _mm_shuffle_epi8(
                    vector,
                    _mm_setr_epi8(1, 2, 3, 0, 5, 6, 7, 4, 9, 10, 11, 8, 13, 14, 15, 12),
                )
            }

            #[target_feature(enable = "sse4.1")]
            #[inline]
            fn rot7(vector: __m128i) -> __m128i {
                _mm_or_si128(_mm_srli_epi32::<7>(vector), _mm_slli_epi32::<25>(vector))
            }

            #[target_feature(enable = "sse4.1")]
            #[inline]
            fn transpose(vectors: [__m128i; LANES_MIN]) -> [__m128i; LANES_MIN] {
                let ab_01 = _mm_unpacklo_epi32(vectors[0], vectors[1]);
                let ab_23 = _mm_unpackhi_epi32(vectors[0], vectors[1]);
                let cd_01 = _mm_unpacklo_epi32(vectors[2], vectors[3]);
                let cd_23 = _mm_unpackhi_epi32(vectors[2], vectors[3]);
                [
                    _mm_unpacklo_epi64(ab_01, cd_01),
                    _mm_unpackhi_epi64(ab_01, cd_01),
                    _mm_unpacklo_epi64(ab_23, cd_23),
                    _mm_unpackhi_epi64(ab_23, cd_23),
                ]
            }

            #[target_feature(enable = "sse4.1")]
            #[inline]
            fn message(blocks: [&[u8; BLOCK_BYTES]; LANES_MIN]) -> [__m128i; 16] {
                let mut message = [_mm_setzero_si128(); 16];
                for (lane, block) in blocks.iter().enumerate() {
                    for (quarter, bytes) in block.as_chunks::<LOAD_BYTES>().0.iter().enumerate() {
                        message[quarter * LANES_MIN + lane] = load(bytes);
                    }
                }
                for square in message.as_chunks_mut::<LANES_MIN>().0 {
                    *square = transpose(*square);
                }
                message
            }

            #[target_feature(enable = "sse4.1")]
            pub(super) fn hash4<const BLOCKS: usize>(
                inputs: [&[[u8; BLOCK_BYTES]; BLOCKS]; LANES_MIN],
                lanes: Lanes,
                outputs: &mut [[u8; CHAINING_BYTES]; LANES_MIN],
            ) {
                let mut h = [set1(0); CHAINING_WORDS];
                for (vector, word) in h.iter_mut().zip(lanes.key) {
                    *vector = set1(word);
                }
                let (counter_low, counter_high) = lanes.counter_words::<LANES_MIN>();
                let counter_low = set4(counter_low);
                let counter_high = set4(counter_high);
                for block in 0..BLOCKS {
                    let m = message(blocks_at(&inputs, block));
                    lanes_compress!(
                        h,
                        m,
                        counter_low,
                        counter_high,
                        lanes.block_flags(block, BLOCKS)
                    );
                }
                let low = transpose([h[0], h[1], h[2], h[3]]);
                let high = transpose([h[4], h[5], h[6], h[7]]);
                for (lane, output) in outputs.iter_mut().enumerate() {
                    let halves = output.as_chunks_mut::<LOAD_BYTES>().0;
                    store(low[lane], &mut halves[0]);
                    store(high[lane], &mut halves[1]);
                }
            }
        }

        pub(super) mod avx2 {
            use super::super::{
                BLOCK_BYTES, CHAINING_BYTES, CHAINING_WORDS, IV, Lanes, MESSAGE_SCHEDULE, blocks_at,
            };
            use core::arch::x86_64::*;

            pub(in super::super) const LANES: usize = 8;
            const LOAD_BYTES: usize = 32;

            #[target_feature(enable = "avx2")]
            #[inline]
            fn load(bytes: &[u8; LOAD_BYTES]) -> __m256i {
                unsafe { _mm256_loadu_si256(bytes.as_ptr().cast()) }
            }

            #[target_feature(enable = "avx2")]
            #[inline]
            pub(in super::super) fn store(vector: __m256i, bytes: &mut [u8; LOAD_BYTES]) {
                unsafe { _mm256_storeu_si256(bytes.as_mut_ptr().cast(), vector) }
            }

            #[target_feature(enable = "avx2")]
            #[inline]
            fn add(left: __m256i, right: __m256i) -> __m256i {
                _mm256_add_epi32(left, right)
            }

            #[target_feature(enable = "avx2")]
            #[inline]
            fn xor(left: __m256i, right: __m256i) -> __m256i {
                _mm256_xor_si256(left, right)
            }

            #[target_feature(enable = "avx2")]
            #[inline]
            fn set1(word: u32) -> __m256i {
                _mm256_set1_epi32(word as i32)
            }

            #[target_feature(enable = "avx2")]
            #[inline]
            fn set8(words: [u32; LANES]) -> __m256i {
                _mm256_setr_epi32(
                    words[0] as i32,
                    words[1] as i32,
                    words[2] as i32,
                    words[3] as i32,
                    words[4] as i32,
                    words[5] as i32,
                    words[6] as i32,
                    words[7] as i32,
                )
            }

            #[target_feature(enable = "avx2")]
            #[inline]
            fn rot16(vector: __m256i) -> __m256i {
                _mm256_shuffle_epi8(
                    vector,
                    _mm256_setr_epi8(
                        2, 3, 0, 1, 6, 7, 4, 5, 10, 11, 8, 9, 14, 15, 12, 13, 2, 3, 0, 1, 6, 7, 4,
                        5, 10, 11, 8, 9, 14, 15, 12, 13,
                    ),
                )
            }

            #[target_feature(enable = "avx2")]
            #[inline]
            fn rot12(vector: __m256i) -> __m256i {
                _mm256_or_si256(
                    _mm256_srli_epi32::<12>(vector),
                    _mm256_slli_epi32::<20>(vector),
                )
            }

            #[target_feature(enable = "avx2")]
            #[inline]
            fn rot8(vector: __m256i) -> __m256i {
                _mm256_shuffle_epi8(
                    vector,
                    _mm256_setr_epi8(
                        1, 2, 3, 0, 5, 6, 7, 4, 9, 10, 11, 8, 13, 14, 15, 12, 1, 2, 3, 0, 5, 6, 7,
                        4, 9, 10, 11, 8, 13, 14, 15, 12,
                    ),
                )
            }

            #[target_feature(enable = "avx2")]
            #[inline]
            fn rot7(vector: __m256i) -> __m256i {
                _mm256_or_si256(
                    _mm256_srli_epi32::<7>(vector),
                    _mm256_slli_epi32::<25>(vector),
                )
            }

            #[target_feature(enable = "avx2")]
            #[inline]
            fn transpose(vectors: [__m256i; LANES]) -> [__m256i; LANES] {
                let ab_0145 = _mm256_unpacklo_epi32(vectors[0], vectors[1]);
                let ab_2367 = _mm256_unpackhi_epi32(vectors[0], vectors[1]);
                let cd_0145 = _mm256_unpacklo_epi32(vectors[2], vectors[3]);
                let cd_2367 = _mm256_unpackhi_epi32(vectors[2], vectors[3]);
                let ef_0145 = _mm256_unpacklo_epi32(vectors[4], vectors[5]);
                let ef_2367 = _mm256_unpackhi_epi32(vectors[4], vectors[5]);
                let gh_0145 = _mm256_unpacklo_epi32(vectors[6], vectors[7]);
                let gh_2367 = _mm256_unpackhi_epi32(vectors[6], vectors[7]);
                let abcd_04 = _mm256_unpacklo_epi64(ab_0145, cd_0145);
                let abcd_15 = _mm256_unpackhi_epi64(ab_0145, cd_0145);
                let abcd_26 = _mm256_unpacklo_epi64(ab_2367, cd_2367);
                let abcd_37 = _mm256_unpackhi_epi64(ab_2367, cd_2367);
                let efgh_04 = _mm256_unpacklo_epi64(ef_0145, gh_0145);
                let efgh_15 = _mm256_unpackhi_epi64(ef_0145, gh_0145);
                let efgh_26 = _mm256_unpacklo_epi64(ef_2367, gh_2367);
                let efgh_37 = _mm256_unpackhi_epi64(ef_2367, gh_2367);
                [
                    _mm256_permute2x128_si256::<0x20>(abcd_04, efgh_04),
                    _mm256_permute2x128_si256::<0x20>(abcd_15, efgh_15),
                    _mm256_permute2x128_si256::<0x20>(abcd_26, efgh_26),
                    _mm256_permute2x128_si256::<0x20>(abcd_37, efgh_37),
                    _mm256_permute2x128_si256::<0x31>(abcd_04, efgh_04),
                    _mm256_permute2x128_si256::<0x31>(abcd_15, efgh_15),
                    _mm256_permute2x128_si256::<0x31>(abcd_26, efgh_26),
                    _mm256_permute2x128_si256::<0x31>(abcd_37, efgh_37),
                ]
            }

            #[target_feature(enable = "avx2")]
            #[inline]
            fn message(blocks: [&[u8; BLOCK_BYTES]; LANES]) -> [__m256i; 16] {
                let mut message = [_mm256_setzero_si256(); 16];
                for (lane, block) in blocks.iter().enumerate() {
                    for (half, bytes) in block.as_chunks::<LOAD_BYTES>().0.iter().enumerate() {
                        message[half * LANES + lane] = load(bytes);
                    }
                }
                for square in message.as_chunks_mut::<LANES>().0 {
                    *square = transpose(*square);
                }
                message
            }

            #[target_feature(enable = "avx2")]
            pub(in super::super) fn hash8<const BLOCKS: usize>(
                inputs: [&[[u8; BLOCK_BYTES]; BLOCKS]; LANES],
                lanes: Lanes,
                outputs: &mut [[u8; CHAINING_BYTES]; LANES],
            ) {
                let mut h = [set1(0); CHAINING_WORDS];
                for (vector, word) in h.iter_mut().zip(lanes.key) {
                    *vector = set1(word);
                }
                let (counter_low, counter_high) = lanes.counter_words::<LANES>();
                let counter_low = set8(counter_low);
                let counter_high = set8(counter_high);
                for block in 0..BLOCKS {
                    let m = message(blocks_at(&inputs, block));
                    lanes_compress!(
                        h,
                        m,
                        counter_low,
                        counter_high,
                        lanes.block_flags(block, BLOCKS)
                    );
                }
                for (output, vector) in outputs.iter_mut().zip(transpose(h)) {
                    store(vector, output);
                }
            }
        }

        pub(super) mod avx512 {
            use super::super::{
                BLOCK_BYTES, CHAINING_BYTES, CHAINING_WORDS, IV, Lanes, MESSAGE_SCHEDULE, blocks_at,
            };
            use core::arch::x86_64::*;

            pub(in super::super) const LANES: usize = 16;

            #[target_feature(enable = "avx512f")]
            #[inline]
            fn load(bytes: &[u8; BLOCK_BYTES]) -> __m512i {
                unsafe { _mm512_loadu_si512(bytes.as_ptr().cast()) }
            }

            #[target_feature(enable = "avx512f")]
            #[inline]
            fn add(left: __m512i, right: __m512i) -> __m512i {
                _mm512_add_epi32(left, right)
            }

            #[target_feature(enable = "avx512f")]
            #[inline]
            fn xor(left: __m512i, right: __m512i) -> __m512i {
                _mm512_xor_si512(left, right)
            }

            #[target_feature(enable = "avx512f")]
            #[inline]
            fn set1(word: u32) -> __m512i {
                _mm512_set1_epi32(word as i32)
            }

            #[target_feature(enable = "avx512f")]
            #[inline]
            fn set16(words: [u32; LANES]) -> __m512i {
                _mm512_setr_epi32(
                    words[0] as i32,
                    words[1] as i32,
                    words[2] as i32,
                    words[3] as i32,
                    words[4] as i32,
                    words[5] as i32,
                    words[6] as i32,
                    words[7] as i32,
                    words[8] as i32,
                    words[9] as i32,
                    words[10] as i32,
                    words[11] as i32,
                    words[12] as i32,
                    words[13] as i32,
                    words[14] as i32,
                    words[15] as i32,
                )
            }

            #[target_feature(enable = "avx512f")]
            #[inline]
            fn rot16(vector: __m512i) -> __m512i {
                _mm512_ror_epi32::<16>(vector)
            }

            #[target_feature(enable = "avx512f")]
            #[inline]
            fn rot12(vector: __m512i) -> __m512i {
                _mm512_ror_epi32::<12>(vector)
            }

            #[target_feature(enable = "avx512f")]
            #[inline]
            fn rot8(vector: __m512i) -> __m512i {
                _mm512_ror_epi32::<8>(vector)
            }

            #[target_feature(enable = "avx512f")]
            #[inline]
            fn rot7(vector: __m512i) -> __m512i {
                _mm512_ror_epi32::<7>(vector)
            }

            #[target_feature(enable = "avx512f")]
            #[inline]
            fn transpose(vectors: [__m512i; LANES]) -> [__m512i; LANES] {
                let mut pairs = vectors;
                for index in 0..LANES / 2 {
                    pairs[2 * index] =
                        _mm512_unpacklo_epi32(vectors[2 * index], vectors[2 * index + 1]);
                    pairs[2 * index + 1] =
                        _mm512_unpackhi_epi32(vectors[2 * index], vectors[2 * index + 1]);
                }
                let mut quads = pairs;
                for group in 0..LANES / 4 {
                    let base = 4 * group;
                    quads[base] = _mm512_unpacklo_epi64(pairs[base], pairs[base + 2]);
                    quads[base + 1] = _mm512_unpackhi_epi64(pairs[base], pairs[base + 2]);
                    quads[base + 2] = _mm512_unpacklo_epi64(pairs[base + 1], pairs[base + 3]);
                    quads[base + 3] = _mm512_unpackhi_epi64(pairs[base + 1], pairs[base + 3]);
                }
                let mut octets = quads;
                for half in 0..2 {
                    let base = 8 * half;
                    for column in 0..4 {
                        octets[base + column] = _mm512_shuffle_i32x4::<0x88>(
                            quads[base + column],
                            quads[base + 4 + column],
                        );
                        octets[base + 4 + column] = _mm512_shuffle_i32x4::<0xdd>(
                            quads[base + column],
                            quads[base + 4 + column],
                        );
                    }
                }
                let mut words = octets;
                for column in 0..8 {
                    words[column] =
                        _mm512_shuffle_i32x4::<0x88>(octets[column], octets[8 + column]);
                    words[8 + column] =
                        _mm512_shuffle_i32x4::<0xdd>(octets[column], octets[8 + column]);
                }
                words
            }

            #[target_feature(enable = "avx512f")]
            pub(in super::super) fn hash16<const BLOCKS: usize>(
                inputs: [&[[u8; BLOCK_BYTES]; BLOCKS]; LANES],
                lanes: Lanes,
                outputs: &mut [[u8; CHAINING_BYTES]; LANES],
            ) {
                let mut h = [set1(0); CHAINING_WORDS];
                for (vector, word) in h.iter_mut().zip(lanes.key) {
                    *vector = set1(word);
                }
                let (counter_low, counter_high) = lanes.counter_words::<LANES>();
                let counter_low = set16(counter_low);
                let counter_high = set16(counter_high);
                for block in 0..BLOCKS {
                    let mut loaded = [set1(0); LANES];
                    for (vector, bytes) in loaded.iter_mut().zip(blocks_at(&inputs, block)) {
                        *vector = load(bytes);
                    }
                    let m = transpose(loaded);
                    lanes_compress!(
                        h,
                        m,
                        counter_low,
                        counter_high,
                        lanes.block_flags(block, BLOCKS)
                    );
                }
                let zero = _mm512_setzero_si512();
                let words = transpose([
                    h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7], zero, zero, zero, zero, zero,
                    zero, zero, zero,
                ]);
                for (output, vector) in outputs.iter_mut().zip(words) {
                    super::avx2::store(_mm512_castsi512_si256(vector), output);
                }
            }
        }
    }

    #[cfg(target_arch = "aarch64")]
    mod neon {
        use super::{
            BLOCK_BYTES, CHAINING_BYTES, CHAINING_WORDS, IV, LANES_MIN, Lanes, MESSAGE_SCHEDULE,
            array, blocks_at,
        };
        use core::arch::aarch64::*;

        const LOAD_BYTES: usize = 16;
        const ROTATE_8_TABLE: [u8; LOAD_BYTES] =
            [1, 2, 3, 0, 5, 6, 7, 4, 9, 10, 11, 8, 13, 14, 15, 12];

        #[target_feature(enable = "neon")]
        pub(super) fn hash_many<const BLOCKS: usize>(
            inputs: &[[[u8; BLOCK_BYTES]; BLOCKS]],
            mut lanes: Lanes,
            outputs: &mut [[u8; CHAINING_BYTES]],
        ) {
            let (groups, inputs_rest) = inputs.as_chunks::<LANES_MIN>();
            let (output_groups, outputs_rest) = outputs.as_chunks_mut::<LANES_MIN>();
            for (group, output) in groups.iter().zip(output_groups) {
                hash4(group.each_ref(), lanes, output);
                lanes = lanes.advanced(LANES_MIN);
            }
            if let Some(last) = inputs_rest.len().checked_sub(1) {
                let mut padded = [[0u8; CHAINING_BYTES]; LANES_MIN];
                hash4(
                    array::from_fn(|lane| &inputs_rest[lane.min(last)]),
                    lanes,
                    &mut padded,
                );
                outputs_rest.copy_from_slice(&padded[..inputs_rest.len()]);
            }
        }

        #[target_feature(enable = "neon")]
        #[inline]
        fn load(bytes: &[u8; LOAD_BYTES]) -> uint32x4_t {
            vreinterpretq_u32_u8(unsafe { vld1q_u8(bytes.as_ptr()) })
        }

        #[target_feature(enable = "neon")]
        #[inline]
        fn store(vector: uint32x4_t, bytes: &mut [u8; LOAD_BYTES]) {
            let lanes = vreinterpretq_u8_u32(vector);
            unsafe { vst1q_u8(bytes.as_mut_ptr(), lanes) }
        }

        #[target_feature(enable = "neon")]
        #[inline]
        fn add(left: uint32x4_t, right: uint32x4_t) -> uint32x4_t {
            vaddq_u32(left, right)
        }

        #[target_feature(enable = "neon")]
        #[inline]
        fn xor(left: uint32x4_t, right: uint32x4_t) -> uint32x4_t {
            veorq_u32(left, right)
        }

        #[target_feature(enable = "neon")]
        #[inline]
        fn set1(word: u32) -> uint32x4_t {
            vdupq_n_u32(word)
        }

        #[target_feature(enable = "neon")]
        #[inline]
        fn set4(words: [u32; LANES_MIN]) -> uint32x4_t {
            let mut bytes = [0u8; LOAD_BYTES];
            for (slot, word) in bytes.as_chunks_mut::<4>().0.iter_mut().zip(words) {
                *slot = word.to_le_bytes();
            }
            load(&bytes)
        }

        #[target_feature(enable = "neon")]
        #[inline]
        fn rot16(vector: uint32x4_t) -> uint32x4_t {
            vreinterpretq_u32_u16(vrev32q_u16(vreinterpretq_u16_u32(vector)))
        }

        #[target_feature(enable = "neon")]
        #[inline]
        fn rot12(vector: uint32x4_t) -> uint32x4_t {
            vsriq_n_u32::<12>(vshlq_n_u32::<20>(vector), vector)
        }

        #[target_feature(enable = "neon")]
        #[inline]
        fn rot8(vector: uint32x4_t) -> uint32x4_t {
            let table = vreinterpretq_u8_u32(load(&ROTATE_8_TABLE));
            vreinterpretq_u32_u8(vqtbl1q_u8(vreinterpretq_u8_u32(vector), table))
        }

        #[target_feature(enable = "neon")]
        #[inline]
        fn rot7(vector: uint32x4_t) -> uint32x4_t {
            vsriq_n_u32::<7>(vshlq_n_u32::<25>(vector), vector)
        }

        #[target_feature(enable = "neon")]
        #[inline]
        fn transpose(vectors: [uint32x4_t; LANES_MIN]) -> [uint32x4_t; LANES_MIN] {
            let rows01 = vtrnq_u32(vectors[0], vectors[1]);
            let rows23 = vtrnq_u32(vectors[2], vectors[3]);
            [
                vcombine_u32(vget_low_u32(rows01.0), vget_low_u32(rows23.0)),
                vcombine_u32(vget_low_u32(rows01.1), vget_low_u32(rows23.1)),
                vcombine_u32(vget_high_u32(rows01.0), vget_high_u32(rows23.0)),
                vcombine_u32(vget_high_u32(rows01.1), vget_high_u32(rows23.1)),
            ]
        }

        #[target_feature(enable = "neon")]
        #[inline]
        fn message(blocks: [&[u8; BLOCK_BYTES]; LANES_MIN]) -> [uint32x4_t; 16] {
            let mut message = [vdupq_n_u32(0); 16];
            for (lane, block) in blocks.iter().enumerate() {
                for (quarter, bytes) in block.as_chunks::<LOAD_BYTES>().0.iter().enumerate() {
                    message[quarter * LANES_MIN + lane] = load(bytes);
                }
            }
            for square in message.as_chunks_mut::<LANES_MIN>().0 {
                *square = transpose(*square);
            }
            message
        }

        #[target_feature(enable = "neon")]
        fn hash4<const BLOCKS: usize>(
            inputs: [&[[u8; BLOCK_BYTES]; BLOCKS]; LANES_MIN],
            lanes: Lanes,
            outputs: &mut [[u8; CHAINING_BYTES]; LANES_MIN],
        ) {
            let mut h = [set1(0); CHAINING_WORDS];
            for (vector, word) in h.iter_mut().zip(lanes.key) {
                *vector = set1(word);
            }
            let (counter_low, counter_high) = lanes.counter_words::<LANES_MIN>();
            let counter_low = set4(counter_low);
            let counter_high = set4(counter_high);
            for block in 0..BLOCKS {
                let m = message(blocks_at(&inputs, block));
                lanes_compress!(
                    h,
                    m,
                    counter_low,
                    counter_high,
                    lanes.block_flags(block, BLOCKS)
                );
            }
            let low = transpose([h[0], h[1], h[2], h[3]]);
            let high = transpose([h[4], h[5], h[6], h[7]]);
            for (lane, output) in outputs.iter_mut().enumerate() {
                let halves = output.as_chunks_mut::<LOAD_BYTES>().0;
                store(low[lane], &mut halves[0]);
                store(high[lane], &mut halves[1]);
            }
        }
    }
}
