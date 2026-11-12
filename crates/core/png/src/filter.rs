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
    match bytes_per_pixel {
        1 => unfilter_pixels::<1>(filter, previous, row),
        2 => unfilter_pixels::<2>(filter, previous, row),
        3 => unfilter_pixels::<3>(filter, previous, row),
        4 => unfilter_pixels::<4>(filter, previous, row),
        6 => unfilter_pixels::<6>(filter, previous, row),
        8 => unfilter_pixels::<8>(filter, previous, row),
        other => unreachable!("PNG pixels are 1, 2, 3, 4, 6 or 8 bytes, not {other}"),
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
