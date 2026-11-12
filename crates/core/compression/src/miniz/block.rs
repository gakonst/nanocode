use alloc::vec::Vec;

use super::tables::{
    CODE_BUFFER_BYTES, CODE_BUFFER_SLACK_BYTES, CODE_FLAGS_BITS, DICTIONARY_BYTES,
    DISTANCE_SYMBOLS, FAT_BLOCK_BYTES, FAT_RATIO_NUMERATOR, FAT_RATIO_SHIFT, LENGTH_SYMBOL,
    LITERAL_CODES_MIN, LITERAL_SYMBOLS, MATCH_CODE_BYTES, MATCH_LENGTH_MAX, MATCH_LENGTH_MIN,
    distance_code,
};

#[derive(Clone, Copy)]
pub(super) enum Code {
    Literal(u8),
    Match { length: u8, offset: u16 },
}

pub(super) struct Block {
    pub(super) codes: Vec<Code>,
    pub(super) position: usize,
    pub(super) total_bytes: u32,
    pub(super) literal_counts: [u16; LITERAL_SYMBOLS],
    pub(super) distance_counts: [u16; DISTANCE_SYMBOLS],
}

impl Block {
    pub(super) const fn new() -> Self {
        Self {
            codes: Vec::new(),
            position: 1,
            total_bytes: 0,
            literal_counts: [0; LITERAL_SYMBOLS],
            distance_counts: [0; DISTANCE_SYMBOLS],
        }
    }

    pub(super) fn reset(&mut self) {
        self.codes.clear();
        self.position = 1;
        self.total_bytes = 0;
        self.literal_counts = [0; LITERAL_SYMBOLS];
        self.distance_counts = [0; DISTANCE_SYMBOLS];
    }

    #[inline]
    pub(super) const fn after_code(&mut self, bytes: usize) {
        self.position += bytes;
        if self.codes.len().is_multiple_of(CODE_FLAGS_BITS) {
            self.position += 1;
        }
    }

    #[inline]
    pub(super) fn literal(&mut self, byte: u8) {
        self.total_bytes += 1;
        self.codes.push(Code::Literal(byte));
        self.literal_counts[usize::from(byte)] += 1;
        self.after_code(1);
    }

    #[inline]
    pub(super) fn matched(&mut self, length: u32, distance: u32) {
        assert!(length as usize >= MATCH_LENGTH_MIN);
        assert!(length as usize <= MATCH_LENGTH_MAX);
        assert!(distance >= 1);
        assert!(distance as usize <= DICTIONARY_BYTES);
        self.total_bytes += length;
        let length = (length as usize - MATCH_LENGTH_MIN) as u8;
        let offset = (distance - 1) as u16;
        self.codes.push(Code::Match { length, offset });
        self.distance_counts[distance_code(usize::from(offset))] += 1;
        self.literal_counts[LITERAL_CODES_MIN + usize::from(LENGTH_SYMBOL[usize::from(length)])] +=
            1;
        self.after_code(MATCH_CODE_BYTES);
    }

    #[inline]
    pub(super) const fn tight(&self) -> bool {
        self.position > CODE_BUFFER_BYTES - CODE_BUFFER_SLACK_BYTES
    }

    #[inline]
    pub(super) const fn fat(&self) -> bool {
        self.total_bytes > FAT_BLOCK_BYTES
            && (self.position * FAT_RATIO_NUMERATOR) >> FAT_RATIO_SHIFT >= self.total_bytes as usize
    }
}
