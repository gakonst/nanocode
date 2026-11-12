use super::bits::{Bits, low_mask};
use super::blocks::{COMMAND, DISTANCE, LITERAL};
use super::dictionary::{self, WORD_LENGTH_MAX, WORD_LENGTH_MIN};
use super::prefix::{CODE_LENGTH_MAX, Entry, decode_at};
use super::tables::{COMMANDS, CONTEXT_LOOKUP, LITERAL_CONTEXTS};
use super::{DISTANCE_RING_SIZE, DecodeError, Decoder, Flow, Phase};

const DISTANCE_SHORT_CODES: u32 = 16;
const DISTANCE_LAST_CODES: u32 = 4;
const DISTANCE_SECOND_ADJUST_CODE: u32 = 10;
const DISTANCE_DIRECT_OFFSET: u32 = DISTANCE_SHORT_CODES - 1;
const DISTANCE_OFFSET_BASE: u32 = 2;
const DISTANCE_OFFSET_BIAS: u32 = 4;
const CONTEXT_SECOND_BYTE_OFFSET: usize = 256;
const CONTEXT_MASK: usize = LITERAL_CONTEXTS - 1;

#[derive(Clone, Copy)]
pub(super) struct PendingCopy {
    length: u32,
    implicit_distance: bool,
    distance_context: u8,
}

#[derive(Clone, Copy)]
pub(super) enum CommandStep {
    Begin,
    Literals { count: u32, copy: PendingCopy },
    Distance { copy: PendingCopy },
    Copy { distance: u32, count: u32 },
    Word { written: u8 },
}

impl Decoder {
    pub(super) fn commands(&mut self, bits: &mut Bits<'_>, step: CommandStep) -> Result<Flow, DecodeError> {
        let mut step = step;
        loop {
            step = match step {
                CommandStep::Begin => {
                    if self.blocks[COMMAND].length == 0 && !self.switch_block(bits, COMMAND) {
                        return Ok(self.pause(step, Flow::NeedsInput));
                    }
                    let Some((insert, copy)) = self.read_command(bits) else {
                        return Ok(self.pause(step, Flow::NeedsInput));
                    };
                    if insert == 0 {
                        CommandStep::Distance { copy }
                    } else {
                        self.remaining -= insert as i32;
                        CommandStep::Literals { count: insert, copy }
                    }
                }
                CommandStep::Literals { count, copy } => {
                    let (left, flow) = self.literals(bits, count)?;
                    if left != 0 {
                        return Ok(self.pause(CommandStep::Literals { count: left, copy }, flow));
                    }
                    if self.remaining <= 0 {
                        return Ok(self.end_meta_block());
                    }
                    if self.window.is_full() {
                        return Ok(self.pause(CommandStep::Distance { copy }, Flow::Continue));
                    }
                    CommandStep::Distance { copy }
                }
                CommandStep::Distance { copy } => {
                    let Some(next) = self.distance(bits, copy)? else {
                        return Ok(self.pause(step, Flow::NeedsInput));
                    };
                    next
                }
                CommandStep::Copy { distance, count } => {
                    if !self.window.reserve() {
                        return Ok(self.pause(step, Flow::Continue));
                    }
                    let copied = self.window.copy(distance as usize, count as usize) as u32;
                    if copied < count {
                        return Ok(self.pause(CommandStep::Copy { distance, count: count - copied }, Flow::Continue));
                    }
                    if self.remaining <= 0 {
                        return Ok(self.end_meta_block());
                    }
                    if self.window.is_full() {
                        return Ok(self.pause(CommandStep::Begin, Flow::Continue));
                    }
                    CommandStep::Begin
                }
                CommandStep::Word { written } => {
                    if !self.window.reserve() {
                        return Ok(self.pause(step, Flow::Continue));
                    }
                    let copied = self.window.write(&self.word.bytes[usize::from(written)..usize::from(self.word.length)]);
                    let written = written + copied as u8;
                    if written < self.word.length {
                        return Ok(self.pause(CommandStep::Word { written }, Flow::Continue));
                    }
                    if self.remaining <= 0 {
                        return Ok(self.end_meta_block());
                    }
                    if self.window.is_full() {
                        return Ok(self.pause(CommandStep::Begin, Flow::Continue));
                    }
                    CommandStep::Begin
                }
            };
        }
    }

    const fn pause(&mut self, step: CommandStep, flow: Flow) -> Flow {
        self.phase = Phase::Commands(step);
        flow
    }

    const fn end_meta_block(&mut self) -> Flow {
        self.phase = Phase::MetaBlockEnd;
        Flow::Continue
    }

    #[inline(always)]
    fn read_command(&mut self, bits: &mut Bits<'_>) -> Option<(u32, PendingCopy)> {
        bits.refill();
        let tree = self.command_trees.offsets[self.command_tree] as usize;
        let (symbol, length) = decode_at(&self.command_trees.tables, tree, bits.word());
        let command = COMMANDS[usize::from(symbol)];
        let insert_extra_bits = u32::from(command.insert_extra_bits);
        let copy_extra_bits = u32::from(command.copy_extra_bits);
        if !bits.has(length + insert_extra_bits + copy_extra_bits) || length > bits.available() {
            return None;
        }
        bits.consume(length);
        let insert = command.insert_base + bits.read(insert_extra_bits);
        let copy = PendingCopy {
            length: command.copy_base + bits.read(copy_extra_bits),
            implicit_distance: command.implicit_distance,
            distance_context: command.distance_context,
        };
        self.blocks[COMMAND].length = self.blocks[COMMAND].length.wrapping_sub(1);
        Some((insert, copy))
    }

    #[inline(always)]
    fn literals(&mut self, bits: &mut Bits<'_>, count: u32) -> Result<(u32, Flow), DecodeError> {
        let mut count = count;
        while count != 0 {
            if !self.window.reserve() {
                return Ok((count, Flow::Continue));
            }
            if self.blocks[LITERAL].length == 0 {
                if self.blocks[LITERAL].types < 2 {
                    return Err(DecodeError::LiteralBlockLengthExceeded);
                }
                if !self.switch_block(bits, LITERAL) {
                    return Ok((count, Flow::NeedsInput));
                }
            }
            let run = count.min(self.blocks[LITERAL].length).min(self.window.space() as u32);
            let decoded = self.literal_run(bits, run);
            count -= decoded;
            self.blocks[LITERAL].length -= decoded;
            if decoded < run {
                return Ok((count, Flow::NeedsInput));
            }
        }
        Ok((0, Flow::Continue))
    }

    #[inline(always)]
    fn literal_run(&mut self, bits: &mut Bits<'_>, run: u32) -> u32 {
        let tables = &self.literal_trees.tables;
        let window = &mut self.window;
        let start = window.position;
        let last = window.previous(1);
        let before_last = window.previous(2);
        let target = &mut window.buffer[start..start + run as usize];
        let decoded = match self.literal_tree {
            Some(tree) => single_tree_literals(bits, tables, tree, target),
            None => {
                let trees = &self.literal_context_trees;
                let lookup = &CONTEXT_LOOKUP[self.literal_mode];
                let mut last = last;
                let mut before_last = before_last;
                let mut decoded = target.len();
                for (index, slot) in target.iter_mut().enumerate() {
                    bits.ensure(CODE_LENGTH_MAX);
                    let context = lookup[usize::from(last)] | lookup[CONTEXT_SECOND_BYTE_OFFSET + usize::from(before_last)];
                    let tree = trees[usize::from(context) & CONTEXT_MASK] as usize;
                    let (symbol, length) = decode_at(tables, tree, bits.word());
                    if length > bits.available() {
                        decoded = index;
                        break;
                    }
                    bits.consume(length);
                    before_last = last;
                    last = symbol as u8;
                    *slot = last;
                }
                decoded
            }
        };
        window.position = start + decoded;
        decoded as u32
    }

    #[inline(always)]
    fn distance(&mut self, bits: &mut Bits<'_>, copy: PendingCopy) -> Result<Option<CommandStep>, DecodeError> {
        let (distance, push) = if copy.implicit_distance {
            (self.distances[(self.distance_index + DISTANCE_RING_SIZE - 1) % DISTANCE_RING_SIZE], false)
        } else {
            if self.blocks[DISTANCE].length == 0 && !self.switch_block(bits, DISTANCE) {
                return Ok(None);
            }
            let Some(read) = self.read_distance(bits, copy.distance_context)? else {
                return Ok(None);
            };
            self.blocks[DISTANCE].length = self.blocks[DISTANCE].length.wrapping_sub(1);
            read
        };
        let max_distance = self.window.max_distance() as u32;
        if distance > max_distance {
            if copy.length < WORD_LENGTH_MIN || copy.length > WORD_LENGTH_MAX {
                return Err(DecodeError::InvalidDictionaryReference);
            }
            self.word = dictionary::word(copy.length, distance - max_distance - 1)?;
            self.remaining -= i32::from(self.word.length);
            return Ok(Some(CommandStep::Word { written: 0 }));
        }
        if push {
            self.distances[self.distance_index % DISTANCE_RING_SIZE] = distance;
            self.distance_index = (self.distance_index + 1) % DISTANCE_RING_SIZE;
        }
        self.remaining -= copy.length as i32;
        Ok(Some(CommandStep::Copy { distance, count: copy.length }))
    }

    #[inline(always)]
    fn read_distance(&mut self, bits: &mut Bits<'_>, context: u8) -> Result<Option<(u32, bool)>, DecodeError> {
        bits.refill();
        let word = bits.word();
        let tree = usize::from(self.distance_map[self.distance_map_offset + usize::from(context)]);
        let (symbol, length) = decode_at(&self.distance_trees.tables, self.distance_trees.offsets[tree] as usize, word);
        let symbol = u32::from(symbol);
        if symbol < DISTANCE_SHORT_CODES + self.direct_codes {
            if length > bits.available() {
                return Ok(None);
            }
            bits.consume(length);
            if symbol >= DISTANCE_SHORT_CODES {
                return Ok(Some((symbol - DISTANCE_DIRECT_OFFSET, true)));
            }
            return Ok(Some((self.short_distance(symbol)?, symbol != 0)));
        }
        let code = symbol - DISTANCE_SHORT_CODES - self.direct_codes;
        let postfix = code & low_mask(self.postfix_bits) as u32;
        let high = code >> self.postfix_bits;
        let extra_bits = (high >> 1) + 1;
        if length + extra_bits > bits.available() {
            return Ok(None);
        }
        let extra = ((word >> length) & low_mask(extra_bits)) as u32;
        bits.consume(length + extra_bits);
        let offset = ((DISTANCE_OFFSET_BASE + (high & 1)) << extra_bits) - DISTANCE_OFFSET_BIAS;
        Ok(Some((((offset + extra) << self.postfix_bits) + postfix + self.direct_codes + 1, true)))
    }

    #[inline(always)]
    fn short_distance(&self, code: u32) -> Result<u32, DecodeError> {
        let back = |count: usize| self.distances[(self.distance_index + DISTANCE_RING_SIZE - count) % DISTANCE_RING_SIZE];
        if code < DISTANCE_LAST_CODES {
            return Ok(back(code as usize + 1));
        }
        let (base, adjust) = if code < DISTANCE_SECOND_ADJUST_CODE {
            (back(1), code - DISTANCE_LAST_CODES)
        } else {
            (back(2), code - DISTANCE_SECOND_ADJUST_CODE)
        };
        let delta = adjust / 2 + 1;
        if adjust % 2 == 1 {
            return Ok(base + delta);
        }
        if base <= delta {
            return Err(DecodeError::InvalidDistance);
        }
        Ok(base - delta)
    }
}

#[inline(always)]
fn single_tree_literals(bits: &mut Bits<'_>, tables: &[Entry], tree: usize, target: &mut [u8]) -> usize {
    for (index, slot) in target.iter_mut().enumerate() {
        bits.ensure(CODE_LENGTH_MAX);
        let (symbol, length) = decode_at(tables, tree, bits.word());
        if length > bits.available() {
            return index;
        }
        bits.consume(length);
        *slot = symbol as u8;
    }
    target.len()
}
