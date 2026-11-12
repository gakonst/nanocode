pub(super) const CODE_BITS_MAX: usize = 15;
pub(super) const OP_LITERAL: u8 = 0;
pub(super) const OP_BASE: u8 = 16;
pub(super) const OP_END_OF_BLOCK: u8 = 32;
pub(super) const OP_INVALID: u8 = 64;
pub(super) const OP_EXTRA_BITS_MASK: u8 = 15;
const OP_KIND_MASK: u8 = 0xF0;
const OP_END_OF_BLOCK_ENTRY: u8 = OP_INVALID | OP_END_OF_BLOCK;
pub(super) const CODE_LENGTH_CODES: usize = 19;
pub(super) const CODE_LENGTH_ROOT_BITS: usize = 7;
pub(super) const CODE_LENGTH_TABLE_ENTRIES: usize = 1 << CODE_LENGTH_ROOT_BITS;
pub(super) const LENGTH_ROOT_BITS: usize = 10;
pub(super) const LENGTH_TABLE_ENTRIES: usize = 1332;
pub(super) const DISTANCE_ROOT_BITS: usize = 9;
pub(super) const DISTANCE_TABLE_ENTRIES: usize = 592;
pub(super) const FIXED_LENGTH_ROOT_BITS: usize = 9;
pub(super) const FIXED_DISTANCE_ROOT_BITS: usize = 5;
pub(super) const END_OF_BLOCK: usize = 256;
pub(super) const CODE_LENGTHS_CAPACITY: usize = 320;
pub(super) const WORK_ENTRIES: usize = 288;
const FIXED_LENGTH_SYMBOLS: usize = 288;
const FIXED_DISTANCE_SYMBOLS: usize = 32;
const FIXED_LITERAL_LOW_END: usize = 144;
const FIXED_LITERAL_HIGH_END: usize = 256;
const FIXED_SHORT_LENGTH_END: usize = 280;
const FIXED_LITERAL_LOW_BITS: u16 = 8;
const FIXED_LITERAL_HIGH_BITS: u16 = 9;
const FIXED_SHORT_LENGTH_BITS: u16 = 7;
const FIXED_LONG_LENGTH_BITS: u16 = 8;
const FIXED_DISTANCE_BITS: u16 = 5;
const LENGTH_FIRST_BASE_SYMBOL: u16 = END_OF_BLOCK as u16 + 1;
const CODE_LENGTH_FIRST_BASE_SYMBOL: u16 = CODE_LENGTH_CODES as u16 + 1;
const HUFFMAN_TOP_BIT: u32 = 1 << (u32::BITS - 1);
const EMPTY_TABLE_ROOT_BITS: usize = 1;

const LENGTH_BASE: [u16; 31] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258, 0, 0,
];
const LENGTH_OPS: [u8; 31] = [
    16, 16, 16, 16, 16, 16, 16, 16, 17, 17, 17, 17, 18, 18, 18, 18, 19, 19, 19, 19, 20, 20, 20, 20,
    21, 21, 21, 21, 16, 77, 202,
];
const DISTANCE_BASE: [u16; 32] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577, 0, 0,
];
const DISTANCE_OPS: [u8; 32] = [
    16, 16, 16, 16, 17, 17, 18, 18, 19, 19, 20, 20, 21, 21, 22, 22, 23, 23, 24, 24, 25, 25, 26, 26,
    27, 27, 28, 28, 29, 29, 64, 64,
];

pub(super) static FIXED_LENGTHS: [Code; 1 << FIXED_LENGTH_ROOT_BITS] = fixed_lengths();
pub(super) static FIXED_DISTANCES: [Code; 1 << FIXED_DISTANCE_ROOT_BITS] = fixed_distances();

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) struct Code {
    pub(super) op: u8,
    pub(super) bits: u8,
    pub(super) value: u16,
}

impl Code {
    pub(super) const EMPTY: Self = Self {
        op: 0,
        bits: 0,
        value: 0,
    };

    pub(super) const fn is_subtable(self) -> bool {
        self.op != OP_LITERAL && self.op & OP_KIND_MASK == 0
    }
}

#[derive(Clone, Copy)]
pub(super) enum CodeKind {
    CodeLengths,
    Lengths,
    Distances,
}

impl CodeKind {
    const fn first_base_symbol(self) -> u16 {
        match self {
            Self::CodeLengths => CODE_LENGTH_FIRST_BASE_SYMBOL,
            Self::Lengths => LENGTH_FIRST_BASE_SYMBOL,
            Self::Distances => 0,
        }
    }

    const fn entry(self, symbol: u16, bits: u8) -> Code {
        let first_base = self.first_base_symbol();
        if symbol >= first_base {
            let index = (symbol - first_base) as usize;
            let (op, value) = match self {
                Self::CodeLengths => panic!("code length symbols are all literals"),
                Self::Lengths => (LENGTH_OPS[index], LENGTH_BASE[index]),
                Self::Distances => (DISTANCE_OPS[index], DISTANCE_BASE[index]),
            };
            Code { op, bits, value }
        } else if symbol + 1 < first_base {
            Code {
                op: OP_LITERAL,
                bits,
                value: symbol,
            }
        } else {
            Code {
                op: OP_END_OF_BLOCK_ENTRY,
                bits,
                value: 0,
            }
        }
    }
}

#[derive(Clone, Copy)]
struct Span {
    root: usize,
    min: usize,
    max: usize,
}

pub(super) const fn build(
    kind: CodeKind,
    lengths: &[u16],
    table: &mut [Code],
    root_bits: usize,
    work: &mut [u16],
) -> Option<usize> {
    let mut count = [0_u16; CODE_BITS_MAX + 1];
    let mut min = CODE_BITS_MAX;
    let mut max = 0;
    let mut symbol = 0;
    while symbol < lengths.len() {
        let length = lengths[symbol] as usize;
        assert!(length <= CODE_BITS_MAX);
        if length > 0 {
            count[length] += 1;
            if length > max {
                max = length;
            }
            if length < min {
                min = length;
            }
        }
        symbol += 1;
    }
    if max == 0 {
        let empty = Code {
            op: OP_INVALID,
            bits: EMPTY_TABLE_ROOT_BITS as u8,
            value: 0,
        };
        table[0] = empty;
        table[1] = empty;
        return Some(EMPTY_TABLE_ROOT_BITS);
    }
    let root = if root_bits < min {
        min
    } else if root_bits > max {
        max
    } else {
        root_bits
    };
    if !complete(&count, kind, max) {
        return None;
    }
    sort(lengths, &count, work);
    if fill(kind, lengths, count, table, Span { root, min, max }, work) {
        Some(root)
    } else {
        None
    }
}

const fn complete(count: &[u16; CODE_BITS_MAX + 1], kind: CodeKind, max: usize) -> bool {
    let mut left: u32 = 1;
    let mut length = 1;
    while length <= CODE_BITS_MAX {
        left <<= 1;
        if left < count[length] as u32 {
            return false;
        }
        left -= count[length] as u32;
        length += 1;
    }
    !(left > 0 && (matches!(kind, CodeKind::CodeLengths) || max != 1))
}

const fn sort(lengths: &[u16], count: &[u16; CODE_BITS_MAX + 1], work: &mut [u16]) {
    let mut offsets = [0_u16; CODE_BITS_MAX + 1];
    let mut length = 1;
    while length < CODE_BITS_MAX {
        offsets[length + 1] = offsets[length] + count[length];
        length += 1;
    }
    let mut symbol = 0;
    while symbol < lengths.len() {
        let length = lengths[symbol] as usize;
        if length != 0 {
            work[offsets[length] as usize] = symbol as u16;
            offsets[length] += 1;
        }
        symbol += 1;
    }
}

const fn fill(
    kind: CodeKind,
    lengths: &[u16],
    mut count: [u16; CODE_BITS_MAX + 1],
    table: &mut [Code],
    span: Span,
    work: &[u16],
) -> bool {
    let root = span.root;
    let mut used = 1_usize << root;
    if used > table.len() {
        return false;
    }
    let mask = used - 1;
    let mut huffman = 0_usize;
    let mut reversed: u32 = 0;
    let mut symbol = 0;
    let mut length = span.min;
    let mut next = 0;
    let mut current = root;
    let mut dropped = 0;
    let mut low = usize::MAX;
    loop {
        let here = kind.entry(work[symbol], (length - dropped) as u8);
        let increment = 1_usize << (length - dropped);
        let entries = 1_usize << current;
        let mut position = entries;
        while position > 0 {
            position -= increment;
            table[next + (huffman >> dropped) + position] = here;
        }
        reversed = reversed.wrapping_add(HUFFMAN_TOP_BIT >> (length - 1));
        huffman = reversed.reverse_bits() as usize;
        symbol += 1;
        count[length] -= 1;
        if count[length] == 0 {
            if length == span.max {
                break;
            }
            length = lengths[work[symbol] as usize] as usize;
        }
        if length > root && (huffman & mask) != low {
            if dropped == 0 {
                dropped = root;
            }
            next += entries;
            current = subtable_bits(&count, length - dropped, dropped, span.max);
            used += 1 << current;
            if used > table.len() {
                return false;
            }
            low = huffman & mask;
            table[low] = Code {
                op: current as u8,
                bits: root as u8,
                value: next as u16,
            };
        }
    }
    if huffman != 0 {
        table[next + huffman] = Code {
            op: OP_INVALID,
            bits: (length - dropped) as u8,
            value: 0,
        };
    }
    true
}

const fn subtable_bits(
    count: &[u16; CODE_BITS_MAX + 1],
    start: usize,
    dropped: usize,
    max: usize,
) -> usize {
    let mut current = start;
    let mut left: i32 = 1 << current;
    while current + dropped < max {
        left -= count[current + dropped] as i32;
        if left <= 0 {
            break;
        }
        current += 1;
        left <<= 1;
    }
    current
}

const fn fixed_lengths() -> [Code; 1 << FIXED_LENGTH_ROOT_BITS] {
    let mut lengths = [0_u16; FIXED_LENGTH_SYMBOLS];
    let mut symbol = 0;
    while symbol < FIXED_LENGTH_SYMBOLS {
        lengths[symbol] = if symbol < FIXED_LITERAL_LOW_END {
            FIXED_LITERAL_LOW_BITS
        } else if symbol < FIXED_LITERAL_HIGH_END {
            FIXED_LITERAL_HIGH_BITS
        } else if symbol < FIXED_SHORT_LENGTH_END {
            FIXED_SHORT_LENGTH_BITS
        } else {
            FIXED_LONG_LENGTH_BITS
        };
        symbol += 1;
    }
    let mut table = [Code::EMPTY; 1 << FIXED_LENGTH_ROOT_BITS];
    let mut work = [0_u16; WORK_ENTRIES];
    let root = build(
        CodeKind::Lengths,
        &lengths,
        &mut table,
        FIXED_LENGTH_ROOT_BITS,
        &mut work,
    );
    assert!(matches!(root, Some(FIXED_LENGTH_ROOT_BITS)));
    table
}

const fn fixed_distances() -> [Code; 1 << FIXED_DISTANCE_ROOT_BITS] {
    let lengths = [FIXED_DISTANCE_BITS; FIXED_DISTANCE_SYMBOLS];
    let mut table = [Code::EMPTY; 1 << FIXED_DISTANCE_ROOT_BITS];
    let mut work = [0_u16; WORK_ENTRIES];
    let root = build(
        CodeKind::Distances,
        &lengths,
        &mut table,
        FIXED_DISTANCE_ROOT_BITS,
        &mut work,
    );
    assert!(matches!(root, Some(FIXED_DISTANCE_ROOT_BITS)));
    table
}
