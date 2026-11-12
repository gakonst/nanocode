use alloc::vec::Vec;

use super::DecodeError;
use super::bits::{Bits, low_mask};
use super::tables::{
    CODE_LENGTH_CODE_ORDER, CODE_LENGTH_CODES, CODE_LENGTH_PREFIX_BITS, CODE_LENGTH_PREFIX_LENGTH,
    CODE_LENGTH_PREFIX_VALUE,
};

pub(super) const ROOT_BITS: u32 = 8;
const ROOT_ENTRIES: usize = 1 << ROOT_BITS;
pub(super) const CODE_LENGTH_MAX: u32 = 15;
const LENGTH_HISTOGRAM: usize = CODE_LENGTH_MAX as usize + 1;
const COMPLETE_CODE: u32 = 1 << (CODE_LENGTH_MAX + 1);
pub(super) const ALPHABET_SIZE_MAX: usize = 704;
const SIMPLE_KIND: u32 = 1;
const KIND_BITS: u32 = 2;
const SIMPLE_COUNT_BITS: u32 = 2;
const SIMPLE_SYMBOLS_MAX: usize = 4;
const TREE_SELECT_BITS: u32 = 1;
const CODE_LENGTH_SPACE: u32 = 32;
const SYMBOL_SPACE: u32 = 1 << CODE_LENGTH_MAX;
const REPEAT_PREVIOUS: u8 = 16;
const REPEAT_PREVIOUS_EXTRA_BITS: u32 = 2;
const REPEAT_ZERO_EXTRA_BITS: u32 = 3;
const REPEAT_MIN: u32 = 3;
const REPEAT_CARRY: u32 = 2;
const INITIAL_PREVIOUS_LENGTH: u8 = 8;

#[derive(Clone, Copy, Default)]
pub(super) struct Entry {
    pub length: u8,
    pub value: u16,
}

#[inline(always)]
pub(super) fn decode(table: &[Entry], word: u64) -> (u16, u32) {
    decode_at(table, 0, word)
}

#[inline(always)]
pub(super) fn decode_at(tables: &[Entry], tree: usize, word: u64) -> (u16, u32) {
    let root = tables[tree + (word & low_mask(ROOT_BITS)) as usize];
    let length = u32::from(root.length);
    if length <= ROOT_BITS {
        return (root.value, length);
    }
    let index = tree + usize::from(root.value) + ((word >> ROOT_BITS) & low_mask(length - ROOT_BITS)) as usize;
    let entry = tables[index];
    (entry.value, ROOT_BITS + u32::from(entry.length))
}

const fn reverse(code: u32, length: u32) -> u32 {
    code.reverse_bits() >> (u32::BITS - length)
}

fn table_bits(remaining: &[u16; LENGTH_HISTOGRAM], length: u32) -> u32 {
    let mut length = length;
    let mut left: i32 = 1 << (length - ROOT_BITS);
    while length < CODE_LENGTH_MAX {
        left -= i32::from(remaining[length as usize]);
        if left <= 0 {
            break;
        }
        length += 1;
        left <<= 1;
    }
    length - ROOT_BITS
}

fn build_single(tables: &mut Vec<Entry>, symbol: u16) -> u32 {
    let start = tables.len();
    tables.resize(start + ROOT_ENTRIES, Entry { length: 0, value: symbol });
    start as u32
}

fn build(tables: &mut Vec<Entry>, histogram: &[u16; LENGTH_HISTOGRAM], sorted: &[u16]) -> u32 {
    let start = tables.len();
    tables.resize(start + ROOT_ENTRIES, Entry::default());
    let mut code: u32 = 0;
    let mut next = 0;
    for length in 1..=ROOT_BITS {
        for _ in 0..histogram[length as usize] {
            let entry = Entry { length: length as u8, value: sorted[next] };
            next += 1;
            let mut index = reverse(code, length) as usize;
            while index < ROOT_ENTRIES {
                tables[start + index] = entry;
                index += 1 << length;
            }
            code += 1;
        }
        code <<= 1;
    }
    let mut remaining = *histogram;
    let mut prefix_current = u32::MAX;
    let mut sub_start = 0;
    let mut sub_entries = 0;
    for length in ROOT_BITS + 1..=CODE_LENGTH_MAX {
        let low_length = length - ROOT_BITS;
        for _ in 0..histogram[length as usize] {
            let prefix = code >> low_length;
            if prefix != prefix_current {
                prefix_current = prefix;
                let sub_bits = table_bits(&remaining, length);
                sub_start = tables.len() - start;
                sub_entries = 1 << sub_bits;
                tables.resize(tables.len() + sub_entries, Entry::default());
                tables[start + reverse(prefix, ROOT_BITS) as usize] =
                    Entry { length: (ROOT_BITS + sub_bits) as u8, value: sub_start as u16 };
            }
            let entry = Entry { length: low_length as u8, value: sorted[next] };
            next += 1;
            let mut index = reverse(code & low_mask(low_length) as u32, low_length) as usize;
            while index < sub_entries {
                tables[start + sub_start + index] = entry;
                index += 1 << low_length;
            }
            remaining[length as usize] -= 1;
            code += 1;
        }
        code <<= 1;
    }
    assert!(code == COMPLETE_CODE);
    assert!(next == sorted.len());
    start as u32
}

#[derive(Clone, Copy)]
struct SymbolLengths {
    symbol: u32,
    space: u32,
    previous: u8,
    repeat: u32,
    repeat_length: u8,
}

#[derive(Clone, Copy)]
enum Step {
    Kind,
    SimpleCount,
    SimpleSymbols { count: usize, read: usize },
    SimpleTreeSelect,
    LengthCodes { index: usize, space: u32, codes: u32 },
    SymbolLengths(SymbolLengths),
}

pub(super) struct CodeReader {
    step: Step,
    symbols: [u16; SIMPLE_SYMBOLS_MAX],
    length_code_lengths: [u8; CODE_LENGTH_CODES],
    length_table: Vec<Entry>,
    lengths: [u8; ALPHABET_SIZE_MAX],
    sorted: [u16; ALPHABET_SIZE_MAX],
}

impl CodeReader {
    pub(super) const fn new() -> Self {
        Self {
            step: Step::Kind,
            symbols: [0; SIMPLE_SYMBOLS_MAX],
            length_code_lengths: [0; CODE_LENGTH_CODES],
            length_table: Vec::new(),
            lengths: [0; ALPHABET_SIZE_MAX],
            sorted: [0; ALPHABET_SIZE_MAX],
        }
    }

    pub(super) fn read(
        &mut self,
        bits: &mut Bits<'_>,
        alphabet_size: u32,
        tables: &mut Vec<Entry>,
    ) -> Result<Option<u32>, DecodeError> {
        assert!(alphabet_size >= 2);
        assert!(alphabet_size as usize <= ALPHABET_SIZE_MAX);
        loop {
            match self.step {
                Step::Kind => {
                    let Some(kind) = bits.try_read(KIND_BITS) else {
                        return Ok(None);
                    };
                    self.step = if kind == SIMPLE_KIND {
                        Step::SimpleCount
                    } else {
                        self.length_code_lengths = [0; CODE_LENGTH_CODES];
                        Step::LengthCodes { index: kind as usize, space: CODE_LENGTH_SPACE, codes: 0 }
                    };
                }
                Step::SimpleCount => {
                    let Some(count) = bits.try_read(SIMPLE_COUNT_BITS) else {
                        return Ok(None);
                    };
                    self.step = Step::SimpleSymbols { count: count as usize + 1, read: 0 };
                }
                Step::SimpleSymbols { count, read } => {
                    if read < count {
                        let width = u32::BITS - (alphabet_size - 1).leading_zeros();
                        let Some(symbol) = bits.try_read(width) else {
                            return Ok(None);
                        };
                        if symbol >= alphabet_size {
                            return Err(DecodeError::SimpleCodeSymbolTooLarge);
                        }
                        self.symbols[read] = symbol as u16;
                        self.step = Step::SimpleSymbols { count, read: read + 1 };
                        continue;
                    }
                    let symbols = &self.symbols[..count];
                    for (index, symbol) in symbols.iter().enumerate() {
                        if symbols[index + 1..].contains(symbol) {
                            return Err(DecodeError::SimpleCodeDuplicateSymbol);
                        }
                    }
                    if count == SIMPLE_SYMBOLS_MAX {
                        self.step = Step::SimpleTreeSelect;
                        continue;
                    }
                    self.step = Step::Kind;
                    return Ok(Some(self.build_simple(tables, count, false)));
                }
                Step::SimpleTreeSelect => {
                    let Some(select) = bits.try_read(TREE_SELECT_BITS) else {
                        return Ok(None);
                    };
                    self.step = Step::Kind;
                    return Ok(Some(self.build_simple(tables, SIMPLE_SYMBOLS_MAX, select == 1)));
                }
                Step::LengthCodes { index, space, codes } => {
                    if !self.read_length_codes(bits, index, space, codes, alphabet_size)? {
                        return Ok(None);
                    }
                }
                Step::SymbolLengths(state) => {
                    let Some(offset) = self.read_symbol_lengths(bits, state, alphabet_size, tables)? else {
                        return Ok(None);
                    };
                    self.step = Step::Kind;
                    return Ok(Some(offset));
                }
            }
        }
    }

    fn build_simple(&self, tables: &mut Vec<Entry>, count: usize, select: bool) -> u32 {
        let lengths: &[u8] = match (count, select) {
            (1, _) => return build_single(tables, self.symbols[0]),
            (2, _) => &[1, 1],
            (3, _) => &[1, 2, 2],
            (_, false) => &[2, 2, 2, 2],
            (_, true) => &[1, 2, 3, 3],
        };
        let mut pairs = [(0u8, 0u16); SIMPLE_SYMBOLS_MAX];
        let mut histogram = [0u16; LENGTH_HISTOGRAM];
        for index in 0..count {
            pairs[index] = (lengths[index], self.symbols[index]);
            histogram[usize::from(lengths[index])] += 1;
        }
        pairs[..count].sort_unstable();
        let mut sorted = [0u16; SIMPLE_SYMBOLS_MAX];
        for index in 0..count {
            sorted[index] = pairs[index].1;
        }
        build(tables, &histogram, &sorted[..count])
    }

    fn read_length_codes(
        &mut self,
        bits: &mut Bits<'_>,
        mut index: usize,
        mut space: u32,
        mut codes: u32,
        alphabet_size: u32,
    ) -> Result<bool, DecodeError> {
        while index < CODE_LENGTH_CODES {
            bits.refill();
            let peek = bits.peek(CODE_LENGTH_PREFIX_BITS) as usize;
            let length = u32::from(CODE_LENGTH_PREFIX_LENGTH[peek]);
            if length > bits.available() {
                self.step = Step::LengthCodes { index, space, codes };
                return Ok(false);
            }
            bits.consume(length);
            let value = CODE_LENGTH_PREFIX_VALUE[peek];
            self.length_code_lengths[usize::from(CODE_LENGTH_CODE_ORDER[index])] = value;
            index += 1;
            if value != 0 {
                space = space.wrapping_sub(CODE_LENGTH_SPACE >> value);
                codes += 1;
                if space.wrapping_sub(1) >= CODE_LENGTH_SPACE {
                    break;
                }
            }
        }
        if codes != 1 && space != 0 {
            return Err(DecodeError::InvalidCodeLengthCode);
        }
        self.length_table.clear();
        if codes == 1 {
            let symbol = self.length_code_lengths.iter().position(|&length| length != 0);
            build_single(&mut self.length_table, symbol.expect("one code length code") as u16);
        } else {
            let mut histogram = [0u16; LENGTH_HISTOGRAM];
            let count = sort_symbols(&self.length_code_lengths, &mut histogram, &mut self.sorted);
            build(&mut self.length_table, &histogram, &self.sorted[..count]);
        }
        self.lengths[..alphabet_size as usize].fill(0);
        self.step = Step::SymbolLengths(SymbolLengths {
            symbol: 0,
            space: SYMBOL_SPACE,
            previous: INITIAL_PREVIOUS_LENGTH,
            repeat: 0,
            repeat_length: 0,
        });
        Ok(true)
    }

    fn read_symbol_lengths(
        &mut self,
        bits: &mut Bits<'_>,
        mut state: SymbolLengths,
        alphabet_size: u32,
        tables: &mut Vec<Entry>,
    ) -> Result<Option<u32>, DecodeError> {
        while state.symbol < alphabet_size && state.space != 0 {
            bits.refill();
            let word = bits.word();
            let entry = self.length_table[(word & low_mask(ROOT_BITS)) as usize];
            let length = u32::from(entry.length);
            let code = entry.value as u8;
            if code < REPEAT_PREVIOUS {
                if length > bits.available() {
                    self.step = Step::SymbolLengths(state);
                    return Ok(None);
                }
                bits.consume(length);
                state.repeat = 0;
                if code != 0 {
                    self.lengths[state.symbol as usize] = code;
                    state.previous = code;
                    state.space = state.space.wrapping_sub(SYMBOL_SPACE >> code);
                }
                state.symbol += 1;
                continue;
            }
            let extra_bits = if code == REPEAT_PREVIOUS { REPEAT_PREVIOUS_EXTRA_BITS } else { REPEAT_ZERO_EXTRA_BITS };
            if length + extra_bits > bits.available() {
                self.step = Step::SymbolLengths(state);
                return Ok(None);
            }
            let delta = ((word >> length) & low_mask(extra_bits)) as u32;
            bits.consume(length + extra_bits);
            let repeat_length = if code == REPEAT_PREVIOUS { state.previous } else { 0 };
            if state.repeat_length != repeat_length {
                state.repeat = 0;
                state.repeat_length = repeat_length;
            }
            let previous_repeat = state.repeat;
            if state.repeat > 0 {
                state.repeat = (state.repeat - REPEAT_CARRY) << extra_bits;
            }
            state.repeat += delta + REPEAT_MIN;
            let count = state.repeat - previous_repeat;
            if state.symbol + count > alphabet_size {
                return Err(DecodeError::InvalidPrefixCode);
            }
            let end = (state.symbol + count) as usize;
            if repeat_length != 0 {
                self.lengths[state.symbol as usize..end].fill(repeat_length);
                state.space = state.space.wrapping_sub(count << (CODE_LENGTH_MAX - u32::from(repeat_length)));
            }
            state.symbol += count;
        }
        if state.space != 0 {
            return Err(DecodeError::InvalidPrefixCode);
        }
        let mut histogram = [0u16; LENGTH_HISTOGRAM];
        let count = sort_symbols(&self.lengths[..alphabet_size as usize], &mut histogram, &mut self.sorted);
        Ok(Some(build(tables, &histogram, &self.sorted[..count])))
    }
}

fn sort_symbols(lengths: &[u8], histogram: &mut [u16; LENGTH_HISTOGRAM], sorted: &mut [u16]) -> usize {
    for &length in lengths {
        histogram[usize::from(length)] += 1;
    }
    histogram[0] = 0;
    let mut offsets = [0usize; LENGTH_HISTOGRAM];
    for length in 1..LENGTH_HISTOGRAM - 1 {
        offsets[length + 1] = offsets[length] + usize::from(histogram[length]);
    }
    for (symbol, &length) in lengths.iter().enumerate() {
        if length != 0 {
            sorted[offsets[usize::from(length)]] = symbol as u16;
            offsets[usize::from(length)] += 1;
        }
    }
    offsets[LENGTH_HISTOGRAM - 1]
}
