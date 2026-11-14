use std::ops::Range;
use std::str;

const RADIX: u32 = 10;
const PAIR_SCALE: u32 = RADIX * RADIX;
const DIGIT_PAIRS: [[u8; 2]; PAIR_SCALE as usize] = digit_pairs();

const fn digit_pairs() -> [[u8; 2]; PAIR_SCALE as usize] {
    let mut pairs = [[0; 2]; PAIR_SCALE as usize];
    let mut index = 0;
    while index < PAIR_SCALE {
        pairs[index as usize] = [b'0' + (index / RADIX) as u8, b'0' + (index % RADIX) as u8];
        index += 1;
    }
    pairs
}

#[derive(Clone, Copy)]
pub struct DecimalText<const BYTES: usize> {
    bytes: [u8; BYTES],
}

impl<const BYTES: usize> Default for DecimalText<BYTES> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const BYTES: usize> DecimalText<BYTES> {
    #[inline]
    #[must_use]
    pub const fn new() -> Self {
        Self { bytes: [b'0'; BYTES] }
    }

    #[inline]
    pub fn put_pair(&mut self, at: usize, pair: u32) {
        self.bytes[at..at + 2].copy_from_slice(&DIGIT_PAIRS[pair as usize]);
    }

    #[inline]
    pub fn put_quad(&mut self, at: usize, quad: u32) {
        let digits = &mut self.bytes[at..at + 4];
        digits[..2].copy_from_slice(&DIGIT_PAIRS[(quad / PAIR_SCALE) as usize]);
        digits[2..].copy_from_slice(&DIGIT_PAIRS[(quad % PAIR_SCALE) as usize]);
    }

    #[inline]
    pub fn put_digit(&mut self, at: usize, digit: u32) {
        assert!(digit < RADIX, "a decimal digit is below ten");
        self.bytes[at] = b'0' + digit as u8;
    }

    #[inline]
    pub const fn put_minus(&mut self, at: usize) {
        self.bytes[at] = b'-';
    }

    #[inline]
    pub const fn put_point(&mut self, at: usize) {
        self.bytes[at] = b'.';
    }

    #[inline]
    pub const fn put_exponent(&mut self, at: usize, negative: bool) {
        self.bytes[at] = b'e';
        self.bytes[at + 1] = if negative { b'-' } else { b'+' };
    }

    #[inline]
    pub fn put_zeros(&mut self, range: Range<usize>) {
        self.bytes[range].fill(b'0');
    }

    #[inline]
    pub fn copy_within(&mut self, source: Range<usize>, dest: usize) {
        self.bytes.copy_within(source, dest);
    }

    #[inline]
    #[must_use]
    pub fn text(&self, start: usize) -> &str {
        self.span(start, BYTES)
    }

    #[inline]
    #[must_use]
    pub fn span(&self, start: usize, end: usize) -> &str {
        let ascii = &self.bytes[start..end];
        unsafe { str::from_utf8_unchecked(ascii) }
    }
}
