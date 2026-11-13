use nanocode_sys::crc32::Crc32Instructions;

const CRC32_POLYNOMIAL: u32 = 0xEDB8_8320;
const CRC32_TABLES: usize = 16;
const BYTE_VALUES: usize = 256;
static CRC32_TABLE: [[u32; BYTE_VALUES]; CRC32_TABLES] = crc32_tables();
const ADLER32_MODULUS: u32 = 65_521;
const ADLER32_REDUCE_BYTES: usize = 5_552;
const ADLER32_LANES: usize = 32;
const ADLER32_BLOCK_CHUNKS: usize = ADLER32_REDUCE_BYTES / ADLER32_LANES;
const ADLER32_HALF_BITS: u32 = 16;
const ADLER32_HALF_MASK: u32 = 0xFFFF;
const ADLER32_UNROLL_BYTES: usize = 16;

const fn crc32_tables() -> [[u32; BYTE_VALUES]; CRC32_TABLES] {
    let mut tables = [[0; BYTE_VALUES]; CRC32_TABLES];
    let mut byte = 0;
    while byte < BYTE_VALUES {
        let mut crc = byte as u32;
        let mut bit = 0;
        while bit < u8::BITS {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ CRC32_POLYNOMIAL
            } else {
                crc >> 1
            };
            bit += 1;
        }
        tables[0][byte] = crc;
        byte += 1;
    }
    let mut table = 1;
    while table < CRC32_TABLES {
        let mut byte = 0;
        while byte < BYTE_VALUES {
            let previous = tables[table - 1][byte];
            tables[table][byte] = (previous >> u8::BITS) ^ tables[0][(previous & 0xFF) as usize];
            byte += 1;
        }
        table += 1;
    }
    tables
}

#[inline]
pub fn crc32(start: u32, data: &[u8]) -> u32 {
    match Crc32Instructions::detect() {
        Some(instructions) => instructions.update(start, data),
        None => crc32_table(start, data),
    }
}

fn crc32_table(start: u32, data: &[u8]) -> u32 {
    let mut crc = !start;
    let (blocks, rest) = data.as_chunks::<CRC32_TABLES>();
    for block in blocks {
        crc = crc32_block(crc, block);
    }
    for byte in rest {
        crc = (crc >> u8::BITS) ^ CRC32_TABLE[0][usize::from(crc.to_le_bytes()[0] ^ byte)];
    }
    !crc
}

fn crc32_block(crc: u32, block: &[u8; CRC32_TABLES]) -> u32 {
    let low = crc.to_le_bytes();
    let entry = |index: usize, byte: u8| CRC32_TABLE[CRC32_TABLES - 1 - index][usize::from(byte)];
    entry(0, block[0] ^ low[0])
        ^ entry(1, block[1] ^ low[1])
        ^ entry(2, block[2] ^ low[2])
        ^ entry(3, block[3] ^ low[3])
        ^ entry(4, block[4])
        ^ entry(5, block[5])
        ^ entry(6, block[6])
        ^ entry(7, block[7])
        ^ entry(8, block[8])
        ^ entry(9, block[9])
        ^ entry(10, block[10])
        ^ entry(11, block[11])
        ^ entry(12, block[12])
        ^ entry(13, block[13])
        ^ entry(14, block[14])
        ^ entry(15, block[15])
}

pub fn adler32(start: u32, data: &[u8]) -> u32 {
    let mut low = start & ADLER32_HALF_MASK;
    let mut high = start >> ADLER32_HALF_BITS;
    if let [byte] = data {
        low = (low + u32::from(*byte)) % ADLER32_MODULUS;
        high = (high + low) % ADLER32_MODULUS;
        return (high << ADLER32_HALF_BITS) | low;
    }
    if data.len() < ADLER32_UNROLL_BYTES {
        for byte in data {
            low = low.wrapping_add(u32::from(*byte));
            high = high.wrapping_add(low);
        }
        if low >= ADLER32_MODULUS {
            low -= ADLER32_MODULUS;
        }
        high %= ADLER32_MODULUS;
        return (high << ADLER32_HALF_BITS) | low;
    }
    let (chunks, rest) = data.as_chunks::<ADLER32_LANES>();
    for block in chunks.chunks(ADLER32_BLOCK_CHUNKS) {
        (low, high) = adler32_lanes(low, high, block);
    }
    for byte in rest {
        low = low.wrapping_add(u32::from(*byte));
        high = high.wrapping_add(low);
    }
    low %= ADLER32_MODULUS;
    high %= ADLER32_MODULUS;
    (high << ADLER32_HALF_BITS) | low
}

fn adler32_lanes(low: u32, high: u32, block: &[[u8; ADLER32_LANES]]) -> (u32, u32) {
    assert!(block.len() <= ADLER32_BLOCK_CHUNKS);
    let mut sums = [0u32; ADLER32_LANES];
    let mut prefixes = [0u32; ADLER32_LANES];
    for chunk in block {
        for lane in 0..ADLER32_LANES {
            prefixes[lane] += sums[lane];
            sums[lane] += u32::from(chunk[lane]);
        }
    }
    let lanes = ADLER32_LANES as u64;
    let mut total = 0u64;
    let mut weighted = 0u64;
    for lane in 0..ADLER32_LANES {
        total += u64::from(sums[lane]);
        weighted +=
            lanes * u64::from(prefixes[lane]) + (lanes - lane as u64) * u64::from(sums[lane]);
    }
    let length = (block.len() * ADLER32_LANES) as u64;
    let modulus = u64::from(ADLER32_MODULUS);
    let high = (u64::from(high) + u64::from(low) * length + weighted) % modulus;
    let low = (u64::from(low) + total) % modulus;
    (low as u32, high as u32)
}
