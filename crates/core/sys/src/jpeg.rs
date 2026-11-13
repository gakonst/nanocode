#[cfg(target_arch = "aarch64")]
use core::arch::aarch64::{
    int16x8_t, int32x4_t, uint8x8_t, uint8x16x3_t, vaddq_s32, vcombine_s16, vcombine_u8,
    vcombine_u16, vdupq_n_s16, vdupq_n_s32, vdupq_n_u16, vget_low_s16, vld1q_s16, vld1q_s32,
    vmaxvq_u32, vminq_u16, vmlal_high_n_s16, vmlal_n_s16, vmulq_n_s32, vorrq_s32, vqmovun_s16,
    vqmovun_s32, vqshrn_n_s32, vreinterpretq_s16_u16, vreinterpretq_s32_s64, vreinterpretq_s64_s32,
    vreinterpretq_u32_s32, vshlq_n_s32, vshrq_n_s32, vst1q_s16, vst3q_u8, vsubq_s16, vsubq_s32,
    vtrn1q_s32, vtrn2q_s32, vzip1q_s64, vzip2q_s64,
};
#[cfg(target_arch = "x86_64")]
use core::arch::x86_64::{
    __m128i, __m256i, _mm_or_si128, _mm_packus_epi16, _mm_set_epi64x, _mm_setzero_si128,
    _mm_shuffle_epi8, _mm_storeu_si128, _mm256_add_epi32, _mm256_castsi256_si128,
    _mm256_extracti128_si256, _mm256_loadu_si256, _mm256_madd_epi16, _mm256_max_epi16,
    _mm256_min_epi16, _mm256_mullo_epi32, _mm256_or_si256, _mm256_packs_epi32,
    _mm256_permute2x128_si256, _mm256_permute4x64_epi64, _mm256_set1_epi16, _mm256_set1_epi32,
    _mm256_setr_epi32, _mm256_setzero_si256, _mm256_slli_epi32, _mm256_srai_epi32,
    _mm256_sub_epi16, _mm256_sub_epi32, _mm256_testz_si256, _mm256_unpackhi_epi16,
    _mm256_unpackhi_epi32, _mm256_unpackhi_epi64, _mm256_unpacklo_epi16, _mm256_unpacklo_epi32,
    _mm256_unpacklo_epi64,
};

const BLOCK_SIDE: usize = 8;
const BLOCK_COEFFICIENTS: usize = BLOCK_SIDE * BLOCK_SIDE;
const CONVERSION_PIXELS: usize = 16;
const RGB_BYTES: usize = 3;

#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
mod factors {
    pub(super) const FIRST_PASS_ROUNDING: i32 = 512;
    pub(super) const SECOND_PASS_ROUNDING: i32 = 512 + 65536 + (128 << 17);
    pub(super) const FIRST_PASS_SHIFT: i32 = 10;
    pub(super) const SECOND_PASS_SHIFT: i32 = 17;
    pub(super) const EVEN_SHIFT: i32 = 12;
    pub(super) const SAMPLE_MAX: i32 = 255;
    pub(super) const FIX_0_298631336: i32 = 1223;
    pub(super) const FIX_0_390180644: i32 = 1597;
    pub(super) const FIX_0_541196100: i32 = 2217;
    pub(super) const FIX_0_765366865: i32 = 3135;
    pub(super) const FIX_0_899976223: i32 = 3685;
    pub(super) const FIX_1_175875602: i32 = 4816;
    pub(super) const FIX_1_501321110: i32 = 6149;
    pub(super) const FIX_1_847759065: i32 = 7567;
    pub(super) const FIX_1_961570560: i32 = 8034;
    pub(super) const FIX_2_053119869: i32 = 8410;
    pub(super) const FIX_2_562915447: i32 = 10497;
    pub(super) const FIX_3_072711026: i32 = 12586;
    pub(super) const Y_FACTOR: i16 = 16384;
    pub(super) const CR_FACTOR: i16 = 22970;
    pub(super) const CB_FACTOR: i16 = 29032;
    pub(super) const GREEN_CR_FACTOR: i16 = -11700;
    pub(super) const GREEN_CB_FACTOR: i16 = -5638;
    pub(super) const COLOR_PRECISION_BITS: i32 = 14;
    pub(super) const COLOR_ROUNDING: i16 = (1 << (COLOR_PRECISION_BITS - 1)) - 1;
    pub(super) const CHROMA_BIAS: i16 = 128;
}

#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
use factors::*;

type Rows<'a> = [&'a mut [i16; BLOCK_SIDE]; BLOCK_SIDE];

#[cfg_attr(
    not(any(target_arch = "x86_64", target_arch = "aarch64")),
    allow(clippy::missing_const_for_fn)
)]
fn vector_unit() -> bool {
    #[cfg(target_arch = "x86_64")]
    {
        std::is_x86_feature_detected!("avx2")
    }
    #[cfg(target_arch = "aarch64")]
    {
        std::arch::is_aarch64_feature_detected!("neon")
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    {
        false
    }
}

pub fn inverse_dct(
    coefficients: &[i32; BLOCK_COEFFICIENTS],
    output: &mut [i16],
    position: usize,
    stride: usize,
) -> bool {
    if stride < BLOCK_SIDE || !vector_unit() {
        return false;
    }
    let end = stride
        .checked_mul(BLOCK_SIDE - 1)
        .and_then(|span| span.checked_add(position))
        .and_then(|start| start.checked_add(BLOCK_SIDE));
    assert!(
        end.is_some_and(|end| end <= output.len()),
        "the block fits the output"
    );
    run_inverse_dct(coefficients, output_rows(output, position, stride));
    true
}

fn output_rows(output: &mut [i16], position: usize, stride: usize) -> Rows<'_> {
    let mut rest = &mut output[position..];
    core::array::from_fn(|row| {
        let (head, tail) = core::mem::take(&mut rest).split_at_mut(BLOCK_SIDE);
        if row + 1 < BLOCK_SIDE {
            rest = &mut tail[stride - BLOCK_SIDE..];
        }
        head.try_into().expect("a row holds eight samples")
    })
}

pub fn ycbcr_to_rgb(luma: &[i16], blue: &[i16], red: &[i16], target: &mut [u8]) -> usize {
    if !vector_unit() {
        return 0;
    }
    assert_eq!(blue.len(), luma.len(), "every plane holds the row");
    assert_eq!(red.len(), luma.len(), "every plane holds the row");
    assert!(
        luma.len()
            .checked_mul(RGB_BYTES)
            .is_some_and(|bytes| bytes <= target.len()),
        "the target holds the row"
    );
    let pixels = luma.len() / CONVERSION_PIXELS * CONVERSION_PIXELS;
    run_ycbcr_to_rgb(
        luma[..pixels].as_chunks::<CONVERSION_PIXELS>().0,
        blue[..pixels].as_chunks::<CONVERSION_PIXELS>().0,
        red[..pixels].as_chunks::<CONVERSION_PIXELS>().0,
        target[..pixels * RGB_BYTES]
            .as_chunks_mut::<{ CONVERSION_PIXELS * RGB_BYTES }>()
            .0,
    );
    pixels
}

type Pixels = [[u8; CONVERSION_PIXELS * RGB_BYTES]];
type Samples = [[i16; CONVERSION_PIXELS]];

#[cfg(target_arch = "x86_64")]
fn run_ycbcr_to_rgb(luma: &Samples, blue: &Samples, red: &Samples, target: &mut Pixels) {
    unsafe { x86::ycbcr_to_rgb(luma, blue, red, target) }
}

#[cfg(target_arch = "aarch64")]
fn run_ycbcr_to_rgb(luma: &Samples, blue: &Samples, red: &Samples, target: &mut Pixels) {
    unsafe { arm::ycbcr_to_rgb(luma, blue, red, target) }
}

#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
fn run_ycbcr_to_rgb(_: &Samples, _: &Samples, _: &Samples, _: &mut Pixels) {
    unreachable!("vector_unit is false without a vector unit");
}

#[cfg(target_arch = "x86_64")]
fn run_inverse_dct(coefficients: &[i32; BLOCK_COEFFICIENTS], rows: Rows<'_>) {
    unsafe { x86::inverse_dct(coefficients, rows) }
}

#[cfg(target_arch = "aarch64")]
fn run_inverse_dct(coefficients: &[i32; BLOCK_COEFFICIENTS], rows: Rows<'_>) {
    unsafe { arm::inverse_dct(coefficients, rows) }
}

#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
fn run_inverse_dct(_: &[i32; BLOCK_COEFFICIENTS], _: Rows<'_>) {
    unreachable!("vector_unit is false without a vector unit");
}

#[cfg(target_arch = "x86_64")]
mod x86 {
    use super::*;

    #[target_feature(enable = "avx2")]
    fn times(value: __m256i, factor: i32) -> __m256i {
        _mm256_mullo_epi32(value, _mm256_set1_epi32(factor))
    }

    #[target_feature(enable = "avx2")]
    fn butterfly<const SHIFT: i32>(input: [__m256i; 8], rounding: __m256i) -> [__m256i; 8] {
        let p1 = times(_mm256_add_epi32(input[2], input[6]), FIX_0_541196100);
        let t2 = _mm256_add_epi32(p1, times(input[6], -FIX_1_847759065));
        let t3 = _mm256_add_epi32(p1, times(input[2], FIX_0_765366865));
        let t0 = _mm256_slli_epi32::<EVEN_SHIFT>(_mm256_add_epi32(input[0], input[4]));
        let t1 = _mm256_slli_epi32::<EVEN_SHIFT>(_mm256_sub_epi32(input[0], input[4]));
        let x0 = _mm256_add_epi32(_mm256_add_epi32(t0, t3), rounding);
        let x3 = _mm256_add_epi32(_mm256_sub_epi32(t0, t3), rounding);
        let x1 = _mm256_add_epi32(_mm256_add_epi32(t1, t2), rounding);
        let x2 = _mm256_add_epi32(_mm256_sub_epi32(t1, t2), rounding);
        let p3 = _mm256_add_epi32(input[7], input[3]);
        let p4 = _mm256_add_epi32(input[5], input[1]);
        let p1 = _mm256_add_epi32(input[7], input[1]);
        let p2 = _mm256_add_epi32(input[5], input[3]);
        let p5 = times(_mm256_add_epi32(p3, p4), FIX_1_175875602);
        let t0 = times(input[7], FIX_0_298631336);
        let t1 = times(input[5], FIX_2_053119869);
        let t2 = times(input[3], FIX_3_072711026);
        let t3 = times(input[1], FIX_1_501321110);
        let p1 = _mm256_add_epi32(p5, times(p1, -FIX_0_899976223));
        let p2 = _mm256_add_epi32(p5, times(p2, -FIX_2_562915447));
        let p3 = times(p3, -FIX_1_961570560);
        let p4 = times(p4, -FIX_0_390180644);
        let t3 = _mm256_add_epi32(t3, _mm256_add_epi32(p1, p4));
        let t2 = _mm256_add_epi32(t2, _mm256_add_epi32(p2, p3));
        let t1 = _mm256_add_epi32(t1, _mm256_add_epi32(p2, p4));
        let t0 = _mm256_add_epi32(t0, _mm256_add_epi32(p1, p3));
        [
            _mm256_srai_epi32::<SHIFT>(_mm256_add_epi32(x0, t3)),
            _mm256_srai_epi32::<SHIFT>(_mm256_add_epi32(x1, t2)),
            _mm256_srai_epi32::<SHIFT>(_mm256_add_epi32(x2, t1)),
            _mm256_srai_epi32::<SHIFT>(_mm256_add_epi32(x3, t0)),
            _mm256_srai_epi32::<SHIFT>(_mm256_sub_epi32(x3, t0)),
            _mm256_srai_epi32::<SHIFT>(_mm256_sub_epi32(x2, t1)),
            _mm256_srai_epi32::<SHIFT>(_mm256_sub_epi32(x1, t2)),
            _mm256_srai_epi32::<SHIFT>(_mm256_sub_epi32(x0, t3)),
        ]
    }

    #[target_feature(enable = "avx2")]
    fn transpose(rows: [__m256i; 8]) -> [__m256i; 8] {
        let t0 = _mm256_unpacklo_epi32(rows[0], rows[1]);
        let t1 = _mm256_unpackhi_epi32(rows[0], rows[1]);
        let t2 = _mm256_unpacklo_epi32(rows[2], rows[3]);
        let t3 = _mm256_unpackhi_epi32(rows[2], rows[3]);
        let t4 = _mm256_unpacklo_epi32(rows[4], rows[5]);
        let t5 = _mm256_unpackhi_epi32(rows[4], rows[5]);
        let t6 = _mm256_unpacklo_epi32(rows[6], rows[7]);
        let t7 = _mm256_unpackhi_epi32(rows[6], rows[7]);
        let u0 = _mm256_unpacklo_epi64(t0, t2);
        let u1 = _mm256_unpackhi_epi64(t0, t2);
        let u2 = _mm256_unpacklo_epi64(t1, t3);
        let u3 = _mm256_unpackhi_epi64(t1, t3);
        let u4 = _mm256_unpacklo_epi64(t4, t6);
        let u5 = _mm256_unpackhi_epi64(t4, t6);
        let u6 = _mm256_unpacklo_epi64(t5, t7);
        let u7 = _mm256_unpackhi_epi64(t5, t7);
        [
            _mm256_permute2x128_si256::<0x20>(u0, u4),
            _mm256_permute2x128_si256::<0x20>(u1, u5),
            _mm256_permute2x128_si256::<0x20>(u2, u6),
            _mm256_permute2x128_si256::<0x20>(u3, u7),
            _mm256_permute2x128_si256::<0x31>(u0, u4),
            _mm256_permute2x128_si256::<0x31>(u1, u5),
            _mm256_permute2x128_si256::<0x31>(u2, u6),
            _mm256_permute2x128_si256::<0x31>(u3, u7),
        ]
    }

    #[target_feature(enable = "avx2")]
    pub(super) fn inverse_dct(coefficients: &[i32; BLOCK_COEFFICIENTS], rows: Rows<'_>) {
        let mut input = [_mm256_setzero_si256(); BLOCK_SIDE];
        for (vector, line) in input
            .iter_mut()
            .zip(coefficients.as_chunks::<BLOCK_SIDE>().0)
        {
            *vector = unsafe { _mm256_loadu_si256(line.as_ptr().cast()) };
        }
        let lower = _mm256_or_si256(
            _mm256_or_si256(input[4], input[5]),
            _mm256_or_si256(input[6], input[7]),
        );
        let upper = _mm256_or_si256(
            _mm256_or_si256(input[0], input[1]),
            _mm256_or_si256(input[2], input[3]),
        );
        let right_lanes = _mm256_setr_epi32(0, 0, 0, 0, -1, -1, -1, -1);
        let lower_zero = _mm256_testz_si256(lower, lower) == 1;
        let right_zero = _mm256_testz_si256(_mm256_or_si256(upper, lower), right_lanes) == 1;
        match (lower_zero, right_zero) {
            (false, false) => transform::<false, false>(input, rows),
            (true, false) => transform::<true, false>(input, rows),
            (false, true) => transform::<false, true>(input, rows),
            (true, true) => transform::<true, true>(input, rows),
        }
    }

    #[target_feature(enable = "avx2")]
    fn transform<const LOWER_ZERO: bool, const RIGHT_ZERO: bool>(
        mut input: [__m256i; BLOCK_SIDE],
        rows: Rows<'_>,
    ) {
        let zero = _mm256_setzero_si256();
        if LOWER_ZERO {
            input[4..].fill(zero);
        }
        let mut columns = transpose(butterfly::<FIRST_PASS_SHIFT>(
            input,
            _mm256_set1_epi32(FIRST_PASS_ROUNDING),
        ));
        if RIGHT_ZERO {
            columns[4..].fill(zero);
        }
        let samples = transpose(butterfly::<SECOND_PASS_SHIFT>(
            columns,
            _mm256_set1_epi32(SECOND_PASS_ROUNDING),
        ));
        let maximum = _mm256_set1_epi16(SAMPLE_MAX as i16);
        let [r0, r1, r2, r3, r4, r5, r6, r7] = rows;
        for (pair, (top, bottom)) in [(r0, r1), (r2, r3), (r4, r5), (r6, r7)]
            .into_iter()
            .enumerate()
        {
            let packed = _mm256_permute4x64_epi64::<0xD8>(_mm256_packs_epi32(
                samples[2 * pair],
                samples[2 * pair + 1],
            ));
            let clamped = _mm256_max_epi16(_mm256_min_epi16(packed, maximum), zero);
            unsafe { _mm_storeu_si128(top.as_mut_ptr().cast(), _mm256_castsi256_si128(clamped)) };
            unsafe {
                _mm_storeu_si128(
                    bottom.as_mut_ptr().cast(),
                    _mm256_extracti128_si256::<1>(clamped),
                );
            }
        }
    }

    const SHUFFLE_ZERO: u8 = 0x80;

    const fn interleave_mask(chunk: usize, channel: usize) -> [i64; 2] {
        let mut halves = [0_i64; 2];
        let mut index = 0;
        while index < CONVERSION_PIXELS {
            let position = chunk * CONVERSION_PIXELS + index;
            let byte = if position % RGB_BYTES == channel {
                (position / RGB_BYTES) as u8
            } else {
                SHUFFLE_ZERO
            };
            halves[index / 8] |= (byte as i64) << (8 * (index % 8));
            index += 1;
        }
        halves
    }

    const INTERLEAVE_MASKS: [[[i64; 2]; RGB_BYTES]; RGB_BYTES] = [
        [
            interleave_mask(0, 0),
            interleave_mask(0, 1),
            interleave_mask(0, 2),
        ],
        [
            interleave_mask(1, 0),
            interleave_mask(1, 1),
            interleave_mask(1, 2),
        ],
        [
            interleave_mask(2, 0),
            interleave_mask(2, 1),
            interleave_mask(2, 2),
        ],
    ];

    #[target_feature(enable = "avx2")]
    fn pair(low: i16, high: i16) -> __m256i {
        _mm256_set1_epi32(i32::from(low as u16) | (i32::from(high) << 16))
    }

    #[target_feature(enable = "avx2")]
    fn channel(base: [__m256i; 2], chroma: [__m256i; 2], weights: __m256i) -> __m128i {
        let low = _mm256_srai_epi32::<COLOR_PRECISION_BITS>(_mm256_add_epi32(
            base[0],
            _mm256_madd_epi16(chroma[0], weights),
        ));
        let high = _mm256_srai_epi32::<COLOR_PRECISION_BITS>(_mm256_add_epi32(
            base[1],
            _mm256_madd_epi16(chroma[1], weights),
        ));
        let words = _mm256_packs_epi32(low, high);
        _mm_packus_epi16(
            _mm256_castsi256_si128(words),
            _mm256_extracti128_si256::<1>(words),
        )
    }

    #[target_feature(enable = "avx2")]
    fn convert(
        luma: &[i16; CONVERSION_PIXELS],
        blue: &[i16; CONVERSION_PIXELS],
        red: &[i16; CONVERSION_PIXELS],
        target: &mut [u8; CONVERSION_PIXELS * RGB_BYTES],
    ) {
        let bias = _mm256_set1_epi16(CHROMA_BIAS);
        let ones = _mm256_set1_epi16(1);
        let y = unsafe { _mm256_loadu_si256(luma.as_ptr().cast()) };
        let cb = _mm256_sub_epi16(unsafe { _mm256_loadu_si256(blue.as_ptr().cast()) }, bias);
        let cr = _mm256_sub_epi16(unsafe { _mm256_loadu_si256(red.as_ptr().cast()) }, bias);
        let base_weights = pair(Y_FACTOR, COLOR_ROUNDING);
        let base = [
            _mm256_madd_epi16(_mm256_unpacklo_epi16(y, ones), base_weights),
            _mm256_madd_epi16(_mm256_unpackhi_epi16(y, ones), base_weights),
        ];
        let chroma = [_mm256_unpacklo_epi16(cr, cb), _mm256_unpackhi_epi16(cr, cb)];
        let channels = [
            channel(base, chroma, pair(CR_FACTOR, 0)),
            channel(base, chroma, pair(GREEN_CR_FACTOR, GREEN_CB_FACTOR)),
            channel(base, chroma, pair(0, CB_FACTOR)),
        ];
        for (output, masks) in target
            .as_chunks_mut::<CONVERSION_PIXELS>()
            .0
            .iter_mut()
            .zip(INTERLEAVE_MASKS)
        {
            let mut bytes = _mm_setzero_si128();
            for (channel, mask) in channels.iter().zip(masks) {
                let shuffled = _mm_shuffle_epi8(*channel, _mm_set_epi64x(mask[1], mask[0]));
                bytes = _mm_or_si128(bytes, shuffled);
            }
            unsafe { _mm_storeu_si128(output.as_mut_ptr().cast(), bytes) };
        }
    }

    #[target_feature(enable = "avx2")]
    pub(super) fn ycbcr_to_rgb(luma: &Samples, blue: &Samples, red: &Samples, target: &mut Pixels) {
        for (((luma, blue), red), target) in luma.iter().zip(blue).zip(red).zip(target) {
            convert(luma, blue, red, target);
        }
    }
}

#[cfg(target_arch = "aarch64")]
mod arm {
    use super::*;

    #[target_feature(enable = "neon")]
    fn butterfly<const SHIFT: i32>(input: [int32x4_t; 8], rounding: int32x4_t) -> [int32x4_t; 8] {
        let p1 = vmulq_n_s32(vaddq_s32(input[2], input[6]), FIX_0_541196100);
        let t2 = vaddq_s32(p1, vmulq_n_s32(input[6], -FIX_1_847759065));
        let t3 = vaddq_s32(p1, vmulq_n_s32(input[2], FIX_0_765366865));
        let t0 = vshlq_n_s32::<EVEN_SHIFT>(vaddq_s32(input[0], input[4]));
        let t1 = vshlq_n_s32::<EVEN_SHIFT>(vsubq_s32(input[0], input[4]));
        let x0 = vaddq_s32(vaddq_s32(t0, t3), rounding);
        let x3 = vaddq_s32(vsubq_s32(t0, t3), rounding);
        let x1 = vaddq_s32(vaddq_s32(t1, t2), rounding);
        let x2 = vaddq_s32(vsubq_s32(t1, t2), rounding);
        let p3 = vaddq_s32(input[7], input[3]);
        let p4 = vaddq_s32(input[5], input[1]);
        let p1 = vaddq_s32(input[7], input[1]);
        let p2 = vaddq_s32(input[5], input[3]);
        let p5 = vmulq_n_s32(vaddq_s32(p3, p4), FIX_1_175875602);
        let t0 = vmulq_n_s32(input[7], FIX_0_298631336);
        let t1 = vmulq_n_s32(input[5], FIX_2_053119869);
        let t2 = vmulq_n_s32(input[3], FIX_3_072711026);
        let t3 = vmulq_n_s32(input[1], FIX_1_501321110);
        let p1 = vaddq_s32(p5, vmulq_n_s32(p1, -FIX_0_899976223));
        let p2 = vaddq_s32(p5, vmulq_n_s32(p2, -FIX_2_562915447));
        let p3 = vmulq_n_s32(p3, -FIX_1_961570560);
        let p4 = vmulq_n_s32(p4, -FIX_0_390180644);
        let t3 = vaddq_s32(t3, vaddq_s32(p1, p4));
        let t2 = vaddq_s32(t2, vaddq_s32(p2, p3));
        let t1 = vaddq_s32(t1, vaddq_s32(p2, p4));
        let t0 = vaddq_s32(t0, vaddq_s32(p1, p3));
        [
            vshrq_n_s32::<SHIFT>(vaddq_s32(x0, t3)),
            vshrq_n_s32::<SHIFT>(vaddq_s32(x1, t2)),
            vshrq_n_s32::<SHIFT>(vaddq_s32(x2, t1)),
            vshrq_n_s32::<SHIFT>(vaddq_s32(x3, t0)),
            vshrq_n_s32::<SHIFT>(vsubq_s32(x3, t0)),
            vshrq_n_s32::<SHIFT>(vsubq_s32(x2, t1)),
            vshrq_n_s32::<SHIFT>(vsubq_s32(x1, t2)),
            vshrq_n_s32::<SHIFT>(vsubq_s32(x0, t3)),
        ]
    }

    #[target_feature(enable = "neon")]
    fn transpose_quarter(rows: [int32x4_t; 4]) -> [int32x4_t; 4] {
        let even_top = vreinterpretq_s64_s32(vtrn1q_s32(rows[0], rows[1]));
        let even_bottom = vreinterpretq_s64_s32(vtrn1q_s32(rows[2], rows[3]));
        let odd_top = vreinterpretq_s64_s32(vtrn2q_s32(rows[0], rows[1]));
        let odd_bottom = vreinterpretq_s64_s32(vtrn2q_s32(rows[2], rows[3]));
        [
            vreinterpretq_s32_s64(vzip1q_s64(even_top, even_bottom)),
            vreinterpretq_s32_s64(vzip1q_s64(odd_top, odd_bottom)),
            vreinterpretq_s32_s64(vzip2q_s64(even_top, even_bottom)),
            vreinterpretq_s32_s64(vzip2q_s64(odd_top, odd_bottom)),
        ]
    }

    #[target_feature(enable = "neon")]
    fn transpose(low: [int32x4_t; 8], high: [int32x4_t; 8]) -> ([int32x4_t; 8], [int32x4_t; 8]) {
        let top_left = transpose_quarter([low[0], low[1], low[2], low[3]]);
        let bottom_left = transpose_quarter([low[4], low[5], low[6], low[7]]);
        let top_right = transpose_quarter([high[0], high[1], high[2], high[3]]);
        let bottom_right = transpose_quarter([high[4], high[5], high[6], high[7]]);
        (
            [
                top_left[0],
                top_left[1],
                top_left[2],
                top_left[3],
                top_right[0],
                top_right[1],
                top_right[2],
                top_right[3],
            ],
            [
                bottom_left[0],
                bottom_left[1],
                bottom_left[2],
                bottom_left[3],
                bottom_right[0],
                bottom_right[1],
                bottom_right[2],
                bottom_right[3],
            ],
        )
    }

    #[target_feature(enable = "neon")]
    fn is_zero(values: &[int32x4_t]) -> bool {
        let mut combined = vdupq_n_s32(0);
        for value in values {
            combined = vorrq_s32(combined, *value);
        }
        vmaxvq_u32(vreinterpretq_u32_s32(combined)) == 0
    }

    #[target_feature(enable = "neon")]
    pub(super) fn inverse_dct(coefficients: &[i32; BLOCK_COEFFICIENTS], rows: Rows<'_>) {
        let lines = coefficients.as_chunks::<4>().0;
        let mut low = [vdupq_n_s32(0); BLOCK_SIDE];
        let mut high = [vdupq_n_s32(0); BLOCK_SIDE];
        for (row, (low, high)) in low.iter_mut().zip(&mut high).enumerate() {
            *low = unsafe { vld1q_s32(lines[2 * row].as_ptr()) };
            *high = unsafe { vld1q_s32(lines[2 * row + 1].as_ptr()) };
        }
        let lower_zero = is_zero(&low[4..]) && is_zero(&high[4..]);
        let right_zero = is_zero(&high);
        match (lower_zero, right_zero) {
            (false, false) => transform::<false, false>(low, high, rows),
            (true, false) => transform::<true, false>(low, high, rows),
            (false, true) => transform::<false, true>(low, high, rows),
            (true, true) => transform::<true, true>(low, high, rows),
        }
    }

    #[target_feature(enable = "neon")]
    fn transform<const LOWER_ZERO: bool, const RIGHT_ZERO: bool>(
        mut low: [int32x4_t; BLOCK_SIDE],
        mut high: [int32x4_t; BLOCK_SIDE],
        rows: Rows<'_>,
    ) {
        let zero = vdupq_n_s32(0);
        if LOWER_ZERO {
            low[4..].fill(zero);
            high[4..].fill(zero);
        }
        let first = vdupq_n_s32(FIRST_PASS_ROUNDING);
        let low = butterfly::<FIRST_PASS_SHIFT>(low, first);
        let high = if RIGHT_ZERO {
            [zero; BLOCK_SIDE]
        } else {
            butterfly::<FIRST_PASS_SHIFT>(high, first)
        };
        let (low, high) = transpose(low, high);
        let second = vdupq_n_s32(SECOND_PASS_ROUNDING);
        let (low, high) = transpose(
            butterfly::<SECOND_PASS_SHIFT>(low, second),
            butterfly::<SECOND_PASS_SHIFT>(high, second),
        );
        let maximum = vdupq_n_u16(SAMPLE_MAX as u16);
        for (row, target) in rows.into_iter().enumerate() {
            let samples = vminq_u16(
                vcombine_u16(vqmovun_s32(low[row]), vqmovun_s32(high[row])),
                maximum,
            );
            unsafe { vst1q_s16(target.as_mut_ptr(), vreinterpretq_s16_u16(samples)) };
        }
    }

    #[target_feature(enable = "neon")]
    fn narrow(low: int32x4_t, high: int32x4_t) -> uint8x8_t {
        vqmovun_s16(vcombine_s16(
            vqshrn_n_s32::<COLOR_PRECISION_BITS>(low),
            vqshrn_n_s32::<COLOR_PRECISION_BITS>(high),
        ))
    }

    #[target_feature(enable = "neon")]
    fn convert(luma: &[i16; 8], blue: &[i16; 8], red: &[i16; 8]) -> [uint8x8_t; RGB_BYTES] {
        let bias = vdupq_n_s16(CHROMA_BIAS);
        let rounding = vdupq_n_s32(i32::from(COLOR_ROUNDING));
        let y = unsafe { vld1q_s16(luma.as_ptr()) };
        let cb: int16x8_t = vsubq_s16(unsafe { vld1q_s16(blue.as_ptr()) }, bias);
        let cr: int16x8_t = vsubq_s16(unsafe { vld1q_s16(red.as_ptr()) }, bias);
        let base_low = vmlal_n_s16(rounding, vget_low_s16(y), Y_FACTOR);
        let base_high = vmlal_high_n_s16(rounding, y, Y_FACTOR);
        let red_low = vmlal_n_s16(base_low, vget_low_s16(cr), CR_FACTOR);
        let red_high = vmlal_high_n_s16(base_high, cr, CR_FACTOR);
        let green_low = vmlal_n_s16(
            vmlal_n_s16(base_low, vget_low_s16(cr), GREEN_CR_FACTOR),
            vget_low_s16(cb),
            GREEN_CB_FACTOR,
        );
        let green_high = vmlal_high_n_s16(
            vmlal_high_n_s16(base_high, cr, GREEN_CR_FACTOR),
            cb,
            GREEN_CB_FACTOR,
        );
        let blue_low = vmlal_n_s16(base_low, vget_low_s16(cb), CB_FACTOR);
        let blue_high = vmlal_high_n_s16(base_high, cb, CB_FACTOR);
        [
            narrow(red_low, red_high),
            narrow(green_low, green_high),
            narrow(blue_low, blue_high),
        ]
    }

    #[target_feature(enable = "neon")]
    pub(super) fn ycbcr_to_rgb(luma: &Samples, blue: &Samples, red: &Samples, target: &mut Pixels) {
        for (((luma, blue), red), target) in luma.iter().zip(blue).zip(red).zip(target) {
            let ([luma_low, luma_high], []) = luma.as_chunks::<8>() else {
                unreachable!("sixteen samples are two halves of eight");
            };
            let ([blue_low, blue_high], []) = blue.as_chunks::<8>() else {
                unreachable!("sixteen samples are two halves of eight");
            };
            let ([red_low, red_high], []) = red.as_chunks::<8>() else {
                unreachable!("sixteen samples are two halves of eight");
            };
            let low = convert(luma_low, blue_low, red_low);
            let high = convert(luma_high, blue_high, red_high);
            let pixels = uint8x16x3_t(
                vcombine_u8(low[0], high[0]),
                vcombine_u8(low[1], high[1]),
                vcombine_u8(low[2], high[2]),
            );
            unsafe { vst3q_u8(target.as_mut_ptr(), pixels) };
        }
    }
}
