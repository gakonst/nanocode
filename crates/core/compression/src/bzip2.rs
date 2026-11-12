mod block_sort;
mod crc;
mod decode;
mod encode;
mod huffman;
mod random;

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::fmt;

use decode::{Decoder, Input, Phase, Step};
use encode::{BitWriter, Tables};

const LEVEL_MIN: u8 = 1;
const LEVEL_MAX: u8 = 9;
const BLOCK_BYTES_PER_LEVEL: usize = 100_000;
const BLOCK_BYTES_RESERVED: usize = 19;
const BYTE_VALUES: usize = 1 << u8::BITS;
const ALPHABET_SIZE_MAX: usize = BYTE_VALUES + 2;
const CODE_LENGTH_SLOTS: usize = 23;
const GROUPS_MAX: usize = 6;
const GROUP_SYMBOLS: usize = 50;
const SELECTORS_MAX: usize = 2 + BLOCK_BYTES_PER_LEVEL * LEVEL_MAX as usize / GROUP_SYMBOLS;
const OVERSHOOT_BYTES: usize = 34;
const RUN_A: u16 = 0;
const RUN_B: u16 = 1;
const RLE_RUN_BYTES: usize = 4;
const RLE_RUN_LENGTH_MAX: usize = 255;
const STREAM_MAGIC: [u8; 3] = *b"BZh";
const BLOCK_MAGIC: [u8; 6] = [0x31, 0x41, 0x59, 0x26, 0x53, 0x59];
const END_MAGIC: [u8; 6] = [0x17, 0x72, 0x45, 0x38, 0x50, 0x90];
const RANDOMISED_BITS: u32 = 1;
const ORIGIN_BITS: u32 = 24;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bzip2Error {
    Sequence,
    Data,
    DataMagic,
    Param,
}

impl fmt::Display for Bzip2Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Sequence => "bzip2: sequence of operations invalid",
            Self::Data => "bzip2: invalid data",
            Self::DataMagic => "bzip2: bz2 header missing",
            Self::Param => "bzip2: invalid parameters",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Level(u8);

impl Level {
    #[must_use]
    pub const fn new(level: u32) -> Option<Self> {
        if level >= LEVEL_MIN as u32 && level <= LEVEL_MAX as u32 {
            Some(Self(level as u8))
        } else {
            None
        }
    }

    const fn block_bytes_max(self) -> usize {
        BLOCK_BYTES_PER_LEVEL * self.0 as usize
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Run,
    Finish,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompressStatus {
    Running,
    Finishing,
    StreamEnd,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecompressStatus {
    InProgress,
    StreamEnd,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Running,
    Finishing { input_remaining: usize },
    Finished,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    Filling,
    Draining,
}

#[derive(Clone, Copy)]
struct Run {
    byte: u8,
    length: usize,
}

pub struct Compressor {
    level: Level,
    mode: Mode,
    stage: Stage,
    block: Vec<u8>,
    block_bytes: usize,
    block_bytes_limit: usize,
    run: Option<Run>,
    in_use: [bool; BYTE_VALUES],
    block_crc: u32,
    combined_crc: u32,
    block_number: u32,
    writer: BitWriter,
    compressed: Vec<u8>,
    compressed_sent: usize,
    quadrant: Vec<u16>,
    order: Vec<u32>,
    pairs: Vec<u32>,
    classes: Vec<u32>,
    tables: Box<Tables>,
    total_in: u64,
    total_out: u64,
}

struct Cursor<'a> {
    input: &'a [u8],
    consumed: usize,
    output: &'a mut [u8],
    produced: usize,
}

impl Compressor {
    #[must_use]
    pub fn new(level: Level) -> Self {
        let block_bytes_max = level.block_bytes_max();
        let mut compressor = Self {
            level,
            mode: Mode::Running,
            stage: Stage::Filling,
            block: alloc::vec![0; block_bytes_max + OVERSHOOT_BYTES],
            block_bytes: 0,
            block_bytes_limit: block_bytes_max - BLOCK_BYTES_RESERVED,
            run: None,
            in_use: [false; BYTE_VALUES],
            block_crc: crc::INITIAL,
            combined_crc: 0,
            block_number: 0,
            writer: BitWriter::new(),
            compressed: Vec::new(),
            compressed_sent: 0,
            quadrant: alloc::vec![0; block_bytes_max + OVERSHOOT_BYTES],
            order: alloc::vec![0; block_bytes_max],
            pairs: alloc::vec![0; block_sort::PAIR_TABLE_ENTRIES],
            classes: alloc::vec![0; block_bytes_max],
            tables: Box::new(Tables::new(block_bytes_max)),
            total_in: 0,
            total_out: 0,
        };
        compressor.prepare_block();
        compressor
    }

    #[must_use]
    pub const fn total_in(&self) -> u64 {
        self.total_in
    }

    #[must_use]
    pub const fn total_out(&self) -> u64 {
        self.total_out
    }

    pub fn compress(
        &mut self,
        input: &[u8],
        output: &mut [u8],
        action: Action,
    ) -> Result<CompressStatus, Bzip2Error> {
        if input.is_empty() && action == Action::Run {
            return Ok(CompressStatus::Running);
        }
        let mut cursor = Cursor {
            input,
            consumed: 0,
            output,
            produced: 0,
        };
        let result = match (self.mode, action) {
            (Mode::Finished, _) | (Mode::Finishing { .. }, Action::Run) => {
                Err(Bzip2Error::Sequence)
            }
            (Mode::Running, Action::Run) => {
                if self.advance(&mut cursor) {
                    Ok(CompressStatus::Running)
                } else {
                    Err(Bzip2Error::Param)
                }
            }
            (Mode::Running, Action::Finish) => {
                self.mode = Mode::Finishing {
                    input_remaining: input.len(),
                };
                self.finish(&mut cursor)
            }
            (Mode::Finishing { input_remaining }, Action::Finish) => {
                if input_remaining == input.len() {
                    self.finish(&mut cursor)
                } else {
                    Err(Bzip2Error::Sequence)
                }
            }
        };
        self.total_in += u64::try_from(cursor.consumed).expect("a slice length fits in u64");
        self.total_out += u64::try_from(cursor.produced).expect("a slice length fits in u64");
        result
    }

    fn finish(&mut self, cursor: &mut Cursor<'_>) -> Result<CompressStatus, Bzip2Error> {
        if !self.advance(cursor) {
            return Err(Bzip2Error::Sequence);
        }
        let Mode::Finishing { input_remaining } = self.mode else {
            unreachable!("finish runs only while finishing");
        };
        if input_remaining > 0 || self.run.is_some() || self.compressed_sent < self.compressed.len()
        {
            return Ok(CompressStatus::Finishing);
        }
        self.mode = Mode::Finished;
        Ok(CompressStatus::StreamEnd)
    }

    fn advance(&mut self, cursor: &mut Cursor<'_>) -> bool {
        let mut progress = false;
        loop {
            if self.stage == Stage::Draining {
                progress |= self.drain(cursor);
                if self.compressed_sent < self.compressed.len() {
                    break;
                }
                if self.mode == (Mode::Finishing { input_remaining: 0 }) && self.run.is_none() {
                    break;
                }
                self.prepare_block();
                self.stage = Stage::Filling;
            }
            progress |= self.fill(cursor);
            if let Mode::Finishing { input_remaining: 0 } = self.mode {
                self.flush_run();
                self.compress_block(true);
                self.stage = Stage::Draining;
            } else if self.block_bytes >= self.block_bytes_limit {
                self.compress_block(false);
                self.stage = Stage::Draining;
            } else if cursor.consumed == cursor.input.len() {
                break;
            }
        }
        progress
    }

    fn drain(&mut self, cursor: &mut Cursor<'_>) -> bool {
        let pending = &self.compressed[self.compressed_sent..];
        let count = pending.len().min(cursor.output.len() - cursor.produced);
        cursor.output[cursor.produced..cursor.produced + count].copy_from_slice(&pending[..count]);
        cursor.produced += count;
        self.compressed_sent += count;
        count > 0
    }

    fn fill(&mut self, cursor: &mut Cursor<'_>) -> bool {
        let available = cursor.input.len() - cursor.consumed;
        let allowed = match self.mode {
            Mode::Running => available,
            Mode::Finishing { input_remaining } => available.min(input_remaining),
            Mode::Finished => unreachable!("a finished stream takes no input"),
        };
        let start = cursor.consumed;
        let mut taken = 0;
        while taken < allowed && self.block_bytes < self.block_bytes_limit {
            self.add_byte(cursor.input[start + taken]);
            taken += 1;
        }
        cursor.consumed += taken;
        if let Mode::Finishing { input_remaining } = &mut self.mode {
            *input_remaining -= taken;
        }
        taken > 0
    }
}

impl Compressor {
    fn add_byte(&mut self, byte: u8) {
        match &mut self.run {
            None => self.run = Some(Run { byte, length: 1 }),
            Some(run) if run.byte != byte && run.length == 1 => {
                let previous = run.byte;
                run.byte = byte;
                self.block_crc = crc::update(self.block_crc, previous);
                self.in_use[usize::from(previous)] = true;
                self.block[self.block_bytes] = previous;
                self.block_bytes += 1;
            }
            Some(run) if run.byte != byte || run.length == RLE_RUN_LENGTH_MAX => {
                let finished = *run;
                *run = Run { byte, length: 1 };
                self.add_run(finished);
            }
            Some(run) => run.length += 1,
        }
    }

    fn add_run(&mut self, run: Run) {
        for _ in 0..run.length {
            self.block_crc = crc::update(self.block_crc, run.byte);
        }
        self.in_use[usize::from(run.byte)] = true;
        let start = self.block_bytes;
        if run.length < RLE_RUN_BYTES {
            self.block[start..start + run.length].fill(run.byte);
            self.block_bytes += run.length;
            return;
        }
        let extra = run.length - RLE_RUN_BYTES;
        self.in_use[extra] = true;
        self.block[start..start + RLE_RUN_BYTES].fill(run.byte);
        self.block[start + RLE_RUN_BYTES] =
            u8::try_from(extra).expect("a run is at most 255 bytes");
        self.block_bytes += RLE_RUN_BYTES + 1;
    }

    fn flush_run(&mut self) {
        if let Some(run) = self.run.take() {
            self.add_run(run);
        }
    }

    fn prepare_block(&mut self) {
        self.block_bytes = 0;
        self.compressed.clear();
        self.compressed_sent = 0;
        self.block_crc = crc::INITIAL;
        self.in_use.fill(false);
        self.block_number += 1;
    }

    fn compress_block(&mut self, last: bool) {
        let mut origin = 0;
        if self.block_bytes > 0 {
            self.block_crc = !self.block_crc;
            self.combined_crc = crc::combine(self.combined_crc, self.block_crc);
            let mut workspace = block_sort::Workspace {
                block: &mut self.block,
                quadrant: &mut self.quadrant,
                order: &mut self.order,
                pairs: &mut self.pairs,
                classes: &mut self.classes,
            };
            origin = block_sort::sort(&mut workspace, self.block_bytes);
        }
        let output = &mut self.compressed;
        if self.block_number == 1 {
            self.writer.reset();
            self.writer.write_bytes(output, &STREAM_MAGIC);
            self.writer.write_bytes(output, &[b'0' + self.level.0]);
        }
        if self.block_bytes > 0 {
            self.writer.write_bytes(output, &BLOCK_MAGIC);
            self.writer
                .write_bytes(output, &self.block_crc.to_be_bytes());
            self.writer.write(output, RANDOMISED_BITS, 0);
            let origin = u32::try_from(origin).expect("a block position fits in u32");
            self.writer.write(output, ORIGIN_BITS, origin);
            let block = encode::Block {
                bytes: &self.block[..self.block_bytes],
                order: &self.order,
                in_use: &self.in_use,
            };
            self.tables.encode(&block, &mut self.writer, output);
        }
        if last {
            self.writer.write_bytes(output, &END_MAGIC);
            self.writer
                .write_bytes(output, &self.combined_crc.to_be_bytes());
            self.writer.finish(output);
        }
    }
}

pub struct Decompressor {
    decoder: Decoder,
    total_in: u64,
    total_out: u64,
}

impl Default for Decompressor {
    fn default() -> Self {
        Self::new()
    }
}

impl Decompressor {
    #[must_use]
    pub fn new() -> Self {
        Self {
            decoder: Decoder::new(),
            total_in: 0,
            total_out: 0,
        }
    }

    #[must_use]
    pub const fn total_in(&self) -> u64 {
        self.total_in
    }

    #[must_use]
    pub const fn total_out(&self) -> u64 {
        self.total_out
    }

    pub fn decompress(
        &mut self,
        input: &[u8],
        output: &mut [u8],
    ) -> Result<DecompressStatus, Bzip2Error> {
        let mut input = Input::new(input);
        let mut written = 0;
        let result = self.run(&mut input, output, &mut written);
        self.total_in += u64::try_from(input.position).expect("a slice length fits in u64");
        self.total_out += u64::try_from(written).expect("a slice length fits in u64");
        if let Err(error) = result
            && !matches!(self.decoder.phase, Phase::Finished)
        {
            self.decoder.phase = Phase::Failed(error);
        }
        result
    }

    fn run(
        &mut self,
        input: &mut Input<'_>,
        output: &mut [u8],
        written: &mut usize,
    ) -> Result<DecompressStatus, Bzip2Error> {
        let decoder = &mut self.decoder;
        loop {
            match decoder.phase {
                Phase::Finished => return Err(Bzip2Error::Sequence),
                Phase::Failed(error) => return Err(error),
                Phase::Output => {
                    decoder.write_output(output, written)?;
                    if !decoder.block_done() {
                        return Ok(DecompressStatus::InProgress);
                    }
                    if !decoder.block_crc_matches() {
                        return Err(Bzip2Error::Data);
                    }
                    decoder.calculated_combined_crc =
                        crc::combine(decoder.calculated_combined_crc, decoder.block_crc());
                    decoder.phase = Phase::BlockHeader;
                }
                _ => match decoder.decode(input)? {
                    Step::StreamEnd => {
                        if decoder.calculated_combined_crc != decoder.stored_combined_crc {
                            return Err(Bzip2Error::Data);
                        }
                        return Ok(DecompressStatus::StreamEnd);
                    }
                    Step::Suspend => return Ok(DecompressStatus::InProgress),
                    Step::BlockReady => {}
                    Step::Continue => unreachable!("decode returns only a stopping step"),
                },
            }
        }
    }
}
