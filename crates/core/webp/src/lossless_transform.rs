use alloc::vec;
use alloc::vec::Vec;

const BYTES_PER_PIXEL: usize = 4;
const GREEN_OFFSET: usize = 1;
const ALPHA_OFFSET: usize = 3;
const OPAQUE_ALPHA_PREDICTION: u8 = 0xff;
const PALETTE_ENTRIES_MAX: usize = 256;
const PALETTE_SMALL_MAX: u16 = 16;
const COLOR_DELTA_SHIFT: u32 = 5;

pub(crate) enum Transform {
    Predictor { size_bits: u8, data: Vec<u8> },
    Color { size_bits: u8, data: Vec<u8> },
    SubtractGreen,
    ColorIndexing { table_size: u16, table: Vec<u8> },
}

const fn subsample(size: usize, bits: u8) -> usize {
    (size + (1 << bits) - 1) >> bits
}

const fn average2(a: u8, b: u8) -> u8 {
    ((a as u16 + b as u16) / 2) as u8
}

fn clamp_add_subtract_full(a: i16, b: i16, c: i16) -> u8 {
    saturate(a + b - c)
}

fn clamp_add_subtract_half(a: i16, b: i16) -> u8 {
    saturate(a + (a - b) / 2)
}

fn saturate(value: i16) -> u8 {
    (value.max(0) as u16).min(u16::from(u8::MAX)) as u8
}

pub(crate) fn apply_predictor(
    image: &mut [u8],
    width: u16,
    height: u16,
    size_bits: u8,
    predictors: &[u8],
) {
    let width = usize::from(width);
    let height = usize::from(height);
    let block_width = subsample(width, size_bits);
    let row_bytes = width * BYTES_PER_PIXEL;
    image[ALPHA_OFFSET] = image[ALPHA_OFFSET].wrapping_add(OPAQUE_ALPHA_PREDICTION);
    for index in BYTES_PER_PIXEL..row_bytes {
        image[index] = image[index].wrapping_add(image[index - BYTES_PER_PIXEL]);
    }
    for y in 1..height {
        for channel in 0..BYTES_PER_PIXEL {
            let index = y * row_bytes + channel;
            image[index] = image[index].wrapping_add(image[index - row_bytes]);
        }
    }
    for y in 1..height {
        for block_x in 0..block_width {
            let block_index = (y >> size_bits) * block_width + block_x;
            let predictor = predictors[block_index * BYTES_PER_PIXEL + GREEN_OFFSET];
            let start = (y * width + (block_x << size_bits).max(1)) * BYTES_PER_PIXEL;
            let end = (y * width + ((block_x + 1) << size_bits).min(width)) * BYTES_PER_PIXEL;
            predict_range(image, start, end, row_bytes, predictor);
        }
    }
}

fn predict_range(image: &mut [u8], start: usize, end: usize, row_bytes: usize, predictor: u8) {
    match predictor {
        0 => predict_pixels(image, start, end, row_bytes, |_, _, _, _| {
            [0, 0, 0, OPAQUE_ALPHA_PREDICTION]
        }),
        1 => predict_pixels(image, start, end, row_bytes, |left, _, _, _| left),
        2 => predict_pixels(image, start, end, row_bytes, |_, _, top, _| top),
        3 => predict_pixels(image, start, end, row_bytes, |_, _, _, top_right| top_right),
        4 => predict_pixels(image, start, end, row_bytes, |_, top_left, _, _| top_left),
        5 => predict_pixels(image, start, end, row_bytes, |left, _, top, top_right| {
            average_pixels(average_pixels(left, top_right), top)
        }),
        6 => predict_pixels(image, start, end, row_bytes, |left, top_left, _, _| {
            average_pixels(left, top_left)
        }),
        7 => predict_pixels(image, start, end, row_bytes, |left, _, top, _| {
            average_pixels(left, top)
        }),
        8 => predict_pixels(image, start, end, row_bytes, |_, top_left, top, _| {
            average_pixels(top_left, top)
        }),
        9 => predict_pixels(image, start, end, row_bytes, |_, _, top, top_right| {
            average_pixels(top, top_right)
        }),
        10 => predict_pixels(
            image,
            start,
            end,
            row_bytes,
            |left, top_left, top, top_right| {
                average_pixels(
                    average_pixels(left, top_left),
                    average_pixels(top, top_right),
                )
            },
        ),
        11 => predict_pixels(image, start, end, row_bytes, select),
        12 => predict_pixels(image, start, end, row_bytes, |left, top_left, top, _| {
            core::array::from_fn(|channel| {
                clamp_add_subtract_full(
                    i16::from(left[channel]),
                    i16::from(top[channel]),
                    i16::from(top_left[channel]),
                )
            })
        }),
        13 => predict_pixels(image, start, end, row_bytes, |left, top_left, top, _| {
            core::array::from_fn(|channel| {
                clamp_add_subtract_half(
                    (i16::from(left[channel]) + i16::from(top[channel])) / 2,
                    i16::from(top_left[channel]),
                )
            })
        }),
        _ => {}
    }
}

fn average_pixels(a: [u8; BYTES_PER_PIXEL], b: [u8; BYTES_PER_PIXEL]) -> [u8; BYTES_PER_PIXEL] {
    core::array::from_fn(|channel| average2(a[channel], b[channel]))
}

fn select(
    left: [u8; BYTES_PER_PIXEL],
    top_left: [u8; BYTES_PER_PIXEL],
    top: [u8; BYTES_PER_PIXEL],
    _: [u8; BYTES_PER_PIXEL],
) -> [u8; BYTES_PER_PIXEL] {
    let mut left_distance = 0i16;
    let mut top_distance = 0i16;
    for channel in 0..BYTES_PER_PIXEL {
        let predict =
            i16::from(left[channel]) + i16::from(top[channel]) - i16::from(top_left[channel]);
        left_distance += (predict - i16::from(left[channel])).abs();
        top_distance += (predict - i16::from(top[channel])).abs();
    }
    if left_distance < top_distance {
        left
    } else {
        top
    }
}

#[inline(always)]
fn predict_pixels(
    image: &mut [u8],
    start: usize,
    end: usize,
    row_bytes: usize,
    prediction: impl Fn(
        [u8; BYTES_PER_PIXEL],
        [u8; BYTES_PER_PIXEL],
        [u8; BYTES_PER_PIXEL],
        [u8; BYTES_PER_PIXEL],
    ) -> [u8; BYTES_PER_PIXEL],
) {
    let (previous, current) = image[..end].split_at_mut(start);
    let above = previous[start - row_bytes - BYTES_PER_PIXEL..end - row_bytes + BYTES_PER_PIXEL]
        .as_chunks::<BYTES_PER_PIXEL>()
        .0;
    let mut left = [0u8; BYTES_PER_PIXEL];
    left.copy_from_slice(&previous[start - BYTES_PER_PIXEL..]);
    for (pixel, neighbours) in current
        .as_chunks_mut::<BYTES_PER_PIXEL>()
        .0
        .iter_mut()
        .zip(above.windows(3))
    {
        let predicted = prediction(left, neighbours[0], neighbours[1], neighbours[2]);
        for (value, predicted) in pixel.iter_mut().zip(predicted) {
            *value = value.wrapping_add(predicted);
        }
        left = *pixel;
    }
}

const fn color_delta(transform: u8, color: u8) -> u32 {
    ((transform as i8 as i32) * (color as i8 as i32)) as u32 >> COLOR_DELTA_SHIFT
}

pub(crate) fn apply_color(image: &mut [u8], width: u16, size_bits: u8, data: &[u8]) {
    let width = usize::from(width);
    let block_width = subsample(width, size_bits);
    for (y, row) in image.chunks_exact_mut(width * BYTES_PER_PIXEL).enumerate() {
        let row_data = &data[(y >> size_bits) * block_width * BYTES_PER_PIXEL..];
        for (block, transform) in row
            .chunks_mut(BYTES_PER_PIXEL << size_bits)
            .zip(row_data.as_chunks::<BYTES_PER_PIXEL>().0.iter())
        {
            let (red_to_blue, green_to_blue, green_to_red) =
                (transform[0], transform[1], transform[2]);
            for pixel in block.as_chunks_mut::<BYTES_PER_PIXEL>().0.iter_mut() {
                let green = pixel[1];
                let red = u32::from(pixel[0]) + color_delta(green_to_red, green);
                let blue = u32::from(pixel[2])
                    + color_delta(green_to_blue, green)
                    + color_delta(red_to_blue, red as u8);
                pixel[0] = red as u8;
                pixel[2] = blue as u8;
            }
        }
    }
}

pub(crate) fn apply_subtract_green(image: &mut [u8]) {
    for pixel in image.as_chunks_mut::<BYTES_PER_PIXEL>().0.iter_mut() {
        pixel[0] = pixel[0].wrapping_add(pixel[1]);
        pixel[2] = pixel[2].wrapping_add(pixel[1]);
    }
}

pub(crate) fn apply_color_indexing(
    image: &mut [u8],
    width: u16,
    height: u16,
    table_size: u16,
    table: &[u8],
) {
    assert!(table_size > 0);
    assert_eq!(table.len(), usize::from(table_size) * BYTES_PER_PIXEL);
    let mut palette = [[0u8; BYTES_PER_PIXEL]; PALETTE_ENTRIES_MAX];
    for (entry, color) in palette
        .iter_mut()
        .zip(table.as_chunks::<BYTES_PER_PIXEL>().0.iter())
    {
        entry.copy_from_slice(color);
    }
    let width = usize::from(width);
    let height = usize::from(height);
    if table_size > PALETTE_SMALL_MAX {
        for pixel in image[..width * height * BYTES_PER_PIXEL]
            .as_chunks_mut::<BYTES_PER_PIXEL>()
            .0
            .iter_mut()
        {
            *pixel = palette[usize::from(pixel[GREEN_OFFSET])];
        }
        return;
    }
    let width_bits: u8 = match table_size {
        0..=2 => 3,
        3..=4 => 2,
        _ => 1,
    };
    let bits_per_entry = 8 >> width_bits;
    let entry_mask = (1u8 << bits_per_entry) - 1;
    let pixels_per_byte_mask = (1usize << width_bits) - 1;
    let packed_width = subsample(width, width_bits);
    let mut packed = vec![0u8; packed_width];
    for y in (0..height).rev() {
        let packed_row =
            &image[y * packed_width * BYTES_PER_PIXEL..][..packed_width * BYTES_PER_PIXEL];
        for (value, pixel) in packed
            .iter_mut()
            .zip(packed_row.as_chunks::<BYTES_PER_PIXEL>().0.iter())
        {
            *value = pixel[GREEN_OFFSET];
        }
        let row = &mut image[y * width * BYTES_PER_PIXEL..][..width * BYTES_PER_PIXEL];
        for (x, pixel) in row
            .as_chunks_mut::<BYTES_PER_PIXEL>()
            .0
            .iter_mut()
            .enumerate()
        {
            let shift = (x & pixels_per_byte_mask) as u8 * bits_per_entry;
            let entry = (packed[x >> width_bits] >> shift) & entry_mask;
            let color = if u16::from(entry) < table_size {
                palette[usize::from(entry)]
            } else {
                [0; BYTES_PER_PIXEL]
            };
            pixel.copy_from_slice(&color);
        }
    }
}
