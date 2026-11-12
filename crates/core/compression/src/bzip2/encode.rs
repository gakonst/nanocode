use alloc::vec::Vec;

use super::huffman;
use super::{
    ALPHABET_SIZE_MAX, BYTE_VALUES, GROUP_SYMBOLS, GROUPS_MAX, RUN_A, RUN_B, SELECTORS_MAX,
};

const CODE_LENGTH_ENCODE_MAX: usize = 17;
const COST_LESSER: u8 = 0;
const COST_GREATER: u8 = 15;
const REFINEMENT_ITERATIONS: usize = 4;
const GROUP_COUNT_THRESHOLDS: [(usize, usize); 4] = [(200, 2), (600, 3), (1200, 4), (2400, 5)];
const GROUP_COUNT_LARGEST: usize = 6;
const BYTE_BITS: u32 = u8::BITS;
const WORD_BITS: u32 = u32::BITS;
const IN_USE_GROUP_BYTES: usize = 16;
const GROUP_COUNT_BITS: u32 = 3;
const SELECTOR_COUNT_BITS: u32 = 15;
const TABLE_START_BITS: u32 = 5;
const LENGTH_DELTA_BITS: u32 = 2;
const LENGTH_INCREMENT: u32 = 2;
const LENGTH_DECREMENT: u32 = 3;
const RUN_DIGITS: usize = 2;

pub(super) struct BitWriter {
    buffer: u32,
    live: u32,
}

impl BitWriter {
    pub(super) const fn new() -> Self {
        Self { buffer: 0, live: 0 }
    }

    pub(super) const fn reset(&mut self) {
        self.buffer = 0;
        self.live = 0;
    }

    pub(super) fn write(&mut self, output: &mut Vec<u8>, count: u32, value: u32) {
        while self.live >= BYTE_BITS {
            output.push(self.buffer.to_be_bytes()[0]);
            self.buffer <<= BYTE_BITS;
            self.live -= BYTE_BITS;
        }
        assert!(count <= WORD_BITS - self.live);
        assert!(count == WORD_BITS || value >> count == 0);
        self.buffer |= value << (WORD_BITS - self.live - count);
        self.live += count;
    }

    pub(super) fn write_bytes(&mut self, output: &mut Vec<u8>, bytes: &[u8]) {
        for &byte in bytes {
            self.write(output, BYTE_BITS, u32::from(byte));
        }
    }

    pub(super) fn finish(&mut self, output: &mut Vec<u8>) {
        for _ in 0..self.live.div_ceil(BYTE_BITS) {
            output.push(self.buffer.to_be_bytes()[0]);
            self.buffer <<= BYTE_BITS;
        }
        self.live = 0;
    }
}

pub(super) struct Tables {
    symbols: Vec<u16>,
    frequencies: [i32; ALPHABET_SIZE_MAX],
    selectors: Vec<u8>,
    lengths: [[u8; ALPHABET_SIZE_MAX]; GROUPS_MAX],
    codes: [[u32; ALPHABET_SIZE_MAX]; GROUPS_MAX],
    group_frequencies: [[i32; ALPHABET_SIZE_MAX]; GROUPS_MAX],
}

pub(super) struct Block<'a> {
    pub(super) bytes: &'a [u8],
    pub(super) order: &'a [u32],
    pub(super) in_use: &'a [bool; BYTE_VALUES],
}

impl Tables {
    pub(super) fn new(block_bytes_max: usize) -> Self {
        Self {
            symbols: Vec::with_capacity(block_bytes_max + 1),
            frequencies: [0; ALPHABET_SIZE_MAX],
            selectors: Vec::with_capacity(SELECTORS_MAX),
            lengths: [[0; ALPHABET_SIZE_MAX]; GROUPS_MAX],
            codes: [[0; ALPHABET_SIZE_MAX]; GROUPS_MAX],
            group_frequencies: [[0; ALPHABET_SIZE_MAX]; GROUPS_MAX],
        }
    }

    pub(super) fn encode(
        &mut self,
        block: &Block<'_>,
        writer: &mut BitWriter,
        output: &mut Vec<u8>,
    ) {
        let symbols_in_use = self.generate_symbols(block);
        let alphabet_size = symbols_in_use + 2;
        let group_count = self.choose_tables(alphabet_size);
        for (codes, lengths) in self.codes[..group_count]
            .iter_mut()
            .zip(&self.lengths[..group_count])
        {
            let lengths = &lengths[..alphabet_size];
            let length_min = lengths
                .iter()
                .copied()
                .min()
                .expect("an alphabet has symbols");
            let length_max = lengths
                .iter()
                .copied()
                .max()
                .expect("an alphabet has symbols");
            assert!(usize::from(length_max) <= CODE_LENGTH_ENCODE_MAX);
            assert!(length_min >= 1);
            huffman::assign_codes(codes, lengths, length_min, length_max);
        }
        write_mapping(block.in_use, writer, output);
        self.write_selectors(group_count, writer, output);
        self.write_lengths(group_count, alphabet_size, writer, output);
        self.write_symbols(writer, output);
    }
}

impl Tables {
    fn generate_symbols(&mut self, block: &Block<'_>) -> usize {
        let mut sequence = [0_u8; BYTE_VALUES];
        let mut symbols_in_use = 0;
        for (byte, &used) in block.in_use.iter().enumerate() {
            if used {
                sequence[byte] = u8::try_from(symbols_in_use).expect("a byte index fits in u8");
                symbols_in_use += 1;
            }
        }
        assert!(symbols_in_use > 0);
        let end_of_block = symbols_in_use + 1;
        self.frequencies[..=end_of_block].fill(0);
        self.symbols.clear();
        let mut recent: [u8; BYTE_VALUES] =
            core::array::from_fn(|index| u8::try_from(index).expect("a byte index fits in u8"));
        let block_bytes = block.bytes.len();
        let mut zeros = 0_usize;
        for &position in &block.order[..block_bytes] {
            let position = position as usize;
            let previous = if position == 0 {
                block_bytes - 1
            } else {
                position - 1
            };
            let value = sequence[usize::from(block.bytes[previous])];
            if recent[0] == value {
                zeros += 1;
                continue;
            }
            self.push_zero_run(zeros);
            zeros = 0;
            let mut carried = recent[1];
            recent[1] = recent[0];
            let mut index = 1;
            while value != carried {
                index += 1;
                core::mem::swap(&mut carried, &mut recent[index]);
            }
            recent[0] = carried;
            self.push_symbol(index + 1);
        }
        self.push_zero_run(zeros);
        self.push_symbol(end_of_block);
        symbols_in_use
    }

    fn push_zero_run(&mut self, zeros: usize) {
        if zeros == 0 {
            return;
        }
        let mut remaining = zeros - 1;
        loop {
            let symbol = if remaining.is_multiple_of(RUN_DIGITS) {
                RUN_A
            } else {
                RUN_B
            };
            self.push_symbol(usize::from(symbol));
            if remaining < RUN_DIGITS {
                return;
            }
            remaining = (remaining - RUN_DIGITS) / RUN_DIGITS;
        }
    }

    fn push_symbol(&mut self, symbol: usize) {
        self.symbols
            .push(u16::try_from(symbol).expect("a symbol under 258 fits in u16"));
        self.frequencies[symbol] += 1;
    }

    fn choose_tables(&mut self, alphabet_size: usize) -> usize {
        let symbol_count = self.symbols.len();
        assert!(symbol_count > 0);
        for lengths in &mut self.lengths {
            lengths[..alphabet_size].fill(COST_GREATER);
        }
        let group_count = GROUP_COUNT_THRESHOLDS
            .iter()
            .find(|&&(limit, _)| symbol_count < limit)
            .map_or(GROUP_COUNT_LARGEST, |&(_, count)| count);
        self.initial_tables(alphabet_size, group_count);
        for _ in 0..REFINEMENT_ITERATIONS {
            self.refine_tables(alphabet_size, group_count);
            for (lengths, frequencies) in self.lengths[..group_count]
                .iter_mut()
                .zip(&self.group_frequencies)
            {
                huffman::make_code_lengths(
                    &mut lengths[..alphabet_size],
                    frequencies,
                    CODE_LENGTH_ENCODE_MAX,
                );
            }
        }
        assert!(self.selectors.len() <= SELECTORS_MAX);
        group_count
    }

    fn initial_tables(&mut self, alphabet_size: usize, group_count: usize) {
        let mut remaining_frequency =
            i32::try_from(self.symbols.len()).expect("a block is under 1 MB");
        let mut start = 0_usize;
        for parts_left in (1..=group_count).rev() {
            let parts = i32::try_from(parts_left).expect("a group count fits in i32");
            let target = remaining_frequency / parts;
            let mut taken = 0;
            let mut frequency = 0;
            while frequency < target && start + taken < alphabet_size {
                frequency += self.frequencies[start + taken];
                taken += 1;
            }
            if taken > 1
                && parts_left != group_count
                && parts_left != 1
                && (group_count - parts_left) % 2 == 1
            {
                taken -= 1;
                frequency -= self.frequencies[start + taken];
            }
            for (symbol, length) in self.lengths[parts_left - 1][..alphabet_size]
                .iter_mut()
                .enumerate()
            {
                *length = if (start..start + taken).contains(&symbol) {
                    COST_LESSER
                } else {
                    COST_GREATER
                };
            }
            start += taken;
            remaining_frequency -= frequency;
        }
    }
}

impl Tables {
    fn refine_tables(&mut self, alphabet_size: usize, group_count: usize) {
        for frequencies in &mut self.group_frequencies[..group_count] {
            frequencies[..alphabet_size].fill(0);
        }
        let mut lengths_by_symbol = [[0_u16; GROUPS_MAX]; ALPHABET_SIZE_MAX];
        for (symbol, lengths) in lengths_by_symbol[..alphabet_size].iter_mut().enumerate() {
            for (group, length) in lengths.iter_mut().enumerate() {
                *length = u16::from(self.lengths[group][symbol]);
            }
        }
        self.selectors.clear();
        for group in self.symbols.chunks(GROUP_SYMBOLS) {
            let mut costs = [0_u16; GROUPS_MAX];
            for &symbol in group {
                let lengths = &lengths_by_symbol[usize::from(symbol)];
                for (cost, &length) in costs.iter_mut().zip(lengths) {
                    *cost = cost.wrapping_add(length);
                }
            }
            let mut best = 0;
            for table in 1..group_count {
                if costs[table] < costs[best] {
                    best = table;
                }
            }
            self.selectors
                .push(u8::try_from(best).expect("a table index fits in u8"));
            let frequencies = &mut self.group_frequencies[best];
            for &symbol in group {
                frequencies[usize::from(symbol)] += 1;
            }
        }
    }

    fn write_selectors(&self, group_count: usize, writer: &mut BitWriter, output: &mut Vec<u8>) {
        let count = u32::try_from(group_count).expect("a group count fits in u32");
        writer.write(output, GROUP_COUNT_BITS, count);
        let selector_count =
            u32::try_from(self.selectors.len()).expect("a selector count fits in u32");
        writer.write(output, SELECTOR_COUNT_BITS, selector_count);
        let mut recent: [u8; GROUPS_MAX] =
            core::array::from_fn(|index| u8::try_from(index).expect("a table index fits in u8"));
        for &selector in &self.selectors {
            let index = recent
                .iter()
                .position(|&table| table == selector)
                .expect("a selector names a table");
            recent.copy_within(..index, 1);
            recent[0] = selector;
            for _ in 0..index {
                writer.write(output, 1, 1);
            }
            writer.write(output, 1, 0);
        }
    }

    fn write_lengths(
        &self,
        group_count: usize,
        alphabet_size: usize,
        writer: &mut BitWriter,
        output: &mut Vec<u8>,
    ) {
        for lengths in &self.lengths[..group_count] {
            let mut current = lengths[0];
            writer.write(output, TABLE_START_BITS, u32::from(current));
            for &length in &lengths[..alphabet_size] {
                while current < length {
                    writer.write(output, LENGTH_DELTA_BITS, LENGTH_INCREMENT);
                    current += 1;
                }
                while current > length {
                    writer.write(output, LENGTH_DELTA_BITS, LENGTH_DECREMENT);
                    current -= 1;
                }
                writer.write(output, 1, 0);
            }
        }
    }

    fn write_symbols(&self, writer: &mut BitWriter, output: &mut Vec<u8>) {
        assert_eq!(
            self.symbols.chunks(GROUP_SYMBOLS).len(),
            self.selectors.len()
        );
        for (group, &selector) in self.symbols.chunks(GROUP_SYMBOLS).zip(&self.selectors) {
            let lengths = &self.lengths[usize::from(selector)];
            let codes = &self.codes[usize::from(selector)];
            for &symbol in group {
                let symbol = usize::from(symbol);
                writer.write(output, u32::from(lengths[symbol]), codes[symbol]);
            }
        }
    }
}

fn write_mapping(in_use: &[bool; BYTE_VALUES], writer: &mut BitWriter, output: &mut Vec<u8>) {
    let groups: [bool; BYTE_VALUES / IN_USE_GROUP_BYTES] = core::array::from_fn(|group| {
        in_use[group * IN_USE_GROUP_BYTES..(group + 1) * IN_USE_GROUP_BYTES]
            .iter()
            .any(|&used| used)
    });
    for &group_used in &groups {
        writer.write(output, 1, u32::from(group_used));
    }
    for (group, &group_used) in groups.iter().enumerate() {
        if group_used {
            for &used in &in_use[group * IN_USE_GROUP_BYTES..(group + 1) * IN_USE_GROUP_BYTES] {
                writer.write(output, 1, u32::from(used));
            }
        }
    }
}
