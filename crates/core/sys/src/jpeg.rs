#[cfg(target_arch = "aarch64")]
use core::arch::aarch64::{
    int16x8_t, int32x4_t, uint8x8_t, uint8x16x3_t, vaddq_s32, vcgeq_f32, vcleq_f32, vcombine_s16,
    vcombine_u8, vcombine_u16, vcreate_u8, vcvtq_f32_s32, vcvtq_s32_f32, vdivq_f32, vdupq_n_f32,
    vdupq_n_s16, vdupq_n_s32, vdupq_n_u16, vget_lane_u64, vget_low_s16, vld1q_s16, vld1q_s32,
    vld3_u8, vmaxvq_u32, vminq_u16, vmlal_high_n_s16, vmlal_n_s16, vmlaq_n_s32, vmlsq_n_s32,
    vmovl_high_s16, vmovl_s16, vmovl_u8, vmovn_s32, vmulq_n_s32, vorrq_s32, vqmovun_s16,
    vqmovun_s32, vqshrn_n_s32, vreinterpret_u64_u8, vreinterpretq_s16_u16, vreinterpretq_s32_s64,
    vreinterpretq_s32_u32, vreinterpretq_s64_s32, vreinterpretq_u32_s32, vshlq_n_s32, vshrq_n_s32,
    vshrq_n_u32, vst1q_s16, vst1q_s32, vst3q_u8, vsubq_f32, vsubq_s16, vsubq_s32, vtrn1q_s32,
    vtrn2q_s32, vzip1q_s64, vzip2q_s64,
};
#[cfg(target_arch = "x86_64")]
use core::arch::x86_64::{
    __m128i, __m256i, _CMP_GE_OQ, _CMP_LE_OQ, _mm_loadu_si128, _mm_or_si128, _mm_packus_epi16,
    _mm_set_epi64x, _mm_setzero_si128, _mm_shuffle_epi8, _mm_storeu_si128, _mm256_add_epi16,
    _mm256_add_epi32, _mm256_castps_si256, _mm256_castsi256_si128, _mm256_cmp_ps,
    _mm256_cvtepi32_ps, _mm256_cvtepu8_epi32, _mm256_cvttps_epi32, _mm256_div_ps,
    _mm256_extract_epi64, _mm256_extracti128_si256, _mm256_loadu_si256, _mm256_madd_epi16,
    _mm256_max_epi16, _mm256_min_epi16, _mm256_mullo_epi16, _mm256_mullo_epi32, _mm256_or_si256,
    _mm256_packs_epi32, _mm256_packus_epi16, _mm256_packus_epi32, _mm256_permute2x128_si256,
    _mm256_permute4x64_epi64, _mm256_permutevar8x32_epi32, _mm256_set1_epi16, _mm256_set1_epi32,
    _mm256_set1_ps, _mm256_setr_epi32, _mm256_setzero_si256, _mm256_slli_epi32, _mm256_srai_epi16,
    _mm256_srai_epi32, _mm256_srli_epi32, _mm256_storeu_si256, _mm256_sub_epi16, _mm256_sub_epi32,
    _mm256_sub_ps, _mm256_testz_si256, _mm256_unpackhi_epi16, _mm256_unpackhi_epi32,
    _mm256_unpackhi_epi64, _mm256_unpacklo_epi16, _mm256_unpacklo_epi32, _mm256_unpacklo_epi64,
};

const BLOCK_SIDE: usize = 8;
const BLOCK_COEFFICIENTS: usize = BLOCK_SIDE * BLOCK_SIDE;
const CONVERSION_PIXELS: usize = 16;
const RGB_BYTES: usize = 3;
const ENCODE_ROW_BYTES: usize = BLOCK_SIDE * RGB_BYTES;
const PLANES: usize = 3;
const UPSAMPLE_CENTERS: usize = 16;
#[cfg(target_arch = "x86_64")]
const UPSAMPLE_OUTPUTS: usize = 2 * UPSAMPLE_CENTERS;
#[cfg(target_arch = "x86_64")]
const UPSAMPLE_WINDOW: usize = UPSAMPLE_CENTERS + 2;

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
    pub(super) const FORWARD_FIX_0_298631336: i32 = 2446;
    pub(super) const FORWARD_FIX_0_390180644: i32 = 3196;
    pub(super) const FORWARD_FIX_0_541196100: i32 = 4433;
    pub(super) const FORWARD_FIX_0_765366865: i32 = 6270;
    pub(super) const FORWARD_FIX_0_899976223: i32 = 7373;
    pub(super) const FORWARD_FIX_1_175875602: i32 = 9633;
    pub(super) const FORWARD_FIX_1_501321110: i32 = 12_299;
    pub(super) const FORWARD_FIX_1_847759065: i32 = 15_137;
    pub(super) const FORWARD_FIX_1_961570560: i32 = 16_069;
    pub(super) const FORWARD_FIX_2_053119869: i32 = 16_819;
    pub(super) const FORWARD_FIX_2_562915447: i32 = 20_995;
    pub(super) const FORWARD_FIX_3_072711026: i32 = 25_172;
    pub(super) const FORWARD_CONSTANT_BITS: i32 = 13;
    pub(super) const FORWARD_PASS_BITS: i32 = 2;
    pub(super) const FORWARD_DC_BIAS: i32 = 8 * 128;
    pub(super) const FORWARD_FIRST_SHIFT: i32 = FORWARD_CONSTANT_BITS - FORWARD_PASS_BITS;
    pub(super) const FORWARD_SECOND_SHIFT: i32 = FORWARD_CONSTANT_BITS + FORWARD_PASS_BITS;
    pub(super) const FORWARD_ODD_ROUNDING: i32 = 1 << (FORWARD_FIRST_SHIFT - 1);
    pub(super) const FORWARD_SECOND_ROUNDING: i32 = 1 << (FORWARD_SECOND_SHIFT - 1);
    pub(super) const FORWARD_SECOND_DC_ROUNDING: i32 = 1 << (FORWARD_PASS_BITS - 1);
    pub(super) const QUANTIZE_SHIFT: i32 = 3;
    pub(super) const QUANTIZE_HALF: f32 = 0.5;
    pub(super) const SIGN_TO_DIVISOR_SHIFT: i32 = 32 - QUANTIZE_SHIFT;
    pub(super) const LUMA_RED: i32 = 19595;
    pub(super) const LUMA_GREEN: i32 = 38469;
    pub(super) const LUMA_BLUE: i32 = 7471;
    pub(super) const LUMA_ROUNDING: i32 = (1 << 15) - 1;
    pub(super) const BLUE_RED: i32 = 11059;
    pub(super) const BLUE_GREEN: i32 = 21709;
    pub(super) const CHROMA_HALF: i32 = 32768;
    pub(super) const CHROMA_ROUNDING: i32 = (128 * (1 << 16)) + ((1 << 15) - 1);
    pub(super) const RED_GREEN: i32 = 27439;
    pub(super) const RED_BLUE: i32 = 5329;
    pub(super) const ENCODE_PRECISION_BITS: i32 = 16;
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

pub fn upsample_horizontal(input: &[i16], output: &mut [i16]) -> usize {
    if !cfg!(target_arch = "x86_64") || !vector_unit() {
        return 0;
    }
    assert_eq!(
        Some(output.len()),
        input.len().checked_mul(2),
        "the output holds two samples per input sample"
    );
    let centers = input.len().saturating_sub(2) / UPSAMPLE_CENTERS * UPSAMPLE_CENTERS;
    if centers == 0 {
        return 0;
    }
    run_upsample_horizontal(&input[..centers + 2], &mut output[2..2 + 2 * centers]);
    centers
}

#[cfg(target_arch = "x86_64")]
fn run_upsample_horizontal(input: &[i16], output: &mut [i16]) {
    unsafe { x86::upsample_horizontal(input, output) }
}

#[cfg(not(target_arch = "x86_64"))]
fn run_upsample_horizontal(_: &[i16], _: &mut [i16]) {
    unreachable!("only the x86_64 kernel is called");
}

pub fn upsample_vertical(input: &[i16], near: &[i16], far: &[i16], output: &mut [i16]) -> usize {
    if !cfg!(target_arch = "x86_64") || !vector_unit() {
        return 0;
    }
    assert_eq!(
        near.len(),
        input.len(),
        "the near row matches the input row"
    );
    assert_eq!(far.len(), input.len(), "the far row matches the input row");
    assert_eq!(
        Some(output.len()),
        input.len().checked_mul(2),
        "the output holds two rows"
    );
    let samples = input.len() / UPSAMPLE_CENTERS * UPSAMPLE_CENTERS;
    if samples == 0 {
        return 0;
    }
    let (top, bottom) = output.split_at_mut(input.len());
    run_upsample_vertical(&input[..samples], &near[..samples], &mut top[..samples]);
    run_upsample_vertical(&input[..samples], &far[..samples], &mut bottom[..samples]);
    samples
}

#[cfg(target_arch = "x86_64")]
fn run_upsample_vertical(input: &[i16], neighbour: &[i16], output: &mut [i16]) {
    unsafe {
        x86::upsample_vertical(
            input.as_chunks::<UPSAMPLE_CENTERS>().0,
            neighbour.as_chunks::<UPSAMPLE_CENTERS>().0,
            output.as_chunks_mut::<UPSAMPLE_CENTERS>().0,
        );
    }
}

#[cfg(not(target_arch = "x86_64"))]
fn run_upsample_vertical(_: &[i16], _: &[i16], _: &mut [i16]) {
    unreachable!("only the x86_64 kernel is called");
}

#[cfg(target_arch = "x86_64")]
fn lanes<const COUNT: usize>(window: &[i16; UPSAMPLE_WINDOW], start: usize) -> &[i16; COUNT] {
    window[start..start + COUNT]
        .try_into()
        .expect("the lanes lie inside the window")
}

#[cfg(target_arch = "x86_64")]
fn upsample_windows<'a>(
    input: &'a [i16],
    output: &'a mut [i16],
) -> impl Iterator<Item = (&'a [i16; UPSAMPLE_WINDOW], &'a mut [i16; UPSAMPLE_OUTPUTS])> {
    output
        .as_chunks_mut::<UPSAMPLE_OUTPUTS>()
        .0
        .iter_mut()
        .enumerate()
        .map(move |(index, target)| {
            let start = index * UPSAMPLE_CENTERS;
            let window: &[i16; UPSAMPLE_WINDOW] = input[start..start + UPSAMPLE_WINDOW]
                .try_into()
                .expect("the window holds the centers and both neighbours");
            (window, target)
        })
}

pub fn forward_dct_quantize(
    samples: &[u8; BLOCK_COEFFICIENTS],
    table: &[u8; BLOCK_COEFFICIENTS],
    output: &mut [i32; BLOCK_COEFFICIENTS],
) -> bool {
    if !vector_unit() {
        return false;
    }
    assert!(
        table.iter().all(|&factor| factor > 0),
        "quantization factors are at least one"
    );
    run_forward_dct_quantize(samples, table, output);
    true
}

pub fn rgb_to_ycbcr(
    rows: &[[u8; ENCODE_ROW_BYTES]; BLOCK_SIDE],
    planes: &mut [[u8; BLOCK_COEFFICIENTS]; PLANES],
) -> bool {
    if !vector_unit() {
        return false;
    }
    run_rgb_to_ycbcr(rows, planes);
    true
}

#[cfg(target_arch = "x86_64")]
fn run_forward_dct_quantize(
    samples: &[u8; BLOCK_COEFFICIENTS],
    table: &[u8; BLOCK_COEFFICIENTS],
    output: &mut [i32; BLOCK_COEFFICIENTS],
) {
    unsafe { x86::forward_dct_quantize(samples, table, output) }
}

#[cfg(target_arch = "aarch64")]
fn run_forward_dct_quantize(
    samples: &[u8; BLOCK_COEFFICIENTS],
    table: &[u8; BLOCK_COEFFICIENTS],
    output: &mut [i32; BLOCK_COEFFICIENTS],
) {
    unsafe { arm::forward_dct_quantize(samples, table, output) }
}

#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
fn run_forward_dct_quantize(
    _: &[u8; BLOCK_COEFFICIENTS],
    _: &[u8; BLOCK_COEFFICIENTS],
    _: &mut [i32; BLOCK_COEFFICIENTS],
) {
    unreachable!("vector_unit is false without a vector unit");
}

#[cfg(target_arch = "x86_64")]
fn run_rgb_to_ycbcr(
    rows: &[[u8; ENCODE_ROW_BYTES]; BLOCK_SIDE],
    planes: &mut [[u8; BLOCK_COEFFICIENTS]; PLANES],
) {
    unsafe { x86::rgb_to_ycbcr(rows, planes) }
}

#[cfg(target_arch = "aarch64")]
fn run_rgb_to_ycbcr(
    rows: &[[u8; ENCODE_ROW_BYTES]; BLOCK_SIDE],
    planes: &mut [[u8; BLOCK_COEFFICIENTS]; PLANES],
) {
    unsafe { arm::rgb_to_ycbcr(rows, planes) }
}

#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
fn run_rgb_to_ycbcr(
    _: &[[u8; ENCODE_ROW_BYTES]; BLOCK_SIDE],
    _: &mut [[u8; BLOCK_COEFFICIENTS]; PLANES],
) {
    unreachable!("vector_unit is false without a vector unit");
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
    pub(super) fn upsample_vertical(
        input: &[[i16; UPSAMPLE_CENTERS]],
        neighbour: &[[i16; UPSAMPLE_CENTERS]],
        output: &mut [[i16; UPSAMPLE_CENTERS]],
    ) {
        let three = _mm256_set1_epi16(3);
        let two = _mm256_set1_epi16(2);
        for ((input, neighbour), output) in input.iter().zip(neighbour).zip(output) {
            let sample = unsafe { _mm256_loadu_si256(input.as_ptr().cast()) };
            let other = unsafe { _mm256_loadu_si256(neighbour.as_ptr().cast()) };
            let sum = _mm256_add_epi16(
                _mm256_add_epi16(_mm256_mullo_epi16(sample, three), two),
                other,
            );
            unsafe { _mm256_storeu_si256(output.as_mut_ptr().cast(), _mm256_srai_epi16::<2>(sum)) };
        }
    }

    #[target_feature(enable = "avx2")]
    pub(super) fn upsample_horizontal(input: &[i16], output: &mut [i16]) {
        let three = _mm256_set1_epi16(3);
        let two = _mm256_set1_epi16(2);
        for (window, target) in upsample_windows(input, output) {
            let left =
                unsafe { _mm256_loadu_si256(lanes::<UPSAMPLE_CENTERS>(window, 0).as_ptr().cast()) };
            let center =
                unsafe { _mm256_loadu_si256(lanes::<UPSAMPLE_CENTERS>(window, 1).as_ptr().cast()) };
            let right =
                unsafe { _mm256_loadu_si256(lanes::<UPSAMPLE_CENTERS>(window, 2).as_ptr().cast()) };
            let base = _mm256_add_epi16(_mm256_mullo_epi16(center, three), two);
            let even = _mm256_srai_epi16::<2>(_mm256_add_epi16(base, left));
            let odd = _mm256_srai_epi16::<2>(_mm256_add_epi16(base, right));
            let low = _mm256_unpacklo_epi16(even, odd);
            let high = _mm256_unpackhi_epi16(even, odd);
            let (first, second) = target.as_chunks_mut::<UPSAMPLE_CENTERS>().0.split_at_mut(1);
            unsafe {
                _mm256_storeu_si256(
                    first[0].as_mut_ptr().cast(),
                    _mm256_permute2x128_si256::<0x20>(low, high),
                );
                _mm256_storeu_si256(
                    second[0].as_mut_ptr().cast(),
                    _mm256_permute2x128_si256::<0x31>(low, high),
                );
            }
        }
    }

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

    #[target_feature(enable = "avx2")]
    fn odd_part(input: [__m256i; 4], rounding: __m256i) -> [__m256i; 4] {
        let [t0, t1, t2, t3] = input;
        let t12 = _mm256_add_epi32(t0, t2);
        let t13 = _mm256_add_epi32(t1, t3);
        let z1 = _mm256_add_epi32(
            times(_mm256_add_epi32(t12, t13), FORWARD_FIX_1_175875602),
            rounding,
        );
        let t12 = _mm256_add_epi32(times(t12, -FORWARD_FIX_0_390180644), z1);
        let t13 = _mm256_add_epi32(times(t13, -FORWARD_FIX_1_961570560), z1);
        let z1 = times(_mm256_add_epi32(t0, t3), -FORWARD_FIX_0_899976223);
        let o0 = _mm256_add_epi32(
            _mm256_add_epi32(times(t0, FORWARD_FIX_1_501321110), z1),
            t12,
        );
        let o3 = _mm256_add_epi32(
            _mm256_add_epi32(times(t3, FORWARD_FIX_0_298631336), z1),
            t13,
        );
        let z1 = times(_mm256_add_epi32(t1, t2), -FORWARD_FIX_2_562915447);
        let o1 = _mm256_add_epi32(
            _mm256_add_epi32(times(t1, FORWARD_FIX_3_072711026), z1),
            t13,
        );
        let o2 = _mm256_add_epi32(
            _mm256_add_epi32(times(t2, FORWARD_FIX_2_053119869), z1),
            t12,
        );
        [o0, o1, o2, o3]
    }

    #[target_feature(enable = "avx2")]
    fn forward_pass<const FIRST: bool>(input: [__m256i; 8]) -> [__m256i; 8] {
        let t0 = _mm256_add_epi32(input[0], input[7]);
        let t1 = _mm256_add_epi32(input[1], input[6]);
        let t2 = _mm256_add_epi32(input[2], input[5]);
        let t3 = _mm256_add_epi32(input[3], input[4]);
        let t10 = _mm256_add_epi32(t0, t3);
        let t12 = _mm256_sub_epi32(t0, t3);
        let t11 = _mm256_add_epi32(t1, t2);
        let t13 = _mm256_sub_epi32(t1, t2);
        let differences = [
            _mm256_sub_epi32(input[0], input[7]),
            _mm256_sub_epi32(input[1], input[6]),
            _mm256_sub_epi32(input[2], input[5]),
            _mm256_sub_epi32(input[3], input[4]),
        ];
        let odd = odd_part(differences, _mm256_set1_epi32(FORWARD_ODD_ROUNDING));
        let even_product = times(_mm256_add_epi32(t12, t13), FORWARD_FIX_0_541196100);
        let mut output = [_mm256_setzero_si256(); 8];
        if FIRST {
            let sum = _mm256_sub_epi32(
                _mm256_add_epi32(t10, t11),
                _mm256_set1_epi32(FORWARD_DC_BIAS),
            );
            output[0] = _mm256_slli_epi32::<FORWARD_PASS_BITS>(sum);
            output[4] = _mm256_slli_epi32::<FORWARD_PASS_BITS>(_mm256_sub_epi32(t10, t11));
            let z1 = _mm256_add_epi32(even_product, _mm256_set1_epi32(FORWARD_ODD_ROUNDING));
            output[2] = _mm256_srai_epi32::<FORWARD_FIRST_SHIFT>(_mm256_add_epi32(
                z1,
                times(t12, FORWARD_FIX_0_765366865),
            ));
            output[6] = _mm256_srai_epi32::<FORWARD_FIRST_SHIFT>(_mm256_sub_epi32(
                z1,
                times(t13, FORWARD_FIX_1_847759065),
            ));
            output[1] = _mm256_srai_epi32::<FORWARD_FIRST_SHIFT>(odd[0]);
            output[3] = _mm256_srai_epi32::<FORWARD_FIRST_SHIFT>(odd[1]);
            output[5] = _mm256_srai_epi32::<FORWARD_FIRST_SHIFT>(odd[2]);
            output[7] = _mm256_srai_epi32::<FORWARD_FIRST_SHIFT>(odd[3]);
        } else {
            let t10 = _mm256_add_epi32(t10, _mm256_set1_epi32(FORWARD_SECOND_DC_ROUNDING));
            output[0] = _mm256_srai_epi32::<FORWARD_PASS_BITS>(_mm256_add_epi32(t10, t11));
            output[4] = _mm256_srai_epi32::<FORWARD_PASS_BITS>(_mm256_sub_epi32(t10, t11));
            let z1 = _mm256_add_epi32(even_product, _mm256_set1_epi32(FORWARD_SECOND_ROUNDING));
            output[2] = _mm256_srai_epi32::<FORWARD_SECOND_SHIFT>(_mm256_add_epi32(
                z1,
                times(t12, FORWARD_FIX_0_765366865),
            ));
            output[6] = _mm256_srai_epi32::<FORWARD_SECOND_SHIFT>(_mm256_sub_epi32(
                z1,
                times(t13, FORWARD_FIX_1_847759065),
            ));
            output[1] = _mm256_srai_epi32::<FORWARD_SECOND_SHIFT>(odd[0]);
            output[3] = _mm256_srai_epi32::<FORWARD_SECOND_SHIFT>(odd[1]);
            output[5] = _mm256_srai_epi32::<FORWARD_SECOND_SHIFT>(odd[2]);
            output[7] = _mm256_srai_epi32::<FORWARD_SECOND_SHIFT>(odd[3]);
        }
        output
    }

    #[target_feature(enable = "avx2")]
    fn widen(bytes: &[u8; BLOCK_SIDE]) -> __m256i {
        _mm256_cvtepu8_epi32(_mm_set_epi64x(0, i64::from_le_bytes(*bytes)))
    }

    #[target_feature(enable = "avx2")]
    fn quantize(coefficients: __m256i, factors: &[u8; BLOCK_SIDE]) -> __m256i {
        let sign =
            _mm256_srli_epi32::<SIGN_TO_DIVISOR_SHIFT>(_mm256_srai_epi32::<31>(coefficients));
        let divided = _mm256_srai_epi32::<QUANTIZE_SHIFT>(_mm256_add_epi32(coefficients, sign));
        let value = _mm256_div_ps(
            _mm256_cvtepi32_ps(divided),
            _mm256_cvtepi32_ps(widen(factors)),
        );
        let whole = _mm256_cvttps_epi32(value);
        let fraction = _mm256_sub_ps(value, _mm256_cvtepi32_ps(whole));
        let up = _mm256_castps_si256(_mm256_cmp_ps::<_CMP_GE_OQ>(
            fraction,
            _mm256_set1_ps(QUANTIZE_HALF),
        ));
        let down = _mm256_castps_si256(_mm256_cmp_ps::<_CMP_LE_OQ>(
            fraction,
            _mm256_set1_ps(-QUANTIZE_HALF),
        ));
        _mm256_add_epi32(_mm256_sub_epi32(whole, up), down)
    }

    #[target_feature(enable = "avx2")]
    pub(super) fn forward_dct_quantize(
        samples: &[u8; BLOCK_COEFFICIENTS],
        table: &[u8; BLOCK_COEFFICIENTS],
        output: &mut [i32; BLOCK_COEFFICIENTS],
    ) {
        let mut rows = [_mm256_setzero_si256(); BLOCK_SIDE];
        for (vector, line) in rows.iter_mut().zip(samples.as_chunks::<BLOCK_SIDE>().0) {
            *vector = widen(line);
        }
        let first = forward_pass::<true>(transpose(rows));
        let second = forward_pass::<false>(transpose(first));
        for ((vector, factors), target) in second
            .into_iter()
            .zip(table.as_chunks::<BLOCK_SIDE>().0)
            .zip(output.as_chunks_mut::<BLOCK_SIDE>().0)
        {
            let quantized = quantize(vector, factors);
            unsafe { _mm256_storeu_si256(target.as_mut_ptr().cast(), quantized) };
        }
    }

    const fn deinterleave_mask(channel: usize, from_high: bool) -> i64 {
        let mut mask = 0_i64;
        let mut index = 0;
        while index < BLOCK_SIDE {
            let position = index * RGB_BYTES + channel;
            let byte = if from_high && position >= CONVERSION_PIXELS {
                (position - CONVERSION_PIXELS) as u8
            } else if !from_high && position < CONVERSION_PIXELS {
                position as u8
            } else {
                SHUFFLE_ZERO
            };
            mask |= (byte as i64) << (8 * index);
            index += 1;
        }
        mask
    }

    const DEINTERLEAVE_MASKS: [[i64; 2]; RGB_BYTES] = [
        [deinterleave_mask(0, false), deinterleave_mask(0, true)],
        [deinterleave_mask(1, false), deinterleave_mask(1, true)],
        [deinterleave_mask(2, false), deinterleave_mask(2, true)],
    ];

    #[target_feature(enable = "avx2")]
    fn weighted(
        channels: [__m256i; RGB_BYTES],
        weights: [i32; RGB_BYTES],
        rounding: i32,
    ) -> __m256i {
        let sum = _mm256_add_epi32(
            _mm256_add_epi32(
                times(channels[0], weights[0]),
                times(channels[1], weights[1]),
            ),
            _mm256_add_epi32(times(channels[2], weights[2]), _mm256_set1_epi32(rounding)),
        );
        _mm256_srai_epi32::<ENCODE_PRECISION_BITS>(sum)
    }

    #[target_feature(enable = "avx2")]
    pub(super) fn rgb_to_ycbcr(
        rows: &[[u8; ENCODE_ROW_BYTES]; BLOCK_SIDE],
        planes: &mut [[u8; BLOCK_COEFFICIENTS]; PLANES],
    ) {
        let order = _mm256_setr_epi32(0, 4, 1, 5, 2, 6, 3, 7);
        for (row, pixels) in rows.iter().enumerate() {
            let (head, tail) = pixels.split_at(CONVERSION_PIXELS);
            let head: &[u8; CONVERSION_PIXELS] =
                head.try_into().expect("a row starts with sixteen bytes");
            let tail: &[u8; BLOCK_SIDE] = tail.try_into().expect("a row ends with eight bytes");
            let low = unsafe { _mm_loadu_si128(head.as_ptr().cast()) };
            let high = _mm_set_epi64x(0, i64::from_le_bytes(*tail));
            let mut channels = [_mm256_setzero_si256(); RGB_BYTES];
            for (channel, masks) in channels.iter_mut().zip(DEINTERLEAVE_MASKS) {
                let bytes = _mm_or_si128(
                    _mm_shuffle_epi8(low, _mm_set_epi64x(0, masks[0])),
                    _mm_shuffle_epi8(high, _mm_set_epi64x(0, masks[1])),
                );
                *channel = _mm256_cvtepu8_epi32(bytes);
            }
            let luma = weighted(channels, [LUMA_RED, LUMA_GREEN, LUMA_BLUE], LUMA_ROUNDING);
            let blue = weighted(
                channels,
                [-BLUE_RED, -BLUE_GREEN, CHROMA_HALF],
                CHROMA_ROUNDING,
            );
            let red = weighted(
                channels,
                [CHROMA_HALF, -RED_GREEN, -RED_BLUE],
                CHROMA_ROUNDING,
            );
            let words = _mm256_packus_epi16(
                _mm256_packus_epi32(luma, blue),
                _mm256_packus_epi32(red, _mm256_setzero_si256()),
            );
            let bytes = _mm256_permutevar8x32_epi32(words, order);
            let lanes = [
                _mm256_extract_epi64::<0>(bytes),
                _mm256_extract_epi64::<1>(bytes),
                _mm256_extract_epi64::<2>(bytes),
            ];
            for (plane, lane) in planes.iter_mut().zip(lanes) {
                plane[row * BLOCK_SIDE..(row + 1) * BLOCK_SIDE]
                    .copy_from_slice(&lane.to_le_bytes());
            }
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

    #[target_feature(enable = "neon")]
    fn odd_part(input: [int32x4_t; 4], rounding: int32x4_t) -> [int32x4_t; 4] {
        let [t0, t1, t2, t3] = input;
        let t12 = vaddq_s32(t0, t2);
        let t13 = vaddq_s32(t1, t3);
        let z1 = vmlaq_n_s32(rounding, vaddq_s32(t12, t13), FORWARD_FIX_1_175875602);
        let t12 = vmlaq_n_s32(z1, t12, -FORWARD_FIX_0_390180644);
        let t13 = vmlaq_n_s32(z1, t13, -FORWARD_FIX_1_961570560);
        let z1 = vmulq_n_s32(vaddq_s32(t0, t3), -FORWARD_FIX_0_899976223);
        let o0 = vaddq_s32(vmlaq_n_s32(z1, t0, FORWARD_FIX_1_501321110), t12);
        let o3 = vaddq_s32(vmlaq_n_s32(z1, t3, FORWARD_FIX_0_298631336), t13);
        let z1 = vmulq_n_s32(vaddq_s32(t1, t2), -FORWARD_FIX_2_562915447);
        let o1 = vaddq_s32(vmlaq_n_s32(z1, t1, FORWARD_FIX_3_072711026), t13);
        let o2 = vaddq_s32(vmlaq_n_s32(z1, t2, FORWARD_FIX_2_053119869), t12);
        [o0, o1, o2, o3]
    }

    #[target_feature(enable = "neon")]
    fn forward_pass<const FIRST: bool>(input: [int32x4_t; 8]) -> [int32x4_t; 8] {
        let t0 = vaddq_s32(input[0], input[7]);
        let t1 = vaddq_s32(input[1], input[6]);
        let t2 = vaddq_s32(input[2], input[5]);
        let t3 = vaddq_s32(input[3], input[4]);
        let t10 = vaddq_s32(t0, t3);
        let t12 = vsubq_s32(t0, t3);
        let t11 = vaddq_s32(t1, t2);
        let t13 = vsubq_s32(t1, t2);
        let differences = [
            vsubq_s32(input[0], input[7]),
            vsubq_s32(input[1], input[6]),
            vsubq_s32(input[2], input[5]),
            vsubq_s32(input[3], input[4]),
        ];
        let odd = odd_part(differences, vdupq_n_s32(FORWARD_ODD_ROUNDING));
        let mut output = [vdupq_n_s32(0); 8];
        if FIRST {
            let sum = vsubq_s32(vaddq_s32(t10, t11), vdupq_n_s32(FORWARD_DC_BIAS));
            output[0] = vshlq_n_s32::<FORWARD_PASS_BITS>(sum);
            output[4] = vshlq_n_s32::<FORWARD_PASS_BITS>(vsubq_s32(t10, t11));
            let z1 = vmlaq_n_s32(
                vdupq_n_s32(FORWARD_ODD_ROUNDING),
                vaddq_s32(t12, t13),
                FORWARD_FIX_0_541196100,
            );
            output[2] =
                vshrq_n_s32::<FORWARD_FIRST_SHIFT>(vmlaq_n_s32(z1, t12, FORWARD_FIX_0_765366865));
            output[6] =
                vshrq_n_s32::<FORWARD_FIRST_SHIFT>(vmlsq_n_s32(z1, t13, FORWARD_FIX_1_847759065));
            output[1] = vshrq_n_s32::<FORWARD_FIRST_SHIFT>(odd[0]);
            output[3] = vshrq_n_s32::<FORWARD_FIRST_SHIFT>(odd[1]);
            output[5] = vshrq_n_s32::<FORWARD_FIRST_SHIFT>(odd[2]);
            output[7] = vshrq_n_s32::<FORWARD_FIRST_SHIFT>(odd[3]);
        } else {
            let t10 = vaddq_s32(t10, vdupq_n_s32(FORWARD_SECOND_DC_ROUNDING));
            output[0] = vshrq_n_s32::<FORWARD_PASS_BITS>(vaddq_s32(t10, t11));
            output[4] = vshrq_n_s32::<FORWARD_PASS_BITS>(vsubq_s32(t10, t11));
            let z1 = vmlaq_n_s32(
                vdupq_n_s32(FORWARD_SECOND_ROUNDING),
                vaddq_s32(t12, t13),
                FORWARD_FIX_0_541196100,
            );
            output[2] =
                vshrq_n_s32::<FORWARD_SECOND_SHIFT>(vmlaq_n_s32(z1, t12, FORWARD_FIX_0_765366865));
            output[6] =
                vshrq_n_s32::<FORWARD_SECOND_SHIFT>(vmlsq_n_s32(z1, t13, FORWARD_FIX_1_847759065));
            output[1] = vshrq_n_s32::<FORWARD_SECOND_SHIFT>(odd[0]);
            output[3] = vshrq_n_s32::<FORWARD_SECOND_SHIFT>(odd[1]);
            output[5] = vshrq_n_s32::<FORWARD_SECOND_SHIFT>(odd[2]);
            output[7] = vshrq_n_s32::<FORWARD_SECOND_SHIFT>(odd[3]);
        }
        output
    }

    #[target_feature(enable = "neon")]
    fn widen(bytes: &[u8; BLOCK_SIDE]) -> (int32x4_t, int32x4_t) {
        let words = vreinterpretq_s16_u16(vmovl_u8(vcreate_u8(u64::from_le_bytes(*bytes))));
        (vmovl_s16(vget_low_s16(words)), vmovl_high_s16(words))
    }

    #[target_feature(enable = "neon")]
    fn quantize(coefficients: int32x4_t, factors: int32x4_t) -> int32x4_t {
        let sign = vreinterpretq_s32_u32(vshrq_n_u32::<SIGN_TO_DIVISOR_SHIFT>(
            vreinterpretq_u32_s32(vshrq_n_s32::<31>(coefficients)),
        ));
        let divided = vshrq_n_s32::<QUANTIZE_SHIFT>(vaddq_s32(coefficients, sign));
        let value = vdivq_f32(vcvtq_f32_s32(divided), vcvtq_f32_s32(factors));
        let whole = vcvtq_s32_f32(value);
        let fraction = vsubq_f32(value, vcvtq_f32_s32(whole));
        let up = vreinterpretq_s32_u32(vcgeq_f32(fraction, vdupq_n_f32(QUANTIZE_HALF)));
        let down = vreinterpretq_s32_u32(vcleq_f32(fraction, vdupq_n_f32(-QUANTIZE_HALF)));
        vaddq_s32(vsubq_s32(whole, up), down)
    }

    #[target_feature(enable = "neon")]
    pub(super) fn forward_dct_quantize(
        samples: &[u8; BLOCK_COEFFICIENTS],
        table: &[u8; BLOCK_COEFFICIENTS],
        output: &mut [i32; BLOCK_COEFFICIENTS],
    ) {
        let mut low = [vdupq_n_s32(0); BLOCK_SIDE];
        let mut high = [vdupq_n_s32(0); BLOCK_SIDE];
        for ((low, high), line) in low
            .iter_mut()
            .zip(&mut high)
            .zip(samples.as_chunks::<BLOCK_SIDE>().0)
        {
            (*low, *high) = widen(line);
        }
        let (low, high) = transpose(low, high);
        let (low, high) = transpose(forward_pass::<true>(low), forward_pass::<true>(high));
        let low = forward_pass::<false>(low);
        let high = forward_pass::<false>(high);
        for (row, (factors, target)) in table
            .as_chunks::<BLOCK_SIDE>()
            .0
            .iter()
            .zip(output.as_chunks_mut::<BLOCK_SIDE>().0)
            .enumerate()
        {
            let ([left, right], []) = target.as_chunks_mut::<4>() else {
                unreachable!("eight coefficients are two halves of four");
            };
            let (factors_low, factors_high) = widen(factors);
            let quantized_low = quantize(low[row], factors_low);
            let quantized_high = quantize(high[row], factors_high);
            unsafe { vst1q_s32(left.as_mut_ptr(), quantized_low) };
            unsafe { vst1q_s32(right.as_mut_ptr(), quantized_high) };
        }
    }

    #[target_feature(enable = "neon")]
    fn weighted(
        channels: [int32x4_t; RGB_BYTES],
        weights: [i32; RGB_BYTES],
        rounding: i32,
    ) -> int32x4_t {
        let sum = vmlaq_n_s32(
            vmlaq_n_s32(
                vmlaq_n_s32(vdupq_n_s32(rounding), channels[0], weights[0]),
                channels[1],
                weights[1],
            ),
            channels[2],
            weights[2],
        );
        vshrq_n_s32::<ENCODE_PRECISION_BITS>(sum)
    }

    #[target_feature(enable = "neon")]
    fn narrow_plane(low: int32x4_t, high: int32x4_t) -> [u8; BLOCK_SIDE] {
        let bytes = vqmovun_s16(vcombine_s16(vmovn_s32(low), vmovn_s32(high)));
        vget_lane_u64::<0>(vreinterpret_u64_u8(bytes)).to_le_bytes()
    }

    #[target_feature(enable = "neon")]
    pub(super) fn rgb_to_ycbcr(
        rows: &[[u8; ENCODE_ROW_BYTES]; BLOCK_SIDE],
        planes: &mut [[u8; BLOCK_COEFFICIENTS]; PLANES],
    ) {
        for (row, pixels) in rows.iter().enumerate() {
            let channels = unsafe { vld3_u8(pixels.as_ptr()) };
            let mut low = [vdupq_n_s32(0); RGB_BYTES];
            let mut high = [vdupq_n_s32(0); RGB_BYTES];
            for (index, channel) in [channels.0, channels.1, channels.2].into_iter().enumerate() {
                let words = vreinterpretq_s16_u16(vmovl_u8(channel));
                low[index] = vmovl_s16(vget_low_s16(words));
                high[index] = vmovl_high_s16(words);
            }
            let luma_weights = [LUMA_RED, LUMA_GREEN, LUMA_BLUE];
            let blue_weights = [-BLUE_RED, -BLUE_GREEN, CHROMA_HALF];
            let red_weights = [CHROMA_HALF, -RED_GREEN, -RED_BLUE];
            let values = [
                narrow_plane(
                    weighted(low, luma_weights, LUMA_ROUNDING),
                    weighted(high, luma_weights, LUMA_ROUNDING),
                ),
                narrow_plane(
                    weighted(low, blue_weights, CHROMA_ROUNDING),
                    weighted(high, blue_weights, CHROMA_ROUNDING),
                ),
                narrow_plane(
                    weighted(low, red_weights, CHROMA_ROUNDING),
                    weighted(high, red_weights, CHROMA_ROUNDING),
                ),
            ];
            for (plane, value) in planes.iter_mut().zip(values) {
                plane[row * BLOCK_SIDE..(row + 1) * BLOCK_SIDE].copy_from_slice(&value);
            }
        }
    }
}
