pub(crate) const LANES: usize = 16;
const HALF_LANES: usize = LANES / 2;
const SIGN_BIT: u8 = 0x80;
const ADJUST_ROUNDING_LOW: i8 = 3;
const ADJUST_ROUNDING_HIGH: i8 = 4;
const ADJUST_SHIFT: u32 = 3;
const WIDE_TAPS: [i16; 3] = [27, 18, 9];
const WIDE_ROUNDING: i16 = 63;
const WIDE_SHIFT: u32 = 7;
const SIMPLE_CHANGED: core::ops::Range<usize> = 3..5;
const NORMAL_ROWS: usize = 8;

type Lanes = [u8; LANES];

#[derive(Clone, Copy)]
pub(crate) enum EdgeKind {
    Macroblock,
    Subblock,
}

#[derive(Clone, Copy)]
pub(crate) struct Thresholds {
    pub(crate) edge: u8,
    pub(crate) interior: u8,
    pub(crate) hev: u8,
}

#[inline(always)]
const fn signed(value: u8) -> i8 {
    (value ^ SIGN_BIT) as i8
}

#[inline(always)]
const fn unsigned(value: i8) -> u8 {
    (value as u8) ^ SIGN_BIT
}

#[inline(always)]
const fn edge_passes(edge: u8, p1: u8, p0: u8, q0: u8, q1: u8) -> bool {
    let near = p0.abs_diff(q0);
    near.saturating_add(near)
        .saturating_add(p1.abs_diff(q1) >> 1)
        <= edge
}

#[inline(always)]
#[allow(clippy::too_many_arguments)]
fn normal_passes(
    thresholds: Thresholds,
    p3: u8,
    p2: u8,
    p1: u8,
    p0: u8,
    q0: u8,
    q1: u8,
    q2: u8,
    q3: u8,
) -> bool {
    let interior_max = p3
        .abs_diff(p2)
        .max(p2.abs_diff(p1))
        .max(p1.abs_diff(p0))
        .max(q3.abs_diff(q2))
        .max(q2.abs_diff(q1))
        .max(q1.abs_diff(q0));
    edge_passes(thresholds.edge, p1, p0, q0, q1) & (interior_max <= thresholds.interior)
}

#[inline(always)]
const fn high_edge_variance(threshold: u8, p1: u8, p0: u8, q0: u8, q1: u8) -> bool {
    (p1.abs_diff(p0) > threshold) | (q1.abs_diff(q0) > threshold)
}

#[inline(always)]
const fn base_adjust(outer: i8, p0: i8, q0: i8) -> i8 {
    let step = q0.saturating_sub(p0);
    outer
        .saturating_add(step)
        .saturating_add(step)
        .saturating_add(step)
}

#[inline(always)]
const fn common_adjust(base: i8, p0: i8, q0: i8) -> (i8, i8, i8) {
    let low = base.saturating_add(ADJUST_ROUNDING_LOW) >> ADJUST_SHIFT;
    let high = base.saturating_add(ADJUST_ROUNDING_HIGH) >> ADJUST_SHIFT;
    (p0.saturating_add(low), q0.saturating_sub(high), high)
}

#[inline(always)]
const fn wide_adjust(tap: i16, weight: i8) -> i8 {
    ((tap * weight as i16 + WIDE_ROUNDING) >> WIDE_SHIFT) as i8
}

#[inline(always)]
const fn select(filter: bool, new: i8, old: u8) -> u8 {
    if filter { unsigned(new) } else { old }
}

fn simple_lanes(edge: u8, rows: &mut [Lanes; NORMAL_ROWS]) {
    assert!(edge < u8::MAX);
    let [_, _, p1, p0, q0, q1, _, _] = rows;
    for lane in 0..LANES {
        let filter = edge_passes(edge, p1[lane], p0[lane], q0[lane], q1[lane]);
        let (sp1, sp0, sq0, sq1) = (
            signed(p1[lane]),
            signed(p0[lane]),
            signed(q0[lane]),
            signed(q1[lane]),
        );
        let base = base_adjust(sp1.saturating_sub(sq1), sp0, sq0);
        let (new_p0, new_q0, _) = common_adjust(base, sp0, sq0);
        p0[lane] = select(filter, new_p0, p0[lane]);
        q0[lane] = select(filter, new_q0, q0[lane]);
    }
}

fn subblock_lanes(thresholds: Thresholds, rows: &mut [Lanes; NORMAL_ROWS]) {
    assert!(thresholds.edge < u8::MAX);
    let [p3, p2, p1, p0, q0, q1, q2, q3] = rows;
    for lane in 0..LANES {
        let filter = normal_passes(
            thresholds, p3[lane], p2[lane], p1[lane], p0[lane], q0[lane], q1[lane], q2[lane],
            q3[lane],
        );
        let variance = high_edge_variance(thresholds.hev, p1[lane], p0[lane], q0[lane], q1[lane]);
        let (sp1, sp0, sq0, sq1) = (
            signed(p1[lane]),
            signed(p0[lane]),
            signed(q0[lane]),
            signed(q1[lane]),
        );
        let outer = if variance { sp1.saturating_sub(sq1) } else { 0 };
        let base = base_adjust(outer, sp0, sq0);
        let (new_p0, new_q0, high) = common_adjust(base, sp0, sq0);
        let adjust = (high + 1) >> 1;
        let new_p1 = if variance {
            sp1
        } else {
            sp1.saturating_add(adjust)
        };
        let new_q1 = if variance {
            sq1
        } else {
            sq1.saturating_sub(adjust)
        };
        p1[lane] = select(filter, new_p1, p1[lane]);
        p0[lane] = select(filter, new_p0, p0[lane]);
        q0[lane] = select(filter, new_q0, q0[lane]);
        q1[lane] = select(filter, new_q1, q1[lane]);
    }
}

fn macroblock_lanes(thresholds: Thresholds, rows: &mut [Lanes; NORMAL_ROWS]) {
    assert!(thresholds.edge < u8::MAX);
    let [p3, p2, p1, p0, q0, q1, q2, q3] = rows;
    let [tap0, tap1, tap2] = WIDE_TAPS;
    for lane in 0..LANES {
        let filter = normal_passes(
            thresholds, p3[lane], p2[lane], p1[lane], p0[lane], q0[lane], q1[lane], q2[lane],
            q3[lane],
        );
        let variance = high_edge_variance(thresholds.hev, p1[lane], p0[lane], q0[lane], q1[lane]);
        let (sp2, sp1, sp0, sq0, sq1, sq2) = (
            signed(p2[lane]),
            signed(p1[lane]),
            signed(p0[lane]),
            signed(q0[lane]),
            signed(q1[lane]),
            signed(q2[lane]),
        );
        let weight = base_adjust(sp1.saturating_sub(sq1), sp0, sq0);
        let (narrow_p0, narrow_q0, _) = common_adjust(weight, sp0, sq0);
        let adjust0 = wide_adjust(tap0, weight);
        let adjust1 = wide_adjust(tap1, weight);
        let adjust2 = wide_adjust(tap2, weight);
        let new_p0 = if variance {
            narrow_p0
        } else {
            sp0.saturating_add(adjust0)
        };
        let new_q0 = if variance {
            narrow_q0
        } else {
            sq0.saturating_sub(adjust0)
        };
        let new_p1 = if variance {
            sp1
        } else {
            sp1.saturating_add(adjust1)
        };
        let new_q1 = if variance {
            sq1
        } else {
            sq1.saturating_sub(adjust1)
        };
        let new_p2 = if variance {
            sp2
        } else {
            sp2.saturating_add(adjust2)
        };
        let new_q2 = if variance {
            sq2
        } else {
            sq2.saturating_sub(adjust2)
        };
        p2[lane] = select(filter, new_p2, p2[lane]);
        p1[lane] = select(filter, new_p1, p1[lane]);
        p0[lane] = select(filter, new_p0, p0[lane]);
        q0[lane] = select(filter, new_q0, q0[lane]);
        q1[lane] = select(filter, new_q1, q1[lane]);
        q2[lane] = select(filter, new_q2, q2[lane]);
    }
}

fn normal_lanes(kind: EdgeKind, thresholds: Thresholds, rows: &mut [Lanes; NORMAL_ROWS]) {
    match kind {
        EdgeKind::Macroblock => macroblock_lanes(thresholds, rows),
        EdgeKind::Subblock => subblock_lanes(thresholds, rows),
    }
}

const fn changed_rows(kind: EdgeKind) -> core::ops::Range<usize> {
    match kind {
        EdgeKind::Macroblock => 1..NORMAL_ROWS - 1,
        EdgeKind::Subblock => 2..NORMAL_ROWS - 2,
    }
}

#[inline(always)]
fn load_rows<const WIDTH: usize>(
    plane: &[u8],
    start: usize,
    stride: usize,
    rows: &mut [Lanes; NORMAL_ROWS],
    lane_offset: usize,
) {
    for (row, values) in rows.iter_mut().enumerate() {
        values[lane_offset..lane_offset + WIDTH]
            .copy_from_slice(&plane[start + row * stride..][..WIDTH]);
    }
}

#[inline(always)]
fn store_rows<const WIDTH: usize>(
    plane: &mut [u8],
    start: usize,
    stride: usize,
    rows: &[Lanes; NORMAL_ROWS],
    lane_offset: usize,
    changed: core::ops::Range<usize>,
) {
    for row in changed {
        plane[start + row * stride..][..WIDTH]
            .copy_from_slice(&rows[row][lane_offset..lane_offset + WIDTH]);
    }
}

#[inline(always)]
fn load_columns(top: &[u8], bottom: &[u8], stride: usize) -> [Lanes; NORMAL_ROWS] {
    if let Some(columns) = nanocode_sys::webp::gather_columns(top, bottom, stride) {
        return columns;
    }
    let mut rows = [[0; LANES]; NORMAL_ROWS];
    for lane in 0..LANES {
        let (plane, row) = if lane < HALF_LANES {
            (top, lane)
        } else {
            (bottom, lane - HALF_LANES)
        };
        let pixels = &plane[row * stride..][..NORMAL_ROWS];
        for (values, &pixel) in rows.iter_mut().zip(pixels) {
            values[lane] = pixel;
        }
    }
    rows
}

#[inline(always)]
fn store_columns(top: &mut [u8], bottom: &mut [u8], stride: usize, rows: &[Lanes; NORMAL_ROWS]) {
    if nanocode_sys::webp::scatter_columns(top, bottom, stride, rows) {
        return;
    }
    for lane in 0..LANES {
        let (plane, row) = if lane < HALF_LANES {
            (&mut *top, lane)
        } else {
            (&mut *bottom, lane - HALF_LANES)
        };
        let pixels = &mut plane[row * stride..][..NORMAL_ROWS];
        for (pixel, values) in pixels.iter_mut().zip(rows) {
            *pixel = values[lane];
        }
    }
}

fn filter_luma(
    plane: &mut [u8],
    point: usize,
    stride: usize,
    vertical: bool,
    filter: impl Fn(&mut [Lanes; NORMAL_ROWS]),
    changed: core::ops::Range<usize>,
) {
    let before = NORMAL_ROWS / 2;
    if vertical {
        let start = point - before;
        let mut rows = load_columns(
            &plane[start..],
            &plane[start + HALF_LANES * stride..],
            stride,
        );
        filter(&mut rows);
        let (top, bottom) = plane[start..].split_at_mut(HALF_LANES * stride);
        store_columns(top, bottom, stride, &rows);
    } else {
        let start = point - before * stride;
        let mut rows = [[0; LANES]; NORMAL_ROWS];
        load_rows::<LANES>(plane, start, stride, &mut rows, 0);
        filter(&mut rows);
        store_rows::<LANES>(plane, start, stride, &rows, 0, changed);
    }
}

pub(crate) fn simple_edge(plane: &mut [u8], point: usize, stride: usize, vertical: bool, edge: u8) {
    let filter = |rows: &mut [Lanes; NORMAL_ROWS]| simple_lanes(edge, rows);
    filter_luma(plane, point, stride, vertical, filter, SIMPLE_CHANGED);
}

pub(crate) fn luma_edge(
    plane: &mut [u8],
    point: usize,
    stride: usize,
    vertical: bool,
    kind: EdgeKind,
    thresholds: Thresholds,
) {
    let filter = |rows: &mut [Lanes; NORMAL_ROWS]| normal_lanes(kind, thresholds, rows);
    filter_luma(plane, point, stride, vertical, filter, changed_rows(kind));
}

pub(crate) fn chroma_edge(
    planes: [&mut [u8]; 2],
    point: usize,
    stride: usize,
    vertical: bool,
    kind: EdgeKind,
    thresholds: Thresholds,
) {
    let [blue, red] = planes;
    let before = NORMAL_ROWS / 2;
    if vertical {
        let start = point - before;
        let mut rows = load_columns(&blue[start..], &red[start..], stride);
        normal_lanes(kind, thresholds, &mut rows);
        store_columns(&mut blue[start..], &mut red[start..], stride, &rows);
    } else {
        let start = point - before * stride;
        let mut rows = [[0; LANES]; NORMAL_ROWS];
        load_rows::<HALF_LANES>(blue, start, stride, &mut rows, 0);
        load_rows::<HALF_LANES>(red, start, stride, &mut rows, HALF_LANES);
        normal_lanes(kind, thresholds, &mut rows);
        let changed = changed_rows(kind);
        store_rows::<HALF_LANES>(blue, start, stride, &rows, 0, changed.clone());
        store_rows::<HALF_LANES>(red, start, stride, &rows, HALF_LANES, changed);
    }
}
