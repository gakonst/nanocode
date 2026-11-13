use crate::error::{ColorSpace, JpegError};

const Y_FACTOR: i32 = 16384;
const CR_FACTOR: i32 = 22970;
const CB_FACTOR: i32 = 29032;
const GREEN_CR_FACTOR: i32 = -11700;
const GREEN_CB_FACTOR: i32 = -5638;
const PRECISION_BITS: u32 = 14;
const ROUNDING: i32 = (1 << (PRECISION_BITS - 1)) - 1;
const CHROMA_BIAS: i16 = 128;
const CONVERSION_PIXELS: usize = 16;

pub(crate) fn convert_row(
    samples: &[&[i16]; 4],
    input: ColorSpace,
    output: ColorSpace,
    target: &mut [u8],
    width: usize,
    padded_width: usize,
) -> Result<(), JpegError> {
    if input.components() == 3 && input == output {
        copy(samples, width, target);
        return Ok(());
    }
    match (input, output) {
        (ColorSpace::YCbCr | ColorSpace::Luma, ColorSpace::Luma) => {
            for (pixel, &sample) in target.iter_mut().take(width).zip(samples[0]) {
                *pixel = sample as u8;
            }
            Ok(())
        }
        (ColorSpace::YCbCr, ColorSpace::Rgb) => ycbcr(samples, width, padded_width, target),
        (ColorSpace::Ycck, ColorSpace::Rgb) => {
            ycbcr(samples, width, padded_width, target)?;
            if !samples[3].is_empty() {
                for (pixel, &black) in target
                    .as_chunks_mut::<3>()
                    .0
                    .iter_mut()
                    .take(width)
                    .zip(samples[3])
                {
                    let black = black as u8;
                    for channel in pixel {
                        *channel = blinn(255 - *channel, black);
                    }
                }
            }
            Ok(())
        }
        (ColorSpace::Cmyk, ColorSpace::Rgb) => {
            if samples.iter().all(|channel| !channel.is_empty()) {
                for (index, pixel) in target
                    .as_chunks_mut::<3>()
                    .0
                    .iter_mut()
                    .take(width)
                    .enumerate()
                {
                    let black = samples[3][index] as u8;
                    for (channel, plane) in pixel.iter_mut().zip(samples) {
                        *channel = blinn(plane[index] as u8, black);
                    }
                }
            }
            Ok(())
        }
        _ => Err(JpegError::ColorConversion { input, output }),
    }
}

fn copy(samples: &[&[i16]; 4], width: usize, target: &mut [u8]) {
    if samples[..3].iter().any(|channel| channel.is_empty()) {
        return;
    }
    for (index, pixel) in target
        .as_chunks_mut::<3>()
        .0
        .iter_mut()
        .take(width)
        .enumerate()
    {
        pixel[0] = samples[0][index] as u8;
        pixel[1] = samples[1][index] as u8;
        pixel[2] = samples[2][index] as u8;
    }
}

fn ycbcr(
    samples: &[&[i16]; 4],
    width: usize,
    padded_width: usize,
    target: &mut [u8],
) -> Result<(), JpegError> {
    if samples[..3].iter().any(|channel| channel.is_empty()) {
        return Ok(());
    }
    if width < CONVERSION_PIXELS && padded_width > CONVERSION_PIXELS {
        return Err(JpegError::LayoutOutOfBounds);
    }
    let converted = nanocode_sys::jpeg::ycbcr_to_rgb(
        &samples[0][..width],
        &samples[1][..width],
        &samples[2][..width],
        target,
    );
    let luma = &samples[0][converted..width];
    let blue = &samples[1][converted..width];
    let red = &samples[2][converted..width];
    let target = &mut target[converted * 3..];
    let width = width - converted;
    let blocks = width / CONVERSION_PIXELS * CONVERSION_PIXELS;
    let (target_blocks, target_rest) = target.split_at_mut(blocks * 3);
    for (index, block) in target_blocks
        .as_chunks_mut::<{ CONVERSION_PIXELS * 3 }>()
        .0
        .iter_mut()
        .enumerate()
    {
        let range = index * CONVERSION_PIXELS..(index + 1) * CONVERSION_PIXELS;
        let converted = convert_block(
            luma[range.clone()]
                .try_into()
                .expect("a block holds 16 samples"),
            blue[range.clone()]
                .try_into()
                .expect("a block holds 16 samples"),
            red[range].try_into().expect("a block holds 16 samples"),
        );
        for (pixel, rgb) in block.as_chunks_mut::<3>().0.iter_mut().zip(converted) {
            *pixel = rgb;
        }
    }
    for (((pixel, &y), &cb), &cr) in target_rest
        .as_chunks_mut::<3>()
        .0
        .iter_mut()
        .zip(&luma[blocks..])
        .zip(&blue[blocks..])
        .zip(&red[blocks..])
    {
        *pixel = convert_pixel(y, cb, cr);
    }
    Ok(())
}

#[inline(always)]
fn convert_block(
    luma: &[i16; CONVERSION_PIXELS],
    blue: &[i16; CONVERSION_PIXELS],
    red: &[i16; CONVERSION_PIXELS],
) -> [[u8; 3]; CONVERSION_PIXELS] {
    let mut pixels = [[0_u8; 3]; CONVERSION_PIXELS];
    for index in 0..CONVERSION_PIXELS {
        pixels[index] = convert_pixel(luma[index], blue[index], red[index]);
    }
    pixels
}

#[inline(always)]
fn convert_pixel(y: i16, cb: i16, cr: i16) -> [u8; 3] {
    let cr = i32::from(cr - CHROMA_BIAS);
    let cb = i32::from(cb - CHROMA_BIAS);
    let base = i32::from(y) * Y_FACTOR + ROUNDING;
    let r = (base + cr * CR_FACTOR) >> PRECISION_BITS;
    let g = (base + cr * GREEN_CR_FACTOR + cb * GREEN_CB_FACTOR) >> PRECISION_BITS;
    let b = (base + cb * CB_FACTOR) >> PRECISION_BITS;
    [
        r.clamp(0, 255) as u8,
        g.clamp(0, 255) as u8,
        b.clamp(0, 255) as u8,
    ]
}

const fn blinn(value: u8, factor: u8) -> u8 {
    let product = value as i32 * factor as i32 + 128;
    ((product + (product >> 8)) >> 8) as u8
}
