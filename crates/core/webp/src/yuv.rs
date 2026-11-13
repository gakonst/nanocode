use crate::error::WebpError;
use crate::lossy::Frame;

const LUMA_SCALE: u32 = 19077;
const RED_FROM_V: u32 = 26149;
const GREEN_FROM_U: u32 = 6419;
const GREEN_FROM_V: u32 = 13320;
const BLUE_FROM_U: u32 = 33050;
const RED_OFFSET: i32 = 14234;
const GREEN_OFFSET: i32 = 8708;
const BLUE_OFFSET: i32 = 17685;
const FIXED_POINT_BITS: i32 = 6;
const MULTIPLY_HIGH_BITS: u32 = 8;
const RGB_BYTES: usize = 3;

#[inline(always)]
const fn multiply_high(value: u8, coefficient: u32) -> i32 {
    ((value as u32 * coefficient) >> MULTIPLY_HIGH_BITS) as i32
}

#[inline(always)]
const fn clip(value: i32) -> u8 {
    let shifted = value >> FIXED_POINT_BITS;
    if shifted < 0 {
        0
    } else if shifted > 255 {
        255
    } else {
        shifted as u8
    }
}

#[inline(always)]
const fn set_pixel(rgb: &mut [u8], y: u8, u: u8, v: u8) {
    let luma = multiply_high(y, LUMA_SCALE);
    rgb[0] = clip(luma + multiply_high(v, RED_FROM_V) - RED_OFFSET);
    rgb[1] =
        clip(luma - multiply_high(u, GREEN_FROM_U) - multiply_high(v, GREEN_FROM_V) + GREEN_OFFSET);
    rgb[2] = clip(luma + multiply_high(u, BLUE_FROM_U) - BLUE_OFFSET);
}

#[inline(always)]
const fn fancy(main: u8, secondary1: u8, secondary2: u8, tertiary: u8) -> u8 {
    ((9 * main as u16 + 3 * secondary1 as u16 + 3 * secondary2 as u16 + tertiary as u16 + 8) / 16)
        as u8
}

pub(crate) fn fill_fancy<const BYTES_PER_PIXEL: usize>(
    output: &mut [u8],
    frame: &Frame,
) -> Result<(), WebpError> {
    let width = usize::from(frame.width);
    let height = usize::from(frame.height);
    let buffer_width = frame.buffer_width();
    let chroma_buffer_width = buffer_width / 2;
    let chroma_width = width.div_ceil(2);
    let row_bytes = width * BYTES_PER_PIXEL;
    let (luma, blue, red) = (&frame.luma, &frame.chroma_blue, &frame.chroma_red);
    if width == 0 || height == 0 || width > luma.len() || row_bytes > output.len() {
        return Err(WebpError::FramePixels);
    }
    fill_row_single::<BYTES_PER_PIXEL>(
        &mut output[..row_bytes],
        &luma[..width],
        &blue[..chroma_width],
        &red[..chroma_width],
    )?;
    let pair_bytes = 2 * row_bytes;
    let rest_bytes = output.len() - row_bytes;
    let chunk_count = rest_bytes / pair_bytes;
    let remainder_bytes = rest_bytes % pair_bytes;
    let luma_pairs = (height - 1) / 2;
    let chroma_rows = blue.len() / chroma_buffer_width;
    let pairs = chunk_count
        .min(luma_pairs)
        .min(chroma_rows.saturating_sub(1));
    for pair in 0..pairs {
        let luma_top = &luma[buffer_width * (1 + 2 * pair)..][..width];
        let luma_bottom = &luma[buffer_width * (2 + 2 * pair)..][..width];
        let blue_top = &blue[pair * chroma_buffer_width..][..chroma_width];
        let blue_bottom = &blue[(pair + 1) * chroma_buffer_width..][..chroma_width];
        let red_top = &red[pair * chroma_buffer_width..][..chroma_width];
        let red_bottom = &red[(pair + 1) * chroma_buffer_width..][..chroma_width];
        let start = row_bytes + pair * pair_bytes;
        let (first, second) = output[start..start + pair_bytes].split_at_mut(row_bytes);
        fill_row_double::<BYTES_PER_PIXEL>(
            first,
            luma_top,
            [blue_top, blue_bottom],
            [red_top, red_bottom],
        );
        fill_row_double::<BYTES_PER_PIXEL>(
            second,
            luma_bottom,
            [blue_bottom, blue_top],
            [red_bottom, red_top],
        );
    }
    if remainder_bytes == 0 {
        return Ok(());
    }
    if (height - 1) % 2 == 0 {
        return Err(WebpError::FramePixels);
    }
    let chroma_start = (height.div_ceil(2) - 1) * chroma_buffer_width;
    let start = row_bytes + chunk_count * pair_bytes;
    fill_row_single::<BYTES_PER_PIXEL>(
        &mut output[start..start + remainder_bytes],
        &luma[buffer_width * (height - 1)..][..width],
        &blue[chroma_start..][..chroma_width],
        &red[chroma_start..][..chroma_width],
    )
}

fn fill_row_single<const BYTES_PER_PIXEL: usize>(
    row: &mut [u8],
    luma: &[u8],
    blue: &[u8],
    red: &[u8],
) -> Result<(), WebpError> {
    if row.len() < BYTES_PER_PIXEL {
        return Err(WebpError::FramePixels);
    }
    set_pixel(&mut row[..RGB_BYTES], luma[0], blue[0], red[0]);
    let rest_bytes = row.len() - BYTES_PER_PIXEL;
    let chunk_count = rest_bytes / (2 * BYTES_PER_PIXEL);
    let remainder_bytes = rest_bytes % (2 * BYTES_PER_PIXEL);
    let pairs = chunk_count.min((luma.len() - 1) / 2).min(blue.len() - 1);
    for pair in 0..pairs {
        let start = BYTES_PER_PIXEL + pair * 2 * BYTES_PER_PIXEL;
        let (blue0, blue1) = (blue[pair], blue[pair + 1]);
        let (red0, red1) = (red[pair], red[pair + 1]);
        set_pixel(
            &mut row[start..start + RGB_BYTES],
            luma[1 + 2 * pair],
            fancy(blue0, blue1, blue0, blue1),
            fancy(red0, red1, red0, red1),
        );
        let start = start + BYTES_PER_PIXEL;
        set_pixel(
            &mut row[start..start + RGB_BYTES],
            luma[2 + 2 * pair],
            fancy(blue1, blue0, blue1, blue0),
            fancy(red1, red0, red1, red0),
        );
    }
    if (luma.len() - 1) % 2 == 1 {
        if remainder_bytes < RGB_BYTES {
            return Err(WebpError::FramePixels);
        }
        let start = BYTES_PER_PIXEL + chunk_count * 2 * BYTES_PER_PIXEL;
        set_pixel(
            &mut row[start..start + RGB_BYTES],
            luma[luma.len() - 1],
            blue[blue.len() - 1],
            red[red.len() - 1],
        );
    }
    Ok(())
}

fn fill_row_double<const BYTES_PER_PIXEL: usize>(
    row: &mut [u8],
    luma: &[u8],
    blue: [&[u8]; 2],
    red: [&[u8]; 2],
) {
    let width = luma.len();
    assert_eq!(row.len(), width * BYTES_PER_PIXEL);
    let [blue_near, blue_far] = blue;
    let [red_near, red_far] = red;
    set_pixel(
        &mut row[..RGB_BYTES],
        luma[0],
        fancy(blue_near[0], blue_near[0], blue_far[0], blue_far[0]),
        fancy(red_near[0], red_near[0], red_far[0], red_far[0]),
    );
    let pairs = (width - 1) / 2;
    for (pair, pixels) in row[BYTES_PER_PIXEL..]
        .chunks_exact_mut(2 * BYTES_PER_PIXEL)
        .take(pairs)
        .enumerate()
    {
        let (b0, b1, b2, b3) = (
            blue_near[pair],
            blue_near[pair + 1],
            blue_far[pair],
            blue_far[pair + 1],
        );
        let (r0, r1, r2, r3) = (
            red_near[pair],
            red_near[pair + 1],
            red_far[pair],
            red_far[pair + 1],
        );
        set_pixel(
            &mut pixels[..RGB_BYTES],
            luma[1 + 2 * pair],
            fancy(b0, b1, b2, b3),
            fancy(r0, r1, r2, r3),
        );
        set_pixel(
            &mut pixels[BYTES_PER_PIXEL..BYTES_PER_PIXEL + RGB_BYTES],
            luma[2 + 2 * pair],
            fancy(b1, b0, b3, b2),
            fancy(r1, r0, r3, r2),
        );
    }
    if width.is_multiple_of(2) {
        let start = (width - 1) * BYTES_PER_PIXEL;
        let last = blue_near.len() - 1;
        set_pixel(
            &mut row[start..start + RGB_BYTES],
            luma[width - 1],
            fancy(
                blue_near[last],
                blue_near[last],
                blue_far[last],
                blue_far[last],
            ),
            fancy(red_near[last], red_near[last], red_far[last], red_far[last]),
        );
    }
}
