const POLYNOMIAL: u32 = 0x04C1_1DB7;
const TABLE_ENTRIES: usize = 1 << u8::BITS;
const HIGH_BIT: u32 = 1 << (u32::BITS - 1);
const INDEX_SHIFT: u32 = u32::BITS - u8::BITS;
static TABLE: [u32; TABLE_ENTRIES] = table();

pub(super) const INITIAL: u32 = u32::MAX;

const fn table() -> [u32; TABLE_ENTRIES] {
    let mut table = [0; TABLE_ENTRIES];
    let mut index = 0;
    while index < TABLE_ENTRIES {
        let mut value = (index as u32) << INDEX_SHIFT;
        let mut bit = 0;
        while bit < u8::BITS {
            value = if value & HIGH_BIT != 0 {
                (value << 1) ^ POLYNOMIAL
            } else {
                value << 1
            };
            bit += 1;
        }
        table[index] = value;
        index += 1;
    }
    table
}

pub(super) fn update(crc: u32, byte: u8) -> u32 {
    (crc << u8::BITS) ^ TABLE[((crc >> INDEX_SHIFT) ^ byte as u32) as usize]
}

pub(super) const fn combine(combined: u32, block: u32) -> u32 {
    combined.rotate_left(1) ^ block
}
