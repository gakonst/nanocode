mod bits;
mod fse;
mod huffman;
mod literals;
mod matcher;
mod sequences;

use crate::{
    block::{BLOCK_HEADER_SIZE, BLOCK_SIZE_MAX, BlockType},
    error::{Error, Result},
    frame::ZSTD_MAGIC_NUMBER,
};
use huffman::Huffman;
use matcher::{Emitter, MATCH_MIN, Matcher, Strategy};
use sequences::SequenceCoder;

const LEVEL_DEFAULT: i32 = 3;
const WINDOW_LOG_MIN: u32 = 10;
const HASH_LOG_MIN: u32 = 6;
const SIZE_CLASSES: [usize; 3] = [256 * 1024, 128 * 1024, 16 * 1024];
const SINGLE_BYTE_CONTENT_SIZE_BELOW: u64 = 256;
const TWO_BYTE_CONTENT_SIZE_OFFSET: u64 = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Parameters {
    window_log: u32,
    chain_log: u32,
    hash_log: u32,
    match_min: u32,
    strategy: Strategy,
}

const fn parameters(
    window_log: u32,
    chain_log: u32,
    hash_log: u32,
    match_min: u32,
    strategy: Strategy,
) -> Parameters {
    Parameters {
        window_log,
        chain_log,
        hash_log,
        match_min,
        strategy,
    }
}

const LEVELS: [[Parameters; 3]; 4] = [
    [
        parameters(19, 13, 14, 7, Strategy::Fast),
        parameters(20, 15, 16, 6, Strategy::Fast),
        parameters(21, 16, 17, 5, Strategy::DoubleFast),
    ],
    [
        parameters(18, 13, 14, 6, Strategy::Fast),
        parameters(18, 14, 14, 5, Strategy::DoubleFast),
        parameters(18, 16, 16, 4, Strategy::DoubleFast),
    ],
    [
        parameters(17, 12, 13, 6, Strategy::Fast),
        parameters(17, 13, 15, 5, Strategy::Fast),
        parameters(17, 15, 16, 5, Strategy::DoubleFast),
    ],
    [
        parameters(14, 14, 15, 5, Strategy::Fast),
        parameters(14, 14, 15, 4, Strategy::Fast),
        parameters(14, 14, 15, 4, Strategy::DoubleFast),
    ],
];

pub const LEVELS_SUPPORTED: std::ops::RangeInclusive<i32> = 1..=3;

fn choose(level: i32, content: usize, dictionary: usize) -> Result<Parameters> {
    let level = if level == 0 { LEVEL_DEFAULT } else { level };
    if !LEVELS_SUPPORTED.contains(&level) {
        return Err(Error::InvalidParameter(
            "the encoder implements compression levels 1 to 3 and 0 for the default",
        ));
    }
    let total = content + dictionary;
    let class = SIZE_CLASSES.iter().filter(|&&size| total <= size).count();
    let mut chosen = LEVELS[class][(level - 1) as usize];
    let total_log = if total < 1 << HASH_LOG_MIN {
        HASH_LOG_MIN
    } else {
        fse::highbit((total - 1) as u32) + 1
    };
    chosen.window_log = chosen.window_log.min(total_log).max(WINDOW_LOG_MIN);
    chosen.hash_log = chosen.hash_log.min(total_log + 1);
    chosen.chain_log = chosen.chain_log.min(total_log);
    Ok(chosen)
}

pub fn compress(content: &[u8], level: i32) -> Result<Vec<u8>> {
    compress_with_dictionary(content, &[], level)
}

pub fn compress_with_dictionary(content: &[u8], dictionary: &[u8], level: i32) -> Result<Vec<u8>> {
    let chosen = choose(level, content.len(), dictionary.len())?;
    let window = 1_usize << chosen.window_log;
    let single_segment = content.len() <= window;
    let mut out = Vec::with_capacity(content.len() / 2 + BLOCK_HEADER_SIZE + FRAME_HEADER_MAX);
    write_frame_header(
        &mut out,
        content.len() as u64,
        chosen.window_log,
        single_segment,
    );
    let joined;
    let data: &[u8] = if dictionary.is_empty() {
        content
    } else {
        joined = [dictionary, content].concat();
        &joined
    };
    let distance_max = if single_segment { data.len() } else { window };
    let mut encoder = Encoder {
        matcher: Matcher::new(
            chosen.strategy,
            chosen.hash_log,
            chosen.chain_log,
            chosen.match_min,
            distance_max,
        ),
        emitter: Emitter::new(BLOCK_SIZE_MAX),
        sequences: SequenceCoder::default(),
        huffman: Huffman::default(),
        block: Vec::with_capacity(BLOCK_SIZE_MAX),
    };
    encoder.emitter.reset();
    encoder.sequences.reset();
    encoder.matcher.prime(data, dictionary.len());
    let frame_start = out.len();
    encoder.blocks(&mut out, data, dictionary.len());
    let blocks = content.len().div_ceil(BLOCK_SIZE_MAX).max(1);
    assert!(
        out.len() - frame_start <= content.len() + blocks * BLOCK_HEADER_SIZE,
        "a frame is never larger than its raw blocks"
    );
    Ok(out)
}

const FRAME_HEADER_MAX: usize = 4 + 1 + 1 + 8;

fn write_frame_header(out: &mut Vec<u8>, size: u64, window_log: u32, single_segment: bool) {
    out.extend_from_slice(&ZSTD_MAGIC_NUMBER.to_le_bytes());
    let size_code: u8 = if single_segment && size < SINGLE_BYTE_CONTENT_SIZE_BELOW {
        0
    } else if size >= TWO_BYTE_CONTENT_SIZE_OFFSET
        && size - TWO_BYTE_CONTENT_SIZE_OFFSET <= u64::from(u16::MAX)
    {
        1
    } else if u32::try_from(size).is_ok() {
        2
    } else {
        3
    };
    assert!(
        single_segment || size_code != 0,
        "a frame that is not a single segment names its size with two or more bytes"
    );
    out.push((size_code << 6) | (u8::from(single_segment) << 5));
    if !single_segment {
        out.push(((window_log - WINDOW_LOG_MIN) << 3) as u8);
    }
    match size_code {
        0 => out.push(size as u8),
        1 => out.extend_from_slice(&((size - TWO_BYTE_CONTENT_SIZE_OFFSET) as u16).to_le_bytes()),
        2 => out.extend_from_slice(&(size as u32).to_le_bytes()),
        _ => out.extend_from_slice(&size.to_le_bytes()),
    }
}

struct Encoder {
    matcher: Matcher,
    emitter: Emitter,
    sequences: SequenceCoder,
    huffman: Huffman,
    block: Vec<u8>,
}

impl Encoder {
    fn blocks(&mut self, out: &mut Vec<u8>, data: &[u8], start: usize) {
        if start == data.len() {
            return write_block_header(out, true, BlockType::Raw, 0);
        }
        let mut position = start;
        while position < data.len() {
            let end = (position + BLOCK_SIZE_MAX).min(data.len());
            self.block(out, data, position, end, end == data.len());
            position = end;
        }
    }

    fn block(&mut self, out: &mut Vec<u8>, data: &[u8], start: usize, end: usize, last: bool) {
        let raw = &data[start..end];
        if raw.len() > 1 && raw.iter().all(|&byte| byte == raw[0]) {
            write_block_header(out, last, BlockType::Rle, raw.len());
            out.push(raw[0]);
            return;
        }
        let repeats = self.emitter.repeats;
        self.emitter.literals.clear();
        self.emitter.sequences.clear();
        self.matcher.block(data, start, end, &mut self.emitter);
        let covered: usize = self.emitter.literals.len()
            + self
                .emitter
                .sequences
                .iter()
                .map(|sequence| sequence.match_length as usize)
                .sum::<usize>();
        assert_eq!(
            covered,
            raw.len(),
            "a block's literals and matches cover it exactly"
        );
        assert!(self.emitter.sequences.len() <= BLOCK_SIZE_MAX / MATCH_MIN);
        self.block.clear();
        literals::write(&mut self.block, &self.emitter.literals, &mut self.huffman);
        self.sequences
            .write(&mut self.block, &self.emitter.sequences);
        if self.block.len() < raw.len() {
            write_block_header(out, last, BlockType::Compressed, self.block.len());
            out.extend_from_slice(&self.block);
            self.sequences.commit();
        } else {
            write_block_header(out, last, BlockType::Raw, raw.len());
            out.extend_from_slice(raw);
            self.emitter.repeats = repeats;
        }
    }
}

fn write_block_header(out: &mut Vec<u8>, last: bool, kind: BlockType, size: usize) {
    assert!(
        size <= BLOCK_SIZE_MAX,
        "a block holds at most {BLOCK_SIZE_MAX} bytes"
    );
    let kind = match kind {
        BlockType::Raw => 0,
        BlockType::Rle => 1,
        BlockType::Compressed => 2,
    };
    let header = u32::from(last) | (kind << 1) | ((size as u32) << 3);
    out.extend_from_slice(&header.to_le_bytes()[..BLOCK_HEADER_SIZE]);
}
