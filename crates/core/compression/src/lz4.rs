use alloc::vec::Vec;
use core::fmt;

const SIZE_PREFIX_BYTES: usize = 4;
const MATCH_LENGTH_MIN: usize = 4;
const LAST_LITERALS_BYTES: usize = 5;
const MATCH_SEARCH_END_BYTES: usize = 12;
const MATCH_END_BYTES: usize = LAST_LITERALS_BYTES + 1;
const INPUT_COMPRESSIBLE_BYTES_MIN: usize = MATCH_SEARCH_END_BYTES + 1;
const DISTANCE_MAX: usize = u16::MAX as usize;
const SMALL_INPUT_BYTES_LIMIT: usize = u16::MAX as usize;
const TABLE_ENTRIES: usize = 4096;
const TABLE_INDEX_SHIFT: u32 = 4;
const HASH4_MULTIPLIER: u32 = 2_654_435_761;
const HASH4_SHIFT: u32 = 16;
const HASH5_MULTIPLIER: u64 = 889_523_592_379;
const HASH5_INPUT_SHIFT: u32 = 24;
const HASH5_SHIFT: u32 = 48;
const SKIP_STEP_SHIFT: u32 = 5;
const NIBBLE_MAX: usize = 0xF;
const LENGTH_BYTE_MAX: usize = 0xFF;
const LITERAL_NIBBLE_SHIFT: u32 = 4;
const WORD_BYTES: usize = 8;
const WILD_COPY_BYTES: usize = 16;
const OUTPUT_BOUND_FIXED_BYTES: usize = 20;
const OUTPUT_BOUND_PERCENT: u64 = 110;
const PERCENT: u64 = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lz4Error {
    MissingSizePrefix,
    ExpectedAnotherByte,
    LiteralOutOfBounds,
    OffsetZero,
    OffsetOutOfBounds,
    OutputOverflow,
    SizeMismatch { expected: usize, actual: usize },
}

impl fmt::Display for Lz4Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingSizePrefix => formatter.write_str("the input has no 4-byte size prefix"),
            Self::ExpectedAnotherByte => formatter.write_str("expected another byte, found none"),
            Self::LiteralOutOfBounds => {
                formatter.write_str("literal is out of bounds of the input")
            }
            Self::OffsetZero => formatter.write_str("0 is not a valid match offset"),
            Self::OffsetOutOfBounds => formatter
                .write_str("the offset to copy is not contained in the decompressed buffer"),
            Self::OutputOverflow => {
                formatter.write_str("the decompressed data is longer than its size prefix")
            }
            Self::SizeMismatch { expected, actual } => write!(
                formatter,
                "the size prefix says {expected} bytes but {actual} were decompressed"
            ),
        }
    }
}

#[derive(Clone, Copy)]
enum Hash {
    FourBytes,
    FiveBytes,
}

impl Hash {
    fn at(self, input: &[u8], position: usize) -> usize {
        let hash = match self {
            Self::FourBytes => {
                let word = u32::from_le_bytes(read_array(input, position));
                word.wrapping_mul(HASH4_MULTIPLIER) >> HASH4_SHIFT
            }
            Self::FiveBytes => {
                let word = u64::from_le_bytes(read_array(input, position));
                let hash =
                    (word << HASH5_INPUT_SHIFT).wrapping_mul(HASH5_MULTIPLIER) >> HASH5_SHIFT;
                u32::try_from(hash).expect("a 64-bit value shifted right by 48 fits in 32 bits")
            }
        };
        usize::try_from(hash >> TABLE_INDEX_SHIFT).expect("a table index fits in usize")
    }
}

fn read_array<const N: usize>(input: &[u8], position: usize) -> [u8; N] {
    let mut bytes = [0; N];
    bytes.copy_from_slice(&input[position..position + N]);
    bytes
}

pub fn compress_with_size(input: &[u8]) -> Vec<u8> {
    let size = u32::try_from(input.len()).expect("an LZ4 block with a size prefix is under 4 GiB");
    let mut output = Vec::with_capacity(SIZE_PREFIX_BYTES + output_bound(input.len()));
    output.extend_from_slice(&size.to_le_bytes());
    compress_block(input, &mut output);
    output
}

fn output_bound(input_bytes: usize) -> usize {
    let input_bytes = u64::try_from(input_bytes).expect("a slice length fits in 64 bits");
    let scaled = usize::try_from(input_bytes * OUTPUT_BOUND_PERCENT / PERCENT)
        .expect("the output bound of an in-memory slice fits in usize");
    OUTPUT_BOUND_FIXED_BYTES + scaled
}

fn compress_block(input: &[u8], output: &mut Vec<u8>) {
    if input.len() < INPUT_COMPRESSIBLE_BYTES_MIN {
        write_last_literals(output, input);
        return;
    }
    let hash = if input.len() < SMALL_INPUT_BYTES_LIMIT {
        Hash::FourBytes
    } else {
        Hash::FiveBytes
    };
    let mut table = alloc::vec![0_usize; TABLE_ENTRIES];
    let search_end = input.len() - MATCH_SEARCH_END_BYTES;
    let mut literal_start = 0;
    table[hash.at(input, 0)] = 0;
    let mut cursor = 1;
    loop {
        let Some((found, candidate)) = find_match(input, hash, &mut table, cursor, search_end)
        else {
            write_last_literals(output, &input[literal_start..]);
            return;
        };
        let (start, candidate) = extend_backwards(input, found, candidate, literal_start);
        let offset = start - candidate;
        let match_start = start + MATCH_LENGTH_MIN;
        let extra_bytes = common_prefix(
            &input[match_start..input.len() - MATCH_END_BYTES],
            &input[candidate + MATCH_LENGTH_MIN..],
        );
        cursor = match_start + extra_bytes;
        table[hash.at(input, cursor - 2)] = cursor - 2;
        write_sequence(output, &input[literal_start..start], offset, extra_bytes);
        literal_start = cursor;
    }
}

fn find_match(
    input: &[u8],
    hash: Hash,
    table: &mut [usize],
    start: usize,
    search_end: usize,
) -> Option<(usize, usize)> {
    let mut misses = 1_usize << SKIP_STEP_SHIFT;
    let mut next = start;
    loop {
        let cursor = next;
        next += misses >> SKIP_STEP_SHIFT;
        misses += 1;
        if cursor > search_end {
            return None;
        }
        let slot = hash.at(input, cursor);
        let candidate = table[slot];
        table[slot] = cursor;
        assert!(candidate < cursor);
        if cursor - candidate > DISTANCE_MAX {
            continue;
        }
        if input[candidate..candidate + MATCH_LENGTH_MIN]
            == input[cursor..cursor + MATCH_LENGTH_MIN]
        {
            return Some((cursor, candidate));
        }
    }
}

const fn extend_backwards(
    input: &[u8],
    mut cursor: usize,
    mut candidate: usize,
    literal_start: usize,
) -> (usize, usize) {
    while candidate > 0 && cursor > literal_start && input[cursor - 1] == input[candidate - 1] {
        cursor -= 1;
        candidate -= 1;
    }
    (cursor, candidate)
}

fn common_prefix(left: &[u8], right: &[u8]) -> usize {
    let mut count = 0;
    let (left_words, _) = left.as_chunks::<WORD_BYTES>();
    let (right_words, _) = right.as_chunks::<WORD_BYTES>();
    for (left_word, right_word) in left_words.iter().zip(right_words) {
        let difference = u64::from_le_bytes(*left_word) ^ u64::from_le_bytes(*right_word);
        if difference != 0 {
            let equal_bytes = difference.trailing_zeros() / u8::BITS;
            return count
                + usize::try_from(equal_bytes).expect("a byte count under 8 fits in usize");
        }
        count += WORD_BYTES;
    }
    count
        + left[count..]
            .iter()
            .zip(&right[count..])
            .take_while(|(left, right)| left == right)
            .count()
}

fn write_sequence(output: &mut Vec<u8>, literals: &[u8], offset: usize, extra_bytes: usize) {
    let literal_nibble = literals.len().min(NIBBLE_MAX);
    let match_nibble = extra_bytes.min(NIBBLE_MAX);
    output.push(token(literal_nibble, match_nibble));
    if literals.len() >= NIBBLE_MAX {
        write_length(output, literals.len() - NIBBLE_MAX);
    }
    output.extend_from_slice(literals);
    let offset = u16::try_from(offset).expect("a match offset is at most 65535");
    output.extend_from_slice(&offset.to_le_bytes());
    if extra_bytes >= NIBBLE_MAX {
        write_length(output, extra_bytes - NIBBLE_MAX);
    }
}

fn write_last_literals(output: &mut Vec<u8>, literals: &[u8]) {
    output.push(token(literals.len().min(NIBBLE_MAX), 0));
    if literals.len() >= NIBBLE_MAX {
        write_length(output, literals.len() - NIBBLE_MAX);
    }
    output.extend_from_slice(literals);
}

fn token(literal_nibble: usize, match_nibble: usize) -> u8 {
    let literal = u8::try_from(literal_nibble).expect("a nibble fits in a byte");
    let matched = u8::try_from(match_nibble).expect("a nibble fits in a byte");
    (literal << LITERAL_NIBBLE_SHIFT) | matched
}

fn write_length(output: &mut Vec<u8>, mut length: usize) {
    while length >= LENGTH_BYTE_MAX {
        length -= LENGTH_BYTE_MAX;
        output.push(u8::MAX);
    }
    output.push(u8::try_from(length).expect("a remainder under 255 fits in a byte"));
}

pub fn decompress_with_size(input: &[u8]) -> Result<Vec<u8>, Lz4Error> {
    let Some((prefix, block)) = input.split_first_chunk::<SIZE_PREFIX_BYTES>() else {
        return Err(Lz4Error::MissingSizePrefix);
    };
    let expected = usize::try_from(u32::from_le_bytes(*prefix)).expect("a u32 fits in usize");
    let mut output = alloc::vec![0; expected];
    let written = decompress_block(block, &mut output)?;
    if written != expected {
        return Err(Lz4Error::SizeMismatch {
            expected,
            actual: written,
        });
    }
    Ok(output)
}

fn decompress_block(block: &[u8], output: &mut [u8]) -> Result<usize, Lz4Error> {
    let mut position = 0;
    let mut written = 0;
    loop {
        let token = *block.get(position).ok_or(Lz4Error::ExpectedAnotherByte)?;
        position += 1;
        let literal_nibble = usize::from(token >> LITERAL_NIBBLE_SHIFT);
        let literal_bytes = read_length(block, &mut position, literal_nibble)?;
        let literals = block
            .get(position..position.saturating_add(literal_bytes))
            .ok_or(Lz4Error::LiteralOutOfBounds)?;
        if literal_bytes > output.len() - written {
            return Err(Lz4Error::OutputOverflow);
        }
        copy_literals(output, written, block, position, literals);
        written += literal_bytes;
        position += literal_bytes;
        if position == block.len() {
            return Ok(written);
        }
        let offset_bytes = block
            .get(position..position + 2)
            .ok_or(Lz4Error::ExpectedAnotherByte)?;
        let offset = usize::from(u16::from_le_bytes([offset_bytes[0], offset_bytes[1]]));
        position += 2;
        if offset == 0 {
            return Err(Lz4Error::OffsetZero);
        }
        if offset > written {
            return Err(Lz4Error::OffsetOutOfBounds);
        }
        let match_nibble = usize::from(token) & NIBBLE_MAX;
        let match_bytes = read_length(block, &mut position, match_nibble)? + MATCH_LENGTH_MIN;
        if match_bytes > output.len() - written {
            return Err(Lz4Error::OutputOverflow);
        }
        copy_match(output, written, offset, match_bytes);
        written += match_bytes;
    }
}

fn copy_literals(
    output: &mut [u8],
    written: usize,
    block: &[u8],
    position: usize,
    literals: &[u8],
) {
    if literals.len() <= WILD_COPY_BYTES
        && block.len() - position >= WILD_COPY_BYTES
        && output.len() - written >= WILD_COPY_BYTES
    {
        let wild: [u8; WILD_COPY_BYTES] = read_array(block, position);
        output[written..written + WILD_COPY_BYTES].copy_from_slice(&wild);
        return;
    }
    output[written..written + literals.len()].copy_from_slice(literals);
}

fn copy_match(output: &mut [u8], written: usize, offset: usize, match_bytes: usize) {
    let source = written - offset;
    if match_bytes <= WILD_COPY_BYTES
        && offset >= WILD_COPY_BYTES
        && output.len() - written >= WILD_COPY_BYTES
    {
        let wild: [u8; WILD_COPY_BYTES] = read_array(output, source);
        output[written..written + WILD_COPY_BYTES].copy_from_slice(&wild);
        return;
    }
    if offset >= match_bytes {
        output.copy_within(source..source + match_bytes, written);
        return;
    }
    let mut copied = 0;
    while copied < match_bytes {
        let copy_bytes = (match_bytes - copied).min(offset + copied);
        output.copy_within(source..source + copy_bytes, written + copied);
        copied += copy_bytes;
    }
}

fn read_length(block: &[u8], position: &mut usize, nibble: usize) -> Result<usize, Lz4Error> {
    let mut length = nibble;
    if nibble != NIBBLE_MAX {
        return Ok(length);
    }
    loop {
        let byte = *block.get(*position).ok_or(Lz4Error::ExpectedAnotherByte)?;
        *position += 1;
        length = length
            .checked_add(usize::from(byte))
            .ok_or(Lz4Error::OutputOverflow)?;
        if usize::from(byte) != LENGTH_BYTE_MAX {
            return Ok(length);
        }
    }
}
