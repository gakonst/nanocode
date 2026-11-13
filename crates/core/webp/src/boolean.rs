use crate::error::WebpError;

const RANGE_INITIAL: u32 = 255;
const BIT_COUNT_INITIAL: i32 = -8;
const WORD_BYTES: usize = 4;
const WORD_BITS: i32 = 32;
const BYTE_BITS: i32 = 8;
const FLAG_PROBABILITY: u8 = 128;
const RANGE_NORMALIZED_LEADING_ZEROS: u32 = 24;
pub(crate) const LEAF: u8 = 0x80;

pub(crate) struct BooleanDecoder<'a> {
    data: &'a [u8],
    position: usize,
    value: u64,
    range: u32,
    bit_count: i32,
    trailing_zero_read: bool,
    exhausted: bool,
}

impl<'a> BooleanDecoder<'a> {
    pub(crate) const fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            position: 0,
            value: 0,
            range: RANGE_INITIAL,
            bit_count: BIT_COUNT_INITIAL,
            trailing_zero_read: false,
            exhausted: false,
        }
    }

    #[cold]
    fn load_tail(&mut self) -> bool {
        if self.position < self.data.len() {
            self.value = (self.value << BYTE_BITS) | u64::from(self.data[self.position]);
            self.position += 1;
            self.bit_count += BYTE_BITS;
            return true;
        }
        if !self.trailing_zero_read {
            self.trailing_zero_read = true;
            self.value <<= BYTE_BITS;
            self.bit_count += BYTE_BITS;
            return true;
        }
        self.exhausted = true;
        false
    }

    #[inline(always)]
    fn load(&mut self) -> bool {
        if let Some(word) = self.data.get(self.position..self.position + WORD_BYTES) {
            let word = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
            self.value = (self.value << WORD_BITS) | u64::from(word);
            self.position += WORD_BYTES;
            self.bit_count += WORD_BITS;
            return true;
        }
        self.load_tail()
    }

    #[inline(always)]
    pub(crate) fn read_bit(&mut self, probability: u8) -> bool {
        if self.bit_count < 0 && !self.load() {
            return false;
        }
        let split = 1 + (((self.range - 1) * u32::from(probability)) >> 8);
        let split_shifted = u64::from(split) << self.bit_count;
        let bit = if self.value >= split_shifted {
            self.range -= split;
            self.value -= split_shifted;
            true
        } else {
            self.range = split;
            false
        };
        let shift = self
            .range
            .leading_zeros()
            .saturating_sub(RANGE_NORMALIZED_LEADING_ZEROS);
        self.range <<= shift;
        self.bit_count -= shift as i32;
        bit
    }

    pub(crate) fn read_flag(&mut self) -> bool {
        self.read_bit(FLAG_PROBABILITY)
    }

    pub(crate) fn read_literal(&mut self, bits: u8) -> u8 {
        let mut value = 0u8;
        for _ in 0..bits {
            value = (value << 1) + u8::from(self.read_flag());
        }
        value
    }

    pub(crate) fn read_signed(&mut self, bits: u8) -> i32 {
        if !self.read_flag() {
            return 0;
        }
        let magnitude = i32::from(self.read_literal(bits));
        if self.read_flag() {
            -magnitude
        } else {
            magnitude
        }
    }

    #[inline(always)]
    pub(crate) fn read_tree(&mut self, tree: &[[u8; 2]], probabilities: &[u8], start: usize) -> u8 {
        let mut index = start;
        for _ in 0..tree.len() {
            let next = tree[index][usize::from(self.read_bit(probabilities[index]))];
            if next & LEAF != 0 {
                return next & !LEAF;
            }
            index = usize::from(next);
        }
        unreachable!("trees are acyclic and end in leaves")
    }

    pub(crate) const fn ensure(&self) -> Result<(), WebpError> {
        if self.exhausted {
            Err(WebpError::Bitstream)
        } else {
            Ok(())
        }
    }
}

pub(crate) const fn tree<const NODES: usize, const BRANCHES: usize>(
    branches: [i8; BRANCHES],
) -> [[u8; 2]; NODES] {
    assert!(BRANCHES == 2 * NODES);
    let mut nodes = [[0u8; 2]; NODES];
    let mut index = 0;
    while index < BRANCHES {
        let branch = branches[index];
        nodes[index / 2][index % 2] = if branch > 0 {
            (branch as u8) / 2
        } else {
            LEAF | ((-branch) as u8)
        };
        index += 1;
    }
    nodes
}
