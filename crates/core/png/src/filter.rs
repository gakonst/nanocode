#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Filter {
    None = 0,
    Sub = 1,
    Up = 2,
    Average = 3,
    Paeth = 4,
}

impl Filter {
    pub(crate) const fn from_byte(byte: u8) -> Option<Self> {
        match byte {
            0 => Some(Self::None),
            1 => Some(Self::Sub),
            2 => Some(Self::Up),
            3 => Some(Self::Average),
            4 => Some(Self::Paeth),
            _ => None,
        }
    }
}

const ADAPTIVE_CANDIDATES: [Filter; 4] = [Filter::Sub, Filter::Up, Filter::Average, Filter::Paeth];
const MAGNITUDE_LANES: usize = 32;
const FILTER_LANES: usize = 32;

#[inline(always)]
const fn average(left: u8, up: u8) -> u8 {
    ((left as u16 + up as u16) / 2) as u8
}

#[inline(always)]
fn paeth_decode(left: u8, up: u8, up_left: u8) -> u8 {
    let threshold = i16::from(up_left) * 3 - (i16::from(left) + i16::from(up));
    let low = left.min(up);
    let high = left.max(up);
    let near = if i16::from(high) <= threshold {
        low
    } else {
        up_left
    };
    if threshold <= i16::from(low) {
        high
    } else {
        near
    }
}

pub(crate) fn unfilter(filter: Filter, bytes_per_pixel: usize, previous: &[u8], row: &mut [u8]) {
    assert_eq!(previous.len(), row.len());
    match (filter, bytes_per_pixel) {
        (Filter::Paeth, 2) => paeth_wide::<2>(previous, row),
        (Filter::Paeth, 4) => paeth_wide::<4>(previous, row),
        (Filter::Paeth, 8) => paeth_wide::<8>(previous, row),
        (_, 1) => unfilter_pixels::<1>(filter, previous, row),
        (_, 2) => unfilter_pixels::<2>(filter, previous, row),
        (_, 3) => unfilter_pixels::<3>(filter, previous, row),
        (_, 4) => unfilter_pixels::<4>(filter, previous, row),
        (_, 6) => unfilter_pixels::<6>(filter, previous, row),
        (_, 8) => unfilter_pixels::<8>(filter, previous, row),
        (_, other) => unreachable!("PNG pixels are 1, 2, 3, 4, 6 or 8 bytes, not {other}"),
    }
}

pub(crate) const fn pairs_paeth(bytes_per_pixel: usize) -> bool {
    matches!(bytes_per_pixel, 1 | 3 | 4 | 6)
}

pub(crate) fn unfilter_paeth_pair(
    bytes_per_pixel: usize,
    previous: &[u8],
    first: &mut [u8],
    second: &mut [u8],
) {
    assert_eq!(previous.len(), first.len());
    assert_eq!(first.len(), second.len());
    match bytes_per_pixel {
        1 => paeth_pair::<1>(previous, first, second),
        #[cfg(target_arch = "aarch64")]
        3 => paeth_pair_wide::<3>(previous, first, second),
        #[cfg(not(target_arch = "aarch64"))]
        3 => paeth_pair::<3>(previous, first, second),
        4 => paeth_pair::<4>(previous, first, second),
        6 => paeth_pair::<6>(previous, first, second),
        other => unreachable!(
            "Paeth rows are decoded in pairs at 1, 3, 4 or 6 bytes per pixel, not {other}"
        ),
    }
}

#[inline(always)]
fn paeth_decode_wide(left: i16, up: i16, up_left: i16) -> i16 {
    let threshold = up_left * 3 - (left + up);
    let low = left.min(up);
    let high = left.max(up);
    let near = if high <= threshold { low } else { up_left };
    if threshold <= low { high } else { near }
}

fn paeth_wide<const N: usize>(previous: &[u8], row: &mut [u8]) {
    let mut left = [0i16; N];
    let mut corner = [0i16; N];
    for (pixel, above) in row
        .as_chunks_mut::<N>()
        .0
        .iter_mut()
        .zip(previous.as_chunks::<N>().0)
    {
        let mut decoded = [0i16; N];
        for lane in 0..N {
            let up = i16::from(above[lane]);
            decoded[lane] = (i16::from(pixel[lane])
                + paeth_decode_wide(left[lane], up, corner[lane]))
                & i16::from(u8::MAX);
            pixel[lane] = decoded[lane] as u8;
            corner[lane] = up;
        }
        left = decoded;
    }
}

fn paeth_pair<const N: usize>(previous: &[u8], first: &mut [u8], second: &mut [u8]) {
    let (aboves, _) = previous.as_chunks::<N>();
    let (firsts, _) = first.as_chunks_mut::<N>();
    let (seconds, _) = second.as_chunks_mut::<N>();
    let pixels = firsts.len();
    assert_eq!(aboves.len(), pixels);
    assert_eq!(seconds.len(), pixels);
    let mut first_left = [0u8; N];
    let mut first_corner = [0u8; N];
    let mut second_left = [0u8; N];
    let mut second_corner = [0u8; N];
    for index in 0..pixels {
        let above = aboves[index];
        let mut upper = firsts[index];
        for lane in 0..N {
            upper[lane] = upper[lane].wrapping_add(paeth_decode(
                first_left[lane],
                above[lane],
                first_corner[lane],
            ));
        }
        firsts[index] = upper;
        let mut lower = seconds[index];
        for lane in 0..N {
            lower[lane] = lower[lane].wrapping_add(paeth_decode(
                second_left[lane],
                upper[lane],
                second_corner[lane],
            ));
        }
        seconds[index] = lower;
        first_left = upper;
        first_corner = above;
        second_left = lower;
        second_corner = upper;
    }
}

#[cfg(target_arch = "aarch64")]
fn paeth_pair_wide<const N: usize>(previous: &[u8], first: &mut [u8], second: &mut [u8]) {
    let mut first_left = [0i16; N];
    let mut first_corner = [0i16; N];
    let mut second_left = [0i16; N];
    let mut second_corner = [0i16; N];
    for ((upper, lower), above) in first
        .as_chunks_mut::<N>()
        .0
        .iter_mut()
        .zip(second.as_chunks_mut::<N>().0.iter_mut())
        .zip(previous.as_chunks::<N>().0)
    {
        let mut upper_decoded = [0i16; N];
        let mut lower_decoded = [0i16; N];
        for lane in 0..N {
            let up = i16::from(above[lane]);
            upper_decoded[lane] = (i16::from(upper[lane])
                + paeth_decode_wide(first_left[lane], up, first_corner[lane]))
                & i16::from(u8::MAX);
            upper[lane] = upper_decoded[lane] as u8;
            lower_decoded[lane] = (i16::from(lower[lane])
                + paeth_decode_wide(second_left[lane], upper_decoded[lane], second_corner[lane]))
                & i16::from(u8::MAX);
            lower[lane] = lower_decoded[lane] as u8;
            first_corner[lane] = up;
        }
        first_left = upper_decoded;
        second_left = lower_decoded;
        second_corner = upper_decoded;
    }
}

fn unfilter_pixels<const N: usize>(filter: Filter, previous: &[u8], row: &mut [u8]) {
    match filter {
        Filter::None => {}
        Filter::Up => {
            for (value, &above) in row.iter_mut().zip(previous) {
                *value = value.wrapping_add(above);
            }
        }
        Filter::Sub => {
            let mut left = [0u8; N];
            for pixel in row.as_chunks_mut::<N>().0 {
                for lane in 0..N {
                    pixel[lane] = pixel[lane].wrapping_add(left[lane]);
                }
                left = *pixel;
            }
        }
        Filter::Average => {
            let mut left = [0u8; N];
            for (pixel, above) in row
                .as_chunks_mut::<N>()
                .0
                .iter_mut()
                .zip(previous.as_chunks::<N>().0)
            {
                for lane in 0..N {
                    pixel[lane] = pixel[lane].wrapping_add(average(left[lane], above[lane]));
                }
                left = *pixel;
            }
        }
        Filter::Paeth => {
            let mut left = [0u8; N];
            let mut corner = [0u8; N];
            for (pixel, above) in row
                .as_chunks_mut::<N>()
                .0
                .iter_mut()
                .zip(previous.as_chunks::<N>().0)
            {
                for lane in 0..N {
                    pixel[lane] = pixel[lane].wrapping_add(paeth_decode(
                        left[lane],
                        above[lane],
                        corner[lane],
                    ));
                }
                left = *pixel;
                corner = *above;
            }
        }
    }
}

#[inline(always)]
fn paeth_encode(left: u8, up: u8, up_left: u8) -> u8 {
    let left_distance = up.max(up_left) - up.min(up_left);
    let up_distance = left.max(up_left) - left.min(up_left);
    let corner_distance = if (left < up_left) == (up_left < up) {
        left_distance.max(up_distance) - left_distance.min(up_distance)
    } else {
        u8::MAX
    };
    if left_distance <= up_distance && left_distance <= corner_distance {
        left
    } else if up_distance <= corner_distance {
        up
    } else {
        up_left
    }
}

#[inline(always)]
const fn average_encode(left: u8, up: u8) -> u8 {
    (left & up) + ((left ^ up) >> 1)
}

fn predicted(
    output: &mut [u8],
    row: &[u8],
    left: &[u8],
    above: &[u8],
    corner: &[u8],
    predict: impl Fn(u8, u8, u8) -> u8,
) {
    let (outputs, output_rest) = output.as_chunks_mut::<FILTER_LANES>();
    let (rows, row_rest) = row.as_chunks::<FILTER_LANES>();
    let (lefts, left_rest) = left.as_chunks::<FILTER_LANES>();
    let (aboves, above_rest) = above.as_chunks::<FILTER_LANES>();
    let (corners, corner_rest) = corner.as_chunks::<FILTER_LANES>();
    for ((((out, row), left), above), corner) in outputs
        .iter_mut()
        .zip(rows)
        .zip(lefts)
        .zip(aboves)
        .zip(corners)
    {
        for lane in 0..FILTER_LANES {
            out[lane] = row[lane].wrapping_sub(predict(left[lane], above[lane], corner[lane]));
        }
    }
    for ((((out, &row), &left), &above), &corner) in output_rest
        .iter_mut()
        .zip(row_rest)
        .zip(left_rest)
        .zip(above_rest)
        .zip(corner_rest)
    {
        *out = row.wrapping_sub(predict(left, above, corner));
    }
}

fn apply(filter: Filter, bytes_per_pixel: usize, previous: &[u8], row: &[u8], output: &mut [u8]) {
    assert_eq!(previous.len(), row.len());
    assert_eq!(output.len(), row.len());
    assert!(bytes_per_pixel <= row.len());
    let lead = bytes_per_pixel;
    let rest = &row[lead..];
    let left = &row[..row.len() - lead];
    let above_rest = &previous[lead..];
    let corner = &previous[..previous.len() - lead];
    let (output_lead, output_rest) = output.split_at_mut(lead);
    let row_lead = &row[..lead];
    match filter {
        Filter::None => {
            output_lead.copy_from_slice(row_lead);
            output_rest.copy_from_slice(rest);
        }
        Filter::Sub => {
            output_lead.copy_from_slice(row_lead);
            predicted(output_rest, rest, left, above_rest, corner, |left, _, _| {
                left
            });
        }
        Filter::Up => {
            predicted(
                output_lead,
                row_lead,
                row_lead,
                &previous[..lead],
                row_lead,
                |_, above, _| above,
            );
            predicted(
                output_rest,
                rest,
                left,
                above_rest,
                corner,
                |_, above, _| above,
            );
        }
        Filter::Average => {
            for ((out, &value), &above) in output_lead.iter_mut().zip(row_lead).zip(previous) {
                *out = value.wrapping_sub(above / 2);
            }
            predicted(
                output_rest,
                rest,
                left,
                above_rest,
                corner,
                |left, above, _| average_encode(left, above),
            );
        }
        Filter::Paeth => {
            for ((out, &value), &above) in output_lead.iter_mut().zip(row_lead).zip(previous) {
                *out = value.wrapping_sub(paeth_encode(0, above, 0));
            }
            predicted(output_rest, rest, left, above_rest, corner, paeth_encode);
        }
    }
}

fn magnitude(bytes: &[u8]) -> u64 {
    let (lanes, rest) = bytes.as_chunks::<MAGNITUDE_LANES>();
    let mut sum = 0u64;
    for lane in lanes {
        let mut partial = 0u32;
        for &byte in lane {
            partial += u32::from(byte.cast_signed().unsigned_abs());
        }
        sum += u64::from(partial);
    }
    sum + rest
        .iter()
        .map(|&byte| u64::from(byte.cast_signed().unsigned_abs()))
        .sum::<u64>()
}

pub(crate) fn adaptive(
    bytes_per_pixel: usize,
    previous: &[u8],
    row: &[u8],
    output: &mut [u8],
) -> Filter {
    let mut best = (u64::MAX, Filter::None);
    for filter in ADAPTIVE_CANDIDATES {
        apply(filter, bytes_per_pixel, previous, row, output);
        let sum = magnitude(output);
        if sum <= best.0 {
            best = (sum, filter);
        }
    }
    if best.1 != Filter::Paeth {
        apply(best.1, bytes_per_pixel, previous, row, output);
    }
    best.1
}
