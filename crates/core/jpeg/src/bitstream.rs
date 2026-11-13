use crate::error::JpegError;
use crate::huffman::{HuffmanTable, LOOKAHEAD_BITS};
use crate::marker::Marker;
use crate::reader::ByteReader;

pub(crate) const UNZIGZAG: [usize; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20,
    13, 6, 7, 14, 21, 28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58, 59,
    52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,
];
const REFILL_BITS_MIN: u8 = 32;
const LONG_CODE_BITS: u8 = 16;
const CODE_LENGTH_LIMIT: i32 = 17;

struct Window {
    buffer: u64,
    aligned: u64,
    bits_left: u8,
}

impl Window {
    const EMPTY: Self = Self {
        buffer: 0,
        aligned: 0,
        bits_left: 0,
    };

    const fn copied(&self) -> Self {
        Self {
            buffer: self.buffer,
            aligned: self.aligned,
            bits_left: self.bits_left,
        }
    }

    const fn store(&mut self, other: &Self) {
        self.buffer = other.buffer;
        self.aligned = other.aligned;
        self.bits_left = other.bits_left;
    }

    const fn align(&mut self) {
        self.aligned = self.buffer << (64 - self.bits_left);
    }

    const fn peek_lookahead(&self) -> usize {
        (self.aligned >> (64 - LOOKAHEAD_BITS)) as usize
    }

    const fn peek_long(&self) -> i32 {
        (self.aligned >> (64 - LONG_CODE_BITS)) as i32
    }

    const fn drop_bits(&mut self, count: u8) {
        self.bits_left = self.bits_left.saturating_sub(count);
        self.aligned <<= count;
    }

    const fn get_bits(&mut self, count: u8) -> i32 {
        let mask = 1_u64.wrapping_shl(count as u32).wrapping_sub(1);
        self.aligned = self.aligned.rotate_left(count as u32);
        let bits = (self.aligned & mask) as i32;
        self.bits_left = self.bits_left.wrapping_sub(count);
        bits
    }

    const fn get_bit(&mut self) -> u8 {
        let bit = (self.aligned >> 63) as u8;
        self.drop_bits(1);
        bit
    }

    #[inline(always)]
    fn decode_symbol(&mut self, table: &HuffmanTable, entry: i32) -> Result<i32, JpegError> {
        let mut code_length = entry >> LOOKAHEAD_BITS;
        let mut symbol = entry & ((1 << LOOKAHEAD_BITS) - 1);
        if code_length > i32::from(LOOKAHEAD_BITS) {
            symbol = self.peek_long();
            while code_length < CODE_LENGTH_LIMIT {
                if symbol < table.maximum_code[code_length as usize] {
                    break;
                }
                code_length += 1;
            }
            if code_length == CODE_LENGTH_LIMIT {
                return Err(JpegError::HuffmanCode(symbol));
            }
            symbol >>= 16 - code_length;
            let index = (symbol.wrapping_add(table.offset[code_length as usize]) & 0xFF) as usize;
            symbol = i32::from(table.values[index]);
        }
        self.drop_bits(code_length as u8);
        Ok(symbol)
    }
}

pub(crate) struct BitStream {
    window: Window,
    pub(crate) marker: Option<Marker>,
    pub(crate) successive_low: u8,
    spectral_start: u8,
    spectral_end: u8,
    pub(crate) end_of_band_run: i32,
    pub(crate) overread: usize,
    pub(crate) seen_end_of_image: bool,
}

impl BitStream {
    pub(crate) const fn new(successive_low: u8, spectral_start: u8, spectral_end: u8) -> Self {
        Self {
            window: Window::EMPTY,
            marker: None,
            successive_low,
            spectral_start,
            spectral_end,
            end_of_band_run: 0,
            overread: 0,
            seen_end_of_image: false,
        }
    }

    pub(crate) const fn bits_left(&self) -> u8 {
        self.window.bits_left
    }

    pub(crate) const fn update_progressive(
        &mut self,
        successive_low: u8,
        spectral_start: u8,
        spectral_end: u8,
    ) {
        self.successive_low = successive_low;
        self.spectral_start = spectral_start;
        self.spectral_end = spectral_end;
    }

    pub(crate) const fn reset(&mut self) {
        self.window = Window::EMPTY;
        self.marker = None;
        self.end_of_band_run = 0;
    }

    #[inline(always)]
    pub(crate) fn refill(&mut self, reader: &mut ByteReader<'_>) {
        if self.window.bits_left >= REFILL_BITS_MIN {
            return;
        }
        self.refill_word_in_place(reader);
    }

    #[inline]
    fn refill_word_in_place(&mut self, reader: &mut ByteReader<'_>) {
        if self.stalled() {
            self.window.buffer <<= 32;
            self.window.bits_left += 32;
            self.window.align();
            return;
        }
        if let Some(word) = reader.read_u32() {
            if !has_marker_byte(word) {
                self.window.bits_left += 32;
                self.window.buffer = (self.window.buffer << 32) | u64::from(word);
                self.window.align();
                return;
            }
            reader.rewind(4);
        }
        self.refill_bytes_in_place(reader);
    }

    #[cold]
    fn refill_bytes_in_place(&mut self, reader: &mut ByteReader<'_>) {
        let mut window = self.window.copied();
        self.refill_bytes(&mut window, reader);
        self.window.store(&window);
    }

    const fn peek_lookahead(&self) -> usize {
        self.window.peek_lookahead()
    }

    const fn drop_bits(&mut self, count: u8) {
        self.window.drop_bits(count);
    }

    const fn get_bits(&mut self, count: u8) -> i32 {
        self.window.get_bits(count)
    }

    const fn get_bit(&mut self) -> u8 {
        self.window.get_bit()
    }

    #[inline(always)]
    fn decode_symbol(&mut self, table: &HuffmanTable, entry: i32) -> Result<i32, JpegError> {
        self.window.decode_symbol(table, entry)
    }

    fn decode_dc_in_place(
        &mut self,
        reader: &mut ByteReader<'_>,
        table: &HuffmanTable,
        prediction: &mut i32,
    ) -> Result<(), JpegError> {
        self.refill(reader);
        let entry = table.lookup[self.peek_lookahead()];
        let mut symbol = self.decode_symbol(table, entry)?;
        if symbol != 0 {
            let bits = self.get_bits(symbol as u8);
            symbol = extend(bits, symbol);
        }
        *prediction = prediction.wrapping_add(symbol);
        Ok(())
    }

    const fn stalled(&self) -> bool {
        self.marker.is_some() || self.overread > 0 || self.seen_end_of_image
    }

    #[inline(always)]
    fn refill_window(
        &mut self,
        window: &mut Window,
        reader: &mut ByteReader<'_>,
        stalled: &mut bool,
    ) {
        if window.bits_left >= REFILL_BITS_MIN {
            return;
        }
        self.refill_word(window, reader, stalled);
    }

    #[inline(always)]
    fn refill_word(
        &mut self,
        window: &mut Window,
        reader: &mut ByteReader<'_>,
        stalled: &mut bool,
    ) {
        if *stalled {
            window.buffer <<= 32;
            window.bits_left += 32;
            window.align();
            return;
        }
        if let Some(word) = reader.read_u32() {
            if !has_marker_byte(word) {
                window.bits_left += 32;
                window.buffer = (window.buffer << 32) | u64::from(word);
                window.align();
                return;
            }
            reader.rewind(4);
        }
        self.refill_bytes(window, reader);
        *stalled = self.stalled();
    }

    #[inline(always)]
    fn refill_bytes(&mut self, window: &mut Window, reader: &mut ByteReader<'_>) {
        for _ in 0..4 {
            if !self.refill_byte(window, reader) {
                return;
            }
        }
        window.align();
    }

    #[inline(always)]
    fn refill_byte(&mut self, window: &mut Window, reader: &mut ByteReader<'_>) -> bool {
        let byte = reader.read_u8_or_zero();
        self.overread += usize::from(reader.eof());
        window.buffer = (window.buffer << 8) | u64::from(byte);
        window.bits_left += 8;
        if byte != 0xFF {
            return true;
        }
        let mut next = reader.read_u8_or_zero();
        if next == 0 {
            return true;
        }
        while next == 0xFF {
            next = reader.read_u8_or_zero();
        }
        if next == 0 {
            return true;
        }
        window.buffer >>= 8;
        window.bits_left -= 8;
        if window.bits_left != 0 {
            window.aligned = window.buffer << (64 - window.bits_left);
        }
        self.marker = Marker::from_byte(next);
        if next == 0xD9 {
            window.buffer <<= 8;
            window.bits_left += 8;
            window.align();
        }
        false
    }

    #[inline(always)]
    fn decode_dc(
        &mut self,
        window: &mut Window,
        reader: &mut ByteReader<'_>,
        stalled: &mut bool,
        table: &HuffmanTable,
        prediction: &mut i32,
    ) -> Result<(), JpegError> {
        self.refill_window(window, reader, stalled);
        let entry = table.lookup[window.peek_lookahead()];
        let mut symbol = window.decode_symbol(table, entry)?;
        if symbol != 0 {
            let bits = window.get_bits(symbol as u8);
            symbol = extend(bits, symbol);
        }
        *prediction = prediction.wrapping_add(symbol);
        Ok(())
    }

    pub(crate) fn decode_block(
        &mut self,
        reader: &mut ByteReader<'_>,
        dc_table: &HuffmanTable,
        ac_table: &HuffmanTable,
        quantization: &[i32; 64],
        block: &mut [i32; 64],
        prediction: &mut i32,
    ) -> Result<(), JpegError> {
        let mut window = self.window.copied();
        let result = self.decode_block_window(
            &mut window,
            reader,
            dc_table,
            ac_table,
            quantization,
            block,
            prediction,
        );
        self.window.store(&window);
        result
    }

    #[inline(always)]
    #[allow(clippy::too_many_arguments)]
    fn decode_block_window(
        &mut self,
        window: &mut Window,
        reader: &mut ByteReader<'_>,
        dc_table: &HuffmanTable,
        ac_table: &HuffmanTable,
        quantization: &[i32; 64],
        block: &mut [i32; 64],
        prediction: &mut i32,
    ) -> Result<(), JpegError> {
        let mut stalled = self.stalled();
        self.decode_dc(window, reader, &mut stalled, dc_table, prediction)?;
        block[0] = prediction.wrapping_mul(quantization[0]);
        let mut position = 1_usize;
        while position < 64 {
            self.refill_window(window, reader, &mut stalled);
            let peeked = window.peek_lookahead();
            let fast = ac_table.fast_ac[peeked];
            if fast != 0 {
                position += ((fast >> 4) & 15) as usize;
                let target = UNZIGZAG[position.min(63)] & 63;
                block[target] = i32::from(fast >> 8).wrapping_mul(quantization[target]);
                window.drop_bits((fast & 15) as u8);
                position += 1;
                continue;
            }
            let symbol = window.decode_symbol(ac_table, ac_table.lookup[peeked])?;
            let run = symbol >> 4;
            let size = symbol & 15;
            if size != 0 {
                position += run as usize;
                let bits = window.get_bits(size as u8);
                let value = extend(bits, size);
                let target = UNZIGZAG[position & 63] & 63;
                block[target] = value.wrapping_mul(quantization[target]);
                position += 1;
            } else if run != 15 {
                return Ok(());
            } else {
                position += 16;
            }
        }
        Ok(())
    }

    pub(crate) fn decode_dc_first(
        &mut self,
        reader: &mut ByteReader<'_>,
        table: &HuffmanTable,
        coefficient: &mut i16,
        prediction: &mut i32,
    ) -> Result<(), JpegError> {
        self.decode_dc_in_place(reader, table, prediction)?;
        *coefficient = (*prediction as i16).wrapping_mul(1_i16 << self.successive_low);
        Ok(())
    }

    pub(crate) fn decode_dc_refine(&mut self, reader: &mut ByteReader<'_>, coefficient: &mut i16) {
        if self.window.bits_left < 1 {
            self.refill(reader);
        }
        if self.get_bit() == 1 {
            *coefficient = coefficient.wrapping_add(1 << self.successive_low);
        }
    }

    pub(crate) fn decode_ac_first(
        &mut self,
        reader: &mut ByteReader<'_>,
        table: &HuffmanTable,
        block: &mut [i16; 64],
    ) -> Result<(), JpegError> {
        let shift = self.successive_low;
        let mut position = usize::from(self.spectral_start);
        loop {
            self.refill(reader);
            let peeked = self.peek_lookahead();
            let fast = table.fast_ac[peeked];
            if fast != 0 {
                position += ((fast >> 4) & 15) as usize;
                block[UNZIGZAG[position.min(63)]] = (fast >> 8).wrapping_mul(1 << shift);
                self.drop_bits((fast & 15) as u8);
                position += 1;
            } else {
                let symbol = self.decode_symbol(table, table.lookup[peeked])?;
                let run = symbol >> 4;
                let size = symbol & 15;
                if size != 0 {
                    position += run as usize;
                    let bits = self.get_bits(size as u8);
                    let value = extend(bits, size);
                    block[UNZIGZAG[position & 63]] = (value as i16).wrapping_mul(1 << shift);
                    position += 1;
                } else if run != 15 {
                    self.end_of_band_run = 1 << run;
                    self.end_of_band_run += self.get_bits(run as u8);
                    self.end_of_band_run -= 1;
                    break;
                } else {
                    position += 16;
                }
            }
            if position > usize::from(self.spectral_end) {
                break;
            }
        }
        Ok(())
    }

    pub(crate) fn decode_ac_refine(
        &mut self,
        reader: &mut ByteReader<'_>,
        table: &HuffmanTable,
        block: &mut [i16; 64],
    ) -> Result<(), JpegError> {
        let bit = (1_i32 << self.successive_low) as i16;
        let mut position = self.spectral_start;
        if self.end_of_band_run == 0 {
            loop {
                self.refill(reader);
                let entry = table.lookup[self.peek_lookahead()];
                let decoded = self.decode_symbol(table, entry)?;
                let mut run = decoded >> 4;
                let size = decoded & 15;
                let mut value = 0;
                if size == 0 {
                    if run != 15 {
                        self.end_of_band_run = 1 << run;
                        self.end_of_band_run += self.get_bits(run as u8);
                        break;
                    }
                } else {
                    if size != 1 {
                        return Err(JpegError::RefineCode);
                    }
                    value = if self.get_bit() == 1 { bit } else { -bit };
                }
                if position <= self.spectral_end {
                    loop {
                        let coefficient = &mut block[UNZIGZAG[usize::from(position) & 63]];
                        if *coefficient != 0 {
                            if self.get_bit() == 1 && (*coefficient & bit) == 0 {
                                if *coefficient >= 0 {
                                    *coefficient = coefficient.wrapping_add(bit);
                                } else {
                                    *coefficient = coefficient.wrapping_sub(bit);
                                }
                            }
                            if self.window.bits_left < 1 {
                                self.refill(reader);
                            }
                        } else {
                            run -= 1;
                            if run < 0 {
                                break;
                            }
                        }
                        if position == self.spectral_end {
                            break;
                        }
                        position += 1;
                    }
                }
                if value != 0 {
                    block[UNZIGZAG[usize::from(position) & 63]] = value;
                }
                position += 1;
                if position > self.spectral_end {
                    break;
                }
            }
        }
        if self.end_of_band_run > 0 {
            if block[1..].iter().any(|&coefficient| coefficient != 0) {
                self.refill(reader);
                while position <= self.spectral_end {
                    let coefficient = &mut block[UNZIGZAG[usize::from(position) & 63]];
                    if *coefficient != 0 && self.get_bit() == 1 && (*coefficient & bit) == 0 {
                        if *coefficient >= 0 {
                            *coefficient = coefficient.wrapping_add(bit);
                        } else {
                            *coefficient = coefficient.wrapping_sub(bit);
                        }
                    }
                    if self.window.bits_left < 1 {
                        self.refill(reader);
                    }
                    position += 1;
                }
            }
            self.end_of_band_run -= 1;
        }
        Ok(())
    }
}

const fn extend(bits: i32, size: i32) -> i32 {
    let threshold = 1_i32.wrapping_shl(size.wrapping_sub(1) as u32);
    let sign = bits.wrapping_sub(threshold) >> 31;
    let offset = (-1_i32).wrapping_shl(size as u32).wrapping_add(1);
    bits.wrapping_add(sign & offset)
}

const fn has_marker_byte(word: u32) -> bool {
    let value = word ^ 0xFFFF_FFFF;
    !((((value & 0x7F7F_7F7F).wrapping_add(0x7F7F_7F7F)) | value) | 0x7F7F_7F7F) != 0
}
