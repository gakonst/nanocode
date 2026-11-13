use alloc::vec;
use alloc::vec::Vec;

const BYTES_PER_PIXEL: usize = 4;
const GREEN_OFFSET: usize = 1;
const ALPHA_OFFSET: usize = 3;
const OPAQUE_ALPHA_PREDICTION: u8 = 0xff;
const PALETTE_ENTRIES_MAX: usize = 256;
const PALETTE_SMALL_MAX: u16 = 16;
const COLOR_DELTA_SHIFT: u32 = 5;
const COLOR_LANES: usize = 8;
const GREEN_ALPHA_MASK: u32 = 0xff00_ff00;
const COLOR_LANE_BYTES: usize = COLOR_LANES * BYTES_PER_PIXEL;

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

const LOW_SEVEN_BITS: u32 = 0x7f7f_7f7f;
const HIGH_BITS: u32 = 0x8080_8080;
const LOW_BITS_CLEARED: u32 = 0xfefe_fefe;

#[inline(always)]
const fn add_pixels(a: u32, b: u32) -> u32 {
    ((a & LOW_SEVEN_BITS) + (b & LOW_SEVEN_BITS)) ^ ((a ^ b) & HIGH_BITS)
}

#[inline(always)]
const fn average_pixels(a: u32, b: u32) -> u32 {
    (a & b) + (((a ^ b) & LOW_BITS_CLEARED) >> 1)
}

#[inline(always)]
const fn channels(pixel: u32) -> [u8; BYTES_PER_PIXEL] {
    pixel.to_le_bytes()
}

fn predict_range(image: &mut [u8], start: usize, end: usize, row_bytes: usize, predictor: u8) {
    let (previous, current) = image[..end].split_at_mut(start);
    let above = &previous[start - row_bytes - BYTES_PER_PIXEL..end - row_bytes + BYTES_PER_PIXEL];
    let top_left = &above[..current.len()];
    let top = &above[BYTES_PER_PIXEL..][..current.len()];
    let top_right = &above[2 * BYTES_PER_PIXEL..][..current.len()];
    let mut left = [0u8; BYTES_PER_PIXEL];
    left.copy_from_slice(&previous[start - BYTES_PER_PIXEL..]);
    let left = u32::from_le_bytes(left);
    match predictor {
        0 => {
            for pixel in current.as_chunks_mut::<BYTES_PER_PIXEL>().0.iter_mut() {
                pixel[ALPHA_OFFSET] = pixel[ALPHA_OFFSET].wrapping_add(OPAQUE_ALPHA_PREDICTION);
            }
        }
        1 => predict_serial(current, above, left, |left, _, _, _| left),
        2 => add_bytes(current, top),
        3 => add_bytes(current, top_right),
        4 => add_bytes(current, top_left),
        5 => predict_serial(current, above, left, |left, _, top, top_right| {
            average_pixels(average_pixels(left, top_right), top)
        }),
        6 => predict_serial(current, above, left, |left, top_left, _, _| {
            average_pixels(left, top_left)
        }),
        7 => predict_serial(current, above, left, |left, _, top, _| {
            average_pixels(left, top)
        }),
        8 => add_averages(current, top_left, top),
        9 => add_averages(current, top, top_right),
        10 => predict_serial(current, above, left, |left, top_left, top, top_right| {
            average_pixels(
                average_pixels(left, top_left),
                average_pixels(top, top_right),
            )
        }),
        11 => predict_serial(current, above, left, select),
        12 => predict_serial(current, above, left, |left, top_left, top, _| {
            let (left, top_left, top) = (channels(left), channels(top_left), channels(top));
            u32::from_le_bytes(core::array::from_fn(|channel| {
                clamp_add_subtract_full(
                    i16::from(left[channel]),
                    i16::from(top[channel]),
                    i16::from(top_left[channel]),
                )
            }))
        }),
        13 => predict_serial(current, above, left, |left, top_left, top, _| {
            let (average, top_left) = (channels(average_pixels(left, top)), channels(top_left));
            u32::from_le_bytes(core::array::from_fn(|channel| {
                clamp_add_subtract_half(i16::from(average[channel]), i16::from(top_left[channel]))
            }))
        }),
        _ => {}
    }
}

#[inline(always)]
fn add_bytes(current: &mut [u8], neighbours: &[u8]) {
    for (value, &neighbour) in current.iter_mut().zip(neighbours) {
        *value = value.wrapping_add(neighbour);
    }
}

#[inline(always)]
fn add_averages(current: &mut [u8], first: &[u8], second: &[u8]) {
    for ((value, &first), &second) in current.iter_mut().zip(first).zip(second) {
        *value = value.wrapping_add(average2(first, second));
    }
}

fn select(left: u32, top_left: u32, top: u32, _: u32) -> u32 {
    let (left_channels, top_left_channels, top_channels) =
        (channels(left), channels(top_left), channels(top));
    let mut left_distance = 0u16;
    let mut top_distance = 0u16;
    for channel in 0..BYTES_PER_PIXEL {
        left_distance += u16::from(top_channels[channel].abs_diff(top_left_channels[channel]));
        top_distance += u16::from(left_channels[channel].abs_diff(top_left_channels[channel]));
    }
    if left_distance < top_distance {
        left
    } else {
        top
    }
}

#[inline(always)]
fn predict_serial(
    current: &mut [u8],
    above: &[u8],
    mut left: u32,
    prediction: impl Fn(u32, u32, u32, u32) -> u32,
) {
    let above = above.as_chunks::<BYTES_PER_PIXEL>().0;
    for (pixel, neighbours) in current
        .as_chunks_mut::<BYTES_PER_PIXEL>()
        .0
        .iter_mut()
        .zip(above.windows(3))
    {
        let predicted = prediction(
            left,
            u32::from_le_bytes(neighbours[0]),
            u32::from_le_bytes(neighbours[1]),
            u32::from_le_bytes(neighbours[2]),
        );
        left = add_pixels(u32::from_le_bytes(*pixel), predicted);
        *pixel = left.to_le_bytes();
    }
}

const fn color_delta(transform: u8, color: u8) -> u8 {
    ((transform as i8 as i16 * color as i8 as i16) >> COLOR_DELTA_SHIFT) as u8
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
            let (lanes, rest) = block.as_chunks_mut::<COLOR_LANE_BYTES>();
            for lane in lanes {
                let pixels: [u32; COLOR_LANES] = core::array::from_fn(|index| {
                    u32::from_le_bytes([
                        lane[index * BYTES_PER_PIXEL],
                        lane[index * BYTES_PER_PIXEL + 1],
                        lane[index * BYTES_PER_PIXEL + 2],
                        lane[index * BYTES_PER_PIXEL + 3],
                    ])
                });
                let mut transformed = [0u32; COLOR_LANES];
                for index in 0..COLOR_LANES {
                    let pixel = pixels[index];
                    let green = (pixel >> 8) as u8;
                    let red = (pixel as u8).wrapping_add(color_delta(green_to_red, green));
                    let blue = ((pixel >> 16) as u8)
                        .wrapping_add(color_delta(green_to_blue, green))
                        .wrapping_add(color_delta(red_to_blue, red));
                    transformed[index] =
                        (pixel & GREEN_ALPHA_MASK) | u32::from(red) | (u32::from(blue) << 16);
                }
                for (index, pixel) in transformed.into_iter().enumerate() {
                    lane[index * BYTES_PER_PIXEL..][..BYTES_PER_PIXEL]
                        .copy_from_slice(&pixel.to_le_bytes());
                }
            }
            for pixel in rest.as_chunks_mut::<BYTES_PER_PIXEL>().0.iter_mut() {
                let green = pixel[1];
                let red = pixel[0].wrapping_add(color_delta(green_to_red, green));
                pixel[0] = red;
                pixel[2] = pixel[2]
                    .wrapping_add(color_delta(green_to_blue, green))
                    .wrapping_add(color_delta(red_to_blue, red));
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
