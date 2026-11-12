#![allow(clippy::needless_range_loop, clippy::too_many_arguments)]

mod bits;
mod fast;
mod literal;
mod matching;
mod medium;
mod quick;
mod slow;
mod state;
mod stored;
mod trees;

use state::{BlockState, FlushRank, Io, State, Status, Wrapper};
use trees::stored_block;

const GZIP_MAGIC: [u8; 2] = [0x1F, 0x8B];
const DEFLATE_METHOD: u8 = 8;
const GZIP_HEADER_FLAGS: u8 = 0;
const GZIP_MODIFICATION_TIME: [u8; 4] = [0; 4];
const GZIP_EXTRA_FLAGS_SLOWEST: u8 = 2;
const GZIP_EXTRA_FLAGS_FASTEST: u8 = 4;
const GZIP_EXTRA_FLAGS_DEFAULT: u8 = 0;
const GZIP_OS_WINDOWS: u8 = 10;
const GZIP_OS_MACOS: u8 = 19;
const GZIP_OS_UNIX: u8 = 3;
const GZIP_OS: u8 = if cfg!(windows) {
    GZIP_OS_WINDOWS
} else if cfg!(target_os = "macos") {
    GZIP_OS_MACOS
} else {
    GZIP_OS_UNIX
};
const GZIP_SIZE_MODULUS: u64 = 1 << u32::BITS;
const LEVEL_SLOWEST: u8 = 9;
const STRATEGY_DEFAULT: i32 = 0;
const STRATEGY_FILTERED: i32 = 1;
const STRATEGY_HUFFMAN_ONLY: i32 = 2;
const STRATEGY_RLE: i32 = 3;
const STRATEGY_FIXED: i32 = 4;
const LEVEL_FAST_BELOW: u8 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeflateFlush {
    NoFlush,
    PartialFlush,
    SyncFlush,
    FullFlush,
    Finish,
    Block,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Strategy {
    Default,
    Filtered,
    HuffmanOnly,
    Rle,
    Fixed,
}

impl TryFrom<i32> for Strategy {
    type Error = DeflateError;

    fn try_from(value: i32) -> Result<Self, Self::Error> {
        match value {
            STRATEGY_DEFAULT => Ok(Self::Default),
            STRATEGY_FILTERED => Ok(Self::Filtered),
            STRATEGY_HUFFMAN_ONLY => Ok(Self::HuffmanOnly),
            STRATEGY_RLE => Ok(Self::Rle),
            STRATEGY_FIXED => Ok(Self::Fixed),
            _ => Err(DeflateError::Stream),
        }
    }
}

impl Strategy {
    const fn is_fast(self) -> bool {
        matches!(self, Self::HuffmanOnly | Self::Rle | Self::Fixed)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeflateConfig {
    pub level: i32,
    pub window_bits: i32,
    pub mem_level: i32,
    pub strategy: Strategy,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeflateStatus {
    Ok,
    StreamEnd,
    BufError,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeflateError {
    Stream,
    Data,
    Memory,
}

impl DeflateError {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Stream => "stream error",
            Self::Data => "data error",
            Self::Memory => "insufficient memory",
        }
    }
}

#[derive(Clone)]
pub struct Deflate {
    state: State,
}

impl Deflate {
    pub fn new(config: DeflateConfig) -> Result<Self, DeflateError> {
        Ok(Self {
            state: State::new(config)?,
        })
    }

    pub fn deflate(
        &mut self,
        input: &[u8],
        output: &mut [u8],
        flush: DeflateFlush,
    ) -> Result<DeflateStatus, DeflateError> {
        let mut io = Io::new(input, output);
        deflate(&mut self.state, &mut io, flush)
    }

    pub fn set_dictionary(&mut self, dictionary: &[u8]) -> Result<(), DeflateError> {
        self.state.set_dictionary(dictionary)
    }

    pub const fn total_in(&self) -> u64 {
        self.state.total_in
    }

    pub const fn total_out(&self) -> u64 {
        self.state.total_out
    }
}

fn deflate(
    state: &mut State,
    io: &mut Io,
    flush: DeflateFlush,
) -> Result<DeflateStatus, DeflateError> {
    if state.status == Status::Finish && flush != DeflateFlush::Finish {
        return Err(DeflateError::Stream);
    }
    if io.output_available() == 0 {
        return Ok(DeflateStatus::BufError);
    }
    let previous_flush = state.previous_flush;
    state.previous_flush = FlushRank::of(flush);
    if state.bits.pending.length() != 0 {
        state.flush_pending(io);
        if io.output_available() == 0 {
            state.previous_flush = FlushRank::Incomplete;
            return Ok(DeflateStatus::Ok);
        }
    } else if io.input_available() == 0
        && FlushRank::of(flush) <= previous_flush
        && flush != DeflateFlush::Finish
    {
        return Ok(DeflateStatus::BufError);
    }
    if state.status == Status::Finish && io.input_available() != 0 {
        return Ok(DeflateStatus::BufError);
    }
    if let Some(status) = write_header(state, io) {
        return Ok(status);
    }
    if let Some(status) = compress(state, io, flush) {
        return Ok(status);
    }
    if flush != DeflateFlush::Finish {
        return Ok(DeflateStatus::Ok);
    }
    Ok(write_trailer(state, io))
}

fn write_header(state: &mut State, io: &mut Io) -> Option<DeflateStatus> {
    if state.status == Status::Init && state.wrapper == Wrapper::Raw {
        state.status = Status::Busy;
    }
    match state.status {
        Status::Init => {
            let header = state.zlib_header();
            state.bits.pending.extend(&header.to_be_bytes());
            if state.string_start != 0 {
                let checksum = state.checksum;
                state.bits.pending.extend(&checksum.to_be_bytes());
            }
            state.checksum = state::ADLER32_INITIAL;
        }
        Status::Gzip => {
            state.checksum = state::CRC32_INITIAL;
            let extra_flags = if state.level == LEVEL_SLOWEST {
                GZIP_EXTRA_FLAGS_SLOWEST
            } else if state.strategy.is_fast() || state.level < LEVEL_FAST_BELOW {
                GZIP_EXTRA_FLAGS_FASTEST
            } else {
                GZIP_EXTRA_FLAGS_DEFAULT
            };
            state.bits.pending.extend(&GZIP_MAGIC);
            state
                .bits
                .pending
                .extend(&[DEFLATE_METHOD, GZIP_HEADER_FLAGS]);
            state.bits.pending.extend(&GZIP_MODIFICATION_TIME);
            state.bits.pending.extend(&[extra_flags, GZIP_OS]);
        }
        Status::Busy | Status::Finish => return None,
    }
    state.status = Status::Busy;
    state.flush_pending(io);
    if state.bits.pending.length() != 0 {
        state.previous_flush = FlushRank::Incomplete;
        return Some(DeflateStatus::Ok);
    }
    None
}

fn compress(state: &mut State, io: &mut Io, flush: DeflateFlush) -> Option<DeflateStatus> {
    let has_work = io.input_available() != 0
        || state.lookahead != 0
        || (flush != DeflateFlush::NoFlush && state.status != Status::Finish);
    if !has_work {
        return None;
    }
    let block_state = run(state, io, flush);
    if matches!(
        block_state,
        BlockState::FinishStarted | BlockState::FinishDone
    ) {
        state.status = Status::Finish;
    }
    match block_state {
        BlockState::NeedMore | BlockState::FinishStarted => {
            if io.output_available() == 0 {
                state.previous_flush = FlushRank::Incomplete;
            }
            Some(DeflateStatus::Ok)
        }
        BlockState::BlockDone => {
            finish_flush(state, flush);
            state.flush_pending(io);
            if io.output_available() == 0 {
                state.previous_flush = FlushRank::Incomplete;
                return Some(DeflateStatus::Ok);
            }
            None
        }
        BlockState::FinishDone => None,
    }
}

fn finish_flush(state: &mut State, flush: DeflateFlush) {
    match flush {
        DeflateFlush::PartialFlush => state.bits.align(),
        DeflateFlush::SyncFlush => stored_block(&mut state.bits, &[], false),
        DeflateFlush::FullFlush => {
            stored_block(&mut state.bits, &[], false);
            state.head.fill(0);
            if state.lookahead == 0 {
                state.string_start = 0;
                state.block_start = 0;
                state.insert = 0;
            }
        }
        DeflateFlush::Block => {}
        DeflateFlush::NoFlush | DeflateFlush::Finish => {
            unreachable!("a block is done only for a flush that is not a finish")
        }
    }
}

fn run(state: &mut State, io: &mut Io, flush: DeflateFlush) -> BlockState {
    if state.level == 0 {
        return stored::deflate_stored(state, io, flush);
    }
    match state.strategy {
        Strategy::HuffmanOnly => literal::deflate_huffman(state, io, flush),
        Strategy::Rle => literal::deflate_rle(state, io, flush),
        Strategy::Default | Strategy::Filtered | Strategy::Fixed => {
            match state::LEVEL_CONFIGS[usize::from(state.level)].algorithm {
                state::Algorithm::Stored => stored::deflate_stored(state, io, flush),
                state::Algorithm::Quick => quick::deflate_quick(state, io, flush),
                state::Algorithm::Fast => fast::deflate_fast(state, io, flush),
                state::Algorithm::Medium => medium::deflate_medium(state, io, flush),
                state::Algorithm::Slow => slow::deflate_slow(state, io, flush),
            }
        }
    }
}

fn write_trailer(state: &mut State, io: &mut Io) -> DeflateStatus {
    if !state.trailer_written {
        match state.wrapper {
            Wrapper::Gzip => {
                let checksum = state.checksum;
                state.bits.pending.extend(&checksum.to_le_bytes());
                let size = u32::try_from(state.total_in % GZIP_SIZE_MODULUS)
                    .expect("a value reduced modulo 2^32 fits in 32 bits");
                state.bits.pending.extend(&size.to_le_bytes());
            }
            Wrapper::Zlib => {
                let checksum = state.checksum;
                state.bits.pending.extend(&checksum.to_be_bytes());
            }
            Wrapper::Raw => {}
        }
    }
    state.flush_pending(io);
    state.trailer_written = true;
    if state.bits.pending.length() == 0 {
        assert_eq!(
            state.bits.bits_valid, 0,
            "the bit buffer is flushed at the end"
        );
        return DeflateStatus::StreamEnd;
    }
    DeflateStatus::Ok
}
