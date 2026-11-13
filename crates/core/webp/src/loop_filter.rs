const SIGNED_MIN: i32 = -128;
const SIGNED_MAX: i32 = 127;
const SIGNED_OFFSET: i32 = 128;

#[inline(always)]
const fn clamp(value: i32) -> i32 {
    if value < SIGNED_MIN {
        SIGNED_MIN
    } else if value > SIGNED_MAX {
        SIGNED_MAX
    } else {
        value
    }
}

#[inline(always)]
fn signed(value: u8) -> i32 {
    i32::from(value) - SIGNED_OFFSET
}

#[inline(always)]
const fn unsigned(value: i32) -> u8 {
    (clamp(value) + SIGNED_OFFSET) as u8
}

#[inline(always)]
fn common_adjust(outer_taps: bool, pixels: &mut [u8], point: usize, stride: usize) -> i32 {
    let p1 = signed(pixels[point - 2 * stride]);
    let p0 = signed(pixels[point - stride]);
    let q0 = signed(pixels[point]);
    let q1 = signed(pixels[point + stride]);
    let outer = if outer_taps { clamp(p1 - q1) } else { 0 };
    let base = clamp(outer + 3 * (q0 - p0));
    let low = clamp(base + 3) >> 3;
    let high = clamp(base + 4) >> 3;
    pixels[point] = unsigned(q0 - high);
    pixels[point - stride] = unsigned(p0 + low);
    high
}

#[inline(always)]
fn simple_threshold(limit: i32, pixels: &[u8], point: usize, stride: usize) -> bool {
    i32::from(pixels[point - stride].abs_diff(pixels[point])) * 2
        + i32::from(pixels[point - 2 * stride].abs_diff(pixels[point + stride])) / 2
        <= limit
}

#[inline(always)]
fn should_filter(interior: u8, edge: u8, pixels: &[u8], point: usize, stride: usize) -> bool {
    simple_threshold(i32::from(edge), pixels, point, stride)
        && pixels[point - 4 * stride].abs_diff(pixels[point - 3 * stride]) <= interior
        && pixels[point - 3 * stride].abs_diff(pixels[point - 2 * stride]) <= interior
        && pixels[point - 2 * stride].abs_diff(pixels[point - stride]) <= interior
        && pixels[point + 3 * stride].abs_diff(pixels[point + 2 * stride]) <= interior
        && pixels[point + 2 * stride].abs_diff(pixels[point + stride]) <= interior
        && pixels[point + stride].abs_diff(pixels[point]) <= interior
}

#[inline(always)]
const fn high_edge_variance(threshold: u8, pixels: &[u8], point: usize, stride: usize) -> bool {
    pixels[point - 2 * stride].abs_diff(pixels[point - stride]) > threshold
        || pixels[point + stride].abs_diff(pixels[point]) > threshold
}

#[inline(always)]
pub(crate) fn simple_segment(edge: u8, pixels: &mut [u8], point: usize, stride: usize) {
    if simple_threshold(i32::from(edge), pixels, point, stride) {
        common_adjust(true, pixels, point, stride);
    }
}

#[inline(always)]
pub(crate) fn subblock_filter(
    hev_threshold: u8,
    interior: u8,
    edge: u8,
    pixels: &mut [u8],
    point: usize,
    stride: usize,
) {
    if should_filter(interior, edge, pixels, point, stride) {
        let variance = high_edge_variance(hev_threshold, pixels, point, stride);
        let adjust = (common_adjust(variance, pixels, point, stride) + 1) >> 1;
        if !variance {
            pixels[point + stride] = unsigned(signed(pixels[point + stride]) - adjust);
            pixels[point - 2 * stride] = unsigned(signed(pixels[point - 2 * stride]) + adjust);
        }
    }
}

#[inline(always)]
pub(crate) fn macroblock_filter(
    hev_threshold: u8,
    interior: u8,
    edge: u8,
    pixels: &mut [u8],
    point: usize,
    stride: usize,
) {
    if !should_filter(interior, edge, pixels, point, stride) {
        return;
    }
    if high_edge_variance(hev_threshold, pixels, point, stride) {
        common_adjust(true, pixels, point, stride);
        return;
    }
    let p2 = signed(pixels[point - 3 * stride]);
    let p1 = signed(pixels[point - 2 * stride]);
    let p0 = signed(pixels[point - stride]);
    let q0 = signed(pixels[point]);
    let q1 = signed(pixels[point + stride]);
    let q2 = signed(pixels[point + 2 * stride]);
    let weight = clamp(clamp(p1 - q1) + 3 * (q0 - p0));
    let adjust = clamp((27 * weight + 63) >> 7);
    pixels[point] = unsigned(q0 - adjust);
    pixels[point - stride] = unsigned(p0 + adjust);
    let adjust = clamp((18 * weight + 63) >> 7);
    pixels[point + stride] = unsigned(q1 - adjust);
    pixels[point - 2 * stride] = unsigned(p1 + adjust);
    let adjust = clamp((9 * weight + 63) >> 7);
    pixels[point + 2 * stride] = unsigned(q2 - adjust);
    pixels[point - 3 * stride] = unsigned(p2 + adjust);
}
