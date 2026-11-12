use super::super::lzma_common::{
    BIT_MODEL_TOTAL, BIT_MODEL_TOTAL_BITS, MOVE_BITS, RANGE_SHIFT_BITS, RANGE_TOP_VALUE,
};

pub(super) const SYMBOLS_MAX: usize = 53;
const FLUSH_SYMBOLS: usize = 5;
const LOW_CARRY_SHIFT_BITS: u32 = 32;
const LOW_CACHE_SHIFT_BITS: u32 = 24;
const LOW_PENDING_LIMIT: u32 = 0xFF00_0000;
const LOW_KEPT_MASK: u64 = 0x00FF_FFFF;
const CACHE_PENDING_BYTE: u8 = 0xFF;

#[derive(Clone, Copy)]
enum Symbol {
    Bit0(usize),
    Bit1(usize),
    Direct0,
    Direct1,
    Flush,
}

pub(super) struct RangeEncoder {
    low: u64,
    cache_size: u64,
    range: u32,
    cache: u8,
    count: usize,
    position: usize,
    symbols: [Symbol; SYMBOLS_MAX],
}

impl RangeEncoder {
    pub(super) const fn new() -> Self {
        Self {
            low: 0,
            cache_size: 1,
            range: u32::MAX,
            cache: 0,
            count: 0,
            position: 0,
            symbols: [Symbol::Flush; SYMBOLS_MAX],
        }
    }

    pub(super) const fn reset(&mut self) {
        self.low = 0;
        self.cache_size = 1;
        self.range = u32::MAX;
        self.cache = 0;
        self.count = 0;
        self.position = 0;
    }

    const fn push(&mut self, symbol: Symbol) {
        self.symbols[self.count] = symbol;
        self.count += 1;
    }

    pub(super) const fn bit(&mut self, probability: usize, bit: u32) {
        self.push(if bit == 0 {
            Symbol::Bit0(probability)
        } else {
            Symbol::Bit1(probability)
        });
    }

    pub(super) fn bittree(&mut self, base: usize, bit_count: u32, symbol: u32) {
        let mut model_index = 1;
        for shift in (0..bit_count).rev() {
            let bit = (symbol >> shift) & 1;
            self.bit(base + model_index, bit);
            model_index = (model_index << 1) + bit as usize;
        }
    }

    pub(super) fn bittree_reverse(&mut self, base: usize, bit_count: u32, symbol: u32) {
        let mut symbol = symbol;
        let mut model_index = 1;
        for _ in 0..bit_count {
            let bit = symbol & 1;
            symbol >>= 1;
            self.bit(base + model_index, bit);
            model_index = (model_index << 1) + bit as usize;
        }
    }

    pub(super) fn direct(&mut self, value: u32, bit_count: u32) {
        for shift in (0..bit_count).rev() {
            self.push(if (value >> shift) & 1 == 0 {
                Symbol::Direct0
            } else {
                Symbol::Direct1
            });
        }
    }

    pub(super) fn flush(&mut self) {
        for _ in 0..FLUSH_SYMBOLS {
            self.push(Symbol::Flush);
        }
    }

    pub(super) const fn pending(&self) -> u64 {
        self.cache_size + FLUSH_SYMBOLS as u64 - 1
    }

    fn shift_low(&mut self, output: &mut [u8], output_position: &mut usize) -> bool {
        let carry = (self.low >> LOW_CARRY_SHIFT_BITS) as u8;
        if (self.low as u32) < LOW_PENDING_LIMIT || carry != 0 {
            loop {
                if *output_position == output.len() {
                    return true;
                }
                output[*output_position] = self.cache.wrapping_add(carry);
                *output_position += 1;
                self.cache = CACHE_PENDING_BYTE;
                self.cache_size -= 1;
                if self.cache_size == 0 {
                    break;
                }
            }
            self.cache = ((self.low >> LOW_CACHE_SHIFT_BITS) & u64::from(u8::MAX)) as u8;
        }
        self.cache_size += 1;
        self.low = (self.low & LOW_KEPT_MASK) << RANGE_SHIFT_BITS;
        false
    }

    pub(super) fn encode(
        &mut self,
        probabilities: &mut [u16],
        output: &mut [u8],
        output_position: &mut usize,
    ) -> bool {
        while self.position < self.count {
            if self.range < RANGE_TOP_VALUE {
                if self.shift_low(output, output_position) {
                    return true;
                }
                self.range <<= RANGE_SHIFT_BITS;
            }
            match self.symbols[self.position] {
                Symbol::Bit0(index) => {
                    let probability = u32::from(probabilities[index]);
                    self.range = (self.range >> BIT_MODEL_TOTAL_BITS) * probability;
                    probabilities[index] =
                        (probability + ((BIT_MODEL_TOTAL - probability) >> MOVE_BITS)) as u16;
                }
                Symbol::Bit1(index) => {
                    let probability = probabilities[index];
                    let bound = u32::from(probability) * (self.range >> BIT_MODEL_TOTAL_BITS);
                    self.low += u64::from(bound);
                    self.range -= bound;
                    probabilities[index] = probability - (probability >> MOVE_BITS);
                }
                Symbol::Direct0 => self.range >>= 1,
                Symbol::Direct1 => {
                    self.range >>= 1;
                    self.low += u64::from(self.range);
                }
                Symbol::Flush => return self.encode_flush(output, output_position),
            }
            self.position += 1;
        }
        self.count = 0;
        self.position = 0;
        false
    }

    fn encode_flush(&mut self, output: &mut [u8], output_position: &mut usize) -> bool {
        self.range = u32::MAX;
        loop {
            if self.shift_low(output, output_position) {
                return true;
            }
            self.position += 1;
            if self.position >= self.count {
                break;
            }
        }
        self.reset();
        false
    }
}
