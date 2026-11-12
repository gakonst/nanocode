pub(super) const CODE_LENGTH_CODES: usize = 18;
pub(super) const CODE_LENGTH_CODE_ORDER: [u8; CODE_LENGTH_CODES] =
    [1, 2, 3, 4, 0, 5, 17, 6, 16, 7, 8, 9, 10, 11, 12, 13, 14, 15];
pub(super) const CODE_LENGTH_PREFIX_BITS: u32 = 4;
pub(super) const CODE_LENGTH_PREFIX_LENGTH: [u8; 16] = [2, 2, 2, 3, 2, 2, 2, 4, 2, 2, 2, 3, 2, 2, 2, 4];
pub(super) const CODE_LENGTH_PREFIX_VALUE: [u8; 16] = [0, 4, 3, 2, 0, 4, 3, 1, 0, 4, 3, 2, 0, 4, 3, 5];

pub(super) const LENGTH_CODES: usize = 24;
const INSERT_BASE: [u32; LENGTH_CODES] = [
    0, 1, 2, 3, 4, 5, 6, 8, 10, 14, 18, 26, 34, 50, 66, 98, 130, 194, 322, 578, 1090, 2114, 6210, 22594,
];
const INSERT_EXTRA_BITS: [u8; LENGTH_CODES] =
    [0, 0, 0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 7, 8, 9, 10, 12, 14, 24];
const COPY_BASE: [u32; LENGTH_CODES] = [
    2, 3, 4, 5, 6, 7, 8, 9, 10, 12, 14, 18, 22, 30, 38, 54, 70, 102, 134, 198, 326, 582, 1094, 2118,
];
const COPY_EXTRA_BITS: [u8; LENGTH_CODES] =
    [0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 7, 8, 9, 10, 24];

pub(super) const BLOCK_LENGTH_CODES: u32 = 26;
pub(super) const BLOCK_LENGTH_BASE: [u32; BLOCK_LENGTH_CODES as usize] = [
    1, 5, 9, 13, 17, 25, 33, 41, 49, 65, 81, 97, 113, 145, 177, 209, 241, 305, 369, 497, 753, 1265,
    2289, 4337, 8433, 16625,
];
pub(super) const BLOCK_LENGTH_EXTRA_BITS: [u8; BLOCK_LENGTH_CODES as usize] =
    [2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 6, 6, 7, 8, 9, 10, 11, 12, 13, 24];

pub(super) const COMMAND_CODES: u32 = 704;
const COMMAND_CELL_SYMBOLS_BITS: u32 = 6;
const COMMAND_CELL_CODE_BITS: u32 = 3;
const COMMAND_CELL_CODE_MASK: u32 = 7;
const INSERT_CELL_BASE: [u32; 11] = [0, 0, 0, 0, 8, 8, 0, 16, 8, 16, 16];
const COPY_CELL_BASE: [u32; 11] = [0, 8, 0, 8, 0, 8, 16, 0, 16, 8, 16];
const IMPLICIT_DISTANCE_CELLS: u32 = 2;
pub(super) const DISTANCE_CONTEXTS: u32 = 4;
const COPY_CODE_DISTANCE_CONTEXT_MAX: u32 = DISTANCE_CONTEXTS - 1;

#[derive(Clone, Copy)]
pub(super) struct Command {
    pub insert_base: u32,
    pub insert_extra_bits: u8,
    pub copy_base: u32,
    pub copy_extra_bits: u8,
    pub implicit_distance: bool,
    pub distance_context: u8,
}

const fn command(symbol: u32) -> Command {
    let cell = (symbol >> COMMAND_CELL_SYMBOLS_BITS) as usize;
    let insert_code = (INSERT_CELL_BASE[cell] + ((symbol >> COMMAND_CELL_CODE_BITS) & COMMAND_CELL_CODE_MASK)) as usize;
    let copy_code = COPY_CELL_BASE[cell] + (symbol & COMMAND_CELL_CODE_MASK);
    let distance_context = if copy_code < COPY_CODE_DISTANCE_CONTEXT_MAX {
        copy_code
    } else {
        COPY_CODE_DISTANCE_CONTEXT_MAX
    };
    Command {
        insert_base: INSERT_BASE[insert_code],
        insert_extra_bits: INSERT_EXTRA_BITS[insert_code],
        copy_base: COPY_BASE[copy_code as usize],
        copy_extra_bits: COPY_EXTRA_BITS[copy_code as usize],
        implicit_distance: (cell as u32) < IMPLICIT_DISTANCE_CELLS,
        distance_context: distance_context as u8,
    }
}

const fn commands() -> [Command; COMMAND_CODES as usize] {
    let mut table = [command(0); COMMAND_CODES as usize];
    let mut symbol = 0;
    while symbol < COMMAND_CODES {
        table[symbol as usize] = command(symbol);
        symbol += 1;
    }
    table
}

pub(super) const COMMANDS: [Command; COMMAND_CODES as usize] = commands();

pub(super) const LITERAL_CONTEXT_BITS: u32 = 6;
pub(super) const LITERAL_CONTEXTS: usize = 1 << LITERAL_CONTEXT_BITS;
pub(super) const CONTEXT_MODES: usize = 4;
const BYTE_VALUES: usize = 256;
pub(super) const CONTEXT_LOOKUP_ENTRIES: usize = 2 * BYTE_VALUES;
const LSB6_MASK: u8 = 0x3f;
const MSB6_SHIFT: u32 = 2;
const SIGNED_HIGH_SHIFT: u32 = 3;

const UTF8_LAST: [u8; BYTE_VALUES] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 4, 4, 0, 0, 4, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    8, 12, 16, 12, 12, 20, 12, 16, 24, 28, 12, 12, 32, 12, 36, 12,
    44, 44, 44, 44, 44, 44, 44, 44, 44, 44, 32, 32, 24, 40, 28, 12,
    12, 48, 52, 52, 52, 48, 52, 52, 52, 48, 52, 52, 52, 52, 52, 48,
    52, 52, 52, 52, 52, 48, 52, 52, 52, 52, 52, 24, 12, 28, 12, 12,
    12, 56, 60, 60, 60, 56, 60, 60, 60, 56, 60, 60, 60, 60, 60, 56,
    60, 60, 60, 60, 60, 56, 60, 60, 60, 60, 60, 24, 12, 28, 12, 0,
    0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1,
    0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1,
    0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1,
    0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1,
    2, 3, 2, 3, 2, 3, 2, 3, 2, 3, 2, 3, 2, 3, 2, 3,
    2, 3, 2, 3, 2, 3, 2, 3, 2, 3, 2, 3, 2, 3, 2, 3,
    2, 3, 2, 3, 2, 3, 2, 3, 2, 3, 2, 3, 2, 3, 2, 3,
    2, 3, 2, 3, 2, 3, 2, 3, 2, 3, 2, 3, 2, 3, 2, 3,
];

const UTF8_BEFORE_LAST: [u8; BYTE_VALUES] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
    2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 1, 1, 1, 1, 1, 1,
    1, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2,
    2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 1, 1, 1, 1, 1,
    1, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3,
    3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 1, 1, 1, 1, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2,
    2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2,
];

const SIGNED: [u8; BYTE_VALUES] = [
    0, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
    2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2,
    2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2,
    2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2,
    3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3,
    3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3,
    3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3,
    3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3,
    4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4,
    4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4,
    4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4,
    4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4,
    5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5,
    5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5,
    5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5,
    6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 7,
];

const fn context_lookup() -> [[u8; CONTEXT_LOOKUP_ENTRIES]; CONTEXT_MODES] {
    let mut table = [[0; CONTEXT_LOOKUP_ENTRIES]; CONTEXT_MODES];
    let mut byte = 0;
    while byte < BYTE_VALUES {
        table[0][byte] = byte as u8 & LSB6_MASK;
        table[1][byte] = byte as u8 >> MSB6_SHIFT;
        table[2][byte] = UTF8_LAST[byte];
        table[2][BYTE_VALUES + byte] = UTF8_BEFORE_LAST[byte];
        table[3][byte] = SIGNED[byte] << SIGNED_HIGH_SHIFT;
        table[3][BYTE_VALUES + byte] = SIGNED[byte];
        byte += 1;
    }
    table
}

pub(super) const CONTEXT_LOOKUP: [[u8; CONTEXT_LOOKUP_ENTRIES]; CONTEXT_MODES] = context_lookup();
