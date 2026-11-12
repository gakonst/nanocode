const POLYNOMIAL_REFLECTED: u64 = 0xC96C_5795_D787_0F42;
const TABLE_ENTRIES: usize = 256;
const SLICE_BYTES: usize = 8;
const BYTE_BITS: u32 = 8;
const BYTE_MASK: u64 = 0xFF;

static SLICE_TABLES: [[u64; TABLE_ENTRIES]; SLICE_BYTES] = slice_tables();

const fn table() -> [u64; TABLE_ENTRIES] {
    let mut table = [0; TABLE_ENTRIES];
    let mut index = 0;
    while index < TABLE_ENTRIES {
        let mut value = index as u64;
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

const fn slice_tables() -> [[u64; TABLE_ENTRIES]; SLICE_BYTES] {
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

const fn byte_at(value: u64, byte: usize) -> usize {
    ((value >> (byte as u32 * BYTE_BITS)) & BYTE_MASK) as usize
}

pub(crate) fn update(crc: u64, input: &[u8]) -> u64 {
    let mut value = !crc;
    let (slices, remainder) = input.as_chunks::<SLICE_BYTES>();
    for slice in slices {
        let word = value ^ u64::from_le_bytes(*slice);
        value = SLICE_TABLES[7][byte_at(word, 0)]
            ^ SLICE_TABLES[6][byte_at(word, 1)]
            ^ SLICE_TABLES[5][byte_at(word, 2)]
            ^ SLICE_TABLES[4][byte_at(word, 3)]
            ^ SLICE_TABLES[3][byte_at(word, 4)]
            ^ SLICE_TABLES[2][byte_at(word, 5)]
            ^ SLICE_TABLES[1][byte_at(word, 6)]
            ^ SLICE_TABLES[0][byte_at(word, 7)];
    }
    for &byte in remainder {
        value = SLICE_TABLES[0][((value ^ u64::from(byte)) & BYTE_MASK) as usize]
            ^ (value >> BYTE_BITS);
    }
    !value
}
