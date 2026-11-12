use alloc::{boxed::Box, vec, vec::Vec};

use super::bits::{BitWriter, Pending};
use super::trees::{
    BIT_LENGTH_CODES, BlockLengths, DISTANCE_CODES, HEAP_SIZE, StaticDescription, TreeDescription,
};
use super::{DeflateConfig, DeflateError, DeflateFlush, Strategy};
use crate::checksum::{adler32, crc32};

pub(super) const HASH_SIZE: usize = 1 << u16::BITS;
pub(super) const STD_MIN_MATCH: usize = 3;
pub(super) const STD_MAX_MATCH: usize = 258;
pub(super) const WANT_MIN_MATCH: usize = 4;
pub(super) const MIN_LOOKAHEAD: usize = STD_MAX_MATCH + STD_MIN_MATCH + 1;
pub(super) const MAX_STORED: usize = u16::MAX as usize;
pub(super) const ADLER32_INITIAL: u32 = 1;
pub(super) const CRC32_INITIAL: u32 = 0;
pub(super) const ROLL_HASH_CHAIN_ABOVE: u16 = 1024;
const LEVEL_DEFAULT_REQUEST: i32 = -1;
const LEVEL_DEFAULT: i32 = 6;
const LEVEL_MAX: i32 = 9;
const WINDOW_BITS_MIN: i32 = 8;
const WINDOW_BITS_MAX: i32 = 15;
const WINDOW_BITS_SMALLEST_USED: i32 = 9;
const GZIP_WINDOW_BITS_OFFSET: i32 = 16;
const MEM_LEVEL_MIN: i32 = 1;
const MEM_LEVEL_MAX: i32 = 9;
const SYMBOL_CAPACITY_MEM_LEVEL_SHIFT: i32 = 6;
const PENDING_BYTES_PER_SYMBOL: usize = 4;
const SYMBOL_BYTES: usize = 3;
const ZLIB_PRESET_DICTIONARY: u16 = 0x20;
const ZLIB_METHOD_DEFLATE: u16 = 8;
const ZLIB_WINDOW_BITS_BIAS: u32 = 8;
const ZLIB_WINDOW_SHIFT: u32 = 4;
const ZLIB_LEVEL_SHIFT: u32 = 6;
const ZLIB_CHECK_MODULUS: u16 = 31;
const ZLIB_LEVEL_FASTEST: u16 = 0;
const ZLIB_LEVEL_FAST: u16 = 1;
const ZLIB_LEVEL_DEFAULT: u16 = 2;
const ZLIB_LEVEL_SLOWEST: u16 = 3;
const ZLIB_LEVEL_FAST_FROM: u8 = 2;
const ZLIB_LEVEL_DEFAULT_IS: u8 = 6;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Status {
    Init,
    Gzip,
    Busy,
    Finish,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Wrapper {
    Raw,
    Zlib,
    Gzip,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum FlushRank {
    Initial,
    Incomplete,
    NoFlush,
    Block,
    PartialFlush,
    SyncFlush,
    FullFlush,
    Finish,
}

impl FlushRank {
    pub(super) const fn of(flush: DeflateFlush) -> Self {
        match flush {
            DeflateFlush::NoFlush => Self::NoFlush,
            DeflateFlush::Block => Self::Block,
            DeflateFlush::PartialFlush => Self::PartialFlush,
            DeflateFlush::SyncFlush => Self::SyncFlush,
            DeflateFlush::FullFlush => Self::FullFlush,
            DeflateFlush::Finish => Self::Finish,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum HashVariant {
    Standard,
    Roll,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum QuickBlock {
    Closed,
    Open,
    OpenLast,
}

#[derive(Debug)]
pub(super) enum BlockState {
    NeedMore,
    BlockDone,
    FinishStarted,
    FinishDone,
}

#[derive(Clone, Copy)]
pub(super) enum Algorithm {
    Stored,
    Quick,
    Fast,
    Medium,
    Slow,
}

pub(super) struct LevelConfig {
    good_length: u16,
    max_lazy: u16,
    nice_length: u16,
    max_chain: u16,
    pub(super) algorithm: Algorithm,
}

const fn level(
    good_length: u16,
    max_lazy: u16,
    nice_length: u16,
    max_chain: u16,
    algorithm: Algorithm,
) -> LevelConfig {
    LevelConfig {
        good_length,
        max_lazy,
        nice_length,
        max_chain,
        algorithm,
    }
}

pub(super) const LEVEL_CONFIGS: [LevelConfig; 10] = [
    level(0, 0, 0, 0, Algorithm::Stored),
    level(0, 0, 0, 0, Algorithm::Quick),
    level(4, 4, 8, 4, Algorithm::Fast),
    level(4, 6, 16, 6, Algorithm::Medium),
    level(4, 12, 32, 24, Algorithm::Medium),
    level(8, 16, 32, 32, Algorithm::Medium),
    level(8, 16, 128, 128, Algorithm::Medium),
    level(8, 32, 128, 256, Algorithm::Slow),
    level(32, 128, 258, 1024, Algorithm::Slow),
    level(32, 258, 258, 4096, Algorithm::Slow),
];

#[derive(Clone)]
pub(super) struct SymbolBuffer {
    bytes: Vec<u8>,
    filled: usize,
}

impl SymbolBuffer {
    fn new(symbol_capacity: usize) -> Self {
        Self {
            bytes: vec![0; symbol_capacity * SYMBOL_BYTES],
            filled: 0,
        }
    }

    pub(super) fn iter(&self) -> impl Iterator<Item = (u16, u8)> + '_ {
        self.bytes[..self.filled]
            .as_chunks::<SYMBOL_BYTES>()
            .0
            .iter()
            .map(|symbol| (u16::from_le_bytes([symbol[0], symbol[1]]), symbol[2]))
    }

    pub(super) const fn is_empty(&self) -> bool {
        self.filled == 0
    }

    pub(super) const fn clear(&mut self) {
        self.filled = 0;
    }

    pub(super) const fn should_flush_block(&self) -> bool {
        self.filled == self.bytes.len() - SYMBOL_BYTES
    }

    pub(super) fn push_literal(&mut self, byte: u8) {
        self.bytes[self.filled..self.filled + SYMBOL_BYTES].copy_from_slice(&[0, 0, byte]);
        self.filled += SYMBOL_BYTES;
    }

    pub(super) fn push_distance(&mut self, distance: u16, length: u8) {
        let [low, high] = distance.to_le_bytes();
        self.bytes[self.filled..self.filled + SYMBOL_BYTES].copy_from_slice(&[low, high, length]);
        self.filled += SYMBOL_BYTES;
    }
}

pub(super) struct Io<'a> {
    input: &'a [u8],
    consumed: usize,
    output: &'a mut [u8],
    produced: usize,
}

impl<'a> Io<'a> {
    pub(super) const fn new(input: &'a [u8], output: &'a mut [u8]) -> Self {
        Self {
            input,
            consumed: 0,
            output,
            produced: 0,
        }
    }

    pub(super) const fn input_available(&self) -> usize {
        self.input.len() - self.consumed
    }

    pub(super) const fn output_available(&self) -> usize {
        self.output.len() - self.produced
    }

    pub(super) fn take_input(&mut self, length: usize) -> &'a [u8] {
        let data = &self.input[self.consumed..self.consumed + length];
        self.consumed += length;
        data
    }

    pub(super) fn consumed_tail(&self, length: usize) -> &'a [u8] {
        &self.input[self.consumed - length..self.consumed]
    }

    pub(super) fn write(&mut self, data: &[u8]) {
        self.output[self.produced..self.produced + data.len()].copy_from_slice(data);
        self.produced += data.len();
    }
}

#[derive(Clone)]
pub(super) struct State {
    pub(super) status: Status,
    pub(super) previous_flush: FlushRank,
    pub(super) wrapper: Wrapper,
    pub(super) trailer_written: bool,
    pub(super) strategy: Strategy,
    pub(super) level: u8,
    pub(super) quick_block: QuickBlock,
    pub(super) hash: HashVariant,
    pub(super) match_available: bool,
    pub(super) good_match: u16,
    pub(super) nice_match: u16,
    pub(super) match_start: u16,
    pub(super) previous_match: u16,
    pub(super) string_start: usize,
    pub(super) window: Vec<u8>,
    pub(super) window_size: usize,
    pub(super) lookahead: usize,
    pub(super) previous: Vec<u16>,
    pub(super) head: Box<[u16; HASH_SIZE]>,
    pub(super) previous_length: u16,
    pub(super) max_chain_length: u16,
    pub(super) max_lazy_match: u16,
    pub(super) matches: u8,
    pub(super) block_start: isize,
    pub(super) symbols: SymbolBuffer,
    pub(super) bits: BitWriter,
    pub(super) lengths: BlockLengths,
    pub(super) insert: usize,
    pub(super) insert_hash: u32,
    pub(super) checksum: u32,
    pub(super) total_in: u64,
    pub(super) total_out: u64,
    pub(super) literal_tree: TreeDescription<HEAP_SIZE>,
    pub(super) distance_tree: TreeDescription<{ 2 * DISTANCE_CODES + 1 }>,
    pub(super) bit_length_tree: TreeDescription<{ 2 * BIT_LENGTH_CODES + 1 }>,
}

struct Parameters {
    level: u8,
    wrapper: Wrapper,
    window_bits: u32,
    symbol_capacity: usize,
}

fn parameters(config: DeflateConfig) -> Result<Parameters, DeflateError> {
    let level = if config.level == LEVEL_DEFAULT_REQUEST {
        LEVEL_DEFAULT
    } else {
        config.level
    };
    let (wrapper, window_bits) = if config.window_bits < 0 {
        if config.window_bits < -WINDOW_BITS_MAX {
            return Err(DeflateError::Stream);
        }
        (Wrapper::Raw, -config.window_bits)
    } else if config.window_bits > WINDOW_BITS_MAX {
        (Wrapper::Gzip, config.window_bits - GZIP_WINDOW_BITS_OFFSET)
    } else {
        (Wrapper::Zlib, config.window_bits)
    };
    if !(MEM_LEVEL_MIN..=MEM_LEVEL_MAX).contains(&config.mem_level)
        || !(WINDOW_BITS_MIN..=WINDOW_BITS_MAX).contains(&window_bits)
        || !(0..=LEVEL_MAX).contains(&level)
        || (window_bits == WINDOW_BITS_MIN && wrapper != Wrapper::Zlib)
    {
        return Err(DeflateError::Stream);
    }
    let window_bits = if window_bits == WINDOW_BITS_MIN {
        WINDOW_BITS_SMALLEST_USED
    } else {
        window_bits
    };
    Ok(Parameters {
        level: u8::try_from(level).expect("a checked level fits in a byte"),
        wrapper,
        window_bits: u32::try_from(window_bits).expect("checked window bits are positive"),
        symbol_capacity: 1 << (config.mem_level + SYMBOL_CAPACITY_MEM_LEVEL_SHIFT),
    })
}

impl State {
    pub(super) fn new(config: DeflateConfig) -> Result<Self, DeflateError> {
        let parameters = parameters(config)?;
        let window_size = 1 << parameters.window_bits;
        let mut state = Self {
            status: Status::Init,
            previous_flush: FlushRank::Initial,
            wrapper: parameters.wrapper,
            trailer_written: false,
            strategy: config.strategy,
            level: parameters.level,
            quick_block: QuickBlock::Closed,
            hash: HashVariant::Standard,
            match_available: false,
            good_match: 0,
            nice_match: 0,
            match_start: 0,
            previous_match: 0,
            string_start: 0,
            window: vec![0; 2 * window_size],
            window_size,
            lookahead: 0,
            previous: vec![0; window_size],
            head: vec![0; HASH_SIZE]
                .into_boxed_slice()
                .try_into()
                .expect("the head table has HASH_SIZE entries"),
            previous_length: 0,
            max_chain_length: 0,
            max_lazy_match: 0,
            matches: 0,
            block_start: 0,
            symbols: SymbolBuffer::new(parameters.symbol_capacity),
            bits: BitWriter::new(Pending::new(
                parameters.symbol_capacity * PENDING_BYTES_PER_SYMBOL,
            )),
            lengths: BlockLengths::default(),
            insert: 0,
            insert_hash: 0,
            checksum: ADLER32_INITIAL,
            total_in: 0,
            total_out: 0,
            literal_tree: TreeDescription::new(&StaticDescription::LITERAL),
            distance_tree: TreeDescription::new(&StaticDescription::DISTANCE),
            bit_length_tree: TreeDescription::new(&StaticDescription::BIT_LENGTH),
        };
        state.reset();
        Ok(state)
    }

    fn reset(&mut self) {
        self.status = match self.wrapper {
            Wrapper::Gzip => Status::Gzip,
            Wrapper::Raw | Wrapper::Zlib => Status::Init,
        };
        self.checksum = match self.wrapper {
            Wrapper::Gzip => CRC32_INITIAL,
            Wrapper::Raw | Wrapper::Zlib => ADLER32_INITIAL,
        };
        self.previous_flush = FlushRank::Initial;
        self.init_block();
        self.head.fill(0);
        self.set_level(self.level);
    }

    fn set_level(&mut self, level: u8) {
        let config = &LEVEL_CONFIGS[usize::from(level)];
        self.max_lazy_match = config.max_lazy;
        self.good_match = config.good_length;
        self.nice_match = config.nice_length;
        self.max_chain_length = config.max_chain;
        self.hash = if config.max_chain > ROLL_HASH_CHAIN_ABOVE {
            HashVariant::Roll
        } else {
            HashVariant::Standard
        };
        self.level = level;
    }

    pub(super) fn init_block(&mut self) {
        self.literal_tree.clear_frequencies();
        self.distance_tree.clear_frequencies();
        self.bit_length_tree.clear_frequencies();
        self.literal_tree.mark_end_of_block();
        self.lengths = BlockLengths::default();
        self.symbols.clear();
        self.matches = 0;
    }

    pub(super) const fn window_mask(&self) -> usize {
        self.window_size - 1
    }

    pub(super) const fn max_distance(&self) -> usize {
        self.window_size - MIN_LOOKAHEAD
    }

    pub(super) const fn within_distance(&self, head: usize) -> bool {
        head < self.string_start && self.string_start - head <= self.max_distance()
    }

    pub(super) const fn window_buffer_size(&self) -> usize {
        self.window.len()
    }

    pub(super) fn zlib_header(&self) -> u16 {
        let dictionary = if self.string_start == 0 {
            0
        } else {
            ZLIB_PRESET_DICTIONARY
        };
        let window_bits = u16::try_from(self.window_size.trailing_zeros() - ZLIB_WINDOW_BITS_BIAS)
            .expect("window bits fit in 16 bits");
        let header = ((ZLIB_METHOD_DEFLATE + (window_bits << ZLIB_WINDOW_SHIFT)) << u8::BITS)
            | (self.zlib_level_flags() << ZLIB_LEVEL_SHIFT)
            | dictionary;
        header + ZLIB_CHECK_MODULUS - (header % ZLIB_CHECK_MODULUS)
    }

    const fn zlib_level_flags(&self) -> u16 {
        if self.strategy.is_fast() || self.level < ZLIB_LEVEL_FAST_FROM {
            ZLIB_LEVEL_FASTEST
        } else if self.level < ZLIB_LEVEL_DEFAULT_IS {
            ZLIB_LEVEL_FAST
        } else if self.level == ZLIB_LEVEL_DEFAULT_IS {
            ZLIB_LEVEL_DEFAULT
        } else {
            ZLIB_LEVEL_SLOWEST
        }
    }

    pub(super) fn flush_pending(&mut self, io: &mut Io) {
        self.bits.flush_bits();
        let length = self.bits.pending.length().min(io.output_available());
        if length == 0 {
            return;
        }
        io.write(&self.bits.pending.bytes()[..length]);
        self.total_out += u64::try_from(length).expect("a buffer length fits in 64 bits");
        self.bits.pending.advance(length);
    }

    pub(super) fn update_checksum(&mut self, data: &[u8]) {
        match self.wrapper {
            Wrapper::Gzip => self.checksum = crc32(self.checksum, data),
            Wrapper::Zlib => self.checksum = adler32(self.checksum, data),
            Wrapper::Raw => {}
        }
    }

    pub(super) fn read_buffer(&mut self, io: &mut Io, offset: usize, size: usize) -> usize {
        let length = io.input_available().min(size);
        if length == 0 {
            return 0;
        }
        let data = io.take_input(length);
        self.window[offset..offset + length].copy_from_slice(data);
        self.update_checksum(data);
        self.total_in += u64::try_from(length).expect("a buffer length fits in 64 bits");
        length
    }

    pub(super) fn set_dictionary(&mut self, dictionary: &[u8]) -> Result<(), DeflateError> {
        let wrapper_active = !self.trailer_written;
        let rejected = match self.wrapper {
            Wrapper::Gzip => wrapper_active,
            Wrapper::Zlib => wrapper_active && self.status != Status::Init,
            Wrapper::Raw => false,
        };
        if rejected || self.lookahead != 0 {
            return Err(DeflateError::Stream);
        }
        if wrapper_active && self.wrapper == Wrapper::Zlib {
            self.checksum = adler32(self.checksum, dictionary);
        }
        let wrapper = core::mem::replace(&mut self.wrapper, Wrapper::Raw);
        let mut dictionary = dictionary;
        if dictionary.len() >= self.window_buffer_size() {
            if wrapper == Wrapper::Raw {
                self.head.fill(0);
                self.string_start = 0;
                self.block_start = 0;
                self.insert = 0;
            }
            dictionary = &dictionary[dictionary.len() - self.window_size..];
        }
        let mut output = [0; 0];
        let mut io = Io::new(dictionary, &mut output);
        self.fill_window(&mut io);
        while self.lookahead >= STD_MIN_MATCH {
            let start = self.string_start;
            let count = self.lookahead - (STD_MIN_MATCH - 1);
            self.insert_string(start, count);
            self.string_start = start + count;
            self.lookahead = STD_MIN_MATCH - 1;
            self.fill_window(&mut io);
        }
        self.string_start += self.lookahead;
        self.block_start = signed(self.string_start);
        self.insert = self.lookahead;
        self.lookahead = 0;
        self.previous_length = 0;
        self.match_available = false;
        self.wrapper = wrapper;
        Ok(())
    }

    pub(super) fn tally_literal(&mut self, byte: u8) -> bool {
        self.symbols.push_literal(byte);
        self.literal_tree.count_literal(byte);
        self.symbols.should_flush_block()
    }

    pub(super) fn tally_distance(&mut self, distance: usize, length: usize) -> bool {
        let distance_code = u16::try_from(distance).expect("a match distance fits in 16 bits");
        let length_code = u8::try_from(length).expect("a match length code fits in a byte");
        self.symbols.push_distance(distance_code, length_code);
        self.matches = self.matches.saturating_add(1);
        assert!(
            distance - 1 < self.max_distance(),
            "a match is within the window"
        );
        self.literal_tree.count_length(length_code);
        self.distance_tree.count_distance(distance - 1);
        self.symbols.should_flush_block()
    }

    pub(super) fn flush_block_only(&mut self, io: &mut Io, last: bool) {
        let window_offset = usize::try_from(self.block_start).ok();
        let stored_length = usize::try_from(signed(self.string_start) - self.block_start)
            .expect("the block starts at or before the current string");
        self.flush_block(window_offset, stored_length, last);
        self.block_start = signed(self.string_start);
        self.flush_pending(io);
    }

    pub(super) fn flush_block_or_stop(&mut self, io: &mut Io, last: bool) -> Option<BlockState> {
        self.flush_block_only(io, last);
        if io.output_available() != 0 {
            return None;
        }
        Some(if last {
            BlockState::FinishStarted
        } else {
            BlockState::NeedMore
        })
    }

    pub(super) fn finish_block(&mut self, io: &mut Io, flush: DeflateFlush) -> BlockState {
        if flush == DeflateFlush::Finish {
            if let Some(stop) = self.flush_block_or_stop(io, true) {
                return stop;
            }
            return BlockState::FinishDone;
        }
        if !self.symbols.is_empty()
            && let Some(stop) = self.flush_block_or_stop(io, false)
        {
            return stop;
        }
        BlockState::BlockDone
    }
}

pub(super) fn signed(position: usize) -> isize {
    isize::try_from(position).expect("a window position fits in isize")
}
