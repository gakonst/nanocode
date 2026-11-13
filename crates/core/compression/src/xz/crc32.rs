const POLYNOMIAL_REFLECTED: u32 = 0xEDB8_8320;
const TABLE_ENTRIES: usize = 256;
const BYTE_BITS: u32 = 8;

pub(crate) static TABLE: [u32; TABLE_ENTRIES] = table();

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

pub(crate) fn update(crc: u32, input: &[u8]) -> u32 {
    crate::checksum::crc32(crc, input)
}
