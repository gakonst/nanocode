use crate::error::JpegError;

pub(crate) const LOOKAHEAD_BITS: u8 = 9;
pub(crate) const LOOKAHEAD_ENTRIES: usize = 1 << LOOKAHEAD_BITS;
const CODE_LENGTH_MAX: usize = 16;
const TOO_LONG_CODE: i32 = (LOOKAHEAD_BITS as i32 + 1) << LOOKAHEAD_BITS;
const FAST_UNUSED: i16 = 255;
const DC_SYMBOL_MAX: u8 = 15;

#[derive(Clone)]
pub(crate) struct HuffmanTable {
    pub(crate) maximum_code: [i32; 18],
    pub(crate) offset: [i32; 18],
    pub(crate) lookup: [i32; LOOKAHEAD_ENTRIES],
    pub(crate) fast_ac: [i16; LOOKAHEAD_ENTRIES],
    pub(crate) values: [u8; 256],
}

impl HuffmanTable {
    pub(crate) fn new(
        counts: &[u8; 17],
        values: [u8; 256],
        is_dc: bool,
    ) -> Result<Self, JpegError> {
        let mut table = Self {
            maximum_code: [0; 18],
            offset: [0; 18],
            lookup: [TOO_LONG_CODE; LOOKAHEAD_ENTRIES],
            fast_ac: [0; LOOKAHEAD_ENTRIES],
            values,
        };
        table.derive(is_dc, counts)?;
        Ok(table)
    }

    pub(crate) fn from_symbols(
        counts: &[u8; 17],
        symbols: &[u8],
        is_dc: bool,
    ) -> Result<Self, JpegError> {
        let mut values = [0; 256];
        values[..symbols.len()].copy_from_slice(symbols);
        Self::new(counts, values, is_dc)
    }

    fn derive(&mut self, is_dc: bool, counts: &[u8; 17]) -> Result<(), JpegError> {
        let mut sizes = [0_u8; 257];
        let mut codes = [0_u32; 257];
        let mut position = 0;
        for (length, &count) in counts.iter().enumerate().take(CODE_LENGTH_MAX + 1).skip(1) {
            for _ in 0..count {
                sizes[position] = length as u8;
                position += 1;
            }
        }
        sizes[position] = 0;
        let symbol_count = position;
        let mut code = 0_u32;
        let mut size = u32::from(sizes[0]);
        position = 0;
        while sizes[position] != 0 {
            while u32::from(sizes[position]) == size {
                codes[position] = code;
                code += 1;
                position += 1;
            }
            self.maximum_code[size as usize] = (code << (16 - size)) as i32;
            if code >= 1 << size {
                return Err(JpegError::HuffmanTable);
            }
            code <<= 1;
            size += 1;
        }
        position = 0;
        for (length, &count) in counts.iter().enumerate().take(CODE_LENGTH_MAX + 1) {
            if count == 0 {
                self.maximum_code[length] = -1;
            } else {
                self.offset[length] = position as i32 - codes[position] as i32;
                position += usize::from(count);
            }
        }
        self.offset[17] = 0;
        self.maximum_code[17] = 0x000F_FFFF;
        self.fill_lookup(counts, &codes);
        if is_dc {
            if self.values[..symbol_count]
                .iter()
                .any(|&symbol| symbol > DC_SYMBOL_MAX)
            {
                return Err(JpegError::HuffmanTable);
            }
        } else {
            self.fill_fast_ac(&sizes, &codes, symbol_count);
        }
        Ok(())
    }

    fn fill_lookup(&mut self, counts: &[u8; 17], codes: &[u32; 257]) {
        let mut position = 0;
        for length in 1..=LOOKAHEAD_BITS {
            for _ in 0..counts[usize::from(length)] {
                let first = (codes[position] as usize) << (LOOKAHEAD_BITS - length);
                let entry =
                    (i32::from(length) << LOOKAHEAD_BITS) | i32::from(self.values[position]);
                let count = 1_usize << (LOOKAHEAD_BITS - length);
                self.lookup[first..first + count].fill(entry);
                position += 1;
            }
        }
    }

    fn fill_fast_ac(&mut self, sizes: &[u8; 257], codes: &[u32; 257], symbol_count: usize) {
        let mut fast = [FAST_UNUSED; LOOKAHEAD_ENTRIES];
        for symbol in 0..symbol_count {
            let size = sizes[symbol];
            if size <= LOOKAHEAD_BITS {
                let first = (codes[symbol] << (LOOKAHEAD_BITS - size)) as usize;
                let count = 1_usize << (LOOKAHEAD_BITS - size);
                fast[first..first + count].fill(symbol as i16);
            }
        }
        let lookahead = i16::from(LOOKAHEAD_BITS);
        for (index, &symbol) in fast.iter().enumerate() {
            if symbol >= FAST_UNUSED {
                continue;
            }
            let run_size = self.values[symbol as usize];
            let run = i16::from((run_size >> 4) & 15);
            let magnitude_bits = i16::from(run_size & 15);
            let length = i16::from(sizes[symbol as usize]);
            if magnitude_bits == 0 || length + magnitude_bits > lookahead {
                continue;
            }
            let mut value = ((index as i16).wrapping_shl(length as u32)
                & ((1 << LOOKAHEAD_BITS) - 1))
                >> (lookahead - magnitude_bits);
            let half = 1 << (magnitude_bits - 1);
            if value < half {
                value += (!0_i16 << magnitude_bits) + 1;
            }
            if (-128..=127).contains(&value) {
                self.fast_ac[index] = (value << 8) + (run << 4) + (length + magnitude_bits);
            }
        }
    }
}
