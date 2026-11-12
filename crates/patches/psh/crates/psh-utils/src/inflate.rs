use crate::codec::{Format, adler32};

const WINDOW_BYTES_MAX: u32 = 32_768;
const ZLIB_METHOD_DEFLATE: u32 = 8;
const ZLIB_CHECK_DIVISOR: u32 = 31;
const ZLIB_PRESET_DICTIONARY_FLAG: u32 = 0x20;
const ZLIB_WINDOW_BITS_OFFSET: u32 = 8;
const ZLIB_HEADER_BITS: u32 = 16;
const ZLIB_CHECKSUM_BYTES: u32 = 4;

const BLOCK_HEADER_BITS: u32 = 3;
const STORED_HEADER_BITS: u32 = 32;
const TABLE_SIZES_BITS: u32 = 14;
const CODE_LENGTH_CODE_BITS: u32 = 3;
const BYTE_BITS: u32 = 8;

const LITERAL_SYMBOLS: usize = 288;
const DISTANCE_SYMBOLS: usize = 32;
const CODE_LENGTH_SYMBOLS: usize = 19;
const LITERAL_CODES_MIN: usize = 257;
const LITERAL_CODES_MAX: usize = 286;
const DISTANCE_CODES_MAX: usize = 30;
const CODE_LENGTH_CODES_MIN: usize = 4;
const END_OF_BLOCK: u16 = 256;
const FIRST_LENGTH_SYMBOL: u16 = 257;
const LAST_LENGTH_SYMBOL: u16 = 285;
const LAST_DISTANCE_SYMBOL: u16 = 29;
const MATCH_LENGTH_MAX: usize = 258;

const REPEAT_PREVIOUS: u16 = 16;
const REPEAT_ZEROS_SHORT: u16 = 17;
const REPEAT_PREVIOUS_EXTRA_BITS: u32 = 2;
const REPEAT_ZEROS_SHORT_EXTRA_BITS: u32 = 3;
const REPEAT_ZEROS_LONG_EXTRA_BITS: u32 = 7;
const REPEAT_SHORT_COUNT_MIN: usize = 3;
const REPEAT_ZEROS_LONG_COUNT_MIN: usize = 11;

const CODE_BITS_MAX: u32 = 15;
const CODE_SPACE: u32 = 1 << (CODE_BITS_MAX + 1);
const LOOKUP_BITS: u32 = 10;
const LOOKUP_ENTRIES: usize = 1 << LOOKUP_BITS;
const LOOKUP_MASK: u64 = (1 << LOOKUP_BITS) - 1;
const TREE_ENTRIES: usize = 2 * LITERAL_SYMBOLS;
const LEAF_LENGTH_SHIFT: u32 = 9;
const SYMBOL_MASK: u16 = (1 << LEAF_LENGTH_SHIFT) - 1;
const LINK: u16 = 1 << 15;
const UNASSIGNED_SYMBOL: u16 = 286;
const UNASSIGNED_ENTRY: u16 = UNASSIGNED_SYMBOL | (1 << LEAF_LENGTH_SHIFT);

const FAST_INPUT_BYTES: usize = 8;
const FAST_BITS_MIN: u32 = 56;
const BUFFER_BITS: u32 = 64;

const STATIC_LITERAL_LENGTHS: [(usize, u8); 4] = [(144, 8), (256, 9), (280, 7), (288, 8)];
const STATIC_DISTANCE_LENGTH: u8 = 5;

const CODE_LENGTH_ORDER: [usize; CODE_LENGTH_SYMBOLS] = [
    16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
];

const LENGTH_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258,
];
const LENGTH_EXTRA_BITS: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
const DISTANCE_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
const DISTANCE_EXTRA_BITS: [u8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputEnd {
    More,
    Final,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Progress {
    Done { consumed: usize },
    NeedsInput,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InflateError {
    Truncated,
    OutputFull,
    ZlibHeader,
    BlockType,
    StoredLength,
    TableSizes,
    IncompleteCode,
    RepeatWithoutLength,
    CodeLengthsOverrun,
    LiteralSymbol,
    DistanceSymbol,
    DistanceTooFar,
    Checksum,
}

#[derive(Clone, Copy)]
struct Sizes {
    literals: usize,
    distances: usize,
    code_lengths: usize,
}

#[derive(Clone, Copy)]
enum Phase {
    ZlibHeader,
    BlockHeader,
    StoredHeader {
        last: bool,
    },
    Stored {
        last: bool,
        remaining: usize,
    },
    TableSizes {
        last: bool,
    },
    CodeLengthCodes {
        last: bool,
        sizes: Sizes,
        read: usize,
    },
    CodeLengths {
        last: bool,
        sizes: Sizes,
        read: usize,
    },
    Codes {
        last: bool,
    },
    Checksum {
        read: u32,
        value: u32,
    },
    Done,
    Failed(InflateError),
}

enum Stop {
    Input,
    Invalid(InflateError),
}

impl From<InflateError> for Stop {
    fn from(error: InflateError) -> Self {
        Stop::Invalid(error)
    }
}

enum Completeness {
    Required,
    UnlessSingle,
}

struct Table {
    lookup: [u16; LOOKUP_ENTRIES],
    tree: [u16; TREE_ENTRIES],
}

impl Table {
    const fn new() -> Self {
        Table {
            lookup: [UNASSIGNED_ENTRY; LOOKUP_ENTRIES],
            tree: [0; TREE_ENTRIES],
        }
    }

    fn build(&mut self, lengths: &[u8], completeness: Completeness) -> Result<(), InflateError> {
        let mut counts = [0u32; CODE_BITS_MAX as usize + 1];
        for &length in lengths {
            counts[usize::from(length)] += 1;
        }
        let mut next = [0u32; CODE_BITS_MAX as usize + 2];
        let mut used = 0;
        for length in 1..=CODE_BITS_MAX as usize {
            used += counts[length];
            next[length + 1] = (next[length] + counts[length]) << 1;
        }
        let complete = next[CODE_BITS_MAX as usize + 1] == CODE_SPACE;
        if !complete && (used > 1 || matches!(completeness, Completeness::Required)) {
            return Err(InflateError::IncompleteCode);
        }
        self.lookup.fill(UNASSIGNED_ENTRY);
        self.tree.fill(0);
        let mut nodes = 0;
        for (symbol, &length) in lengths.iter().enumerate() {
            if length == 0 {
                continue;
            }
            let length = u32::from(length);
            let code = next[length as usize];
            next[length as usize] += 1;
            let reversed = (code as u16).reverse_bits() >> (u16::BITS - length);
            self.insert(symbol as u16, length, usize::from(reversed), &mut nodes);
        }
        Ok(())
    }

    fn insert(&mut self, symbol: u16, length: u32, reversed: usize, nodes: &mut usize) {
        if length <= LOOKUP_BITS {
            let entry = symbol | ((length as u16) << LEAF_LENGTH_SHIFT);
            for slot in (reversed..LOOKUP_ENTRIES).step_by(1 << length) {
                self.lookup[slot] = entry;
            }
            return;
        }
        let slot = reversed & (LOOKUP_ENTRIES - 1);
        if self.lookup[slot] == UNASSIGNED_ENTRY {
            self.lookup[slot] = LINK | *nodes as u16;
            *nodes += 1;
        }
        assert!(self.lookup[slot] & LINK != 0);
        let mut node = usize::from(self.lookup[slot] & !LINK);
        for depth in LOOKUP_BITS..length - 1 {
            let child = 2 * node + (reversed >> depth & 1);
            if self.tree[child] == 0 {
                assert!(2 * *nodes + 1 < TREE_ENTRIES);
                self.tree[child] = LINK | *nodes as u16;
                *nodes += 1;
            }
            assert!(self.tree[child] & LINK != 0);
            node = usize::from(self.tree[child] & !LINK);
        }
        self.tree[2 * node + (reversed >> (length - 1) & 1)] = symbol;
    }

    fn decode(&self, bits: u64) -> (u16, u32) {
        let entry = self.lookup[(bits & LOOKUP_MASK) as usize];
        if entry & LINK == 0 {
            return (entry & SYMBOL_MASK, u32::from(entry >> LEAF_LENGTH_SHIFT));
        }
        let mut node = usize::from(entry & !LINK);
        for depth in LOOKUP_BITS..CODE_BITS_MAX {
            let child = self.tree[2 * node + (bits >> depth & 1) as usize];
            if child & LINK == 0 {
                return (child, depth + 1);
            }
            node = usize::from(child & !LINK);
        }
        unreachable!("a code is at most fifteen bits long")
    }
}

struct Stream<'a> {
    input: &'a [u8],
    position: usize,
    buffer: u64,
    count: u32,
}

impl Stream<'_> {
    fn fill(&mut self, bits: u32) -> Result<(), Stop> {
        assert!(bits <= BUFFER_BITS - BYTE_BITS);
        while self.count < bits {
            let Some(&byte) = self.input.get(self.position) else {
                return Err(Stop::Input);
            };
            self.position += 1;
            self.buffer |= u64::from(byte) << self.count;
            self.count += BYTE_BITS;
        }
        Ok(())
    }

    fn peek(&self, offset: u32, bits: u32) -> u32 {
        assert!(offset + bits <= self.count);
        ((self.buffer >> offset) & ((1u64 << bits) - 1)) as u32
    }

    fn consume(&mut self, bits: u32) {
        assert!(bits <= self.count);
        self.buffer >>= bits;
        self.count -= bits;
    }

    fn take(&mut self, bits: u32) -> Result<u32, Stop> {
        self.fill(bits)?;
        let value = self.peek(0, bits);
        self.consume(bits);
        Ok(value)
    }

    fn symbol(&mut self, table: &Table, offset: u32) -> Result<(u16, u32), Stop> {
        loop {
            let (symbol, length) = table.decode(self.buffer >> offset);
            if offset + length <= self.count {
                return Ok((symbol, length));
            }
            self.fill(self.count + BYTE_BITS)?;
        }
    }

    fn align(&mut self) {
        self.consume(self.count % BYTE_BITS);
    }

    fn refill(&mut self) {
        let word = u64::from_le_bytes(
            self.input[self.position..self.position + FAST_INPUT_BYTES]
                .try_into()
                .expect("eight bytes"),
        );
        let bytes = (BUFFER_BITS - 1 - self.count) / BYTE_BITS;
        self.buffer |= word << self.count;
        self.position += bytes as usize;
        self.count += bytes * BYTE_BITS;
        self.buffer &= (1u64 << self.count) - 1;
    }
}

fn copy_match(output: &mut Vec<u8>, distance: usize, length: usize) {
    let start = output.len() - distance;
    if distance >= length {
        output.extend_from_within(start..start + length);
    } else if distance == 1 {
        let byte = output[start];
        output.resize(output.len() + length, byte);
    } else {
        let end = output.len() + length;
        while output.len() < end {
            let chunk = (end - output.len()).min(output.len() - start);
            output.extend_from_within(start..start + chunk);
        }
    }
}

pub struct Inflater {
    format: Format,
    phase: Phase,
    buffer: u64,
    count: u32,
    adler: u32,
    code_length_lengths: [u8; CODE_LENGTH_SYMBOLS],
    lengths: [u8; LITERAL_CODES_MAX + DISTANCE_CODES_MAX],
    code_lengths: Table,
    literals: Table,
    distances: Table,
}

impl Inflater {
    pub fn new(format: Format) -> Self {
        Inflater {
            format,
            phase: Self::first_phase(format),
            buffer: 0,
            count: 0,
            adler: 1,
            code_length_lengths: [0; CODE_LENGTH_SYMBOLS],
            lengths: [0; LITERAL_CODES_MAX + DISTANCE_CODES_MAX],
            code_lengths: Table::new(),
            literals: Table::new(),
            distances: Table::new(),
        }
    }

    pub fn reset(&mut self) {
        self.phase = Self::first_phase(self.format);
        self.buffer = 0;
        self.count = 0;
        self.adler = 1;
    }

    fn first_phase(format: Format) -> Phase {
        match format {
            Format::Raw => Phase::BlockHeader,
            Format::Zlib => Phase::ZlibHeader,
        }
    }

    pub fn inflate(
        &mut self,
        input: &[u8],
        end: InputEnd,
        output: &mut Vec<u8>,
        output_bytes_max: usize,
    ) -> Result<Progress, InflateError> {
        assert!(output.len() <= output_bytes_max);
        let start = output.len();
        let mut stream = Stream {
            input,
            position: 0,
            buffer: self.buffer,
            count: self.count,
        };
        let result = self.run(&mut stream, output, output_bytes_max, start);
        self.buffer = stream.buffer;
        self.count = stream.count;
        let error = match result {
            Ok(()) => {
                let unread = (stream.count / BYTE_BITS) as usize;
                assert!(unread <= stream.position);
                return Ok(Progress::Done {
                    consumed: stream.position - unread,
                });
            }
            Err(Stop::Invalid(error)) => error,
            Err(Stop::Input) => match end {
                InputEnd::Final => InflateError::Truncated,
                InputEnd::More
                    if output.len() == output_bytes_max
                        && !matches!(self.phase, Phase::Checksum { .. }) =>
                {
                    InflateError::OutputFull
                }
                InputEnd::More => {
                    assert!(stream.position == input.len());
                    if let Format::Zlib = self.format {
                        self.adler = adler32(self.adler, &output[start..]);
                    }
                    return Ok(Progress::NeedsInput);
                }
            },
        };
        self.phase = Phase::Failed(error);
        Err(error)
    }

    fn run(
        &mut self,
        stream: &mut Stream,
        output: &mut Vec<u8>,
        output_bytes_max: usize,
        start: usize,
    ) -> Result<(), Stop> {
        loop {
            match self.phase {
                Phase::Done => return Ok(()),
                Phase::Failed(error) => return Err(Stop::Invalid(error)),
                Phase::ZlibHeader => self.zlib_header(stream, output)?,
                Phase::BlockHeader => self.block_header(stream)?,
                Phase::StoredHeader { last } => self.stored_header(stream, last)?,
                Phase::Stored { last, remaining } => {
                    self.stored(stream, output, output_bytes_max, last, remaining)?
                }
                Phase::TableSizes { last } => self.table_sizes(stream, last)?,
                Phase::CodeLengthCodes { last, sizes, read } => {
                    self.code_length_codes(stream, last, sizes, read)?
                }
                Phase::CodeLengths { last, sizes, read } => {
                    self.code_lengths(stream, last, sizes, read)?
                }
                Phase::Codes { last } => self.codes(stream, output, output_bytes_max, last)?,
                Phase::Checksum { read, value } => {
                    self.checksum(stream, &output[start..], read, value)?
                }
            }
        }
    }

    fn zlib_header(&mut self, stream: &mut Stream, output: &[u8]) -> Result<(), Stop> {
        assert!(output.is_empty());
        let header = stream.take(ZLIB_HEADER_BITS)?;
        let method = header & 0xff;
        let flags = header >> BYTE_BITS;
        let window_bytes = 1u32 << ((method >> 4) + ZLIB_WINDOW_BITS_OFFSET);
        let valid = ((method << BYTE_BITS) | flags).is_multiple_of(ZLIB_CHECK_DIVISOR)
            && flags & ZLIB_PRESET_DICTIONARY_FLAG == 0
            && method & 0x0f == ZLIB_METHOD_DEFLATE
            && window_bytes <= WINDOW_BYTES_MAX;
        if !valid {
            return Err(InflateError::ZlibHeader.into());
        }
        self.phase = Phase::BlockHeader;
        Ok(())
    }

    fn block_header(&mut self, stream: &mut Stream) -> Result<(), Stop> {
        let header = stream.take(BLOCK_HEADER_BITS)?;
        let last = header & 1 != 0;
        self.phase = match header >> 1 {
            0 => Phase::StoredHeader { last },
            1 => {
                self.static_tables();
                Phase::Codes { last }
            }
            2 => Phase::TableSizes { last },
            _ => return Err(InflateError::BlockType.into()),
        };
        Ok(())
    }

    fn static_tables(&mut self) {
        let mut lengths = [0u8; LITERAL_SYMBOLS];
        let mut from = 0;
        for (to, length) in STATIC_LITERAL_LENGTHS {
            lengths[from..to].fill(length);
            from = to;
        }
        let distances = [STATIC_DISTANCE_LENGTH; DISTANCE_SYMBOLS];
        self.distances
            .build(&distances, Completeness::UnlessSingle)
            .expect("the static distance code is complete");
        self.literals
            .build(&lengths, Completeness::UnlessSingle)
            .expect("the static literal code is complete");
    }

    fn end_block(&mut self, stream: &mut Stream, last: bool) {
        self.phase = match (last, self.format) {
            (false, _) => Phase::BlockHeader,
            (true, Format::Raw) => {
                stream.align();
                Phase::Done
            }
            (true, Format::Zlib) => {
                stream.align();
                Phase::Checksum { read: 0, value: 0 }
            }
        };
    }

    fn stored_header(&mut self, stream: &mut Stream, last: bool) -> Result<(), Stop> {
        stream.align();
        let header = stream.take(STORED_HEADER_BITS)?;
        let length = header & 0xffff;
        if length != !(header >> 16) & 0xffff {
            return Err(InflateError::StoredLength.into());
        }
        if length == 0 {
            self.end_block(stream, last);
        } else {
            self.phase = Phase::Stored {
                last,
                remaining: length as usize,
            };
        }
        Ok(())
    }

    fn stored(
        &mut self,
        stream: &mut Stream,
        output: &mut Vec<u8>,
        output_bytes_max: usize,
        last: bool,
        mut remaining: usize,
    ) -> Result<(), Stop> {
        while remaining > 0 && stream.count >= BYTE_BITS {
            if output.len() == output_bytes_max {
                return Err(InflateError::OutputFull.into());
            }
            output.push(stream.take(BYTE_BITS)? as u8);
            remaining -= 1;
        }
        let room = output_bytes_max - output.len();
        let available = stream.input.len() - stream.position;
        let bytes = remaining.min(room).min(available);
        output.extend_from_slice(&stream.input[stream.position..stream.position + bytes]);
        stream.position += bytes;
        remaining -= bytes;
        if remaining == 0 {
            self.end_block(stream, last);
            return Ok(());
        }
        self.phase = Phase::Stored { last, remaining };
        if output.len() == output_bytes_max {
            return Err(InflateError::OutputFull.into());
        }
        Err(Stop::Input)
    }

    fn table_sizes(&mut self, stream: &mut Stream, last: bool) -> Result<(), Stop> {
        let bits = stream.take(TABLE_SIZES_BITS)?;
        let sizes = Sizes {
            literals: (bits & 0x1f) as usize + LITERAL_CODES_MIN,
            distances: (bits >> 5 & 0x1f) as usize + 1,
            code_lengths: (bits >> 10) as usize + CODE_LENGTH_CODES_MIN,
        };
        if sizes.literals > LITERAL_CODES_MAX || sizes.distances > DISTANCE_CODES_MAX {
            return Err(InflateError::TableSizes.into());
        }
        self.code_length_lengths = [0; CODE_LENGTH_SYMBOLS];
        self.phase = Phase::CodeLengthCodes {
            last,
            sizes,
            read: 0,
        };
        Ok(())
    }

    fn code_length_codes(
        &mut self,
        stream: &mut Stream,
        last: bool,
        sizes: Sizes,
        mut read: usize,
    ) -> Result<(), Stop> {
        while read < sizes.code_lengths {
            let Ok(length) = stream.take(CODE_LENGTH_CODE_BITS) else {
                self.phase = Phase::CodeLengthCodes { last, sizes, read };
                return Err(Stop::Input);
            };
            self.code_length_lengths[CODE_LENGTH_ORDER[read]] = length as u8;
            read += 1;
        }
        self.code_lengths
            .build(&self.code_length_lengths, Completeness::Required)?;
        self.phase = Phase::CodeLengths {
            last,
            sizes,
            read: 0,
        };
        Ok(())
    }

    fn code_lengths(
        &mut self,
        stream: &mut Stream,
        last: bool,
        sizes: Sizes,
        mut read: usize,
    ) -> Result<(), Stop> {
        let total = sizes.literals + sizes.distances;
        while read < total {
            match self.code_length(stream, read, total) {
                Ok(next) => read = next,
                Err(Stop::Input) => {
                    self.phase = Phase::CodeLengths { last, sizes, read };
                    return Err(Stop::Input);
                }
                Err(stop) => return Err(stop),
            }
        }
        let (literals, distances) = self.lengths[..total].split_at(sizes.literals);
        self.distances
            .build(distances, Completeness::UnlessSingle)?;
        self.literals.build(literals, Completeness::UnlessSingle)?;
        self.phase = Phase::Codes { last };
        Ok(())
    }

    fn code_length(
        &mut self,
        stream: &mut Stream,
        read: usize,
        total: usize,
    ) -> Result<usize, Stop> {
        let (symbol, bits) = stream.symbol(&self.code_lengths, 0)?;
        if symbol < REPEAT_PREVIOUS {
            stream.consume(bits);
            self.lengths[read] = symbol as u8;
            return Ok(read + 1);
        }
        if symbol == REPEAT_PREVIOUS && read == 0 {
            return Err(InflateError::RepeatWithoutLength.into());
        }
        let (extra_bits, count_min, value) = match symbol {
            REPEAT_PREVIOUS => (
                REPEAT_PREVIOUS_EXTRA_BITS,
                REPEAT_SHORT_COUNT_MIN,
                self.lengths[read - 1],
            ),
            REPEAT_ZEROS_SHORT => (REPEAT_ZEROS_SHORT_EXTRA_BITS, REPEAT_SHORT_COUNT_MIN, 0),
            _ => (REPEAT_ZEROS_LONG_EXTRA_BITS, REPEAT_ZEROS_LONG_COUNT_MIN, 0),
        };
        stream.fill(bits + extra_bits)?;
        let count = stream.peek(bits, extra_bits) as usize + count_min;
        stream.consume(bits + extra_bits);
        if read + count > total {
            return Err(InflateError::CodeLengthsOverrun.into());
        }
        self.lengths[read..read + count].fill(value);
        Ok(read + count)
    }

    fn codes(
        &mut self,
        stream: &mut Stream,
        output: &mut Vec<u8>,
        output_bytes_max: usize,
        last: bool,
    ) -> Result<(), Stop> {
        loop {
            let fast = stream.input.len() - stream.position >= FAST_INPUT_BYTES
                && output_bytes_max - output.len() >= MATCH_LENGTH_MAX;
            let ended = if fast {
                self.code_fast(stream, output)?
            } else {
                self.code_slow(stream, output, output_bytes_max)?
            };
            if ended {
                self.end_block(stream, last);
                return Ok(());
            }
        }
    }

    fn code_fast(&self, stream: &mut Stream, output: &mut Vec<u8>) -> Result<bool, Stop> {
        if stream.count < FAST_BITS_MIN {
            stream.refill();
        }
        let (symbol, bits) = self.literals.decode(stream.buffer);
        if symbol < END_OF_BLOCK {
            stream.consume(bits);
            output.push(symbol as u8);
            return Ok(false);
        }
        if symbol == END_OF_BLOCK {
            stream.consume(bits);
            return Ok(true);
        }
        let (distance, length, used) = self.match_codes(stream, symbol, bits)?;
        if distance > output.len() {
            return Err(InflateError::DistanceTooFar.into());
        }
        stream.consume(used);
        copy_match(output, distance, length);
        Ok(false)
    }

    fn match_codes(
        &self,
        stream: &mut Stream,
        symbol: u16,
        bits: u32,
    ) -> Result<(usize, usize, u32), Stop> {
        if symbol > LAST_LENGTH_SYMBOL {
            return Err(InflateError::LiteralSymbol.into());
        }
        let index = usize::from(symbol - FIRST_LENGTH_SYMBOL);
        let extra = u32::from(LENGTH_EXTRA_BITS[index]);
        stream.fill(bits + extra)?;
        let length = usize::from(LENGTH_BASE[index]) + stream.peek(bits, extra) as usize;
        let mut used = bits + extra;
        let (code, code_bits) = stream.symbol(&self.distances, used)?;
        if code > LAST_DISTANCE_SYMBOL {
            return Err(InflateError::DistanceSymbol.into());
        }
        used += code_bits;
        let extra = u32::from(DISTANCE_EXTRA_BITS[usize::from(code)]);
        stream.fill(used + extra)?;
        let distance =
            usize::from(DISTANCE_BASE[usize::from(code)]) + stream.peek(used, extra) as usize;
        Ok((distance, length, used + extra))
    }

    fn code_slow(
        &self,
        stream: &mut Stream,
        output: &mut Vec<u8>,
        output_bytes_max: usize,
    ) -> Result<bool, Stop> {
        let (symbol, bits) = stream.symbol(&self.literals, 0)?;
        if symbol == END_OF_BLOCK {
            stream.consume(bits);
            return Ok(true);
        }
        if symbol < END_OF_BLOCK {
            stream.consume(bits);
            if output.len() == output_bytes_max {
                return Err(InflateError::OutputFull.into());
            }
            output.push(symbol as u8);
            return Ok(false);
        }
        let (distance, length, used) = self.match_codes(stream, symbol, bits)?;
        if distance > output.len() {
            return Err(InflateError::DistanceTooFar.into());
        }
        stream.consume(used);
        if output_bytes_max - output.len() < length {
            return Err(InflateError::OutputFull.into());
        }
        copy_match(output, distance, length);
        Ok(false)
    }

    fn checksum(
        &mut self,
        stream: &mut Stream,
        output: &[u8],
        mut read: u32,
        mut value: u32,
    ) -> Result<(), Stop> {
        while read < ZLIB_CHECKSUM_BYTES {
            let Ok(byte) = stream.take(BYTE_BITS) else {
                self.phase = Phase::Checksum { read, value };
                return Err(Stop::Input);
            };
            value = value << BYTE_BITS | byte;
            read += 1;
        }
        if adler32(self.adler, output) != value {
            return Err(InflateError::Checksum.into());
        }
        self.phase = Phase::Done;
        Ok(())
    }
}
