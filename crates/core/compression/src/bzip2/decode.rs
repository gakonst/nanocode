use alloc::boxed::Box;
use alloc::vec::Vec;

use super::huffman::DecodeTable;
use super::{
    ALPHABET_SIZE_MAX, BLOCK_BYTES_PER_LEVEL, BLOCK_MAGIC, BYTE_VALUES, Bzip2Error, END_MAGIC,
    GROUP_SYMBOLS, GROUPS_MAX, LEVEL_MAX, LEVEL_MIN, RLE_RUN_BYTES, RUN_A, RUN_B, SELECTORS_MAX,
    STREAM_MAGIC,
};

const WIDE_REFILL_BYTES: usize = u64::BITS as usize / u8::BITS as usize;
const WIDE_REFILL_BITS_MAX: u32 = u64::BITS - 1;
const BYTE_ALIGNMENT_MASK: u32 = !(u8::BITS - 1);
const CRC_BYTES: usize = 4;
const ORIGIN_BYTES: usize = 3;
const ORIGIN_SLACK: u32 = 10;
const IN_USE_GROUPS: usize = 16;
const IN_USE_GROUP_BYTES: usize = 16;
const GROUPS_MIN: u32 = 2;
const GROUP_COUNT_BITS: u32 = 3;
const SELECTOR_COUNT_BITS: u32 = 15;
const TABLE_START_BITS: u32 = 5;
const CODE_LENGTH_DECODE_MAX: u8 = 20;
const RUN_LOG_LIMIT: u8 = 21;
const MTF_SLOTS: usize = 4096;
const MTF_LIST_BYTES: usize = 16;
const MTF_LISTS: usize = BYTE_VALUES / MTF_LIST_BYTES;
const RLE_RUN_LENGTH_MIN: u32 = RLE_RUN_BYTES as u32;

pub(super) struct Input<'a> {
    bytes: &'a [u8],
    pub(super) position: usize,
}

impl<'a> Input<'a> {
    pub(super) const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }

    const fn remaining(&self) -> usize {
        self.bytes.len() - self.position
    }
}

struct BitReader {
    buffer: u64,
    live: u32,
}

impl BitReader {
    #[inline(always)]
    fn bits(&mut self, input: &mut Input<'_>, count: u32, wide: bool) -> Option<u32> {
        loop {
            if self.live >= count {
                let value = (self.buffer >> (self.live - count)) & ((1 << count) - 1);
                self.live -= count;
                return Some(u32::try_from(value).expect("at most 20 bits fit in u32"));
            }
            if wide && input.remaining() >= WIDE_REFILL_BYTES {
                let mut word = [0; WIDE_REFILL_BYTES];
                word.copy_from_slice(
                    &input.bytes[input.position..input.position + WIDE_REFILL_BYTES],
                );
                let read = u64::from_be_bytes(word);
                let refill_bits = (WIDE_REFILL_BITS_MAX - self.live) & BYTE_ALIGNMENT_MASK;
                self.buffer = (self.buffer << refill_bits) | (read >> (u64::BITS - refill_bits));
                self.live += refill_bits;
                input.position += (refill_bits / u8::BITS) as usize;
                continue;
            }
            let &byte = input.bytes.get(input.position)?;
            input.position += 1;
            self.buffer = (self.buffer << u8::BITS) | u64::from(byte);
            self.live += u8::BITS;
        }
    }
}

#[derive(Clone, Copy)]
pub(super) enum Phase {
    StreamMagic {
        index: usize,
    },
    BlockSize,
    BlockHeader,
    BlockMagic {
        index: usize,
    },
    EndMagic {
        index: usize,
    },
    BlockCrc {
        index: usize,
    },
    Randomised,
    Origin {
        index: usize,
    },
    GroupInUse {
        group: usize,
    },
    ByteInUse {
        group: usize,
        index: usize,
    },
    TableCount,
    SelectorCount,
    Selector {
        index: usize,
        value: u8,
    },
    TableStart {
        table: usize,
    },
    LengthBit {
        table: usize,
        symbol: usize,
        length: u8,
    },
    LengthDirection {
        table: usize,
        symbol: usize,
        length: u8,
    },
    SymbolStart,
    SymbolExtend {
        length: u8,
        code: i32,
    },
    Output,
    CombinedCrc {
        index: usize,
    },
    Finished,
    Failed(Bzip2Error),
}

impl Phase {
    const fn wide(self) -> bool {
        !matches!(self, Self::EndMagic { .. } | Self::CombinedCrc { .. })
    }
}

pub(super) enum Step {
    Continue,
    Suspend,
    BlockReady,
    StreamEnd,
}

struct Tables {
    group_in_use: [bool; IN_USE_GROUPS],
    in_use: [bool; BYTE_VALUES],
    sequence_to_byte: [u8; BYTE_VALUES],
    symbols_in_use: usize,
    alphabet_size: usize,
    group_count: usize,
    selector_count: usize,
    selector_moves: [u8; SELECTORS_MAX],
    selectors: [u8; SELECTORS_MAX],
    lengths: [[u8; ALPHABET_SIZE_MAX]; GROUPS_MAX],
    decode: [DecodeTable; GROUPS_MAX],
}

struct Symbols {
    recent: [u8; MTF_SLOTS],
    list_starts: [u16; MTF_LISTS],
    counts: [u32; BYTE_VALUES],
    next_group: usize,
    group_remaining: usize,
    group_table: usize,
    block_bytes: usize,
    run: Option<(u32, u8)>,
}

#[derive(Clone, Copy)]
pub(super) struct Output {
    position: u32,
    current: u8,
    used: usize,
    run_length: u32,
    run_byte: u8,
    crc: u32,
    random: Option<(u16, usize)>,
}

pub(super) struct Decoder {
    pub(super) phase: Phase,
    reader: BitReader,
    block_size: usize,
    links: Vec<u32>,
    stored_block_crc: u32,
    pub(super) stored_combined_crc: u32,
    pub(super) calculated_combined_crc: u32,
    randomised: bool,
    origin: u32,
    tables: Box<Tables>,
    symbols: Box<Symbols>,
    pub(super) output: Output,
}

impl Decoder {
    pub(super) fn new() -> Self {
        Self {
            phase: Phase::StreamMagic { index: 0 },
            reader: BitReader { buffer: 0, live: 0 },
            block_size: 0,
            links: Vec::new(),
            stored_block_crc: 0,
            stored_combined_crc: 0,
            calculated_combined_crc: 0,
            randomised: false,
            origin: 0,
            tables: Box::new(Tables {
                group_in_use: [false; IN_USE_GROUPS],
                in_use: [false; BYTE_VALUES],
                sequence_to_byte: [0; BYTE_VALUES],
                symbols_in_use: 0,
                alphabet_size: 0,
                group_count: 0,
                selector_count: 0,
                selector_moves: [0; SELECTORS_MAX],
                selectors: [0; SELECTORS_MAX],
                lengths: [[0; ALPHABET_SIZE_MAX]; GROUPS_MAX],
                decode: core::array::from_fn(|_| DecodeTable::new()),
            }),
            symbols: Box::new(Symbols {
                recent: [0; MTF_SLOTS],
                list_starts: [0; MTF_LISTS],
                counts: [0; BYTE_VALUES],
                next_group: 0,
                group_remaining: 0,
                group_table: 0,
                block_bytes: 0,
                run: None,
            }),
            output: Output {
                position: 0,
                current: 0,
                used: 0,
                run_length: 0,
                run_byte: 0,
                crc: 0,
                random: None,
            },
        }
    }

    fn bits(&mut self, input: &mut Input<'_>, count: u32) -> Option<u32> {
        let wide = self.phase.wide();
        self.reader.bits(input, count, wide)
    }

    fn byte(&mut self, input: &mut Input<'_>) -> Option<u8> {
        self.bits(input, u8::BITS)
            .map(|value| u8::try_from(value).expect("eight bits fit in a byte"))
    }

    pub(super) fn decode(&mut self, input: &mut Input<'_>) -> Result<Step, Bzip2Error> {
        loop {
            let step = match self.phase {
                Phase::StreamMagic { .. }
                | Phase::BlockSize
                | Phase::BlockHeader
                | Phase::BlockMagic { .. }
                | Phase::EndMagic { .. }
                | Phase::CombinedCrc { .. } => self.frame_step(input)?,
                Phase::BlockCrc { .. }
                | Phase::Randomised
                | Phase::Origin { .. }
                | Phase::GroupInUse { .. }
                | Phase::ByteInUse { .. } => self.block_header_step(input)?,
                Phase::TableCount | Phase::SelectorCount | Phase::Selector { .. } => {
                    self.selector_step(input)?
                }
                Phase::TableStart { .. }
                | Phase::LengthBit { .. }
                | Phase::LengthDirection { .. } => self.length_step(input)?,
                Phase::SymbolStart | Phase::SymbolExtend { .. } => self.symbol_step(input)?,
                Phase::Output | Phase::Finished | Phase::Failed(_) => {
                    unreachable!("the stream driver decodes only in a decoding phase")
                }
            };
            if !matches!(step, Step::Continue) {
                return Ok(step);
            }
        }
    }

    fn frame_step(&mut self, input: &mut Input<'_>) -> Result<Step, Bzip2Error> {
        let phase = self.phase;
        let Some(byte) = self.byte(input) else {
            return Ok(Step::Suspend);
        };
        match phase {
            Phase::StreamMagic { index } => {
                if byte != STREAM_MAGIC[index] {
                    return Err(Bzip2Error::DataMagic);
                }
                self.phase = if index + 1 == STREAM_MAGIC.len() {
                    Phase::BlockSize
                } else {
                    Phase::StreamMagic { index: index + 1 }
                };
            }
            Phase::BlockSize => {
                let digit = byte.wrapping_sub(b'0');
                if !(LEVEL_MIN..=LEVEL_MAX).contains(&digit) {
                    return Err(Bzip2Error::DataMagic);
                }
                self.block_size = usize::from(digit);
                self.links = alloc::vec![0; self.block_size * BLOCK_BYTES_PER_LEVEL];
                self.phase = Phase::BlockHeader;
            }
            Phase::BlockHeader => {
                self.phase = if byte == BLOCK_MAGIC[0] {
                    Phase::BlockMagic { index: 1 }
                } else if byte == END_MAGIC[0] {
                    Phase::EndMagic { index: 1 }
                } else {
                    return Err(Bzip2Error::Data);
                };
            }
            Phase::BlockMagic { index } => {
                if byte != BLOCK_MAGIC[index] {
                    return Err(Bzip2Error::Data);
                }
                self.phase = if index + 1 == BLOCK_MAGIC.len() {
                    self.stored_block_crc = 0;
                    Phase::BlockCrc { index: 0 }
                } else {
                    Phase::BlockMagic { index: index + 1 }
                };
            }
            Phase::EndMagic { index } => {
                if byte != END_MAGIC[index] {
                    return Err(Bzip2Error::Data);
                }
                self.phase = if index + 1 == END_MAGIC.len() {
                    self.stored_combined_crc = 0;
                    Phase::CombinedCrc { index: 0 }
                } else {
                    Phase::EndMagic { index: index + 1 }
                };
            }
            Phase::CombinedCrc { index } => {
                self.stored_combined_crc = (self.stored_combined_crc << u8::BITS) | u32::from(byte);
                if index + 1 == CRC_BYTES {
                    self.phase = Phase::Finished;
                    return Ok(Step::StreamEnd);
                }
                self.phase = Phase::CombinedCrc { index: index + 1 };
            }
            _ => unreachable!("frame_step runs only in a frame phase"),
        }
        Ok(Step::Continue)
    }
}

impl Decoder {
    fn block_header_step(&mut self, input: &mut Input<'_>) -> Result<Step, Bzip2Error> {
        match self.phase {
            Phase::BlockCrc { index } => {
                let Some(byte) = self.byte(input) else {
                    return Ok(Step::Suspend);
                };
                self.stored_block_crc = (self.stored_block_crc << u8::BITS) | u32::from(byte);
                self.phase = if index + 1 == CRC_BYTES {
                    Phase::Randomised
                } else {
                    Phase::BlockCrc { index: index + 1 }
                };
            }
            Phase::Randomised => {
                let Some(bit) = self.bits(input, 1) else {
                    return Ok(Step::Suspend);
                };
                self.randomised = bit != 0;
                self.origin = 0;
                self.phase = Phase::Origin { index: 0 };
            }
            Phase::Origin { index } => {
                let Some(byte) = self.byte(input) else {
                    return Ok(Step::Suspend);
                };
                self.origin = (self.origin << u8::BITS) | u32::from(byte);
                if index + 1 < ORIGIN_BYTES {
                    self.phase = Phase::Origin { index: index + 1 };
                    return Ok(Step::Continue);
                }
                let block_size = u32::try_from(self.block_size).expect("a level fits in u32");
                let level_bytes = u32::try_from(BLOCK_BYTES_PER_LEVEL).expect("100000 fits in u32");
                if self.origin > ORIGIN_SLACK + level_bytes * block_size {
                    return Err(Bzip2Error::Data);
                }
                self.phase = Phase::GroupInUse { group: 0 };
            }
            Phase::GroupInUse { group } => {
                let Some(bit) = self.bits(input, 1) else {
                    return Ok(Step::Suspend);
                };
                self.tables.group_in_use[group] = bit == 1;
                if group + 1 < IN_USE_GROUPS {
                    self.phase = Phase::GroupInUse { group: group + 1 };
                    return Ok(Step::Continue);
                }
                self.tables.in_use.fill(false);
                return self.next_used_group(0);
            }
            Phase::ByteInUse { group, index } => {
                let Some(bit) = self.bits(input, 1) else {
                    return Ok(Step::Suspend);
                };
                if bit == 1 {
                    self.tables.in_use[group * IN_USE_GROUP_BYTES + index] = true;
                }
                if index + 1 < IN_USE_GROUP_BYTES {
                    self.phase = Phase::ByteInUse {
                        group,
                        index: index + 1,
                    };
                    return Ok(Step::Continue);
                }
                return self.next_used_group(group + 1);
            }
            _ => unreachable!("block_header_step runs only in a block header phase"),
        }
        Ok(Step::Continue)
    }

    fn next_used_group(&mut self, first: usize) -> Result<Step, Bzip2Error> {
        if let Some(group) = (first..IN_USE_GROUPS).find(|&group| self.tables.group_in_use[group]) {
            self.phase = Phase::ByteInUse { group, index: 0 };
            return Ok(Step::Continue);
        }
        let tables = &mut *self.tables;
        tables.symbols_in_use = 0;
        for (byte, &used) in tables.in_use.iter().enumerate() {
            if used {
                tables.sequence_to_byte[tables.symbols_in_use] =
                    u8::try_from(byte).expect("a byte index fits in u8");
                tables.symbols_in_use += 1;
            }
        }
        if tables.symbols_in_use == 0 {
            return Err(Bzip2Error::Data);
        }
        tables.alphabet_size = tables.symbols_in_use + 2;
        self.phase = Phase::TableCount;
        Ok(Step::Continue)
    }

    fn selector_step(&mut self, input: &mut Input<'_>) -> Result<Step, Bzip2Error> {
        match self.phase {
            Phase::TableCount => {
                let Some(count) = self.bits(input, GROUP_COUNT_BITS) else {
                    return Ok(Step::Suspend);
                };
                let groups_max = u32::try_from(GROUPS_MAX).expect("6 fits in u32");
                if !(GROUPS_MIN..=groups_max).contains(&count) {
                    return Err(Bzip2Error::Data);
                }
                self.tables.group_count = count as usize;
                self.phase = Phase::SelectorCount;
            }
            Phase::SelectorCount => {
                let Some(count) = self.bits(input, SELECTOR_COUNT_BITS) else {
                    return Ok(Step::Suspend);
                };
                if count < 1 {
                    return Err(Bzip2Error::Data);
                }
                self.tables.selector_count = count as usize;
                self.phase = Phase::Selector { index: 0, value: 0 };
            }
            Phase::Selector { index, value } => {
                let Some(bit) = self.bits(input, 1) else {
                    return Ok(Step::Suspend);
                };
                if bit != 0 {
                    let value = value + 1;
                    if usize::from(value) >= self.tables.group_count {
                        return Err(Bzip2Error::Data);
                    }
                    self.phase = Phase::Selector { index, value };
                    return Ok(Step::Continue);
                }
                if index < SELECTORS_MAX {
                    self.tables.selector_moves[index] = value;
                }
                if index + 1 < self.tables.selector_count {
                    self.phase = Phase::Selector {
                        index: index + 1,
                        value: 0,
                    };
                    return Ok(Step::Continue);
                }
                self.decode_selectors();
                self.phase = Phase::TableStart { table: 0 };
            }
            _ => unreachable!("selector_step runs only in a selector phase"),
        }
        Ok(Step::Continue)
    }

    fn decode_selectors(&mut self) {
        let tables = &mut *self.tables;
        tables.selector_count = tables.selector_count.min(SELECTORS_MAX);
        let mut recent: [u8; GROUPS_MAX] =
            core::array::from_fn(|index| u8::try_from(index).expect("a table index fits in u8"));
        for index in 0..tables.selector_count {
            let moves = usize::from(tables.selector_moves[index]);
            recent[..=moves].rotate_right(1);
            tables.selectors[index] = recent[0];
        }
    }
}

impl Decoder {
    fn length_step(&mut self, input: &mut Input<'_>) -> Result<Step, Bzip2Error> {
        match self.phase {
            Phase::TableStart { table } => {
                let Some(length) = self.bits(input, TABLE_START_BITS) else {
                    return Ok(Step::Suspend);
                };
                let length = u8::try_from(length).expect("five bits fit in a byte");
                self.enter_length(table, 0, length)
            }
            Phase::LengthBit {
                table,
                symbol,
                length,
            } => {
                let Some(bit) = self.bits(input, 1) else {
                    return Ok(Step::Suspend);
                };
                if bit != 0 {
                    self.phase = Phase::LengthDirection {
                        table,
                        symbol,
                        length,
                    };
                    return Ok(Step::Continue);
                }
                self.tables.lengths[table][symbol] = length;
                self.enter_length(table, symbol + 1, length)
            }
            Phase::LengthDirection {
                table,
                symbol,
                length,
            } => {
                let Some(bit) = self.bits(input, 1) else {
                    return Ok(Step::Suspend);
                };
                let length = if bit == 0 { length + 1 } else { length - 1 };
                self.enter_length(table, symbol, length)
            }
            _ => unreachable!("length_step runs only in a code length phase"),
        }
    }

    fn enter_length(
        &mut self,
        table: usize,
        symbol: usize,
        length: u8,
    ) -> Result<Step, Bzip2Error> {
        if symbol < self.tables.alphabet_size {
            if !(1..=CODE_LENGTH_DECODE_MAX).contains(&length) {
                return Err(Bzip2Error::Data);
            }
            self.phase = Phase::LengthBit {
                table,
                symbol,
                length,
            };
            return Ok(Step::Continue);
        }
        if table + 1 < self.tables.group_count {
            self.phase = Phase::TableStart { table: table + 1 };
            return Ok(Step::Continue);
        }
        self.start_symbols()?;
        self.phase = Phase::SymbolStart;
        Ok(Step::Continue)
    }

    fn start_symbols(&mut self) -> Result<(), Bzip2Error> {
        let tables = &mut *self.tables;
        for (decode, lengths) in tables.decode[..tables.group_count]
            .iter_mut()
            .zip(&tables.lengths)
        {
            decode.build(&lengths[..tables.alphabet_size]);
        }
        let symbols = &mut *self.symbols;
        symbols.counts.fill(0);
        let mut slot = MTF_SLOTS;
        for list in (0..MTF_LISTS).rev() {
            for offset in (0..MTF_LIST_BYTES).rev() {
                slot -= 1;
                symbols.recent[slot] =
                    u8::try_from(list * MTF_LIST_BYTES + offset).expect("a byte index fits in u8");
            }
            symbols.list_starts[list] = u16::try_from(slot).expect("an MTF slot fits in u16");
        }
        symbols.block_bytes = 0;
        symbols.next_group = 0;
        symbols.group_remaining = 0;
        symbols.run = None;
        self.next_group_position()
    }

    fn next_group_position(&mut self) -> Result<(), Bzip2Error> {
        let symbols = &mut *self.symbols;
        if symbols.group_remaining == 0 {
            let tables = &*self.tables;
            let Some(&table) = tables.selectors[..tables.selector_count].get(symbols.next_group)
            else {
                return Err(Bzip2Error::Data);
            };
            symbols.next_group += 1;
            symbols.group_table = usize::from(table);
            symbols.group_remaining = GROUP_SYMBOLS;
        }
        symbols.group_remaining -= 1;
        Ok(())
    }
}

impl Decoder {
    fn symbol_step(&mut self, input: &mut Input<'_>) -> Result<Step, Bzip2Error> {
        let wide = self.phase.wide();
        loop {
            let table = &self.tables.decode[self.symbols.group_table];
            let reader = &mut self.reader;
            let (mut length, mut code) = match self.phase {
                Phase::SymbolStart => {
                    let length = table.length_min;
                    let Some(code) = reader.bits(input, u32::from(length), wide) else {
                        return Ok(Step::Suspend);
                    };
                    (length, code as i32)
                }
                Phase::SymbolExtend { length, code } => {
                    let Some(bit) = reader.bits(input, 1, wide) else {
                        return Ok(Step::Suspend);
                    };
                    (length, (code << 1) | bit as i32)
                }
                _ => unreachable!("symbol_step runs only in a symbol phase"),
            };
            let symbol = loop {
                if length > CODE_LENGTH_DECODE_MAX {
                    return Err(Bzip2Error::Data);
                }
                let slot = usize::from(length);
                if code <= table.limit[slot] {
                    let index = code - table.base[slot];
                    let Some(&symbol) = usize::try_from(index)
                        .ok()
                        .and_then(|index| table.permutation.get(index))
                    else {
                        return Err(Bzip2Error::Data);
                    };
                    break symbol;
                }
                length += 1;
                let Some(bit) = reader.bits(input, 1, wide) else {
                    self.phase = Phase::SymbolExtend { length, code };
                    return Ok(Step::Suspend);
                };
                code = (code << 1) | bit as i32;
            };
            self.phase = Phase::SymbolStart;
            if self.accept_symbol(symbol)? {
                self.finish_block()?;
                return Ok(Step::BlockReady);
            }
        }
    }

    fn accept_symbol(&mut self, symbol: u16) -> Result<bool, Bzip2Error> {
        let is_run = symbol == RUN_A || symbol == RUN_B;
        if let Some((accumulated, log)) = self.symbols.run {
            if is_run {
                return self.extend_run(symbol, accumulated, log).map(|()| false);
            }
            let symbols = &mut *self.symbols;
            let front = symbols.recent[usize::from(symbols.list_starts[0])];
            let byte = self.tables.sequence_to_byte[usize::from(front)];
            symbols.counts[usize::from(byte)] += accumulated;
            let start = symbols.block_bytes;
            let end = start + accumulated as usize;
            let Some(slots) = self.links.get_mut(start..end) else {
                return Err(Bzip2Error::Data);
            };
            slots.fill(u32::from(byte));
            symbols.block_bytes = end;
            symbols.run = None;
        }
        if usize::from(symbol) == self.tables.symbols_in_use + 1 {
            return Ok(true);
        }
        if is_run {
            return self.extend_run(symbol, 0, 0).map(|()| false);
        }
        if self.symbols.block_bytes >= BLOCK_BYTES_PER_LEVEL * self.block_size {
            return Err(Bzip2Error::Data);
        }
        let position = move_to_front(&mut self.symbols, usize::from(symbol - 1));
        let byte = self.tables.sequence_to_byte[usize::from(position)];
        let symbols = &mut *self.symbols;
        symbols.counts[usize::from(byte)] += 1;
        self.links[symbols.block_bytes] = u32::from(byte);
        symbols.block_bytes += 1;
        self.next_group_position()?;
        Ok(false)
    }

    fn extend_run(&mut self, symbol: u16, accumulated: u32, log: u8) -> Result<(), Bzip2Error> {
        if log >= RUN_LOG_LIMIT {
            return Err(Bzip2Error::Data);
        }
        let digit = u32::from(symbol - RUN_A) + 1;
        self.symbols.run = Some((accumulated + (digit << log), log + 1));
        self.next_group_position()
    }
}

fn move_to_front(symbols: &mut Symbols, index: usize) -> u8 {
    let recent = &mut symbols.recent;
    let starts = &mut symbols.list_starts;
    if index < MTF_LIST_BYTES {
        let start = usize::from(starts[0]);
        let value = recent[start + index];
        recent[start..=start + index].rotate_right(1);
        return value;
    }
    let mut list = index / MTF_LIST_BYTES;
    let offset = index % MTF_LIST_BYTES;
    let base = usize::from(starts[list]);
    let value = recent[base + offset];
    recent.copy_within(base..base + offset, base + 1);
    starts[list] += 1;
    while list > 0 {
        starts[list] -= 1;
        recent[usize::from(starts[list])] =
            recent[usize::from(starts[list - 1]) + MTF_LIST_BYTES - 1];
        list -= 1;
    }
    starts[0] -= 1;
    recent[usize::from(starts[0])] = value;
    if starts[0] == 0 {
        let mut slot = MTF_SLOTS;
        for list in (0..MTF_LISTS).rev() {
            for offset in (0..MTF_LIST_BYTES).rev() {
                slot -= 1;
                recent[slot] = recent[usize::from(starts[list]) + offset];
            }
            starts[list] = u16::try_from(slot).expect("an MTF slot fits in u16");
        }
    }
    value
}

impl Decoder {
    fn finish_block(&mut self) -> Result<(), Bzip2Error> {
        let block_bytes = self.symbols.block_bytes;
        let origin = self.origin as usize;
        if origin >= block_bytes {
            return Err(Bzip2Error::Data);
        }
        let block_bytes_u32 = u32::try_from(block_bytes).expect("a block is under 1 MB");
        if self
            .symbols
            .counts
            .iter()
            .any(|&count| count > block_bytes_u32)
        {
            return Err(Bzip2Error::Data);
        }
        let mut starts = [0_u32; BYTE_VALUES + 1];
        for (byte, &count) in self.symbols.counts.iter().enumerate() {
            starts[byte + 1] = starts[byte] + count;
        }
        if starts.iter().any(|&start| start > block_bytes_u32) {
            return Err(Bzip2Error::Data);
        }
        let links = &mut self.links[..block_bytes];
        for index in 0..block_bytes {
            let byte = usize::from(links[index].to_le_bytes()[0]);
            let slot = &mut starts[byte];
            links[*slot as usize] |=
                u32::try_from(index << u8::BITS).expect("a block is under 1 MB");
            *slot += 1;
        }
        let first = (links[origin] >> u8::BITS) as usize;
        let link = links[first];
        assert!(((link >> u8::BITS) as usize) < block_bytes);
        let mut current = link.to_le_bytes()[0];
        let mut random = None;
        if self.randomised {
            let mut state = (0, 0);
            next_random(&mut state);
            current ^= u8::from(state.0 == 1);
            random = Some(state);
        }
        self.output = Output {
            position: link >> u8::BITS,
            current,
            used: 1,
            run_length: 0,
            run_byte: 0,
            crc: super::crc::INITIAL,
            random,
        };
        self.phase = Phase::Output;
        Ok(())
    }

    pub(super) const fn block_done(&self) -> bool {
        self.output.used == self.symbols.block_bytes + 1 && self.output.run_length == 0
    }

    pub(super) const fn block_crc_matches(&self) -> bool {
        !self.output.crc == self.stored_block_crc
    }

    pub(super) const fn block_crc(&self) -> u32 {
        !self.output.crc
    }

    pub(super) fn write_output(
        &mut self,
        output: &mut [u8],
        written: &mut usize,
    ) -> Result<(), Bzip2Error> {
        if self.output.random.is_some() {
            self.write_randomised(output, written)
        } else {
            self.write_plain(output, written)
        }
    }
}

fn next_random(state: &mut (u16, usize)) {
    if state.0 == 0 {
        state.0 = super::random::RANDOM_RUN_LENGTHS[state.1];
        state.1 += 1;
        if state.1 == super::random::RANDOM_RUN_LENGTHS.len() {
            state.1 = 0;
        }
    }
    state.0 -= 1;
}

#[inline(always)]
fn follow(links: &[u32], position: u32) -> Result<(u32, u8), Bzip2Error> {
    let &link = links.get(position as usize).ok_or(Bzip2Error::Data)?;
    Ok((link >> u8::BITS, link.to_le_bytes()[0]))
}

enum RunStart {
    End,
    Byte,
    Run,
}

#[inline(always)]
fn start_run(links: &[u32], end: usize, state: &mut Output) -> Result<RunStart, Bzip2Error> {
    if state.used > end {
        return Err(Bzip2Error::Data);
    }
    if state.used == end {
        state.run_length = 0;
        return Ok(RunStart::End);
    }
    state.run_byte = state.current;
    let next;
    (state.position, next) = follow(links, state.position)?;
    state.used += 1;
    if next != state.current || state.used == end {
        state.current = next;
        return Ok(RunStart::Byte);
    }
    for run_length in [2, 3] {
        state.run_length = run_length;
        let next;
        (state.position, next) = follow(links, state.position)?;
        state.used += 1;
        if state.used == end {
            return Ok(RunStart::Run);
        }
        if next != state.current {
            state.current = next;
            return Ok(RunStart::Run);
        }
    }
    let extra;
    (state.position, extra) = follow(links, state.position)?;
    state.used += 1;
    state.run_length = u32::from(extra) + RLE_RUN_LENGTH_MIN;
    (state.position, state.current) = follow(links, state.position)?;
    state.used += 1;
    Ok(RunStart::Run)
}

impl Decoder {
    fn write_plain(&mut self, output: &mut [u8], written: &mut usize) -> Result<(), Bzip2Error> {
        let links = &self.links[..self.block_size * BLOCK_BYTES_PER_LEVEL];
        let end = self.symbols.block_bytes + 1;
        let mut state = self.output;
        let mut cursor = *written;
        'refill: loop {
            if state.run_length > 0 {
                let bound = (output.len() - cursor).min(state.run_length as usize);
                output[cursor..cursor + bound].fill(state.run_byte);
                for _ in 0..bound {
                    state.crc = super::crc::update(state.crc, state.run_byte);
                }
                cursor += bound;
                state.run_length -=
                    u32::try_from(bound).expect("a bound under a run length fits in u32");
                if cursor == output.len() {
                    break 'refill;
                }
            }
            loop {
                match start_run(links, end, &mut state)? {
                    RunStart::End => break 'refill,
                    RunStart::Run => continue 'refill,
                    RunStart::Byte => {
                        if cursor == output.len() {
                            state.run_length = 1;
                            break 'refill;
                        }
                        output[cursor] = state.run_byte;
                        state.crc = super::crc::update(state.crc, state.run_byte);
                        cursor += 1;
                    }
                }
            }
        }
        self.output = state;
        *written = cursor;
        Ok(())
    }
}

impl Decoder {
    fn next_randomised(&mut self) -> Result<u8, Bzip2Error> {
        let state = &mut self.output;
        let (position, byte) = follow(&self.links, state.position)?;
        state.position = position;
        let random = state
            .random
            .as_mut()
            .expect("a randomised block carries its random state");
        next_random(random);
        Ok(byte ^ u8::from(random.0 == 1))
    }

    fn write_randomised(
        &mut self,
        output: &mut [u8],
        written: &mut usize,
    ) -> Result<(), Bzip2Error> {
        let end = self.symbols.block_bytes + 1;
        loop {
            loop {
                let state = &mut self.output;
                if state.run_length == 0 {
                    if *written == output.len() {
                        return Ok(());
                    }
                    break;
                }
                if *written == output.len() {
                    return Ok(());
                }
                output[*written] = state.run_byte;
                *written += 1;
                state.crc = super::crc::update(state.crc, state.run_byte);
                state.run_length -= 1;
            }
            if self.output.used == end {
                return Ok(());
            }
            if self.output.used > end {
                return Err(Bzip2Error::Data);
            }
            self.output.run_byte = self.output.current;
            self.output.run_length = 1;
            if !self.randomised_run_continues(end)? {
                continue;
            }
            self.output.run_length = 2;
            if !self.randomised_run_continues(end)? {
                continue;
            }
            self.output.run_length = 3;
            if !self.randomised_run_continues(end)? {
                continue;
            }
            let extra = self.next_randomised()?;
            self.output.used += 1;
            self.output.run_length = u32::from(extra) + RLE_RUN_LENGTH_MIN;
            self.output.current = self.next_randomised()?;
            self.output.used += 1;
        }
    }

    fn randomised_run_continues(&mut self, end: usize) -> Result<bool, Bzip2Error> {
        let next = self.next_randomised()?;
        self.output.used += 1;
        if self.output.used == end {
            return Ok(false);
        }
        if next != self.output.current {
            self.output.current = next;
            return Ok(false);
        }
        Ok(true)
    }
}
