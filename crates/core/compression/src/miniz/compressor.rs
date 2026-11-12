use alloc::vec::Vec;

use super::Format;
use super::bits::{BitWriter, Mark};
use super::block::{Block, Code};
use super::dictionary::Dictionary;
use super::huffman::{Huffman, Packer};
use super::tables::{
    BLOCK_TYPE_BITS, BYTE_BITS, CODE_BITS_MAX, CODE_LENGTH_BITS_MAX, CODE_LENGTH_CODES_MIN,
    CODE_LENGTH_ORDER, CODE_LENGTH_SYMBOLS, DICTIONARY_BYTES, DICTIONARY_MASK, DISTANCE_BASE,
    DISTANCE_CODES_MAX, DISTANCE_EXTRA_BITS, DISTANCE_SYMBOLS, DYNAMIC_BLOCK_BYTES_MIN,
    DYNAMIC_BLOCK_TYPE, END_OF_BLOCK, FAR_DISTANCE_MIN, FAST_HASH_MASK, FAST_HASH_SHIFT_BITS,
    FAST_LOOKAHEAD_BYTES, FAST_LOOKAHEAD_BYTES_MIN, FAT_BLOCK_BYTES, LAZY_LENGTH_MAX, LENGTH_BASE,
    LENGTH_EXTRA_BITS, LENGTH_SYMBOL, LITERAL_CODES_MAX, LITERAL_CODES_MIN, LITERAL_SYMBOLS,
    MATCH_LENGTH_MAX, MATCH_LENGTH_MIN, POSITION_MASK, REPEAT_EXTRA_BITS, REPEAT_PREVIOUS,
    STAGING_BYTES, STAGING_SLACK_BYTES, STATIC_BLOCK_TYPE, STATIC_DISTANCE_LENGTH,
    STATIC_LITERAL_LENGTHS, STORED_BLOCK_BYTES_MIN, STORED_BLOCK_TYPE, STORED_LENGTH_BITS,
    STORED_LENGTH_MASK, TRIGRAM_MASK, distance_code,
};
use crate::checksum::adler32;

const ADLER32_INITIAL: u32 = 1;
const SYNC_MARKER_TYPE_BITS: u32 = 3;
const SYNC_MARKER_LENGTH: u32 = 0;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Parsing {
    Stored,
    Fast,
    Greedy,
    Lazy,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Flush {
    None,
    Sync,
    Finish,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Run {
    Complete,
    Interrupted,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    Open,
    Finished,
    Done,
}

#[derive(Clone, Copy)]
struct Pending {
    literal: u8,
    distance: u32,
    length: u32,
}

#[derive(Clone, Copy)]
pub(super) struct Progress {
    pub(super) consumed: usize,
    pub(super) written: usize,
    pub(super) stream_end: bool,
}

pub(super) struct Sink<'a> {
    pub(super) bytes: &'a mut Vec<u8>,
    pub(super) space: usize,
}

impl Sink<'_> {
    fn take(&mut self, bytes: &[u8]) -> usize {
        let count = bytes.len().min(self.space);
        self.bytes.extend_from_slice(&bytes[..count]);
        self.space -= count;
        count
    }
}

pub(super) struct Settings {
    pub(super) format: Format,
    pub(super) parsing: Parsing,
    pub(super) probes: [u32; 2],
    pub(super) zlib_header: [u8; 2],
}

pub(super) struct Compressor {
    settings: Settings,
    dictionary: Dictionary,
    block: Block,
    writer: BitWriter,
    literals: Huffman<LITERAL_SYMBOLS>,
    distances: Huffman<DISTANCE_SYMBOLS>,
    lengths: Huffman<CODE_LENGTH_SYMBOLS>,
    pending: Option<Pending>,
    blocks: u32,
    adler: u32,
    flush: Flush,
    staged: usize,
    stage: Stage,
}

impl Compressor {
    pub(super) fn new(settings: Settings) -> Self {
        Self {
            settings,
            dictionary: Dictionary::new(),
            block: Block::new(),
            writer: BitWriter::new(),
            literals: Huffman::new(),
            distances: Huffman::new(),
            lengths: Huffman::new(),
            pending: None,
            blocks: 0,
            adler: ADLER32_INITIAL,
            flush: Flush::None,
            staged: 0,
            stage: Stage::Open,
        }
    }

    pub(super) fn deflate(&mut self, input: &[u8], sink: &mut Sink, flush: Flush) -> Progress {
        let mut progress = Progress {
            consumed: 0,
            written: 0,
            stream_end: false,
        };
        if sink.space == 0 {
            return progress;
        }
        if self.stage == Stage::Done {
            progress.stream_end = flush == Flush::Finish;
            return progress;
        }
        loop {
            let space = sink.space;
            let consumed = self.compress(&input[progress.consumed..], sink, flush);
            progress.consumed += consumed;
            progress.written += space - sink.space;
            if self.stage == Stage::Done {
                progress.stream_end = true;
                return progress;
            }
            if sink.space == 0 {
                return progress;
            }
            if progress.consumed == input.len() && flush != Flush::Finish {
                return progress;
            }
        }
    }

    const fn remaining(&self) -> usize {
        self.writer.bytes.len() - self.staged
    }

    fn compress(&mut self, input: &[u8], sink: &mut Sink, flush: Flush) -> usize {
        assert!(self.stage != Stage::Done);
        assert!(self.flush != Flush::Finish || flush == Flush::Finish);
        self.flush = flush;
        if self.remaining() != 0 || self.stage == Stage::Finished {
            self.drain(sink);
            return 0;
        }
        let mut source = 0;
        let run = match self.settings.parsing {
            Parsing::Stored => self.run_stored(input, &mut source, sink),
            Parsing::Fast => self.run_fast(input, &mut source, sink),
            Parsing::Greedy | Parsing::Lazy => self.run_normal(input, &mut source, sink),
        };
        if self.settings.format == Format::Zlib {
            self.adler = adler32(self.adler, &input[..source]);
        }
        let remaining = source < input.len() || self.remaining() != 0;
        assert!(run == Run::Complete || remaining);
        if flush != Flush::None && self.dictionary.lookahead_size == 0 && !remaining {
            self.flush_block(flush, sink);
            if flush == Flush::Finish {
                self.stage = Stage::Finished;
            }
        }
        self.drain(sink);
        source
    }

    fn drain(&mut self, sink: &mut Sink) {
        let copied = sink.take(&self.writer.bytes[self.staged..]);
        self.staged += copied;
        if self.stage == Stage::Finished && self.remaining() == 0 {
            self.stage = Stage::Done;
        }
    }

    fn load_limit(&self, input: &[u8], source: usize, lookahead_max: usize) -> usize {
        (input.len() - source).min(lookahead_max - self.dictionary.lookahead_size)
    }

    fn shrink_window(&mut self) {
        let dictionary = &mut self.dictionary;
        dictionary.size = dictionary
            .size
            .min(DICTIONARY_BYTES - dictionary.lookahead_size);
    }

    fn waits_for_input(&self, lookahead_max: usize) -> bool {
        self.flush == Flush::None && self.dictionary.lookahead_size < lookahead_max
    }

    fn has_work(&self, input: &[u8], source: usize) -> bool {
        source < input.len() || (self.flush != Flush::None && self.dictionary.lookahead_size != 0)
    }

    fn run_stored(&mut self, input: &[u8], source: &mut usize, sink: &mut Sink) -> Run {
        while self.has_work(input, *source) {
            let bytes = self.load_limit(input, *source, MATCH_LENGTH_MAX);
            self.dictionary
                .load_hashed(&input[*source..*source + bytes]);
            *source += bytes;
            self.shrink_window();
            if self.waits_for_input(MATCH_LENGTH_MAX) {
                break;
            }
            self.block.total_bytes += 1;
            self.advance(1);
            if self.block.total_bytes > FAT_BLOCK_BYTES && self.flush_block(Flush::None, sink) {
                return Run::Interrupted;
            }
        }
        Run::Complete
    }

    fn run_normal(&mut self, input: &[u8], source: &mut usize, sink: &mut Sink) -> Run {
        while self.has_work(input, *source) {
            let bytes = self.load_limit(input, *source, MATCH_LENGTH_MAX);
            self.dictionary
                .load_hashed(&input[*source..*source + bytes]);
            *source += bytes;
            self.shrink_window();
            if self.waits_for_input(MATCH_LENGTH_MAX) {
                break;
            }
            self.step_normal();
            if (self.block.tight() || self.block.fat()) && self.flush_block(Flush::None, sink) {
                return Run::Interrupted;
            }
        }
        Run::Complete
    }

    fn step_normal(&mut self) {
        let dictionary = &self.dictionary;
        let position = dictionary.lookahead_position & DICTIONARY_MASK;
        let length = self
            .pending
            .map_or(MATCH_LENGTH_MIN as u32 - 1, |pending| pending.length);
        let (mut distance, mut length) = dictionary.find_match(
            self.settings.probes,
            dictionary.lookahead_size as u32,
            length,
        );
        if (length == MATCH_LENGTH_MIN as u32 && distance >= FAR_DISTANCE_MIN)
            || position == distance as usize
        {
            distance = 0;
            length = 0;
        }
        let byte = dictionary.bytes[position];
        let mut advance = 1;
        match self.pending {
            Some(pending) if length > pending.length => {
                self.block.literal(pending.literal);
                if length >= LAZY_LENGTH_MAX {
                    self.block.matched(length, distance);
                    self.pending = None;
                    advance = length as usize;
                } else {
                    self.pending = Some(Pending {
                        literal: byte,
                        distance,
                        length,
                    });
                }
            }
            Some(pending) => {
                self.block.matched(pending.length, pending.distance);
                advance = pending.length as usize - 1;
                self.pending = None;
            }
            None if distance == 0 => self.block.literal(byte),
            None if self.settings.parsing == Parsing::Greedy || length >= LAZY_LENGTH_MAX => {
                self.block.matched(length, distance);
                advance = length as usize;
            }
            None => {
                self.pending = Some(Pending {
                    literal: byte,
                    distance,
                    length,
                })
            }
        }
        self.advance(advance);
    }

    fn advance(&mut self, bytes: usize) {
        let dictionary = &mut self.dictionary;
        assert!(dictionary.lookahead_size >= bytes);
        dictionary.lookahead_position += bytes;
        dictionary.lookahead_size -= bytes;
        dictionary.size = (dictionary.size + bytes).min(DICTIONARY_BYTES);
    }

    fn run_fast(&mut self, input: &[u8], source: &mut usize, sink: &mut Sink) -> Run {
        while self.has_work(input, *source) {
            let bytes = self.load_limit(input, *source, FAST_LOOKAHEAD_BYTES);
            self.dictionary.load_plain(&input[*source..*source + bytes]);
            *source += bytes;
            self.shrink_window();
            if self.waits_for_input(FAST_LOOKAHEAD_BYTES) {
                break;
            }
            while self.dictionary.lookahead_size >= FAST_LOOKAHEAD_BYTES_MIN {
                self.step_fast();
                if self.block.tight() && self.flush_block(Flush::None, sink) {
                    return Run::Interrupted;
                }
            }
            while self.dictionary.lookahead_size != 0 {
                let position = self.dictionary.lookahead_position & DICTIONARY_MASK;
                self.block.literal(self.dictionary.bytes[position]);
                self.advance(1);
                if self.block.tight() && self.flush_block(Flush::None, sink) {
                    return Run::Interrupted;
                }
            }
        }
        Run::Complete
    }

    fn step_fast(&mut self) {
        let dictionary = &mut self.dictionary;
        let position = dictionary.lookahead_position & DICTIONARY_MASK;
        let first = dictionary.read_u32(position) & TRIGRAM_MASK;
        let hash = ((first ^ (first >> FAST_HASH_SHIFT_BITS)) & FAST_HASH_MASK) as usize;
        let candidate = usize::from(dictionary.hash[hash]);
        dictionary.hash[hash] = dictionary.lookahead_position as u16;
        let distance = dictionary.lookahead_position.wrapping_sub(candidate) & POSITION_MASK;
        let mut length = 1;
        if distance != 0 && distance <= dictionary.size {
            let candidate = candidate & DICTIONARY_MASK;
            if dictionary.read_u32(candidate) & TRIGRAM_MASK == first {
                let matched = dictionary
                    .compare(
                        position + MATCH_LENGTH_MIN,
                        candidate + MATCH_LENGTH_MIN,
                        position,
                    )
                    .unwrap_or(MATCH_LENGTH_MAX);
                if matched > MATCH_LENGTH_MIN
                    || (matched == MATCH_LENGTH_MIN && (distance as u32) < FAR_DISTANCE_MIN)
                {
                    length = matched.min(dictionary.lookahead_size);
                }
            }
        }
        if length == 1 {
            self.block.literal(first as u8);
        } else {
            self.block.matched(length as u32, distance as u32);
        }
        self.advance(length);
    }

    fn flush_block(&mut self, flush: Flush, sink: &mut Sink) -> bool {
        assert!(self.remaining() == 0);
        self.writer.bytes.clear();
        self.staged = 0;
        if self.blocks == 0 && self.settings.format == Format::Zlib {
            let [method, flags] = self.settings.zlib_header;
            self.writer.put(u32::from(method), BYTE_BITS);
            self.writer.put(u32::from(flags), BYTE_BITS);
        }
        self.writer.put(u32::from(flush == Flush::Finish), 1);
        let mark = self.writer.mark();
        let total = self.block.total_bytes;
        let windowed = self.dictionary.lookahead_position - self.dictionary.block_start
            <= self.dictionary.size;
        if self.settings.parsing == Parsing::Stored {
            assert!(windowed);
            self.stored_block(mark, total);
        } else {
            self.coded_block(mark, total, windowed);
        }
        self.end_block(flush);
        self.block.reset();
        self.dictionary.block_start += total as usize;
        self.blocks += 1;
        self.writer.flush_bytes();
        assert!(self.writer.bytes.len() <= STAGING_BYTES - STAGING_SLACK_BYTES);
        self.staged = sink.take(&self.writer.bytes);
        self.remaining() != 0
    }

    fn coded_block(&mut self, mark: Mark, total: u32, windowed: bool) {
        let marked = self.writer.len();
        if total < DYNAMIC_BLOCK_BYTES_MIN {
            self.static_header();
        } else {
            self.dynamic_header();
        }
        self.write_codes();
        if total > STORED_BLOCK_BYTES_MIN
            && self.writer.len() - marked + 1 >= total as usize
            && windowed
        {
            self.stored_block(mark, total);
        }
    }

    fn end_block(&mut self, flush: Flush) {
        match flush {
            Flush::None => {}
            Flush::Finish => {
                self.writer.align();
                if self.settings.format == Format::Zlib {
                    for byte in self.adler.to_be_bytes() {
                        self.writer.put(u32::from(byte), BYTE_BITS);
                    }
                }
            }
            Flush::Sync => {
                self.writer.put(STORED_BLOCK_TYPE, SYNC_MARKER_TYPE_BITS);
                self.writer.align();
                self.writer.put(SYNC_MARKER_LENGTH, STORED_LENGTH_BITS);
                self.writer
                    .put(!SYNC_MARKER_LENGTH & STORED_LENGTH_MASK, STORED_LENGTH_BITS);
            }
        }
    }

    fn stored_block(&mut self, mark: Mark, total: u32) {
        let writer = &mut self.writer;
        writer.rewind(mark);
        writer.put(STORED_BLOCK_TYPE, BLOCK_TYPE_BITS);
        writer.align();
        writer.put(total & STORED_LENGTH_MASK, STORED_LENGTH_BITS);
        writer.put(!total & STORED_LENGTH_MASK, STORED_LENGTH_BITS);
        assert!(writer.is_aligned());
        let bytes = &self.dictionary.bytes;
        let start = self.dictionary.block_start & DICTIONARY_MASK;
        let end = (self.dictionary.block_start + total as usize) & DICTIONARY_MASK;
        if start < end {
            writer.bytes.extend_from_slice(&bytes[start..end]);
        } else if total > 0 {
            writer
                .bytes
                .extend_from_slice(&bytes[start..DICTIONARY_BYTES]);
            writer.bytes.extend_from_slice(&bytes[..end]);
        }
    }

    fn static_header(&mut self) {
        let mut from = 0;
        for (to, length) in STATIC_LITERAL_LENGTHS {
            self.literals.sizes[from..to].fill(length);
            from = to;
        }
        self.literals.assign_codes(CODE_BITS_MAX);
        self.distances.sizes = [STATIC_DISTANCE_LENGTH; DISTANCE_SYMBOLS];
        self.distances.assign_codes(CODE_BITS_MAX);
        self.writer.put(STATIC_BLOCK_TYPE, BLOCK_TYPE_BITS);
    }

    fn dynamic_header(&mut self) {
        self.block.literal_counts[END_OF_BLOCK] = 1;
        self.literals
            .build_from_counts(&self.block.literal_counts, CODE_BITS_MAX);
        self.distances
            .build_from_counts(&self.block.distance_counts, CODE_BITS_MAX);
        let unused = |sizes: &[u8]| sizes.iter().rev().take_while(|&&size| size == 0).count();
        let literal_codes =
            LITERAL_CODES_MAX - unused(&self.literals.sizes[LITERAL_CODES_MIN..LITERAL_CODES_MAX]);
        let distance_codes =
            DISTANCE_CODES_MAX - unused(&self.distances.sizes[1..DISTANCE_CODES_MAX]);
        let mut lengths = [0u8; LITERAL_CODES_MAX + DISTANCE_CODES_MAX];
        lengths[..literal_codes].copy_from_slice(&self.literals.sizes[..literal_codes]);
        lengths[literal_codes..literal_codes + distance_codes]
            .copy_from_slice(&self.distances.sizes[..distance_codes]);
        let packer = Packer::pack(&lengths[..literal_codes + distance_codes]);
        self.lengths
            .build_from_counts(&packer.counts, CODE_LENGTH_BITS_MAX);
        self.write_dynamic_header(literal_codes, distance_codes, &packer);
    }

    fn write_dynamic_header(
        &mut self,
        literal_codes: usize,
        distance_codes: usize,
        packer: &Packer,
    ) {
        let writer = &mut self.writer;
        writer.put(DYNAMIC_BLOCK_TYPE, BLOCK_TYPE_BITS);
        writer.put((literal_codes - LITERAL_CODES_MIN) as u32, 5);
        writer.put((distance_codes - 1) as u32, 5);
        let trailing = CODE_LENGTH_ORDER
            .iter()
            .rev()
            .take_while(|&&symbol| self.lengths.sizes[symbol] == 0)
            .count();
        let code_length_codes = (CODE_LENGTH_SYMBOLS - trailing).max(CODE_LENGTH_CODES_MIN);
        writer.put((code_length_codes - CODE_LENGTH_CODES_MIN) as u32, 4);
        for &symbol in &CODE_LENGTH_ORDER[..code_length_codes] {
            writer.put(u32::from(self.lengths.sizes[symbol]), 3);
        }
        for &(symbol, extra) in &packer.packed {
            self.lengths.put(writer, usize::from(symbol));
            if symbol >= REPEAT_PREVIOUS {
                writer.put(
                    u32::from(extra),
                    REPEAT_EXTRA_BITS[usize::from(symbol - REPEAT_PREVIOUS)],
                );
            }
        }
    }

    fn write_codes(&mut self) {
        let Self {
            block,
            writer,
            literals,
            distances,
            ..
        } = self;
        for &code in &block.codes {
            match code {
                Code::Literal(byte) => literals.put(writer, usize::from(byte)),
                Code::Match { length, offset } => {
                    let symbol = usize::from(LENGTH_SYMBOL[usize::from(length)]);
                    literals.put(writer, LITERAL_CODES_MIN + symbol);
                    let extra =
                        usize::from(length) + MATCH_LENGTH_MIN - usize::from(LENGTH_BASE[symbol]);
                    writer.put(extra as u32, u32::from(LENGTH_EXTRA_BITS[symbol]));
                    let symbol = distance_code(usize::from(offset));
                    distances.put(writer, symbol);
                    let extra = usize::from(offset) + 1 - usize::from(DISTANCE_BASE[symbol]);
                    writer.put(extra as u32, u32::from(DISTANCE_EXTRA_BITS[symbol]));
                }
            }
        }
        literals.put(writer, END_OF_BLOCK);
    }
}
