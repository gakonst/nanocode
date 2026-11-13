use std::fmt;

const PAD: u8 = b'=';
const INVALID: u8 = 0xff;
const SEXTET_MASK: u8 = 0x3f;
const SEXTET_BITS: u32 = 6;
const GROUP_BYTES: usize = 3;
const GROUP_CHARACTERS: usize = 4;
const STANDARD_SYMBOLS: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
const URL_SAFE_SYMBOLS: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
const STANDARD_DECODE: [u8; 256] = decode_table(STANDARD_SYMBOLS);
const URL_SAFE_DECODE: [u8; 256] = decode_table(URL_SAFE_SYMBOLS);
const DECODE_CHUNK_CHARACTERS: usize = 8;
const DECODE_CHUNK_BYTES: usize = 6;
const WORD_BYTES: usize = 8;
const ENCODE_WORDS: usize = 4;
const ENCODE_WORD_BYTES: usize = 6;
const ENCODE_WORD_CHARACTERS: usize = 8;
const ENCODE_READ_BYTES: usize = (ENCODE_WORDS - 1) * ENCODE_WORD_BYTES + WORD_BYTES;
const ENCODE_WRITE_BYTES: usize = ENCODE_WORDS * ENCODE_WORD_CHARACTERS;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Alphabet {
    Standard,
    UrlSafe,
}

impl Alphabet {
    const fn symbols(self) -> &'static [u8; 64] {
        match self {
            Self::Standard => STANDARD_SYMBOLS,
            Self::UrlSafe => URL_SAFE_SYMBOLS,
        }
    }

    const fn decode_table(self) -> &'static [u8; 256] {
        match self {
            Self::Standard => &STANDARD_DECODE,
            Self::UrlSafe => &URL_SAFE_DECODE,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecodePadding {
    Canonical,
    Indifferent,
    Forbidden,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Engine {
    alphabet: Alphabet,
    encode_padding: bool,
    decode_padding: DecodePadding,
    decode_trailing_bits: bool,
}

pub const STANDARD: Engine = Engine::new(Alphabet::Standard, true, DecodePadding::Canonical, false);
pub const STANDARD_NO_PAD: Engine = Engine::new(Alphabet::Standard, false, DecodePadding::Forbidden, false);
pub const URL_SAFE: Engine = Engine::new(Alphabet::UrlSafe, true, DecodePadding::Canonical, false);
pub const URL_SAFE_NO_PAD: Engine = Engine::new(Alphabet::UrlSafe, false, DecodePadding::Forbidden, false);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DecodeError {
    InvalidByte(usize, u8),
    InvalidLength(usize),
    InvalidLastSymbol(usize, u8),
    InvalidPadding,
}

impl fmt::Display for DecodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::InvalidByte(index, byte) => write!(formatter, "Invalid symbol {byte}, offset {index}."),
            Self::InvalidLength(length) => write!(formatter, "Invalid input length: {length}"),
            Self::InvalidLastSymbol(index, byte) => {
                write!(formatter, "Invalid last symbol {byte}, offset {index}.")
            }
            Self::InvalidPadding => write!(formatter, "Invalid padding"),
        }
    }
}

impl std::error::Error for DecodeError {}

pub const fn encoded_length(bytes: usize, padding: bool) -> Option<usize> {
    let remainder = bytes % GROUP_BYTES;
    let Some(complete) = (bytes / GROUP_BYTES).checked_mul(GROUP_CHARACTERS) else {
        return None;
    };
    if remainder == 0 {
        Some(complete)
    } else if padding {
        complete.checked_add(GROUP_CHARACTERS)
    } else {
        complete.checked_add(remainder + 1)
    }
}

impl Engine {
    pub const fn new(
        alphabet: Alphabet,
        encode_padding: bool,
        decode_padding: DecodePadding,
        decode_trailing_bits: bool,
    ) -> Self {
        Self {
            alphabet,
            encode_padding,
            decode_padding,
            decode_trailing_bits,
        }
    }

    pub fn encode(&self, input: impl AsRef<[u8]>) -> String {
        let mut output = String::new();
        self.encode_string(input, &mut output);
        output
    }

    pub fn encode_string(&self, input: impl AsRef<[u8]>, output: &mut String) {
        let input = input.as_ref();
        let length = encoded_length(input.len(), self.encode_padding)
            .expect("integer overflow when calculating buffer size");
        let start = output.len();
        let end = start.checked_add(length).expect("an encoded string fits in usize");
        let bytes = unsafe { output.as_mut_vec() };
        bytes.resize(end, 0);
        let written = self.encode_into(input, &mut bytes[start..]);
        if written != length || !bytes[start..].is_ascii() {
            bytes.truncate(start);
            panic!("the encoder fills the encoded length with alphabet symbols and padding");
        }
    }

    fn encode_into(&self, input: &[u8], output: &mut [u8]) -> usize {
        let symbols = self.alphabet.symbols();
        let consumed = encode_blocks(input, output, self.alphabet);
        assert_eq!(consumed % GROUP_BYTES, 0, "the block encoder takes whole groups");
        let mut written = consumed / GROUP_BYTES * GROUP_CHARACTERS;
        let mut position = consumed;
        while input.len() - position >= ENCODE_READ_BYTES {
            let chunk: &[u8; ENCODE_READ_BYTES] =
                input[position..position + ENCODE_READ_BYTES].try_into().expect("a fast chunk");
            let target: &mut [u8; ENCODE_WRITE_BYTES] =
                (&mut output[written..written + ENCODE_WRITE_BYTES]).try_into().expect("a fast target");
            for part in 0..ENCODE_WORDS {
                let start = part * ENCODE_WORD_BYTES;
                let word = u64::from_be_bytes(chunk[start..start + WORD_BYTES].try_into().expect("eight bytes"));
                for index in 0..ENCODE_WORD_CHARACTERS {
                    let shift = u64::BITS - SEXTET_BITS * (index as u32 + 1);
                    target[part * ENCODE_WORD_CHARACTERS + index] =
                        symbols[(word >> shift) as usize & usize::from(SEXTET_MASK)];
                }
            }
            position += ENCODE_WORDS * ENCODE_WORD_BYTES;
            written += ENCODE_WRITE_BYTES;
        }
        let (groups, remainder) = input[position..].as_chunks::<GROUP_BYTES>();
        let (targets, _) = output[written..].as_chunks_mut::<GROUP_CHARACTERS>();
        for (group, target) in groups.iter().zip(targets) {
            target[0] = symbols[usize::from(group[0] >> 2)];
            target[1] = symbols[usize::from((group[0] << 4 | group[1] >> 4) & SEXTET_MASK)];
            target[2] = symbols[usize::from((group[1] << 2 | group[2] >> 6) & SEXTET_MASK)];
            target[3] = symbols[usize::from(group[2] & SEXTET_MASK)];
        }
        written += groups.len() * GROUP_CHARACTERS;
        match *remainder {
            [first] => {
                output[written] = symbols[usize::from(first >> 2)];
                output[written + 1] = symbols[usize::from((first << 4) & SEXTET_MASK)];
                written += 2;
            }
            [first, second] => {
                output[written] = symbols[usize::from(first >> 2)];
                output[written + 1] = symbols[usize::from((first << 4 | second >> 4) & SEXTET_MASK)];
                output[written + 2] = symbols[usize::from((second << 2) & SEXTET_MASK)];
                written += 3;
            }
            _ => {}
        }
        if self.encode_padding {
            let padding = (GROUP_CHARACTERS - written % GROUP_CHARACTERS) % GROUP_CHARACTERS;
            output[written..written + padding].fill(PAD);
            written += padding;
        }
        written
    }

    pub fn decode(&self, input: impl AsRef<[u8]>) -> Result<Vec<u8>, DecodeError> {
        let mut output = Vec::new();
        self.decode_vec(input, &mut output)?;
        Ok(output)
    }

    pub fn decode_vec(&self, input: impl AsRef<[u8]>, output: &mut Vec<u8>) -> Result<(), DecodeError> {
        let input = input.as_ref();
        let table = self.alphabet.decode_table();
        let remainder = input.len() % GROUP_CHARACTERS;
        if remainder == 1 {
            let last = input[input.len() - 1];
            if last != PAD && table[usize::from(last)] == INVALID {
                return Err(DecodeError::InvalidByte(input.len() - 1, last));
            }
        }
        let body = input
            .len()
            .saturating_sub(remainder)
            .saturating_sub(usize::from(remainder == 0) * GROUP_CHARACTERS);
        let estimate = (input.len() / GROUP_CHARACTERS + usize::from(remainder > 0)) * GROUP_BYTES;
        let start = output.len();
        output.resize(start.checked_add(estimate).expect("a decoded buffer fits in usize"), 0);
        let target = &mut output[start..];
        let body_bytes = body / GROUP_CHARACTERS * GROUP_BYTES;
        let (body_target, suffix_target) = target.split_at_mut(body_bytes);
        let result = decode_body(&input[..body], body_target, self.alphabet)
            .and_then(|()| self.decode_suffix(input, body, suffix_target));
        match result {
            Ok(suffix_bytes) => {
                output.truncate(start + body_bytes + suffix_bytes);
                Ok(())
            }
            Err(error) => {
                output.truncate(start);
                Err(error)
            }
        }
    }

    fn decode_suffix(&self, input: &[u8], body: usize, output: &mut [u8]) -> Result<usize, DecodeError> {
        let table = self.alphabet.decode_table();
        let mut morsel_count = 0;
        let mut padding_count = 0;
        let mut first_padding = 0;
        let mut last_symbol = 0_u8;
        let mut morsels = [0_u8; GROUP_CHARACTERS];
        for (offset, &byte) in input[body..].iter().enumerate() {
            if byte == PAD {
                if offset < 2 {
                    return Err(DecodeError::InvalidByte(body + offset, byte));
                }
                if padding_count == 0 {
                    first_padding = offset;
                }
                padding_count += 1;
                continue;
            }
            if padding_count > 0 {
                return Err(DecodeError::InvalidByte(body + first_padding, PAD));
            }
            last_symbol = byte;
            let morsel = table[usize::from(byte)];
            if morsel == INVALID {
                return Err(DecodeError::InvalidByte(body + offset, byte));
            }
            morsels[morsel_count] = morsel;
            morsel_count += 1;
        }
        if !input.is_empty() && morsel_count < 2 {
            return Err(DecodeError::InvalidLength(body + morsel_count));
        }
        match self.decode_padding {
            DecodePadding::Indifferent => {}
            DecodePadding::Canonical => {
                if (padding_count + morsel_count) % GROUP_CHARACTERS != 0 {
                    return Err(DecodeError::InvalidPadding);
                }
            }
            DecodePadding::Forbidden => {
                if padding_count > 0 {
                    return Err(DecodeError::InvalidPadding);
                }
            }
        }
        let bytes = morsel_count * SEXTET_BITS as usize / u8::BITS as usize;
        let word = (u32::from(morsels[0]) << 26)
            | (u32::from(morsels[1]) << 20)
            | (u32::from(morsels[2]) << 14)
            | (u32::from(morsels[3]) << 8);
        let mask = u32::MAX >> (bytes * u8::BITS as usize);
        if !self.decode_trailing_bits && word & mask != 0 {
            return Err(DecodeError::InvalidLastSymbol(body + morsel_count - 1, last_symbol));
        }
        output[..bytes].copy_from_slice(&word.to_be_bytes()[..bytes]);
        Ok(bytes)
    }
}

fn decode_body(input: &[u8], output: &mut [u8], alphabet: Alphabet) -> Result<(), DecodeError> {
    assert_eq!(input.len() % GROUP_CHARACTERS, 0, "the body holds whole quads");
    assert_eq!(output.len(), input.len() / GROUP_CHARACTERS * GROUP_BYTES, "the body's output is sized");
    let consumed = decode_blocks(input, output, alphabet);
    assert_eq!(consumed % GROUP_CHARACTERS, 0, "the block decoder takes whole quads");
    let table = alphabet.decode_table();
    let mut position = consumed;
    let mut written = consumed / GROUP_CHARACTERS * GROUP_BYTES;
    let (chunks, _) = input[position..].as_chunks::<DECODE_CHUNK_CHARACTERS>();
    for chunk in chunks {
        let mut word = 0_u64;
        for (offset, &byte) in chunk.iter().enumerate() {
            let morsel = table[usize::from(byte)];
            if morsel == INVALID {
                return Err(DecodeError::InvalidByte(position + offset, byte));
            }
            word = word << SEXTET_BITS | u64::from(morsel);
        }
        let aligned = word << (u64::BITS - SEXTET_BITS * DECODE_CHUNK_CHARACTERS as u32);
        output[written..written + DECODE_CHUNK_BYTES].copy_from_slice(&aligned.to_be_bytes()[..DECODE_CHUNK_BYTES]);
        position += DECODE_CHUNK_CHARACTERS;
        written += DECODE_CHUNK_BYTES;
    }
    if position < input.len() {
        let mut word = 0_u32;
        for (offset, &byte) in input[position..position + GROUP_CHARACTERS].iter().enumerate() {
            let morsel = table[usize::from(byte)];
            if morsel == INVALID {
                return Err(DecodeError::InvalidByte(position + offset, byte));
            }
            word = word << SEXTET_BITS | u32::from(morsel);
        }
        output[written..written + GROUP_BYTES].copy_from_slice(&(word << u8::BITS).to_be_bytes()[..GROUP_BYTES]);
        position += GROUP_CHARACTERS;
    }
    assert_eq!(position, input.len(), "the body decodes every quad");
    Ok(())
}

const _: () = assert!(sextets_or_invalid(&STANDARD_DECODE) && sextets_or_invalid(&URL_SAFE_DECODE));

const fn sextets_or_invalid(table: &[u8; 256]) -> bool {
    let mut index = 0;
    while index < table.len() {
        if table[index] != INVALID && table[index] > SEXTET_MASK {
            return false;
        }
        index += 1;
    }
    true
}

const fn decode_table(symbols: &[u8; 64]) -> [u8; 256] {
    let mut table = [INVALID; 256];
    let mut index = 0;
    while index < symbols.len() {
        table[symbols[index] as usize] = index as u8;
        index += 1;
    }
    table
}

#[cfg(target_arch = "x86_64")]
fn simd() -> bool {
    std::is_x86_feature_detected!("avx2")
}

#[cfg(target_arch = "aarch64")]
const fn simd() -> bool {
    true
}

#[cfg(target_arch = "x86_64")]
fn encode_blocks(input: &[u8], output: &mut [u8], alphabet: Alphabet) -> usize {
    if simd() { x86::encode(input, output, alphabet) } else { 0 }
}

#[cfg(target_arch = "x86_64")]
fn decode_blocks(input: &[u8], output: &mut [u8], alphabet: Alphabet) -> usize {
    if simd() { x86::decode(input, output, alphabet) } else { 0 }
}

#[cfg(target_arch = "aarch64")]
fn encode_blocks(input: &[u8], output: &mut [u8], alphabet: Alphabet) -> usize {
    if simd() { arm::encode(input, output, alphabet) } else { 0 }
}

#[cfg(target_arch = "aarch64")]
fn decode_blocks(input: &[u8], output: &mut [u8], alphabet: Alphabet) -> usize {
    if simd() { arm::decode(input, output, alphabet) } else { 0 }
}

#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
const fn encode_blocks(_input: &[u8], _output: &mut [u8], _alphabet: Alphabet) -> usize {
    0
}

#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
const fn decode_blocks(_input: &[u8], _output: &mut [u8], _alphabet: Alphabet) -> usize {
    0
}

#[cfg(target_arch = "x86_64")]
mod x86 {
    use std::arch::x86_64::*;

    use super::Alphabet;

    const ENCODE_INPUT_BYTES: usize = 24;
    const ENCODE_READ_BYTES: usize = 28;
    const ENCODE_OUTPUT_BYTES: usize = 32;
    const DECODE_INPUT_BYTES: usize = 32;
    const DECODE_OUTPUT_BYTES: usize = 24;
    const LANE_BYTES: usize = 16;
    const UPPER_FIRST: i8 = b'A' as i8;
    const LOWER_STEP: i8 = (b'a' - b'A') as i8 - 26;
    const DIGIT_STEP: i8 = (b'0' as i8 - 52) - (b'a' as i8 - 26);
    const DIGIT_OFFSET: i8 = b'0' as i8 - 52;

    pub(super) fn encode(input: &[u8], output: &mut [u8], alphabet: Alphabet) -> usize {
        let readable = if input.len() >= ENCODE_READ_BYTES {
            (input.len() - ENCODE_READ_BYTES) / ENCODE_INPUT_BYTES + 1
        } else {
            0
        };
        let blocks = readable.min(output.len() / ENCODE_OUTPUT_BYTES);
        if blocks == 0 {
            return 0;
        }
        assert!((blocks - 1) * ENCODE_INPUT_BYTES + ENCODE_READ_BYTES <= input.len());
        assert!(blocks * ENCODE_OUTPUT_BYTES <= output.len());
        let symbols = alphabet.symbols();
        unsafe { encode_avx2(input.as_ptr(), output.as_mut_ptr(), blocks, symbols[62], symbols[63]) };
        blocks * ENCODE_INPUT_BYTES
    }

    #[target_feature(enable = "avx2")]
    unsafe fn encode_avx2(input: *const u8, output: *mut u8, blocks: usize, symbol62: u8, symbol63: u8) {
        let spread = _mm256_setr_epi8(
            1, 0, 2, 1, 4, 3, 5, 4, 7, 6, 8, 7, 10, 9, 11, 10, 1, 0, 2, 1, 4, 3, 5, 4, 7, 6, 8, 7, 10, 9, 11, 10,
        );
        let mask_high = _mm256_set1_epi32(0x0fc0_fc00);
        let multiply_high = _mm256_set1_epi32(0x0400_0040);
        let mask_low = _mm256_set1_epi32(0x003f_03f0);
        let multiply_low = _mm256_set1_epi32(0x0100_0010);
        let above25 = _mm256_set1_epi8(25);
        let above51 = _mm256_set1_epi8(51);
        let is62 = _mm256_set1_epi8(62);
        let is63 = _mm256_set1_epi8(63);
        let first = _mm256_set1_epi8(UPPER_FIRST);
        let lower = _mm256_set1_epi8(LOWER_STEP);
        let digit = _mm256_set1_epi8(DIGIT_STEP);
        let step62 = _mm256_set1_epi8((symbol62 as i8).wrapping_sub(62).wrapping_sub(DIGIT_OFFSET));
        let step63 = _mm256_set1_epi8((symbol63 as i8).wrapping_sub(63).wrapping_sub(DIGIT_OFFSET));
        for block in 0..blocks {
            let source = unsafe { input.add(block * ENCODE_INPUT_BYTES) };
            let low = unsafe { _mm_loadu_si128(source.cast()) };
            let high = unsafe { _mm_loadu_si128(source.add(ENCODE_INPUT_BYTES / 2).cast()) };
            let bytes = _mm256_shuffle_epi8(_mm256_set_m128i(high, low), spread);
            let upper_sextets = _mm256_mulhi_epu16(_mm256_and_si256(bytes, mask_high), multiply_high);
            let lower_sextets = _mm256_mullo_epi16(_mm256_and_si256(bytes, mask_low), multiply_low);
            let sextets = _mm256_or_si256(upper_sextets, lower_sextets);
            let mut offsets = first;
            offsets = _mm256_add_epi8(offsets, _mm256_and_si256(_mm256_cmpgt_epi8(sextets, above25), lower));
            offsets = _mm256_add_epi8(offsets, _mm256_and_si256(_mm256_cmpgt_epi8(sextets, above51), digit));
            offsets = _mm256_add_epi8(offsets, _mm256_and_si256(_mm256_cmpeq_epi8(sextets, is62), step62));
            offsets = _mm256_add_epi8(offsets, _mm256_and_si256(_mm256_cmpeq_epi8(sextets, is63), step63));
            let symbols = _mm256_add_epi8(sextets, offsets);
            unsafe { _mm256_storeu_si256(output.add(block * ENCODE_OUTPUT_BYTES).cast(), symbols) };
        }
    }

    pub(super) fn decode(input: &[u8], output: &mut [u8], alphabet: Alphabet) -> usize {
        let blocks = (input.len() / DECODE_INPUT_BYTES).min(output.len() / DECODE_OUTPUT_BYTES);
        if blocks == 0 {
            return 0;
        }
        assert!(blocks * DECODE_INPUT_BYTES <= input.len());
        assert!(blocks * DECODE_OUTPUT_BYTES <= output.len());
        let symbols = alphabet.symbols();
        let decoded = unsafe { decode_avx2(input.as_ptr(), output.as_mut_ptr(), blocks, symbols[62], symbols[63]) };
        assert!(decoded <= blocks);
        decoded * DECODE_INPUT_BYTES
    }

    #[target_feature(enable = "avx2")]
    fn in_range(characters: __m256i, low: u8, count: u8) -> __m256i {
        let shifted = _mm256_xor_si256(
            _mm256_sub_epi8(characters, _mm256_set1_epi8(low as i8)),
            _mm256_set1_epi8(i8::MIN),
        );
        _mm256_cmpgt_epi8(_mm256_set1_epi8(i8::MIN.wrapping_add(count as i8)), shifted)
    }

    #[target_feature(enable = "avx2")]
    unsafe fn decode_avx2(input: *const u8, output: *mut u8, blocks: usize, symbol62: u8, symbol63: u8) -> usize {
        let character62 = _mm256_set1_epi8(symbol62 as i8);
        let character63 = _mm256_set1_epi8(symbol63 as i8);
        let upper_offset = _mm256_set1_epi8(-(b'A' as i8));
        let lower_offset = _mm256_set1_epi8(26 - b'a' as i8);
        let digit_offset = _mm256_set1_epi8(-DIGIT_OFFSET);
        let offset62 = _mm256_set1_epi8(62_i8.wrapping_sub(symbol62 as i8));
        let offset63 = _mm256_set1_epi8(63_i8.wrapping_sub(symbol63 as i8));
        let merge_pairs = _mm256_set1_epi32(0x0140_0140);
        let merge_quads = _mm256_set1_epi32(0x0001_1000);
        let pack = _mm256_setr_epi8(
            2, 1, 0, 6, 5, 4, 10, 9, 8, 14, 13, 12, -1, -1, -1, -1, 2, 1, 0, 6, 5, 4, 10, 9, 8, 14, 13, 12, -1, -1,
            -1, -1,
        );
        let gather = _mm256_setr_epi32(0, 1, 2, 4, 5, 6, 3, 7);
        for block in 0..blocks {
            let characters = unsafe { _mm256_loadu_si256(input.add(block * DECODE_INPUT_BYTES).cast()) };
            let upper = in_range(characters, b'A', 26);
            let lower = in_range(characters, b'a', 26);
            let digit = in_range(characters, b'0', 10);
            let is62 = _mm256_cmpeq_epi8(characters, character62);
            let is63 = _mm256_cmpeq_epi8(characters, character63);
            let letters = _mm256_or_si256(upper, lower);
            let others = _mm256_or_si256(digit, _mm256_or_si256(is62, is63));
            if _mm256_movemask_epi8(_mm256_or_si256(letters, others)) != -1 {
                return block;
            }
            let letter_offsets = _mm256_or_si256(
                _mm256_and_si256(upper, upper_offset),
                _mm256_and_si256(lower, lower_offset),
            );
            let other_offsets = _mm256_or_si256(
                _mm256_and_si256(digit, digit_offset),
                _mm256_or_si256(_mm256_and_si256(is62, offset62), _mm256_and_si256(is63, offset63)),
            );
            let sextets = _mm256_add_epi8(characters, _mm256_or_si256(letter_offsets, other_offsets));
            let pairs = _mm256_maddubs_epi16(sextets, merge_pairs);
            let quads = _mm256_madd_epi16(pairs, merge_quads);
            let packed = _mm256_permutevar8x32_epi32(_mm256_shuffle_epi8(quads, pack), gather);
            let target = unsafe { output.add(block * DECODE_OUTPUT_BYTES) };
            unsafe { _mm_storeu_si128(target.cast(), _mm256_castsi256_si128(packed)) };
            unsafe { _mm_storel_epi64(target.add(LANE_BYTES).cast(), _mm256_extracti128_si256::<1>(packed)) };
        }
        blocks
    }
}

#[cfg(target_arch = "aarch64")]
mod arm {
    use std::arch::aarch64::*;

    use super::{Alphabet, SEXTET_MASK};

    const ENCODE_INPUT_BYTES: usize = 48;
    const ENCODE_OUTPUT_BYTES: usize = 64;
    const DECODE_INPUT_BYTES: usize = 64;
    const DECODE_OUTPUT_BYTES: usize = 48;
    const TABLE_HALF: usize = 64;
    const HIGH_BIT: u8 = 0x80;

    pub(super) fn encode(input: &[u8], output: &mut [u8], alphabet: Alphabet) -> usize {
        let blocks = (input.len() / ENCODE_INPUT_BYTES).min(output.len() / ENCODE_OUTPUT_BYTES);
        if blocks == 0 {
            return 0;
        }
        assert!(blocks * ENCODE_INPUT_BYTES <= input.len());
        assert!(blocks * ENCODE_OUTPUT_BYTES <= output.len());
        unsafe { encode_neon(input.as_ptr(), output.as_mut_ptr(), blocks, alphabet.symbols()) };
        blocks * ENCODE_INPUT_BYTES
    }

    #[target_feature(enable = "neon")]
    unsafe fn encode_neon(input: *const u8, output: *mut u8, blocks: usize, symbols: &[u8; 64]) {
        let table = unsafe { vld1q_u8_x4(symbols.as_ptr()) };
        let mask = vdupq_n_u8(SEXTET_MASK);
        for block in 0..blocks {
            let bytes = unsafe { vld3q_u8(input.add(block * ENCODE_INPUT_BYTES)) };
            let first = vshrq_n_u8::<2>(bytes.0);
            let second = vandq_u8(vorrq_u8(vshlq_n_u8::<4>(bytes.0), vshrq_n_u8::<4>(bytes.1)), mask);
            let third = vandq_u8(vorrq_u8(vshlq_n_u8::<2>(bytes.1), vshrq_n_u8::<6>(bytes.2)), mask);
            let fourth = vandq_u8(bytes.2, mask);
            let symbols = uint8x16x4_t(
                vqtbl4q_u8(table, first),
                vqtbl4q_u8(table, second),
                vqtbl4q_u8(table, third),
                vqtbl4q_u8(table, fourth),
            );
            unsafe { vst4q_u8(output.add(block * ENCODE_OUTPUT_BYTES), symbols) };
        }
    }

    pub(super) fn decode(input: &[u8], output: &mut [u8], alphabet: Alphabet) -> usize {
        let blocks = (input.len() / DECODE_INPUT_BYTES).min(output.len() / DECODE_OUTPUT_BYTES);
        if blocks == 0 {
            return 0;
        }
        assert!(blocks * DECODE_INPUT_BYTES <= input.len());
        assert!(blocks * DECODE_OUTPUT_BYTES <= output.len());
        let table = alphabet.decode_table();
        let decoded = unsafe { decode_neon(input.as_ptr(), output.as_mut_ptr(), blocks, table) };
        assert!(decoded <= blocks);
        decoded * DECODE_INPUT_BYTES
    }

    #[target_feature(enable = "neon")]
    fn translate(
        characters: uint8x16_t,
        low: uint8x16x4_t,
        high: uint8x16x4_t,
        offset: uint8x16_t,
    ) -> (uint8x16_t, uint8x16_t) {
        let values = vqtbx4q_u8(vqtbl4q_u8(low, characters), high, vsubq_u8(characters, offset));
        (values, vorrq_u8(values, characters))
    }

    #[target_feature(enable = "neon")]
    unsafe fn decode_neon(input: *const u8, output: *mut u8, blocks: usize, table: &[u8; 256]) -> usize {
        let low = unsafe { vld1q_u8_x4(table.as_ptr()) };
        let high = unsafe { vld1q_u8_x4(table.as_ptr().add(TABLE_HALF)) };
        let offset = vdupq_n_u8(TABLE_HALF as u8);
        for block in 0..blocks {
            let characters = unsafe { vld4q_u8(input.add(block * DECODE_INPUT_BYTES)) };
            let (first, first_check) = translate(characters.0, low, high, offset);
            let (second, second_check) = translate(characters.1, low, high, offset);
            let (third, third_check) = translate(characters.2, low, high, offset);
            let (fourth, fourth_check) = translate(characters.3, low, high, offset);
            let check = vorrq_u8(vorrq_u8(first_check, second_check), vorrq_u8(third_check, fourth_check));
            if vmaxvq_u8(check) >= HIGH_BIT {
                return block;
            }
            let bytes = uint8x16x3_t(
                vorrq_u8(vshlq_n_u8::<2>(first), vshrq_n_u8::<4>(second)),
                vorrq_u8(vshlq_n_u8::<4>(second), vshrq_n_u8::<2>(third)),
                vorrq_u8(vshlq_n_u8::<6>(third), fourth),
            );
            unsafe { vst3q_u8(output.add(block * DECODE_OUTPUT_BYTES), bytes) };
        }
        blocks
    }
}
