use nanocode_compression::checksum::{adler32, crc32};
use nanocode_compression::miniz::{self, Format, Level};

use crate::Color;
use crate::filter::adaptive;

const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n'];
const HEADER: [u8; 4] = *b"IHDR";
const ICC_PROFILE: [u8; 4] = *b"iCCP";
const EXIF: [u8; 4] = *b"eXIf";
const DATA: [u8; 4] = *b"IDAT";
const END: [u8; 4] = *b"IEND";
const ICC_PROFILE_NAME: &[u8] = b"_";
const ICC_COMPRESSION_METHOD: u8 = 0;
const DATA_CHUNK_BYTES_MAX: usize = (u32::MAX >> 1) as usize;
const LITERAL_SYMBOLS: usize = 286;
const END_OF_BLOCK: usize = 256;
const LONGEST_MATCH: u32 = 258;
const SHORT_RUN_MAX: u32 = 4;
const MATCH_LENGTH_MIN: u32 = 3;
const DISTANCE_CODE_BITS: u8 = 1;
const STORED_BLOCK_BYTES_MAX: usize = u16::MAX as usize;
const STORED_BLOCK_HEADER_BYTES: usize = 5;
const ZLIB_HEADER: [u8; 2] = [0x78, 0x01];
const ZLIB_TRAILER_BYTES: usize = 4;
const ADLER32_INITIAL: u32 = 1;
const BLOCK_HEADER: [u8; 54] = [
    120, 1, 237, 192, 3, 160, 36, 89, 150, 198, 241, 255, 119, 238, 141, 200, 204, 167, 114, 75,
    99, 174, 109, 219, 182, 109, 219, 182, 109, 219, 182, 109, 105, 140, 158, 150, 74, 175, 158,
    50, 51, 34, 238, 249, 118, 183, 106, 122, 166, 135, 59, 107, 213, 15,
];
const BLOCK_HEADER_TAIL_BITS: u8 = 5;
const LENGTH_BASES: [u32; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258,
];
const LENGTH_EXTRA_BITS: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
const FIRST_LENGTH_SYMBOL: usize = 257;
const LITERAL_LENGTHS: [u8; LITERAL_SYMBOLS] = [
    2, 3, 4, 5, 5, 6, 6, 7, 7, 7, 8, 8, 8, 8, 8, 9, 9, 9, 9, 9, 9, 9, 10, 10, 10, 10, 10, 10, 10,
    10, 10, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12,
    12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12,
    12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12,
    12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12,
    12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12,
    12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12,
    12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12,
    12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 11, 11, 11, 11, 11, 11, 11,
    11, 11, 11, 10, 11, 10, 10, 10, 10, 10, 10, 10, 10, 10, 9, 9, 9, 9, 9, 8, 9, 8, 8, 8, 8, 8, 7,
    7, 7, 6, 6, 6, 5, 4, 3, 12, 12, 12, 9, 9, 11, 10, 11, 11, 10, 11, 11, 11, 11, 11, 11, 12, 11,
    12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 9,
];
const LITERAL_CODES: [u16; LITERAL_SYMBOLS] = canonical_codes(&LITERAL_LENGTHS);

const fn canonical_codes(lengths: &[u8; LITERAL_SYMBOLS]) -> [u16; LITERAL_SYMBOLS] {
    let mut codes = [0u16; LITERAL_SYMBOLS];
    let mut code = 0u32;
    let mut length = 1;
    while length <= 16 {
        let mut symbol = 0;
        while symbol < LITERAL_SYMBOLS {
            if lengths[symbol] == length {
                codes[symbol] = (code as u16).reverse_bits() >> (16 - length);
                code += 1;
            }
            symbol += 1;
        }
        code <<= 1;
        length += 1;
    }
    assert!(code == 2 << 16);
    codes
}

pub enum Pixels<'a> {
    Eight(&'a [u8]),
    Sixteen(&'a [u16]),
}

pub struct Encoding<'a> {
    pub width: u32,
    pub height: u32,
    pub color: Color,
    pub pixels: Pixels<'a>,
    pub icc_profile: Option<&'a [u8]>,
    pub exif: Option<&'a [u8]>,
}

const fn color_type(color: Color) -> u8 {
    match color {
        Color::L8 | Color::L16 => 0,
        Color::Rgb8 | Color::Rgb16 => 2,
        Color::La8 | Color::La16 => 4,
        Color::Rgba8 | Color::Rgba16 => 6,
    }
}

fn write_chunk(output: &mut Vec<u8>, kind: [u8; 4], data: &[u8]) {
    let length = u32::try_from(data.len()).expect("a PNG chunk holds less than 4 GiB");
    output.extend_from_slice(&length.to_be_bytes());
    output.extend_from_slice(&kind);
    output.extend_from_slice(data);
    output.extend_from_slice(&crc32(crc32(0, &kind), data).to_be_bytes());
}

pub fn encode(encoding: &Encoding<'_>) -> Vec<u8> {
    let row_bytes = encoding.width as usize * encoding.color.bytes_per_pixel();
    let rows = encoding.height as usize;
    let bytes = match encoding.pixels {
        Pixels::Eight(samples) => {
            assert_eq!(encoding.color.bytes_per_sample(), 1);
            std::borrow::Cow::Borrowed(samples)
        }
        Pixels::Sixteen(samples) => {
            assert_eq!(encoding.color.bytes_per_sample(), 2);
            std::borrow::Cow::Owned(
                samples
                    .iter()
                    .flat_map(|sample| sample.to_be_bytes())
                    .collect(),
            )
        }
    };
    assert_eq!(bytes.len(), row_bytes * rows);
    let mut output = Vec::with_capacity(bytes.len() / 2);
    output.extend_from_slice(&SIGNATURE);
    let mut header = [0u8; 13];
    header[..4].copy_from_slice(&encoding.width.to_be_bytes());
    header[4..8].copy_from_slice(&encoding.height.to_be_bytes());
    header[8] = 8 * encoding.color.bytes_per_sample() as u8;
    header[9] = color_type(encoding.color);
    write_chunk(&mut output, HEADER, &header);
    if let Some(profile) = encoding.icc_profile {
        let mut data = Vec::with_capacity(ICC_PROFILE_NAME.len() + 2 + profile.len() * 3 / 4);
        data.extend_from_slice(ICC_PROFILE_NAME);
        data.push(0);
        data.push(ICC_COMPRESSION_METHOD);
        data.extend_from_slice(&miniz::compress(profile, Level::DEFAULT, Format::Zlib));
        write_chunk(&mut output, ICC_PROFILE, &data);
    }
    if let Some(exif) = encoding.exif {
        write_chunk(&mut output, EXIF, exif);
    }
    let stride = encoding.color.bytes_per_pixel();
    let compressed = compress_rows(&bytes, row_bytes, rows, stride);
    for part in compressed.chunks(DATA_CHUNK_BYTES_MAX) {
        write_chunk(&mut output, DATA, part);
    }
    write_chunk(&mut output, END, &[]);
    output
}

fn compress_rows(bytes: &[u8], row_bytes: usize, rows: usize, stride: usize) -> Vec<u8> {
    let mut compressor = Fast::new(bytes.len() / 4);
    let zeros = vec![0u8; row_bytes];
    let mut previous = zeros.as_slice();
    let mut current = vec![0u8; row_bytes + 1];
    for row in bytes.chunks_exact(row_bytes.max(1)).take(rows) {
        let filter = adaptive(stride, previous, row, &mut current[1..]);
        current[0] = filter as u8;
        compressor.write(&current);
        previous = row;
    }
    let compressed = compressor.finish();
    if compressed.len() <= stored_size((row_bytes + 1) * rows) {
        return compressed;
    }
    let mut stored = Stored::new();
    for row in bytes.chunks_exact(row_bytes.max(1)).take(rows) {
        stored.write(&[0]);
        stored.write(row);
    }
    stored.finish()
}

const fn stored_size(raw: usize) -> usize {
    (raw.saturating_sub(1) / STORED_BLOCK_BYTES_MAX)
        * (STORED_BLOCK_BYTES_MAX + STORED_BLOCK_HEADER_BYTES)
        + (raw % STORED_BLOCK_BYTES_MAX + STORED_BLOCK_HEADER_BYTES)
        + ZLIB_HEADER.len()
        + ZLIB_TRAILER_BYTES
}

struct Fast {
    output: Vec<u8>,
    buffer: u64,
    bits: u32,
    checksum: u32,
}

impl Fast {
    fn new(capacity: usize) -> Self {
        let mut fast = Self {
            output: Vec::with_capacity(capacity),
            buffer: 0,
            bits: 0,
            checksum: ADLER32_INITIAL,
        };
        let (whole, tail) = BLOCK_HEADER.split_at(BLOCK_HEADER.len() - 1);
        fast.output.extend_from_slice(whole);
        fast.put(u64::from(tail[0]), BLOCK_HEADER_TAIL_BITS);
        fast
    }

    fn put(&mut self, value: u64, count: u8) {
        let count = u32::from(count);
        debug_assert!(count <= 64);
        debug_assert!(count == 64 || value >> count == 0);
        if count == 0 {
            return;
        }
        self.buffer |= value << self.bits;
        self.bits += count;
        if self.bits >= 64 {
            self.output.extend_from_slice(&self.buffer.to_le_bytes());
            self.bits -= 64;
            self.buffer = value.checked_shr(count - self.bits).unwrap_or(0);
        }
    }

    fn literal(&mut self, byte: u8) {
        let symbol = usize::from(byte);
        self.put(u64::from(LITERAL_CODES[symbol]), LITERAL_LENGTHS[symbol]);
    }

    fn run(&mut self, mut length: u32) {
        self.literal(0);
        length -= 1;
        while length >= LONGEST_MATCH {
            let symbol = LITERAL_SYMBOLS - 1;
            self.put(
                u64::from(LITERAL_CODES[symbol]),
                LITERAL_LENGTHS[symbol] + DISTANCE_CODE_BITS,
            );
            length -= LONGEST_MATCH;
        }
        if length > SHORT_RUN_MAX {
            let index = LENGTH_BASES.partition_point(|&base| base <= length) - 1;
            let symbol = FIRST_LENGTH_SYMBOL + index;
            self.put(u64::from(LITERAL_CODES[symbol]), LITERAL_LENGTHS[symbol]);
            let extra_bits = LENGTH_EXTRA_BITS[index];
            let extra = u64::from((length - MATCH_LENGTH_MIN) & ((1u32 << extra_bits) - 1));
            self.put(extra, extra_bits + DISTANCE_CODE_BITS);
        } else {
            self.put(0, length as u8 * LITERAL_LENGTHS[0]);
        }
    }

    fn write(&mut self, data: &[u8]) {
        self.checksum = adler32(self.checksum, data);
        let mut run = 0u32;
        let (words, remainder) = data.as_chunks::<8>();
        for word in words {
            let value = u64::from_le_bytes(*word);
            if value == 0 {
                run += 8;
                continue;
            }
            if run > 0 {
                let run_extra = value.trailing_zeros() / 8;
                self.run(run + run_extra);
                run = 0;
                if run_extra > 0 {
                    run = value.leading_zeros() / 8;
                    for &byte in &word[run_extra as usize..8 - run as usize] {
                        self.literal(byte);
                    }
                    continue;
                }
            }
            let run_start = value.leading_zeros() / 8;
            if run_start > 0 {
                for &byte in &word[..8 - run_start as usize] {
                    self.literal(byte);
                }
                run = run_start;
                continue;
            }
            self.four(&word[..4]);
            self.four(&word[4..]);
        }
        if run > 0 {
            self.run(run);
        }
        for &byte in remainder {
            self.literal(byte);
        }
    }

    fn four(&mut self, bytes: &[u8]) {
        let mut value = 0u64;
        let mut count = 0u8;
        for &byte in bytes {
            let symbol = usize::from(byte);
            value |= u64::from(LITERAL_CODES[symbol]) << count;
            count += LITERAL_LENGTHS[symbol];
        }
        self.put(value, count);
    }

    fn finish(mut self) -> Vec<u8> {
        self.put(
            u64::from(LITERAL_CODES[END_OF_BLOCK]),
            LITERAL_LENGTHS[END_OF_BLOCK],
        );
        if !self.bits.is_multiple_of(8) {
            self.put(0, (8 - self.bits % 8) as u8);
        }
        let bytes = self.bits as usize / 8;
        self.output
            .extend_from_slice(&self.buffer.to_le_bytes()[..bytes]);
        self.output.extend_from_slice(&self.checksum.to_be_bytes());
        self.output
    }
}

struct Stored {
    output: Vec<u8>,
    block_start: usize,
    block_bytes: usize,
    checksum: u32,
}

impl Stored {
    fn new() -> Self {
        let mut output = Vec::new();
        output.extend_from_slice(&ZLIB_HEADER);
        let block_start = output.len();
        output.extend_from_slice(&[0; STORED_BLOCK_HEADER_BYTES]);
        Self {
            output,
            block_start,
            block_bytes: 0,
            checksum: ADLER32_INITIAL,
        }
    }

    fn close_block(&mut self, last: bool) {
        let size =
            u16::try_from(self.block_bytes).expect("a stored block holds at most 65,535 bytes");
        let header =
            &mut self.output[self.block_start..self.block_start + STORED_BLOCK_HEADER_BYTES];
        header[0] = u8::from(last);
        header[1..3].copy_from_slice(&size.to_le_bytes());
        header[3..5].copy_from_slice(&(!size).to_le_bytes());
    }

    fn write(&mut self, mut data: &[u8]) {
        self.checksum = adler32(self.checksum, data);
        while !data.is_empty() {
            if self.block_bytes == STORED_BLOCK_BYTES_MAX {
                self.close_block(false);
                self.block_start = self.output.len();
                self.output
                    .extend_from_slice(&[0; STORED_BLOCK_HEADER_BYTES]);
                self.block_bytes = 0;
            }
            let take = data.len().min(STORED_BLOCK_BYTES_MAX - self.block_bytes);
            self.output.extend_from_slice(&data[..take]);
            self.block_bytes += take;
            data = &data[take..];
        }
    }

    fn finish(mut self) -> Vec<u8> {
        self.close_block(true);
        self.output.extend_from_slice(&self.checksum.to_be_bytes());
        self.output
    }
}
