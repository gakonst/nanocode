use super::super::lzma_common::{
    BIT_MODEL_TOTAL, BIT_MODEL_TOTAL_BITS, MOVE_BITS, RANGE_SHIFT_BITS, RANGE_TOP_VALUE,
};
use super::super::stream::Error;

const INIT_BYTES: u32 = 5;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Initialization {
    NeedsInput,
    Ready,
}

#[derive(Clone, Copy)]
pub(super) struct RangeDecoder {
    range: u32,
    code: u32,
    init_bytes_left: u32,
}

impl RangeDecoder {
    pub(super) const fn new() -> Self {
        Self {
            range: u32::MAX,
            code: 0,
            init_bytes_left: INIT_BYTES,
        }
    }

    pub(super) fn read_init(
        &mut self,
        input: &[u8],
        input_position: &mut usize,
    ) -> Result<Initialization, Error> {
        while self.init_bytes_left > 0 {
            let Some(&byte) = input.get(*input_position) else {
                return Ok(Initialization::NeedsInput);
            };
            if self.init_bytes_left == INIT_BYTES && byte != 0 {
                return Err(Error::Data);
            }
            self.code = (self.code << RANGE_SHIFT_BITS) | u32::from(byte);
            *input_position += 1;
            self.init_bytes_left -= 1;
        }
        Ok(Initialization::Ready)
    }

    pub(super) const fn cursor(self, input: &[u8], position: usize) -> RangeCursor<'_> {
        RangeCursor {
            range: self.range,
            code: self.code,
            input,
            position,
        }
    }

    pub(super) const fn save(&mut self, cursor: &RangeCursor<'_>) {
        self.range = cursor.range;
        self.code = cursor.code;
    }
}

#[derive(Clone, Copy)]
pub(super) struct RangeCursor<'a> {
    range: u32,
    code: u32,
    input: &'a [u8],
    pub(super) position: usize,
}

impl RangeCursor<'_> {
    pub(super) const fn is_finished(&self) -> bool {
        self.code == 0
    }

    #[inline(always)]
    fn normalize(&mut self) {
        if self.range < RANGE_TOP_VALUE {
            self.range <<= RANGE_SHIFT_BITS;
            self.code = (self.code << RANGE_SHIFT_BITS) | u32::from(self.input[self.position]);
            self.position += 1;
        }
    }

    #[inline(always)]
    pub(super) fn normalize_safe(&mut self) -> bool {
        if self.range < RANGE_TOP_VALUE {
            let Some(&byte) = self.input.get(self.position) else {
                return false;
            };
            self.range <<= RANGE_SHIFT_BITS;
            self.code = (self.code << RANGE_SHIFT_BITS) | u32::from(byte);
            self.position += 1;
        }
        true
    }

    #[inline(always)]
    fn decide(&mut self, probability: &mut u16) -> bool {
        let current = u32::from(*probability);
        let bound = (self.range >> BIT_MODEL_TOTAL_BITS) * current;
        let bit = self.code >= bound;
        let one_mask = 0_u32.wrapping_sub(u32::from(bit));
        let increase = ((BIT_MODEL_TOTAL - current) >> MOVE_BITS) & !one_mask;
        let decrease = (current >> MOVE_BITS) & one_mask;
        self.range = (bound & !one_mask) | (self.range.wrapping_sub(bound) & one_mask);
        self.code -= bound & one_mask;
        *probability = (current + increase - decrease) as u16;
        bit
    }

    #[inline(always)]
    pub(super) fn bit(&mut self, probability: &mut u16) -> bool {
        self.normalize();
        self.decide(probability)
    }

    #[inline(always)]
    pub(super) fn bit_safe(&mut self, probability: &mut u16) -> Option<bool> {
        if !self.normalize_safe() {
            return None;
        }
        Some(self.decide(probability))
    }

    #[inline(always)]
    const fn direct_step(&mut self) -> u32 {
        self.range >>= 1;
        self.code = self.code.wrapping_sub(self.range);
        let mask = 0_u32.wrapping_sub(self.code >> 31);
        self.code = self.code.wrapping_add(self.range & mask);
        mask.wrapping_add(1)
    }

    #[inline(always)]
    pub(super) fn direct(&mut self, value: u32) -> u32 {
        self.normalize();
        (value << 1).wrapping_add(self.direct_step())
    }

    #[inline(always)]
    pub(super) fn direct_safe(&mut self, value: u32) -> Option<u32> {
        if !self.normalize_safe() {
            return None;
        }
        Some((value << 1).wrapping_add(self.direct_step()))
    }
}
