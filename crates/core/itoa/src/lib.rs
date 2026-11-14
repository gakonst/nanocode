use nanocode_sys::DecimalText;

const U128_DIGITS_MAX: usize = u128::MAX.ilog10() as usize + 1;
const I128_TEXT_BYTES_MAX: usize = i128::MAX.ilog10() as usize + 2;
const RADIX: u32 = 10;
const PAIR_SCALE: u32 = RADIX * RADIX;
const QUAD_SCALE: u32 = PAIR_SCALE * PAIR_SCALE;
const OCTET_SCALE: u32 = 100_000_000;
const CHUNK_SCALE: u64 = 10_000_000_000_000_000;
const CHUNK_DIGITS: usize = 16;
const CHUNK_RECIPROCAL: u128 = 76_624_777_043_294_442_917_917_351_357_515_459_181;
const CHUNK_RECIPROCAL_SHIFT: u32 = 51;
const LOW_CHUNK_START: usize = I128_TEXT_BYTES_MAX - CHUNK_DIGITS;
const MIDDLE_CHUNK_START: usize = LOW_CHUNK_START - CHUNK_DIGITS;
const HALF_BITS: u32 = 64;

type Text = DecimalText<I128_TEXT_BYTES_MAX>;

#[derive(Clone, Copy, Default)]
pub struct Buffer {
    text: Text,
}

impl Buffer {
    #[inline]
    #[must_use]
    pub const fn new() -> Self {
        Self { text: Text::new() }
    }

    #[inline]
    pub fn format<I: Integer>(&mut self, value: I) -> &str {
        let start = value.write(&mut self.text);
        assert!(I128_TEXT_BYTES_MAX - start <= I::MAX_STR_LEN);
        self.text.text(start)
    }
}

pub trait Integer: private::Sealed {
    const MAX_STR_LEN: usize;
}

mod private {
    pub trait Sealed: Copy {
        fn write(self, text: &mut super::Text) -> usize;
    }
}

#[inline(always)]
fn write_small(text: &mut Text, mut end: usize, value: u32) -> usize {
    let mut rest = value;
    if rest >= PAIR_SCALE {
        end -= 2;
        text.put_pair(end, rest % PAIR_SCALE);
        rest /= PAIR_SCALE;
    }
    if rest >= RADIX {
        end -= 2;
        text.put_pair(end, rest);
    } else {
        end -= 1;
        text.put_digit(end, rest);
    }
    end
}

#[inline(always)]
fn write_u32(text: &mut Text, mut end: usize, mut value: u32) -> usize {
    while value >= QUAD_SCALE {
        end -= 4;
        text.put_quad(end, value % QUAD_SCALE);
        value /= QUAD_SCALE;
    }
    write_small(text, end, value)
}

#[inline(always)]
fn write_u64(text: &mut Text, mut end: usize, mut value: u64) -> usize {
    while value >= u64::from(QUAD_SCALE) {
        end -= 4;
        text.put_quad(end, (value % u64::from(QUAD_SCALE)) as u32);
        value /= u64::from(QUAD_SCALE);
    }
    write_small(text, end, value as u32)
}

#[inline(always)]
fn put_octet(text: &mut Text, at: usize, value: u32) {
    assert!(value < OCTET_SCALE);
    text.put_quad(at, value / QUAD_SCALE);
    text.put_quad(at + 4, value % QUAD_SCALE);
}

#[inline(always)]
fn write_chunk<const START: usize>(text: &mut Text, value: u64) {
    let high = u32::try_from(value / u64::from(OCTET_SCALE)).expect("a chunk has sixteen digits");
    put_octet(text, START, high);
    put_octet(text, START + 8, (value % u64::from(OCTET_SCALE)) as u32);
}

#[inline(always)]
fn multiply_wide(left: u64, right: u64) -> u128 {
    u128::from(left) * u128::from(right)
}

#[inline]
fn multiply_high(left: u128, right: u128) -> u128 {
    let (left_high, left_low) = ((left >> HALF_BITS) as u64, left as u64);
    let (right_high, right_low) = ((right >> HALF_BITS) as u64, right as u64);
    let low_carry = multiply_wide(left_low, right_low) >> HALF_BITS;
    let cross_left = multiply_wide(left_high, right_low) + low_carry;
    let cross_right = multiply_wide(left_low, right_high) + u128::from(cross_left as u64);
    multiply_wide(left_high, right_high) + (cross_left >> HALF_BITS) + (cross_right >> HALF_BITS)
}

#[inline]
fn split_chunk(value: u128) -> (u128, u64) {
    let quotient = multiply_high(value, CHUNK_RECIPROCAL) >> CHUNK_RECIPROCAL_SHIFT;
    let remainder = value - quotient * u128::from(CHUNK_SCALE);
    assert!(remainder < u128::from(CHUNK_SCALE));
    (quotient, remainder as u64)
}

#[inline(always)]
fn write_u128(text: &mut Text, end: usize, value: u128) -> usize {
    if value < u128::from(CHUNK_SCALE) {
        return write_u64(text, end, value as u64);
    }
    write_wide(text, end, value)
}

fn write_wide(text: &mut Text, end: usize, value: u128) -> usize {
    assert_eq!(end, I128_TEXT_BYTES_MAX);
    let (high, low) = split_chunk(value);
    write_chunk::<LOW_CHUNK_START>(text, low);
    if high < u128::from(CHUNK_SCALE) {
        return write_u64(text, LOW_CHUNK_START, high as u64);
    }
    let (top, middle) = split_chunk(high);
    write_chunk::<MIDDLE_CHUNK_START>(text, middle);
    let top = u32::try_from(top).expect("a u128 over 10^32 leaves at most seven digits");
    let start = write_u32(text, MIDDLE_CHUNK_START, top);
    assert!(I128_TEXT_BYTES_MAX - start <= U128_DIGITS_MAX);
    start
}

#[inline(always)]
const fn put_sign(text: &mut Text, start: usize, negative: bool) -> usize {
    if negative {
        text.put_minus(start - 1);
        return start - 1;
    }
    start
}

macro_rules! integers {
    ($($unsigned:ty, $signed:ty, $write:ident, $wide:ty;)*) => {$(
        impl Integer for $unsigned {
            const MAX_STR_LEN: usize = <$unsigned>::MAX.ilog10() as usize + 1;
        }

        impl private::Sealed for $unsigned {
            #[inline]
            fn write(self, text: &mut Text) -> usize {
                $write(text, I128_TEXT_BYTES_MAX, <$wide>::from(self))
            }
        }

        impl Integer for $signed {
            const MAX_STR_LEN: usize = <$signed>::MAX.ilog10() as usize + 2;
        }

        impl private::Sealed for $signed {
            #[inline]
            fn write(self, text: &mut Text) -> usize {
                let start = $write(text, I128_TEXT_BYTES_MAX, <$wide>::from(self.unsigned_abs()));
                put_sign(text, start, self < 0)
            }
        }
    )*};
}

integers! {
    u8, i8, write_u32, u32;
    u16, i16, write_u32, u32;
    u32, i32, write_u32, u32;
    u64, i64, write_u64, u64;
    u128, i128, write_u128, u128;
}

#[cfg(target_pointer_width = "16")]
type PointerUnsigned = u16;
#[cfg(target_pointer_width = "32")]
type PointerUnsigned = u32;
#[cfg(target_pointer_width = "64")]
type PointerUnsigned = u64;
#[cfg(target_pointer_width = "16")]
type PointerSigned = i16;
#[cfg(target_pointer_width = "32")]
type PointerSigned = i32;
#[cfg(target_pointer_width = "64")]
type PointerSigned = i64;

impl Integer for usize {
    const MAX_STR_LEN: Self = PointerUnsigned::MAX_STR_LEN;
}

impl private::Sealed for usize {
    #[inline]
    fn write(self, text: &mut Text) -> usize {
        PointerUnsigned::try_from(self)
            .expect("usize has the width of its pointer-sized counterpart")
            .write(text)
    }
}

impl Integer for isize {
    const MAX_STR_LEN: usize = PointerSigned::MAX_STR_LEN;
}

impl private::Sealed for isize {
    #[inline]
    fn write(self, text: &mut Text) -> usize {
        PointerSigned::try_from(self)
            .expect("isize has the width of its pointer-sized counterpart")
            .write(text)
    }
}
