const POLYNOMIAL_REFLECTED: u32 = 0xEDB8_8320;
const TABLE_ENTRIES: usize = 256;
const SLICE_BYTES: usize = 8;
const WORD_BYTES: usize = 4;
const BYTE_BITS: u32 = 8;
const BYTE_MASK: u32 = 0xFF;

pub(crate) static TABLE: [u32; TABLE_ENTRIES] = table();
static SLICE_TABLES: [[u32; TABLE_ENTRIES]; SLICE_BYTES] = slice_tables();

const fn table() -> [u32; TABLE_ENTRIES] {
    let mut table = [0; TABLE_ENTRIES];
    let mut index = 0;
    while index < TABLE_ENTRIES {
        let mut value = index as u32;
        let mut bit = 0;
        while bit < BYTE_BITS {
            value = if value & 1 == 1 {
                (value >> 1) ^ POLYNOMIAL_REFLECTED
            } else {
                value >> 1
            };
            bit += 1;
        }
        table[index] = value;
        index += 1;
    }
    table
}

const fn slice_tables() -> [[u32; TABLE_ENTRIES]; SLICE_BYTES] {
    let mut tables = [[0; TABLE_ENTRIES]; SLICE_BYTES];
    tables[0] = table();
    let mut slice = 1;
    while slice < SLICE_BYTES {
        let mut index = 0;
        while index < TABLE_ENTRIES {
            let previous = tables[slice - 1][index];
            tables[slice][index] =
                (previous >> BYTE_BITS) ^ tables[0][(previous & BYTE_MASK) as usize];
            index += 1;
        }
        slice += 1;
    }
    tables
}

const fn byte_at(value: u32, byte: usize) -> usize {
    ((value >> (byte as u32 * BYTE_BITS)) & BYTE_MASK) as usize
}

pub(crate) fn update(crc: u32, input: &[u8]) -> u32 {
    let mut value = !crc;
    let (slices, remainder) = input.as_chunks::<SLICE_BYTES>();
    for slice in slices {
        let (low, high) = slice.as_chunks::<WORD_BYTES>().0.split_at(1);
        let low = value ^ u32::from_le_bytes(low[0]);
        let high = u32::from_le_bytes(high[0]);
        value = SLICE_TABLES[7][byte_at(low, 0)]
            ^ SLICE_TABLES[6][byte_at(low, 1)]
            ^ SLICE_TABLES[5][byte_at(low, 2)]
            ^ SLICE_TABLES[4][byte_at(low, 3)]
            ^ SLICE_TABLES[3][byte_at(high, 0)]
            ^ SLICE_TABLES[2][byte_at(high, 1)]
            ^ SLICE_TABLES[1][byte_at(high, 2)]
            ^ SLICE_TABLES[0][byte_at(high, 3)];
    }
    for &byte in remainder {
        value = TABLE[((value ^ u32::from(byte)) & BYTE_MASK) as usize] ^ (value >> BYTE_BITS);
    }
    !value
}
