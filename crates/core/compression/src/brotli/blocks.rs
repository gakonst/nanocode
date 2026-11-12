use super::bits::{Bits, low_mask};
use super::commands::CommandStep;
use super::prefix::{ALPHABET_SIZE_MAX, decode};
use super::tables::{
    BLOCK_LENGTH_BASE, BLOCK_LENGTH_CODES, BLOCK_LENGTH_EXTRA_BITS, COMMAND_CODES, DISTANCE_CONTEXTS,
    LITERAL_CONTEXTS,
};
use super::{DecodeError, Decoder, Flow, Phase};

pub(super) const LITERAL: usize = 0;
pub(super) const COMMAND: usize = 1;
pub(super) const DISTANCE: usize = 2;
pub(super) const CATEGORIES: usize = 3;
const META_BLOCK_LENGTH_MAX: u32 = 1 << 24;
const BLOCK_TYPE_EXTRA_CODES: u32 = 2;
const LITERAL_CODES: u32 = 256;
const DISTANCE_SHORT_CODES: u32 = 16;
const DISTANCE_EXTRA_CODES: u32 = 48;
const POSTFIX_BITS: u32 = 2;
const DIRECT_CODES_BITS: u32 = 4;
const CONTEXT_MODE_BITS: u32 = 2;
const RUN_LENGTH_PEEK_BITS: u32 = 5;
const RUN_LENGTH_FLAG_BITS: u32 = 1;
const COUNT_FLAG_BITS: u32 = 1;
const COUNT_WIDTH_END: u32 = 4;
const COUNT_WIDTH_MASK: u64 = 7;
const MOVE_TO_FRONT_SIZE: usize = 256;
const BLOCK_TYPE_PREVIOUS: u32 = 0;
const BLOCK_TYPE_NEXT: u32 = 1;

#[derive(Clone, Copy)]
pub(super) struct BlockState {
    pub types: u32,
    pub ring: [u32; 2],
    pub length: u32,
    pub type_table: usize,
    pub length_table: usize,
}

impl BlockState {
    pub(super) const fn new() -> Self {
        Self { types: 1, ring: [1, 0], length: META_BLOCK_LENGTH_MAX, type_table: 0, length_table: 0 }
    }
}

#[derive(Clone, Copy)]
pub(super) enum BlockStep {
    Count,
    TypeCode,
    LengthCode,
    Length,
}

#[derive(Clone, Copy)]
pub(super) enum MapStep {
    Trees,
    RunLength { trees: u32 },
    Code { trees: u32, run_length_max: u32 },
    Values { run_length_max: u32, index: usize },
    Transform,
}

fn read_count(bits: &mut Bits<'_>) -> Option<u32> {
    bits.refill();
    let available = bits.available();
    if available < COUNT_FLAG_BITS {
        return None;
    }
    let word = bits.word();
    if word & 1 == 0 {
        bits.consume(COUNT_FLAG_BITS);
        return Some(0);
    }
    if available < COUNT_WIDTH_END {
        return None;
    }
    let width = ((word >> COUNT_FLAG_BITS) & COUNT_WIDTH_MASK) as u32;
    if width == 0 {
        bits.consume(COUNT_WIDTH_END);
        return Some(1);
    }
    if available < COUNT_WIDTH_END + width {
        return None;
    }
    let value = (1 << width) + ((word >> COUNT_WIDTH_END) & low_mask(width)) as u32;
    bits.consume(COUNT_WIDTH_END + width);
    Some(value)
}

fn block_length(word: u64, table: &[super::prefix::Entry]) -> (u32, u32) {
    let (code, length) = decode(table, word);
    let extra_bits = u32::from(BLOCK_LENGTH_EXTRA_BITS[usize::from(code)]);
    let extra = ((word >> length) & low_mask(extra_bits)) as u32;
    (BLOCK_LENGTH_BASE[usize::from(code)] + extra, length + extra_bits)
}

impl Decoder {
    pub(super) fn begin_compressed(&mut self) {
        self.blocks = [BlockState::new(); CATEGORIES];
        self.block_tables.clear();
        self.literal_trees.clear();
        self.command_trees.clear();
        self.distance_trees.clear();
    }

    pub(super) fn block_types(
        &mut self,
        bits: &mut Bits<'_>,
        category: usize,
        step: BlockStep,
    ) -> Result<Flow, DecodeError> {
        let next = match step {
            BlockStep::Count => {
                let Some(count) = read_count(bits) else {
                    return Ok(Flow::NeedsInput);
                };
                self.blocks[category].types = count + 1;
                if count == 0 {
                    self.next_category(category);
                    return Ok(Flow::Continue);
                }
                BlockStep::TypeCode
            }
            BlockStep::TypeCode => {
                let alphabet_size = self.blocks[category].types + BLOCK_TYPE_EXTRA_CODES;
                let Some(offset) = self.code.read(bits, alphabet_size, &mut self.block_tables)? else {
                    return Ok(Flow::NeedsInput);
                };
                self.blocks[category].type_table = offset as usize;
                BlockStep::LengthCode
            }
            BlockStep::LengthCode => {
                let Some(offset) = self.code.read(bits, BLOCK_LENGTH_CODES, &mut self.block_tables)? else {
                    return Ok(Flow::NeedsInput);
                };
                self.blocks[category].length_table = offset as usize;
                BlockStep::Length
            }
            BlockStep::Length => {
                bits.refill();
                let table = &self.block_tables[self.blocks[category].length_table..];
                let (length, needed) = block_length(bits.word(), table);
                if needed > bits.available() {
                    return Ok(Flow::NeedsInput);
                }
                bits.consume(needed);
                self.blocks[category].length = length;
                self.next_category(category);
                return Ok(Flow::Continue);
            }
        };
        self.phase = Phase::BlockTypes { category, step: next };
        Ok(Flow::Continue)
    }

    const fn next_category(&mut self, category: usize) {
        self.phase = if category + 1 < CATEGORIES {
            Phase::BlockTypes { category: category + 1, step: BlockStep::Count }
        } else {
            Phase::DistanceParameters
        };
    }

    pub(super) fn distance_parameters(&mut self, bits: &mut Bits<'_>) -> Flow {
        let Some(parameters) = bits.try_read(POSTFIX_BITS + DIRECT_CODES_BITS) else {
            return Flow::NeedsInput;
        };
        self.postfix_bits = parameters & low_mask(POSTFIX_BITS) as u32;
        self.direct_codes = (parameters >> POSTFIX_BITS) << self.postfix_bits;
        self.context_modes.clear();
        self.context_modes.resize(self.blocks[LITERAL].types as usize, 0);
        self.phase = Phase::ContextModes { next: 0 };
        Flow::Continue
    }

    pub(super) fn context_modes(&mut self, bits: &mut Bits<'_>, next: usize) -> Flow {
        let mut next = next;
        while next < self.context_modes.len() {
            let Some(mode) = bits.try_read(CONTEXT_MODE_BITS) else {
                self.phase = Phase::ContextModes { next };
                return Flow::NeedsInput;
            };
            self.context_modes[next] = mode as u8;
            next += 1;
        }
        self.phase = Phase::ContextMap { distance: false, step: MapStep::Trees };
        Flow::Continue
    }

    pub(super) fn context_map(&mut self, bits: &mut Bits<'_>, distance: bool, step: MapStep) -> Result<Flow, DecodeError> {
        let next = match step {
            MapStep::Trees => {
                let Some(count) = read_count(bits) else {
                    return Ok(Flow::NeedsInput);
                };
                let trees = count + 1;
                let (map, size) = if distance {
                    self.distance_tree_count = trees;
                    (&mut self.distance_map, self.blocks[DISTANCE].types as usize * DISTANCE_CONTEXTS as usize)
                } else {
                    self.literal_tree_count = trees;
                    (&mut self.literal_map, self.blocks[LITERAL].types as usize * LITERAL_CONTEXTS)
                };
                map.clear();
                map.resize(size, 0);
                if trees < 2 {
                    self.finish_map(distance);
                    return Ok(Flow::Continue);
                }
                MapStep::RunLength { trees }
            }
            MapStep::RunLength { trees } => {
                bits.refill();
                if bits.available() < RUN_LENGTH_PEEK_BITS {
                    return Ok(Flow::NeedsInput);
                }
                let peek = bits.peek(RUN_LENGTH_PEEK_BITS);
                let run_length_max = if peek & 1 == 0 {
                    bits.consume(RUN_LENGTH_FLAG_BITS);
                    0
                } else {
                    bits.consume(RUN_LENGTH_PEEK_BITS);
                    (peek >> RUN_LENGTH_FLAG_BITS) + 1
                };
                self.map_table.clear();
                MapStep::Code { trees, run_length_max }
            }
            MapStep::Code { trees, run_length_max } => {
                let alphabet_size = trees + run_length_max;
                assert!(alphabet_size as usize <= ALPHABET_SIZE_MAX);
                if self.code.read(bits, alphabet_size, &mut self.map_table)?.is_none() {
                    return Ok(Flow::NeedsInput);
                }
                MapStep::Values { run_length_max, index: 0 }
            }
            MapStep::Values { run_length_max, index } => {
                if !self.map_values(bits, distance, run_length_max, index)? {
                    return Ok(Flow::NeedsInput);
                }
                MapStep::Transform
            }
            MapStep::Transform => {
                let Some(transform) = bits.try_read(1) else {
                    return Ok(Flow::NeedsInput);
                };
                if transform == 1 {
                    inverse_move_to_front(if distance { &mut self.distance_map } else { &mut self.literal_map });
                }
                self.finish_map(distance);
                return Ok(Flow::Continue);
            }
        };
        self.phase = Phase::ContextMap { distance, step: next };
        Ok(Flow::Continue)
    }

    fn map_values(
        &mut self,
        bits: &mut Bits<'_>,
        distance: bool,
        run_length_max: u32,
        index: usize,
    ) -> Result<bool, DecodeError> {
        let map = if distance { &mut self.distance_map } else { &mut self.literal_map };
        let mut index = index;
        while index < map.len() {
            bits.refill();
            let word = bits.word();
            let (symbol, length) = decode(&self.map_table, word);
            let symbol = u32::from(symbol);
            if symbol == 0 || symbol > run_length_max {
                if length > bits.available() {
                    self.phase = Phase::ContextMap { distance, step: MapStep::Values { run_length_max, index } };
                    return Ok(false);
                }
                bits.consume(length);
                if symbol != 0 {
                    map[index] = (symbol - run_length_max) as u8;
                }
                index += 1;
                continue;
            }
            if length + symbol > bits.available() {
                self.phase = Phase::ContextMap { distance, step: MapStep::Values { run_length_max, index } };
                return Ok(false);
            }
            let repeat = (1 << symbol) + ((word >> length) & low_mask(symbol)) as usize;
            bits.consume(length + symbol);
            if index + repeat > map.len() {
                return Err(DecodeError::InvalidContextMapRepeat);
            }
            index += repeat;
        }
        Ok(true)
    }

    const fn finish_map(&mut self, distance: bool) {
        self.phase = if distance {
            Phase::TreeGroup { group: 0 }
        } else {
            Phase::ContextMap { distance: true, step: MapStep::Trees }
        };
    }

    pub(super) fn tree_group(&mut self, bits: &mut Bits<'_>, group: usize) -> Result<Flow, DecodeError> {
        let (trees, count, alphabet_size) = match group {
            LITERAL => (&mut self.literal_trees, self.literal_tree_count, LITERAL_CODES),
            COMMAND => (&mut self.command_trees, self.blocks[COMMAND].types, COMMAND_CODES),
            _ => (
                &mut self.distance_trees,
                self.distance_tree_count,
                DISTANCE_SHORT_CODES + self.direct_codes + (DISTANCE_EXTRA_CODES << self.postfix_bits),
            ),
        };
        while trees.offsets.len() < count as usize {
            let Some(offset) = self.code.read(bits, alphabet_size, &mut trees.tables)? else {
                return Ok(Flow::NeedsInput);
            };
            trees.offsets.push(offset);
        }
        if group + 1 < CATEGORIES {
            self.phase = Phase::TreeGroup { group: group + 1 };
            return Ok(Flow::Continue);
        }
        self.select_literal_block(0);
        self.command_tree = 0;
        self.distance_map_offset = 0;
        self.phase = Phase::Commands(CommandStep::Begin);
        Ok(Flow::Continue)
    }

    fn select_literal_block(&mut self, block_type: u32) {
        let offset = block_type as usize * LITERAL_CONTEXTS;
        let map = &self.literal_map[offset..offset + LITERAL_CONTEXTS];
        self.literal_mode = usize::from(self.context_modes[block_type as usize]);
        for (slot, &tree) in self.literal_context_trees.iter_mut().zip(map) {
            *slot = self.literal_trees.offsets[usize::from(tree)];
        }
        self.literal_tree = if map.iter().all(|&tree| tree == map[0]) {
            Some(self.literal_trees.offsets[usize::from(map[0])] as usize)
        } else {
            None
        };
    }

    #[inline]
    pub(super) fn switch_block(&mut self, bits: &mut Bits<'_>, category: usize) -> bool {
        let block = self.blocks[category];
        if block.types < 2 {
            return true;
        }
        bits.refill();
        let word = bits.word();
        let (code, type_length) = decode(&self.block_tables[block.type_table..], word);
        let (length, length_bits) = block_length(word >> type_length, &self.block_tables[block.length_table..]);
        if type_length + length_bits > bits.available() {
            return false;
        }
        bits.consume(type_length + length_bits);
        let code = u32::from(code);
        let mut block_type = match code {
            BLOCK_TYPE_PREVIOUS => block.ring[0],
            BLOCK_TYPE_NEXT => block.ring[1] + 1,
            _ => code - BLOCK_TYPE_EXTRA_CODES,
        };
        if block_type >= block.types {
            block_type -= block.types;
        }
        self.blocks[category].ring = [block.ring[1], block_type];
        self.blocks[category].length = length;
        match category {
            LITERAL => self.select_literal_block(block_type),
            COMMAND => self.command_tree = block_type as usize,
            _ => self.distance_map_offset = block_type as usize * DISTANCE_CONTEXTS as usize,
        }
        true
    }
}

fn inverse_move_to_front(map: &mut [u8]) {
    let mut list = [0u8; MOVE_TO_FRONT_SIZE];
    for (index, entry) in list.iter_mut().enumerate() {
        *entry = index as u8;
    }
    for value in map.iter_mut() {
        let index = usize::from(*value);
        let symbol = list[index];
        list.copy_within(..index, 1);
        list[0] = symbol;
        *value = symbol;
    }
}
