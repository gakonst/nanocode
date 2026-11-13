pub const COLUMN_LANES: usize = 16;
pub const COLUMN_TAPS: usize = 8;
const HALF_LANES: usize = COLUMN_LANES / 2;

pub type Columns = [[u8; COLUMN_LANES]; COLUMN_TAPS];

#[cfg(target_arch = "x86_64")]
const fn accelerated() -> bool {
    cfg!(target_feature = "sse2")
}

#[cfg(target_arch = "aarch64")]
const fn accelerated() -> bool {
    cfg!(target_feature = "neon")
}

#[inline]
fn read_rows(plane: &[u8], stride: usize) -> [u64; HALF_LANES] {
    assert!(stride >= COLUMN_TAPS);
    core::array::from_fn(|row| {
        let bytes: [u8; COLUMN_TAPS] = plane[row * stride..][..COLUMN_TAPS]
            .try_into()
            .expect("a row has COLUMN_TAPS bytes");
        u64::from_le_bytes(bytes)
    })
}

#[inline]
fn write_rows(plane: &mut [u8], stride: usize, rows: [u64; HALF_LANES]) {
    assert!(stride >= COLUMN_TAPS);
    for (row, value) in rows.into_iter().enumerate() {
        plane[row * stride..][..COLUMN_TAPS].copy_from_slice(&value.to_le_bytes());
    }
}

#[inline]
pub fn gather_columns(top: &[u8], bottom: &[u8], stride: usize) -> Option<Columns> {
    #[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
    if accelerated() {
        let (top, bottom) = (read_rows(top, stride), read_rows(bottom, stride));
        return Some(unsafe { kernel::gather(top, bottom) });
    }
    let _ = (top, bottom, stride, read_rows);
    None
}

#[inline]
pub fn scatter_columns(
    top: &mut [u8],
    bottom: &mut [u8],
    stride: usize,
    columns: &Columns,
) -> bool {
    #[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
    if accelerated() {
        let (top_rows, bottom_rows) = unsafe { kernel::scatter(columns) };
        write_rows(top, stride, top_rows);
        write_rows(bottom, stride, bottom_rows);
        return true;
    }
    let _ = (top, bottom, stride, columns, write_rows);
    false
}

#[cfg(target_arch = "x86_64")]
mod kernel {
    use super::{COLUMN_LANES, COLUMN_TAPS, Columns, HALF_LANES};
    use core::arch::x86_64::{
        __m128i, _mm_cvtsi64_si128, _mm_cvtsi128_si64, _mm_set_epi64x, _mm_unpackhi_epi8,
        _mm_unpackhi_epi16, _mm_unpackhi_epi32, _mm_unpackhi_epi64, _mm_unpacklo_epi8,
        _mm_unpacklo_epi16, _mm_unpacklo_epi32, _mm_unpacklo_epi64,
    };

    #[inline]
    #[target_feature(enable = "sse2")]
    fn vector(low: u64, high: u64) -> __m128i {
        _mm_set_epi64x(high.cast_signed(), low.cast_signed())
    }

    #[inline]
    #[target_feature(enable = "sse2")]
    fn halves(value: __m128i) -> (u64, u64) {
        let low = _mm_cvtsi128_si64(value).cast_unsigned();
        let high = _mm_cvtsi128_si64(_mm_unpackhi_epi64(value, value)).cast_unsigned();
        (low, high)
    }

    #[inline]
    #[target_feature(enable = "sse2")]
    fn lanes(value: __m128i) -> [u8; COLUMN_LANES] {
        let (low, high) = halves(value);
        let mut bytes = [0; COLUMN_LANES];
        bytes[..HALF_LANES].copy_from_slice(&low.to_le_bytes());
        bytes[HALF_LANES..].copy_from_slice(&high.to_le_bytes());
        bytes
    }

    #[inline]
    #[target_feature(enable = "sse2")]
    pub(super) fn gather(top: [u64; HALF_LANES], bottom: [u64; HALF_LANES]) -> Columns {
        let rows: [__m128i; COLUMN_LANES] = core::array::from_fn(|row| {
            let value = if row < HALF_LANES {
                top[row]
            } else {
                bottom[row - HALF_LANES]
            };
            _mm_cvtsi64_si128(value.cast_signed())
        });
        let pairs: [__m128i; 8] =
            core::array::from_fn(|pair| _mm_unpacklo_epi8(rows[2 * pair], rows[2 * pair + 1]));
        let quads_low: [__m128i; 4] =
            core::array::from_fn(|quad| _mm_unpacklo_epi16(pairs[2 * quad], pairs[2 * quad + 1]));
        let quads_high: [__m128i; 4] =
            core::array::from_fn(|quad| _mm_unpackhi_epi16(pairs[2 * quad], pairs[2 * quad + 1]));
        let octets: [[__m128i; 4]; 2] = core::array::from_fn(|half| {
            let (first, second) = (2 * half, 2 * half + 1);
            [
                _mm_unpacklo_epi32(quads_low[first], quads_low[second]),
                _mm_unpackhi_epi32(quads_low[first], quads_low[second]),
                _mm_unpacklo_epi32(quads_high[first], quads_high[second]),
                _mm_unpackhi_epi32(quads_high[first], quads_high[second]),
            ]
        });
        core::array::from_fn(|column| {
            let pair = column / 2;
            let joined = if column % 2 == 0 {
                _mm_unpacklo_epi64(octets[0][pair], octets[1][pair])
            } else {
                _mm_unpackhi_epi64(octets[0][pair], octets[1][pair])
            };
            lanes(joined)
        })
    }

    #[inline]
    #[target_feature(enable = "sse2")]
    pub(super) fn scatter(columns: &Columns) -> ([u64; HALF_LANES], [u64; HALF_LANES]) {
        let vectors: [__m128i; COLUMN_TAPS] = core::array::from_fn(|column| {
            let low: [u8; HALF_LANES] = columns[column][..HALF_LANES]
                .try_into()
                .expect("a column has COLUMN_LANES bytes");
            let high: [u8; HALF_LANES] = columns[column][HALF_LANES..]
                .try_into()
                .expect("a column has COLUMN_LANES bytes");
            vector(u64::from_le_bytes(low), u64::from_le_bytes(high))
        });
        let pairs: [[__m128i; 4]; 2] = [
            core::array::from_fn(|pair| {
                _mm_unpacklo_epi8(vectors[2 * pair], vectors[2 * pair + 1])
            }),
            core::array::from_fn(|pair| {
                _mm_unpackhi_epi8(vectors[2 * pair], vectors[2 * pair + 1])
            }),
        ];
        let rows = |half: usize| -> [u64; HALF_LANES] {
            let pairs = pairs[half];
            let first_low = _mm_unpacklo_epi16(pairs[0], pairs[1]);
            let first_high = _mm_unpackhi_epi16(pairs[0], pairs[1]);
            let second_low = _mm_unpacklo_epi16(pairs[2], pairs[3]);
            let second_high = _mm_unpackhi_epi16(pairs[2], pairs[3]);
            let joined = [
                halves(_mm_unpacklo_epi32(first_low, second_low)),
                halves(_mm_unpackhi_epi32(first_low, second_low)),
                halves(_mm_unpacklo_epi32(first_high, second_high)),
                halves(_mm_unpackhi_epi32(first_high, second_high)),
            ];
            core::array::from_fn(|row| {
                let (low, high) = joined[row / 2];
                if row % 2 == 0 { low } else { high }
            })
        };
        (rows(0), rows(1))
    }
}

#[cfg(target_arch = "aarch64")]
mod kernel {
    use super::{COLUMN_LANES, COLUMN_TAPS, Columns, HALF_LANES};
    use core::arch::aarch64::{
        uint8x8_t, vcreate_u8, vget_lane_u64, vreinterpret_u8_u16, vreinterpret_u8_u32,
        vreinterpret_u16_u8, vreinterpret_u32_u16, vreinterpret_u64_u8, vtrn_u8, vtrn_u16,
        vtrn_u32,
    };

    #[inline]
    #[target_feature(enable = "neon")]
    fn transpose(rows: [u64; HALF_LANES]) -> [u64; HALF_LANES] {
        let rows: [uint8x8_t; HALF_LANES] = core::array::from_fn(|row| vcreate_u8(rows[row]));
        let bytes: [_; 4] =
            core::array::from_fn(|pair| vtrn_u8(rows[2 * pair], rows[2 * pair + 1]));
        let words = |first: uint8x8_t, second: uint8x8_t| {
            vtrn_u16(vreinterpret_u16_u8(first), vreinterpret_u16_u8(second))
        };
        let quad0 = words(bytes[0].0, bytes[1].0);
        let quad1 = words(bytes[0].1, bytes[1].1);
        let quad2 = words(bytes[2].0, bytes[3].0);
        let quad3 = words(bytes[2].1, bytes[3].1);
        let doubles = |first, second| {
            let pair = vtrn_u32(vreinterpret_u32_u16(first), vreinterpret_u32_u16(second));
            (vreinterpret_u8_u32(pair.0), vreinterpret_u8_u32(pair.1))
        };
        let (column0, column4) = doubles(quad0.0, quad2.0);
        let (column1, column5) = doubles(quad1.0, quad3.0);
        let (column2, column6) = doubles(quad0.1, quad2.1);
        let (column3, column7) = doubles(quad1.1, quad3.1);
        let _ = vreinterpret_u8_u16;
        [
            column0, column1, column2, column3, column4, column5, column6, column7,
        ]
        .map(|column| vget_lane_u64::<0>(vreinterpret_u64_u8(column)))
    }

    #[inline]
    #[target_feature(enable = "neon")]
    pub(super) fn gather(top: [u64; HALF_LANES], bottom: [u64; HALF_LANES]) -> Columns {
        let top = transpose(top);
        let bottom = transpose(bottom);
        core::array::from_fn(|column| {
            let mut bytes = [0; COLUMN_LANES];
            bytes[..HALF_LANES].copy_from_slice(&top[column].to_le_bytes());
            bytes[HALF_LANES..].copy_from_slice(&bottom[column].to_le_bytes());
            bytes
        })
    }

    #[inline]
    #[target_feature(enable = "neon")]
    pub(super) fn scatter(columns: &Columns) -> ([u64; HALF_LANES], [u64; HALF_LANES]) {
        let half = |offset: usize| -> [u64; COLUMN_TAPS] {
            core::array::from_fn(|column| {
                let bytes: [u8; HALF_LANES] = columns[column][offset..offset + HALF_LANES]
                    .try_into()
                    .expect("a column has COLUMN_LANES bytes");
                u64::from_le_bytes(bytes)
            })
        };
        (transpose(half(0)), transpose(half(HALF_LANES)))
    }
}
