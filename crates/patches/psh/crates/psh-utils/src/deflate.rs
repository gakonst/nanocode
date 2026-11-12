use crate::codec::{Format, adler32};

const DICTIONARY_BYTES: usize = 32_768;
const DICTIONARY_MASK: usize = DICTIONARY_BYTES - 1;
const MATCH_LENGTH_MIN: usize = 3;
const MATCH_LENGTH_MAX: usize = 258;
const MIRROR_BYTES: usize = MATCH_LENGTH_MAX - 1;
const DICTIONARY_PADDED_BYTES: usize = DICTIONARY_BYTES + MATCH_LENGTH_MAX;
const POSITION_MASK: usize = u16::MAX as usize;

const HASH_BITS: u32 = 15;
const HASH_ENTRIES: usize = 1 << HASH_BITS;
const HASH_MASK: u32 = (1 << HASH_BITS) - 1;
const HASH_SHIFT_BITS: u32 = HASH_BITS.div_ceil(3);
const FAST_HASH_MASK: u32 = (1 << 12) - 1;
const FAST_HASH_SHIFT_BITS: u32 = 24 - (HASH_BITS - 8);
const TRIGRAM_MASK: u32 = 0x00ff_ffff;
const FAST_LOOKAHEAD_BYTES: usize = 4_096;
const FAST_LOOKAHEAD_BYTES_MIN: usize = 4;
const COMPARE_WORDS: usize = 32;
const WORD_BYTES: usize = 8;

const LEVEL_PROBES: [u32; 10] = [0, 1, 6, 32, 16, 32, 128, 256, 512, 768];
const GREEDY_LEVEL_MAX: u8 = 3;
const ZLIB_BEST_PROBES: u32 = LEVEL_PROBES[9];
const LONG_MATCH_LENGTH: u32 = 32;
const LAZY_LENGTH_MAX: u32 = 128;
const FAR_DISTANCE_MIN: u32 = 8 * 1024;

const CODE_BUFFER_BYTES: usize = 64 * 1024;
const CODE_BUFFER_SLACK_BYTES: usize = 8;
const CODE_FLAGS_BITS: usize = 8;
const MATCH_CODE_BYTES: usize = 3;
const FAT_BLOCK_BYTES: u32 = 31 * 1024;
const FAT_RATIO_NUMERATOR: usize = 115;
const FAT_RATIO_SHIFT: u32 = 7;
const DYNAMIC_BLOCK_BYTES_MIN: u32 = 48;
const STORED_BLOCK_BYTES_MIN: u32 = 32;
const STAGING_BYTES: usize = CODE_BUFFER_BYTES * 13 / 10;
const STAGING_SLACK_BYTES: usize = 16;
const OUTPUT_SPACE_BYTES_MIN: usize = 30;

const LITERAL_SYMBOLS: usize = 288;
const DISTANCE_SYMBOLS: usize = 32;
const CODE_LENGTH_SYMBOLS: usize = 19;
const LITERAL_CODES_MIN: usize = 257;
const LITERAL_CODES_MAX: usize = 286;
const DISTANCE_CODES_MAX: usize = 30;
const CODE_LENGTH_CODES_MIN: usize = 4;
const END_OF_BLOCK: usize = 256;
const CODE_BITS_MAX: usize = 15;
const CODE_LENGTH_BITS_MAX: usize = 7;
const DEPTHS_MAX: usize = 32;
const STATIC_LITERAL_LENGTHS: [(usize, u8); 4] = [(144, 8), (256, 9), (280, 7), (288, 8)];
const STATIC_DISTANCE_LENGTH: u8 = 5;
const CODE_LENGTH_ORDER: [usize; CODE_LENGTH_SYMBOLS] = [
    16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
];

const REPEAT_PREVIOUS: u8 = 16;
const REPEAT_ZEROS_SHORT: u8 = 17;
const REPEAT_ZEROS_LONG: u8 = 18;
const REPEAT_EXTRA_BITS: [u32; 3] = [2, 3, 7];
const REPEAT_COUNT_MIN: usize = 3;
const REPEAT_PREVIOUS_COUNT_MAX: usize = 6;
const REPEAT_ZEROS_SHORT_COUNT_MAX: usize = 10;
const REPEAT_ZEROS_LONG_COUNT_MIN: usize = 11;
const REPEAT_ZEROS_LONG_COUNT_MAX: usize = 138;
const NO_PREVIOUS_LENGTH: u8 = 0xff;

const STATIC_BLOCK_TYPE: u32 = 1;
const DYNAMIC_BLOCK_TYPE: u32 = 2;
const BLOCK_TYPE_BITS: u32 = 2;
const STORED_LENGTH_BITS: u32 = 16;
const BYTE_BITS: u32 = 8;
const ZLIB_METHOD_AND_WINDOW: u8 = 0x78;
const ZLIB_CHECK_DIVISOR: u32 = 31;
const ZLIB_LEVEL_SHIFT: u32 = 6;

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
const DISTANCE_SMALL: usize = 512;
const DISTANCE_LARGE_SHIFT: u32 = 8;

const LENGTH_SYMBOL: [u8; MATCH_LENGTH_MAX - MATCH_LENGTH_MIN + 1] = {
    let mut table = [0u8; MATCH_LENGTH_MAX - MATCH_LENGTH_MIN + 1];
    let mut code = 0;
    while code < LENGTH_BASE.len() {
        let mut length = LENGTH_BASE[code] as usize;
        while length < MATCH_LENGTH_MAX + 1 {
            table[length - MATCH_LENGTH_MIN] = code as u8;
            length += 1;
        }
        code += 1;
    }
    table
};

const DISTANCE_SYMBOL_SMALL: [u8; DISTANCE_SMALL] = {
    let mut table = [0u8; DISTANCE_SMALL];
    let mut index = 0;
    while index < DISTANCE_SMALL {
        table[index] = distance_symbol(index);
        index += 1;
    }
    table
};

const DISTANCE_SYMBOL_LARGE: [u8; DICTIONARY_BYTES >> DISTANCE_LARGE_SHIFT] = {
    let mut table = [0u8; DICTIONARY_BYTES >> DISTANCE_LARGE_SHIFT];
    let mut index = DISTANCE_SMALL >> DISTANCE_LARGE_SHIFT;
    while index < table.len() {
        table[index] = distance_symbol(index << DISTANCE_LARGE_SHIFT);
        index += 1;
    }
    table
};

const fn distance_symbol(offset: usize) -> u8 {
    let mut code = 0;
    while code + 1 < DISTANCE_BASE.len() && DISTANCE_BASE[code + 1] as usize <= offset + 1 {
        code += 1;
    }
    code as u8
}

fn distance_code(offset: usize) -> usize {
    if offset < DISTANCE_SMALL {
        usize::from(DISTANCE_SYMBOL_SMALL[offset])
    } else {
        usize::from(DISTANCE_SYMBOL_LARGE[offset >> DISTANCE_LARGE_SHIFT])
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Parsing {
    Fast,
    Greedy,
    Lazy,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Flow {
    Finished,
    Interrupted,
}

#[derive(Clone, Copy)]
enum Code {
    Literal(u8),
    Match { length: u8, offset: u16 },
}

#[derive(Clone, Copy)]
struct Pending {
    literal: u8,
    distance: u32,
    length: u32,
}

struct Writer {
    bytes: Vec<u8>,
    buffer: u64,
    count: u32,
}

#[derive(Clone, Copy)]
struct Mark {
    length: usize,
    buffer: u64,
    count: u32,
}

impl Writer {
    fn put(&mut self, bits: u32, count: u32) {
        assert!(count <= u16::BITS);
        assert!(u64::from(bits) >> count == 0);
        self.buffer |= u64::from(bits) << self.count;
        self.count += count;
        if self.count >= u32::BITS {
            self.bytes
                .extend_from_slice(&(self.buffer as u32).to_le_bytes());
            self.buffer >>= u32::BITS;
            self.count -= u32::BITS;
        }
    }

    fn len(&self) -> usize {
        self.bytes.len() + (self.count / BYTE_BITS) as usize
    }

    fn mark(&self) -> Mark {
        Mark {
            length: self.bytes.len(),
            buffer: self.buffer,
            count: self.count,
        }
    }

    fn rewind(&mut self, mark: Mark) {
        self.bytes.truncate(mark.length);
        self.buffer = mark.buffer;
        self.count = mark.count;
    }

    fn align(&mut self) {
        self.put(0, (BYTE_BITS - self.count % BYTE_BITS) % BYTE_BITS);
        while self.count >= BYTE_BITS {
            self.bytes.push(self.buffer as u8);
            self.buffer >>= BYTE_BITS;
            self.count -= BYTE_BITS;
        }
        assert!(self.count == 0);
    }
}

struct Sink {
    capacity: usize,
    position: usize,
    offset: usize,
    pending: usize,
}

impl Sink {
    fn block(&mut self, bytes: usize) -> bool {
        assert!(bytes <= STAGING_BYTES - STAGING_SLACK_BYTES);
        let space = self.capacity - self.position - self.offset;
        if space >= STAGING_BYTES {
            self.offset += bytes;
            return false;
        }
        let copied = bytes.min(space);
        self.offset += copied;
        self.pending = bytes - copied;
        self.pending != 0
    }

    fn drain(&mut self) {
        let copied = (self.capacity - self.position - self.offset).min(self.pending);
        self.offset += copied;
        self.pending -= copied;
    }

    fn end_call(&mut self) {
        self.position += self.offset;
        self.offset = 0;
        if self.capacity - self.position < OUTPUT_SPACE_BYTES_MIN {
            self.capacity *= 2;
        }
    }
}

struct Dictionary {
    bytes: Box<[u8]>,
    next: Box<[u16]>,
    hash: Box<[u16]>,
    size: usize,
    lookahead_size: usize,
    lookahead_position: usize,
    block_start: usize,
}

impl Dictionary {
    fn read_u16(&self, index: usize) -> u16 {
        u16::from_le_bytes([self.bytes[index], self.bytes[index + 1]])
    }

    fn read_u32(&self, position: usize) -> u32 {
        let index = position & DICTIONARY_MASK;
        u32::from_le_bytes(self.bytes[index..index + 4].try_into().expect("four bytes"))
    }

    fn read_u64(&self, position: usize) -> u64 {
        let index = position & DICTIONARY_MASK;
        u64::from_le_bytes(
            self.bytes[index..index + WORD_BYTES]
                .try_into()
                .expect("eight bytes"),
        )
    }

    fn store(&mut self, destination: usize, byte: u8) {
        self.bytes[destination] = byte;
        if destination < MIRROR_BYTES {
            self.bytes[DICTIONARY_BYTES + destination] = byte;
        }
    }

    fn load_plain(&mut self, chunk: &[u8]) {
        let mut destination = (self.lookahead_position + self.lookahead_size) & DICTIONARY_MASK;
        self.lookahead_size += chunk.len();
        let mut rest = chunk;
        while !rest.is_empty() {
            let bytes = (DICTIONARY_BYTES - destination).min(rest.len());
            self.bytes[destination..destination + bytes].copy_from_slice(&rest[..bytes]);
            if destination < MIRROR_BYTES {
                let mirrored = bytes.min(MIRROR_BYTES - destination);
                self.bytes
                    [DICTIONARY_BYTES + destination..DICTIONARY_BYTES + destination + mirrored]
                    .copy_from_slice(&rest[..mirrored]);
            }
            rest = &rest[bytes..];
            destination = (destination + bytes) & DICTIONARY_MASK;
        }
    }

    fn load_hashed(&mut self, chunk: &[u8]) {
        if self.lookahead_size + self.size >= MATCH_LENGTH_MIN - 1 && !chunk.is_empty() {
            let mut destination = (self.lookahead_position + self.lookahead_size) & DICTIONARY_MASK;
            let first_insert = self.lookahead_position + self.lookahead_size - 2;
            let mut hash = roll(
                u32::from(self.bytes[first_insert & DICTIONARY_MASK]),
                self.bytes[(first_insert + 1) & DICTIONARY_MASK],
            );
            self.lookahead_size += chunk.len();
            for (insert, &byte) in (first_insert..).zip(chunk) {
                self.store(destination, byte);
                hash = roll(hash, byte);
                self.next[insert & DICTIONARY_MASK] = self.hash[hash as usize];
                self.hash[hash as usize] = insert as u16;
                destination = (destination + 1) & DICTIONARY_MASK;
            }
            return;
        }
        for &byte in chunk {
            let destination = (self.lookahead_position + self.lookahead_size) & DICTIONARY_MASK;
            self.store(destination, byte);
            self.lookahead_size += 1;
            if self.lookahead_size + self.size >= MATCH_LENGTH_MIN {
                let insert = self.lookahead_position + self.lookahead_size - MATCH_LENGTH_MIN;
                let first = u32::from(self.bytes[insert & DICTIONARY_MASK]);
                let second = u32::from(self.bytes[(insert + 1) & DICTIONARY_MASK]);
                let hash = ((first << (2 * HASH_SHIFT_BITS))
                    ^ (second << HASH_SHIFT_BITS)
                    ^ u32::from(byte))
                    & HASH_MASK;
                self.next[insert & DICTIONARY_MASK] = self.hash[hash as usize];
                self.hash[hash as usize] = insert as u16;
            }
        }
    }

    fn find_match(&self, probes: [u32; 2], longest: u32, length: u32) -> (u32, u32) {
        let longest = longest.min(MATCH_LENGTH_MAX as u32);
        let mut best_length = length.max(1);
        let mut best_distance = 0;
        if longest <= best_length {
            return (best_distance, best_length);
        }
        let position = self.lookahead_position & DICTIONARY_MASK;
        let mut probe = position;
        let mut probes_left = if best_length < LONG_MATCH_LENGTH {
            probes[0]
        } else {
            probes[1]
        };
        let mut tail = self.read_u16(position + best_length as usize - 1);
        let head = self.read_u16(position);
        loop {
            let Some(distance) =
                self.next_candidate(&mut probe, &mut probes_left, best_length, tail)
            else {
                return (best_distance, best_length);
            };
            if distance == 0 {
                return (best_distance, best_length);
            }
            if self.read_u16(probe) != head {
                continue;
            }
            let Some(matched) = self.compare(position + 2, probe + 2, position) else {
                return (distance, longest);
            };
            if matched > best_length as usize {
                best_distance = distance;
                best_length = longest.min(matched as u32);
                if best_length >= longest {
                    return (best_distance, best_length);
                }
                tail = self.read_u16((position + best_length as usize).saturating_sub(1));
            }
        }
    }

    fn next_candidate(
        &self,
        probe: &mut usize,
        probes_left: &mut u32,
        length: u32,
        tail: u16,
    ) -> Option<u32> {
        loop {
            *probes_left -= 1;
            if *probes_left == 0 {
                return None;
            }
            for _ in 0..3 {
                let candidate = usize::from(self.next[*probe]);
                let distance = self.lookahead_position.wrapping_sub(candidate) & POSITION_MASK;
                if candidate == 0
                    || distance > self.size
                    || length == 0
                    || length as usize > MATCH_LENGTH_MAX
                {
                    return None;
                }
                *probe = candidate & DICTIONARY_MASK;
                if self.read_u16(*probe + length as usize - 1) == tail {
                    return Some(distance as u32);
                }
            }
        }
    }

    fn compare(&self, mut current: usize, mut candidate: usize, start: usize) -> Option<usize> {
        for _ in 0..COMPARE_WORDS {
            let difference = self.read_u64(current) ^ self.read_u64(candidate);
            if difference != 0 {
                return Some(current - start + (difference.trailing_zeros() / BYTE_BITS) as usize);
            }
            current += WORD_BYTES;
            candidate += WORD_BYTES;
        }
        None
    }
}

fn roll(hash: u32, byte: u8) -> u32 {
    ((hash << HASH_SHIFT_BITS) ^ u32::from(byte)) & HASH_MASK
}

struct Block {
    codes: Vec<Code>,
    position: usize,
    total_bytes: u32,
    literal_counts: [u16; LITERAL_SYMBOLS],
    distance_counts: [u16; DISTANCE_SYMBOLS],
}

impl Block {
    fn new() -> Self {
        Block {
            codes: Vec::new(),
            position: 1,
            total_bytes: 0,
            literal_counts: [0; LITERAL_SYMBOLS],
            distance_counts: [0; DISTANCE_SYMBOLS],
        }
    }

    fn reset(&mut self) {
        self.codes.clear();
        self.position = 1;
        self.total_bytes = 0;
        self.literal_counts = [0; LITERAL_SYMBOLS];
        self.distance_counts = [0; DISTANCE_SYMBOLS];
    }

    fn after_code(&mut self, bytes: usize) {
        self.position += bytes;
        if self.codes.len().is_multiple_of(CODE_FLAGS_BITS) {
            self.position += 1;
        }
    }

    fn literal(&mut self, byte: u8) {
        self.total_bytes += 1;
        self.codes.push(Code::Literal(byte));
        self.literal_counts[usize::from(byte)] += 1;
        self.after_code(1);
    }

    fn matched(&mut self, length: u32, distance: u32) {
        assert!(length as usize >= MATCH_LENGTH_MIN);
        assert!(length as usize <= MATCH_LENGTH_MAX);
        assert!(distance >= 1);
        assert!(distance as usize <= DICTIONARY_BYTES);
        self.total_bytes += length;
        let length = (length as usize - MATCH_LENGTH_MIN) as u8;
        let offset = (distance - 1) as u16;
        self.codes.push(Code::Match { length, offset });
        self.distance_counts[distance_code(usize::from(offset))] += 1;
        self.literal_counts[LITERAL_CODES_MIN + usize::from(LENGTH_SYMBOL[usize::from(length)])] +=
            1;
        self.after_code(MATCH_CODE_BYTES);
    }

    fn tight(&self) -> bool {
        self.position > CODE_BUFFER_BYTES - CODE_BUFFER_SLACK_BYTES
    }

    fn fat(&self) -> bool {
        self.total_bytes > FAT_BLOCK_BYTES
            && (self.position * FAT_RATIO_NUMERATOR) >> FAT_RATIO_SHIFT >= self.total_bytes as usize
    }
}

struct Huffman<const N: usize> {
    sizes: [u8; N],
    codes: [u16; N],
}

impl<const N: usize> Huffman<N> {
    fn new() -> Self {
        Huffman {
            sizes: [0; N],
            codes: [0; N],
        }
    }

    fn build_from_counts(&mut self, counts: &[u16; N], limit: usize) {
        let mut order = [0u16; N];
        let mut used = 0;
        for (symbol, &count) in counts.iter().enumerate() {
            if count != 0 {
                order[used] = symbol as u16;
                used += 1;
            }
        }
        let order = &mut order[..used];
        order.sort_by_key(|&symbol| counts[usize::from(symbol)]);
        let mut keys = [0u16; N];
        for (key, &symbol) in keys.iter_mut().zip(order.iter()) {
            *key = counts[usize::from(symbol)];
        }
        let keys = &mut keys[..used];
        minimum_redundancy(keys);
        let mut lengths = [0i32; DEPTHS_MAX + 1];
        for &depth in keys.iter() {
            lengths[usize::from(depth)] += 1;
        }
        limit_lengths(&mut lengths, used, limit);
        self.sizes = [0; N];
        let mut last = used;
        for (length, &count) in lengths.iter().enumerate().take(limit + 1).skip(1) {
            let first = last - count as usize;
            for &symbol in &order[first..last] {
                self.sizes[usize::from(symbol)] = length as u8;
            }
            last = first;
        }
        self.assign_codes(limit);
    }

    fn assign_codes(&mut self, limit: usize) {
        let mut lengths = [0u32; CODE_BITS_MAX + 1];
        for &size in &self.sizes {
            lengths[usize::from(size)] += 1;
        }
        let mut next = [0u32; CODE_BITS_MAX + 1];
        let mut code = 0;
        for length in 2..=limit {
            code = (code + lengths[length - 1]) << 1;
            next[length] = code;
        }
        for (size, slot) in self.sizes.iter().zip(self.codes.iter_mut()) {
            if *size == 0 {
                *slot = 0;
                continue;
            }
            let code = next[usize::from(*size)];
            next[usize::from(*size)] += 1;
            *slot = (code as u16).reverse_bits() >> (u16::BITS - u32::from(*size));
        }
    }

    fn put(&self, writer: &mut Writer, symbol: usize) {
        writer.put(u32::from(self.codes[symbol]), u32::from(self.sizes[symbol]));
    }
}

fn minimum_redundancy(keys: &mut [u16]) {
    let count = keys.len();
    if count == 0 {
        return;
    }
    if count == 1 {
        keys[0] = 1;
        return;
    }
    keys[0] = keys[0].wrapping_add(keys[1]);
    let (mut root, mut leaf) = (0, 2);
    for next in 1..count - 1 {
        if leaf >= count || keys[root] < keys[leaf] {
            keys[next] = keys[root];
            keys[root] = next as u16;
            root += 1;
        } else {
            keys[next] = keys[leaf];
            leaf += 1;
        }
        if leaf >= count || (root < next && keys[root] < keys[leaf]) {
            keys[next] = keys[next].wrapping_add(keys[root]);
            keys[root] = next as u16;
            root += 1;
        } else {
            keys[next] = keys[next].wrapping_add(keys[leaf]);
            leaf += 1;
        }
    }
    keys[count - 2] = 0;
    for next in (0..count - 2).rev() {
        keys[next] = keys[usize::from(keys[next])].wrapping_add(1);
    }
    assign_depths(keys);
}

fn assign_depths(keys: &mut [u16]) {
    let count = keys.len();
    let (mut available, mut used, mut depth) = (1usize, 0usize, 0u16);
    let mut root = count as isize - 2;
    let mut next = count as isize - 1;
    while available > 0 {
        assert!(usize::from(depth) <= count);
        while root >= 0 && keys[root as usize] == depth {
            used += 1;
            root -= 1;
        }
        while available > used {
            keys[next as usize] = depth;
            next -= 1;
            available -= 1;
        }
        available = 2 * used;
        depth += 1;
        used = 0;
    }
}

fn limit_lengths(lengths: &mut [i32; DEPTHS_MAX + 1], used: usize, limit: usize) {
    if used <= 1 {
        return;
    }
    let overflow: i32 = lengths[limit + 1..].iter().sum();
    lengths[limit] += overflow;
    let total: u32 = (1..=limit)
        .map(|length| (lengths[length] as u32) << (limit - length))
        .sum();
    for _ in (1u32 << limit)..total {
        lengths[limit] -= 1;
        for length in (1..limit).rev() {
            if lengths[length] != 0 {
                lengths[length] -= 1;
                lengths[length + 1] += 2;
                break;
            }
        }
    }
}

struct Compressor {
    format: Format,
    parsing: Parsing,
    probes: [u32; 2],
    level: u8,
    dictionary: Dictionary,
    block: Block,
    writer: Writer,
    literals: Huffman<LITERAL_SYMBOLS>,
    distances: Huffman<DISTANCE_SYMBOLS>,
    lengths: Huffman<CODE_LENGTH_SYMBOLS>,
    pending: Option<Pending>,
    blocks: u32,
    adler: u32,
}

pub fn compress(data: &[u8], level: u8, format: Format) -> Vec<u8> {
    assert!((1..=9).contains(&level));
    let mut compressor = Compressor::new(level, format, data);
    let mut sink = Sink {
        capacity: (data.len() / 2).max(2),
        position: 0,
        offset: 0,
        pending: 0,
    };
    let mut source = 0;
    let mut finished = false;
    loop {
        if sink.pending == 0 && !finished {
            let flow = compressor.run(data, &mut source, &mut sink);
            if flow == Flow::Finished && sink.pending == 0 {
                assert!(source == data.len());
                assert!(compressor.dictionary.lookahead_size == 0);
                compressor.flush_block(true, &mut sink);
                finished = true;
            }
        }
        sink.drain();
        if finished && sink.pending == 0 {
            break;
        }
        sink.end_call();
    }
    compressor.writer.align();
    compressor.writer.bytes
}

impl Compressor {
    fn new(level: u8, format: Format, data: &[u8]) -> Self {
        let probes = LEVEL_PROBES[usize::from(level)];
        let parsing = match level {
            1 => Parsing::Fast,
            2..=GREEDY_LEVEL_MAX => Parsing::Greedy,
            _ => Parsing::Lazy,
        };
        let adler = match format {
            Format::Raw => 1,
            Format::Zlib => adler32(1, data),
        };
        Compressor {
            format,
            parsing,
            probes: [1 + probes.div_ceil(3), 1 + (probes >> 2).div_ceil(3)],
            level,
            dictionary: Dictionary {
                bytes: vec![0; DICTIONARY_PADDED_BYTES].into_boxed_slice(),
                next: vec![0; DICTIONARY_BYTES].into_boxed_slice(),
                hash: vec![0; HASH_ENTRIES].into_boxed_slice(),
                size: 0,
                lookahead_size: 0,
                lookahead_position: 0,
                block_start: 0,
            },
            block: Block::new(),
            writer: Writer {
                bytes: Vec::new(),
                buffer: 0,
                count: 0,
            },
            literals: Huffman::new(),
            distances: Huffman::new(),
            lengths: Huffman::new(),
            pending: None,
            blocks: 0,
            adler,
        }
    }

    fn run(&mut self, data: &[u8], source: &mut usize, sink: &mut Sink) -> Flow {
        match self.parsing {
            Parsing::Fast => self.run_fast(data, source, sink),
            Parsing::Greedy | Parsing::Lazy => self.run_normal(data, source, sink),
        }
    }

    fn run_normal(&mut self, data: &[u8], source: &mut usize, sink: &mut Sink) -> Flow {
        while *source < data.len() || self.dictionary.lookahead_size != 0 {
            let bytes =
                (data.len() - *source).min(MATCH_LENGTH_MAX - self.dictionary.lookahead_size);
            self.dictionary.load_hashed(&data[*source..*source + bytes]);
            *source += bytes;
            let dictionary = &mut self.dictionary;
            dictionary.size = dictionary
                .size
                .min(DICTIONARY_BYTES - dictionary.lookahead_size);
            self.step_normal();
            if (self.block.tight() || self.block.fat()) && self.flush_block(false, sink) {
                return Flow::Interrupted;
            }
        }
        Flow::Finished
    }

    fn step_normal(&mut self) {
        let dictionary = &self.dictionary;
        let position = dictionary.lookahead_position & DICTIONARY_MASK;
        let length = self
            .pending
            .map_or(MATCH_LENGTH_MIN as u32 - 1, |pending| pending.length);
        let (mut distance, mut length) =
            dictionary.find_match(self.probes, dictionary.lookahead_size as u32, length);
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
            None if self.parsing == Parsing::Greedy || length >= LAZY_LENGTH_MAX => {
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

    fn run_fast(&mut self, data: &[u8], source: &mut usize, sink: &mut Sink) -> Flow {
        while *source < data.len() || self.dictionary.lookahead_size > 0 {
            let bytes =
                (data.len() - *source).min(FAST_LOOKAHEAD_BYTES - self.dictionary.lookahead_size);
            self.dictionary.load_plain(&data[*source..*source + bytes]);
            *source += bytes;
            let dictionary = &mut self.dictionary;
            dictionary.size = dictionary
                .size
                .min(DICTIONARY_BYTES - dictionary.lookahead_size);
            while self.dictionary.lookahead_size >= FAST_LOOKAHEAD_BYTES_MIN {
                self.step_fast();
                if self.block.tight() && self.flush_block(false, sink) {
                    return Flow::Interrupted;
                }
            }
            while self.dictionary.lookahead_size != 0 {
                let position = self.dictionary.lookahead_position & DICTIONARY_MASK;
                self.block.literal(self.dictionary.bytes[position]);
                self.advance(1);
                if self.block.tight() && self.flush_block(false, sink) {
                    return Flow::Interrupted;
                }
            }
        }
        Flow::Finished
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
}

struct Packer {
    zeros: usize,
    repeats: usize,
    previous: u8,
    packed: Vec<(u8, u8)>,
    counts: [u16; CODE_LENGTH_SYMBOLS],
}

impl Packer {
    fn pack(lengths: &[u8]) -> Self {
        let mut packer = Packer {
            zeros: 0,
            repeats: 0,
            previous: NO_PREVIOUS_LENGTH,
            packed: Vec::new(),
            counts: [0; CODE_LENGTH_SYMBOLS],
        };
        for &length in lengths {
            if length == 0 {
                packer.flush_repeats();
                packer.zeros += 1;
                if packer.zeros == REPEAT_ZEROS_LONG_COUNT_MAX {
                    packer.flush_zeros();
                }
            } else {
                packer.flush_zeros();
                if length != packer.previous {
                    packer.flush_repeats();
                    packer.emit(length, 0, 1);
                } else {
                    packer.repeats += 1;
                    if packer.repeats == REPEAT_PREVIOUS_COUNT_MAX {
                        packer.flush_repeats();
                    }
                }
            }
            packer.previous = length;
        }
        if packer.repeats != 0 {
            packer.flush_repeats();
        } else {
            packer.flush_zeros();
        }
        packer
    }

    fn emit(&mut self, symbol: u8, extra: u8, times: usize) {
        let slot = &mut self.counts[usize::from(symbol)];
        *slot = slot.wrapping_add(times as u16);
        for _ in 0..times {
            self.packed.push((symbol, extra));
        }
    }

    fn flush_repeats(&mut self) {
        match self.repeats {
            0 => return,
            repeats if repeats < REPEAT_COUNT_MIN => self.emit(self.previous, 0, repeats),
            repeats => self.emit(REPEAT_PREVIOUS, (repeats - REPEAT_COUNT_MIN) as u8, 1),
        }
        self.repeats = 0;
    }

    fn flush_zeros(&mut self) {
        match self.zeros {
            0 => return,
            zeros if zeros < REPEAT_COUNT_MIN => self.emit(0, 0, zeros),
            zeros if zeros <= REPEAT_ZEROS_SHORT_COUNT_MAX => {
                self.emit(REPEAT_ZEROS_SHORT, (zeros - REPEAT_COUNT_MIN) as u8, 1)
            }
            zeros => self.emit(
                REPEAT_ZEROS_LONG,
                (zeros - REPEAT_ZEROS_LONG_COUNT_MIN) as u8,
                1,
            ),
        }
        self.zeros = 0;
    }
}

impl Compressor {
    fn flush_block(&mut self, last: bool, sink: &mut Sink) -> bool {
        let start = self.writer.len();
        if self.blocks == 0
            && let Format::Zlib = self.format
        {
            let [method, flags] = self.zlib_header();
            self.writer.put(u32::from(method), BYTE_BITS);
            self.writer.put(u32::from(flags), BYTE_BITS);
        }
        self.writer.put(u32::from(last), 1);
        let mark = self.writer.mark();
        let marked = self.writer.len();
        let total = self.block.total_bytes;
        if total < DYNAMIC_BLOCK_BYTES_MIN {
            self.static_header();
        } else {
            self.dynamic_header();
        }
        self.write_codes();
        let dictionary = &self.dictionary;
        let windowed = dictionary.lookahead_position - dictionary.block_start <= dictionary.size;
        if total > STORED_BLOCK_BYTES_MIN
            && self.writer.len() - marked + 1 >= total as usize
            && windowed
        {
            self.stored_block(mark, total);
        }
        if last {
            self.writer.align();
            if let Format::Zlib = self.format {
                for byte in self.adler.to_be_bytes() {
                    self.writer.put(u32::from(byte), BYTE_BITS);
                }
            }
        }
        self.block.reset();
        self.dictionary.block_start += total as usize;
        self.blocks += 1;
        sink.block(self.writer.len() - start)
    }

    fn zlib_header(&self) -> [u8; 2] {
        let probes = LEVEL_PROBES[usize::from(self.level)];
        let level: u32 = match self.parsing {
            Parsing::Fast | Parsing::Greedy if probes <= 1 => 0,
            Parsing::Fast | Parsing::Greedy => 1,
            Parsing::Lazy if probes >= ZLIB_BEST_PROBES => 3,
            Parsing::Lazy => 2,
        };
        let method = u32::from(ZLIB_METHOD_AND_WINDOW);
        let flags = level << ZLIB_LEVEL_SHIFT;
        let remainder = ((method << BYTE_BITS) + flags) % ZLIB_CHECK_DIVISOR;
        [
            ZLIB_METHOD_AND_WINDOW,
            ((flags & 0xe0) + (ZLIB_CHECK_DIVISOR - remainder)) as u8,
        ]
    }

    fn stored_block(&mut self, mark: Mark, total: u32) {
        let writer = &mut self.writer;
        writer.rewind(mark);
        writer.put(0, BLOCK_TYPE_BITS);
        writer.align();
        writer.put(total & 0xffff, STORED_LENGTH_BITS);
        writer.put(!total & 0xffff, STORED_LENGTH_BITS);
        assert!(writer.count == 0);
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
        let Compressor {
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
