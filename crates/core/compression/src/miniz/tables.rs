pub(super) const DICTIONARY_BYTES: usize = 32_768;
pub(super) const DICTIONARY_MASK: usize = DICTIONARY_BYTES - 1;
pub(super) const MATCH_LENGTH_MIN: usize = 3;
pub(super) const MATCH_LENGTH_MAX: usize = 258;
pub(super) const MIRROR_BYTES: usize = MATCH_LENGTH_MAX - 1;
pub(super) const DICTIONARY_PADDED_BYTES: usize = DICTIONARY_BYTES + MATCH_LENGTH_MAX;
pub(super) const POSITION_MASK: usize = u16::MAX as usize;

pub(super) const HASH_BITS: u32 = 15;
pub(super) const HASH_ENTRIES: usize = 1 << HASH_BITS;
pub(super) const HASH_MASK: u32 = (1 << HASH_BITS) - 1;
pub(super) const HASH_SHIFT_BITS: u32 = HASH_BITS.div_ceil(3);
pub(super) const FAST_HASH_MASK: u32 = (1 << 12) - 1;
pub(super) const FAST_HASH_SHIFT_BITS: u32 = 24 - (HASH_BITS - 8);
pub(super) const TRIGRAM_MASK: u32 = 0x00ff_ffff;
pub(super) const FAST_LOOKAHEAD_BYTES: usize = 4_096;
pub(super) const FAST_LOOKAHEAD_BYTES_MIN: usize = 4;
pub(super) const COMPARE_WORDS: usize = 32;
pub(super) const WORD_BYTES: usize = 8;

pub(super) const LEVEL_PROBES: [u32; 11] = [0, 1, 6, 32, 16, 32, 128, 256, 512, 768, 1500];
pub(super) const GREEDY_LEVEL_MAX: u8 = 3;
pub(super) const ZLIB_BEST_PROBES: u32 = LEVEL_PROBES[9];
pub(super) const LONG_MATCH_LENGTH: u32 = 32;
pub(super) const LAZY_LENGTH_MAX: u32 = 128;
pub(super) const FAR_DISTANCE_MIN: u32 = 8 * 1024;

pub(super) const CODE_BUFFER_BYTES: usize = 64 * 1024;
pub(super) const CODE_BUFFER_SLACK_BYTES: usize = 8;
pub(super) const CODE_FLAGS_BITS: usize = 8;
pub(super) const MATCH_CODE_BYTES: usize = 3;
pub(super) const FAT_BLOCK_BYTES: u32 = 31 * 1024;
pub(super) const FAT_RATIO_NUMERATOR: usize = 115;
pub(super) const FAT_RATIO_SHIFT: u32 = 7;
pub(super) const DYNAMIC_BLOCK_BYTES_MIN: u32 = 48;
pub(super) const STORED_BLOCK_BYTES_MIN: u32 = 32;
pub(super) const STAGING_BYTES: usize = CODE_BUFFER_BYTES * 13 / 10;
pub(super) const STAGING_SLACK_BYTES: usize = 16;

pub(super) const LITERAL_SYMBOLS: usize = 288;
pub(super) const DISTANCE_SYMBOLS: usize = 32;
pub(super) const CODE_LENGTH_SYMBOLS: usize = 19;
pub(super) const LITERAL_CODES_MIN: usize = 257;
pub(super) const LITERAL_CODES_MAX: usize = 286;
pub(super) const DISTANCE_CODES_MAX: usize = 30;
pub(super) const CODE_LENGTH_CODES_MIN: usize = 4;
pub(super) const END_OF_BLOCK: usize = 256;
pub(super) const CODE_BITS_MAX: usize = 15;
pub(super) const CODE_LENGTH_BITS_MAX: usize = 7;
pub(super) const DEPTHS_MAX: usize = 32;
pub(super) const STATIC_LITERAL_LENGTHS: [(usize, u8); 4] =
    [(144, 8), (256, 9), (280, 7), (288, 8)];
pub(super) const STATIC_DISTANCE_LENGTH: u8 = 5;
pub(super) const CODE_LENGTH_ORDER: [usize; CODE_LENGTH_SYMBOLS] = [
    16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
];

pub(super) const REPEAT_PREVIOUS: u8 = 16;
pub(super) const REPEAT_ZEROS_SHORT: u8 = 17;
pub(super) const REPEAT_ZEROS_LONG: u8 = 18;
pub(super) const REPEAT_EXTRA_BITS: [u32; 3] = [2, 3, 7];
pub(super) const REPEAT_COUNT_MIN: usize = 3;
pub(super) const REPEAT_PREVIOUS_COUNT_MAX: usize = 6;
pub(super) const REPEAT_ZEROS_SHORT_COUNT_MAX: usize = 10;
pub(super) const REPEAT_ZEROS_LONG_COUNT_MIN: usize = 11;
pub(super) const REPEAT_ZEROS_LONG_COUNT_MAX: usize = 138;
pub(super) const NO_PREVIOUS_LENGTH: u8 = 0xff;

pub(super) const STORED_BLOCK_TYPE: u32 = 0;
pub(super) const STATIC_BLOCK_TYPE: u32 = 1;
pub(super) const DYNAMIC_BLOCK_TYPE: u32 = 2;
pub(super) const BLOCK_TYPE_BITS: u32 = 2;
pub(super) const STORED_LENGTH_BITS: u32 = 16;
pub(super) const STORED_LENGTH_MASK: u32 = (1 << STORED_LENGTH_BITS) - 1;
pub(super) const BYTE_BITS: u32 = 8;
pub(super) const ZLIB_METHOD: u8 = 8;
pub(super) const ZLIB_WINDOW_INFO: u8 = 7 << 4;
pub(super) const ZLIB_CHECK_DIVISOR: u32 = 31;
pub(super) const ZLIB_LEVEL_SHIFT: u32 = 6;
pub(super) const ZLIB_LEVEL_MASK: u32 = 0xe0;

pub(super) const LENGTH_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258,
];
pub(super) const LENGTH_EXTRA_BITS: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
pub(super) const DISTANCE_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
pub(super) const DISTANCE_EXTRA_BITS: [u8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];
const DISTANCE_SMALL: usize = 512;
const DISTANCE_LARGE_SHIFT: u32 = 8;

pub(super) const LENGTH_SYMBOL: [u8; MATCH_LENGTH_MAX - MATCH_LENGTH_MIN + 1] = {
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

pub(super) fn distance_code(offset: usize) -> usize {
    if offset < DISTANCE_SMALL {
        usize::from(DISTANCE_SYMBOL_SMALL[offset])
    } else {
        usize::from(DISTANCE_SYMBOL_LARGE[offset >> DISTANCE_LARGE_SHIFT])
    }
}
