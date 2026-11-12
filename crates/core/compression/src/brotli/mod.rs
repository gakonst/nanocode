use alloc::vec::Vec;
use core::fmt;

mod bits;
mod blocks;
mod commands;
mod dictionary;
mod header;
mod prefix;
mod tables;
mod window;

use bits::{BitBuffer, Bits};
use blocks::{BlockState, BlockStep, CATEGORIES, MapStep};
use commands::CommandStep;
use dictionary::Word;
use header::HeaderStep;
use prefix::{CodeReader, Entry};
use window::Window;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    NeedsInput,
    NeedsOutput,
    Done,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Progress {
    pub consumed: usize,
    pub written: usize,
    pub status: Status,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecodeError {
    InvalidWindowBits,
    ExuberantNibble,
    ReservedBitSet,
    ExuberantMetadataByte,
    PaddingBitsNotZero,
    SimpleCodeSymbolTooLarge,
    SimpleCodeDuplicateSymbol,
    InvalidCodeLengthCode,
    InvalidPrefixCode,
    InvalidContextMapRepeat,
    MetaBlockLengthExceeded,
    LiteralBlockLengthExceeded,
    InvalidDistance,
    InvalidDictionaryReference,
    InvalidTransform,
}

impl DecodeError {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidWindowBits => "invalid window bits",
            Self::ExuberantNibble => "meta-block length has a zero last nibble",
            Self::ReservedBitSet => "reserved bit set",
            Self::ExuberantMetadataByte => "metadata length has a zero last byte",
            Self::PaddingBitsNotZero => "padding bits not zero",
            Self::SimpleCodeSymbolTooLarge => "simple prefix code symbol outside the alphabet",
            Self::SimpleCodeDuplicateSymbol => "simple prefix code repeats a symbol",
            Self::InvalidCodeLengthCode => "code length code is not complete",
            Self::InvalidPrefixCode => "prefix code is not complete",
            Self::InvalidContextMapRepeat => "context map run exceeds the map",
            Self::MetaBlockLengthExceeded => "commands exceed the meta-block length",
            Self::LiteralBlockLengthExceeded => "literals exceed the literal block length",
            Self::InvalidDistance => "invalid distance",
            Self::InvalidDictionaryReference => "dictionary reference with an invalid length",
            Self::InvalidTransform => "invalid dictionary transform",
        }
    }
}

impl fmt::Display for DecodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Clone, Copy)]
enum Phase {
    StreamHeader,
    Header(HeaderStep),
    Metadata,
    Uncompressed,
    BlockTypes { category: usize, step: BlockStep },
    DistanceParameters,
    ContextModes { next: usize },
    ContextMap { distance: bool, step: MapStep },
    TreeGroup { group: usize },
    Commands(CommandStep),
    MetaBlockEnd,
    Flush,
    Done,
    Failed(DecodeError),
}

enum Flow {
    Continue,
    NeedsInput,
    NeedsOutput,
    Done,
}

struct Group {
    tables: Vec<Entry>,
    offsets: Vec<u32>,
}

impl Group {
    const fn new() -> Self {
        Self { tables: Vec::new(), offsets: Vec::new() }
    }

    fn clear(&mut self) {
        self.tables.clear();
        self.offsets.clear();
    }
}

const DISTANCE_RING_SIZE: usize = 4;
const INITIAL_DISTANCES: [u32; DISTANCE_RING_SIZE] = [16, 15, 11, 4];

pub struct Decoder {
    phase: Phase,
    bits: BitBuffer,
    window: Window,
    window_size: usize,
    remaining: i32,
    is_last: bool,
    distances: [u32; DISTANCE_RING_SIZE],
    distance_index: usize,
    blocks: [BlockState; CATEGORIES],
    block_tables: Vec<Entry>,
    context_modes: Vec<u8>,
    literal_map: Vec<u8>,
    distance_map: Vec<u8>,
    map_table: Vec<Entry>,
    literal_tree_count: u32,
    distance_tree_count: u32,
    literal_trees: Group,
    command_trees: Group,
    distance_trees: Group,
    code: CodeReader,
    postfix_bits: u32,
    direct_codes: u32,
    literal_mode: usize,
    literal_tree: Option<usize>,
    literal_context_trees: [u32; tables::LITERAL_CONTEXTS],
    word: Word,
    command_tree: usize,
    distance_map_offset: usize,
}

impl Default for Decoder {
    fn default() -> Self {
        Self::new()
    }
}

impl Decoder {
    pub const fn new() -> Self {
        Self {
            phase: Phase::StreamHeader,
            bits: BitBuffer::new(),
            window: Window::new(),
            window_size: 0,
            remaining: 0,
            is_last: false,
            distances: INITIAL_DISTANCES,
            distance_index: 0,
            blocks: [BlockState::new(); CATEGORIES],
            block_tables: Vec::new(),
            context_modes: Vec::new(),
            literal_map: Vec::new(),
            distance_map: Vec::new(),
            map_table: Vec::new(),
            literal_tree_count: 0,
            distance_tree_count: 0,
            literal_trees: Group::new(),
            command_trees: Group::new(),
            distance_trees: Group::new(),
            code: CodeReader::new(),
            postfix_bits: 0,
            direct_codes: 0,
            literal_mode: 0,
            literal_tree: None,
            literal_context_trees: [0; tables::LITERAL_CONTEXTS],
            word: Word::EMPTY,
            command_tree: 0,
            distance_map_offset: 0,
        }
    }

    pub fn decompress(&mut self, input: &[u8], output: &mut [u8]) -> Result<Progress, DecodeError> {
        if let Phase::Failed(error) = self.phase {
            return Err(error);
        }
        let mut bits = Bits::new(self.bits, input);
        let mut written = 0;
        match self.run(&mut bits, output, &mut written) {
            Ok(status) => {
                self.bits = bits.finish();
                Ok(Progress { consumed: bits.position(), written, status })
            }
            Err(error) => {
                self.phase = Phase::Failed(error);
                Err(error)
            }
        }
    }

    fn run(&mut self, bits: &mut Bits<'_>, output: &mut [u8], written: &mut usize) -> Result<Status, DecodeError> {
        loop {
            match self.step(bits, output, written)? {
                Flow::Continue => {}
                Flow::NeedsInput => {
                    self.check_length()?;
                    *written += self.window.flush(&mut output[*written..]);
                    bits.absorb();
                    return Ok(Status::NeedsInput);
                }
                Flow::NeedsOutput => {
                    bits.unload();
                    return Ok(Status::NeedsOutput);
                }
                Flow::Done => return Ok(Status::Done),
            }
        }
    }

    fn step(&mut self, bits: &mut Bits<'_>, output: &mut [u8], written: &mut usize) -> Result<Flow, DecodeError> {
        if self.window.is_full() {
            self.check_length()?;
            *written += self.window.flush(&mut output[*written..]);
            if self.window.pending() != 0 {
                return Ok(Flow::NeedsOutput);
            }
            self.window.wrap();
        }
        match self.phase {
            Phase::StreamHeader => self.stream_header(bits),
            Phase::Header(step) => self.header(bits, step),
            Phase::Metadata => Ok(self.metadata(bits)),
            Phase::Uncompressed => Ok(self.uncompressed(bits)),
            Phase::BlockTypes { category, step } => self.block_types(bits, category, step),
            Phase::DistanceParameters => Ok(self.distance_parameters(bits)),
            Phase::ContextModes { next } => Ok(self.context_modes(bits, next)),
            Phase::ContextMap { distance, step } => self.context_map(bits, distance, step),
            Phase::TreeGroup { group } => self.tree_group(bits, group),
            Phase::Commands(step) => self.commands(bits, step),
            Phase::MetaBlockEnd => self.meta_block_end(bits),
            Phase::Flush => self.flush(output, written),
            Phase::Done => Ok(Flow::Done),
            Phase::Failed(error) => Err(error),
        }
    }

    const fn check_length(&self) -> Result<(), DecodeError> {
        if self.remaining < 0 {
            return Err(DecodeError::MetaBlockLengthExceeded);
        }
        Ok(())
    }

    fn meta_block_end(&mut self, bits: &mut Bits<'_>) -> Result<Flow, DecodeError> {
        if !self.is_last {
            self.remaining = 0;
            self.phase = Phase::Header(HeaderStep::Last);
            return Ok(Flow::Continue);
        }
        if bits.align() != 0 {
            return Err(DecodeError::PaddingBitsNotZero);
        }
        bits.unload();
        self.phase = Phase::Flush;
        Ok(Flow::Continue)
    }

    fn flush(&mut self, output: &mut [u8], written: &mut usize) -> Result<Flow, DecodeError> {
        self.check_length()?;
        *written += self.window.flush(&mut output[*written..]);
        if self.window.pending() != 0 {
            return Ok(Flow::NeedsOutput);
        }
        self.phase = Phase::Done;
        Ok(Flow::Done)
    }
}
