use alloc::vec::Vec;

use super::tables::BYTE_BITS;

pub(super) struct BitWriter {
    pub(super) bytes: Vec<u8>,
    buffer: u64,
    count: u32,
}

#[derive(Clone, Copy)]
pub(super) struct Mark {
    length: usize,
    buffer: u64,
    count: u32,
}

impl BitWriter {
    pub(super) const fn new() -> Self {
        Self {
            bytes: Vec::new(),
            buffer: 0,
            count: 0,
        }
    }

    #[inline]
    pub(super) fn put(&mut self, bits: u32, count: u32) {
        debug_assert!(count <= u16::BITS);
        debug_assert!(u64::from(bits) >> count == 0);
        self.buffer |= u64::from(bits) << self.count;
        self.count += count;
        if self.count >= u32::BITS {
            self.bytes
                .extend_from_slice(&(self.buffer as u32).to_le_bytes());
            self.buffer >>= u32::BITS;
            self.count -= u32::BITS;
        }
    }

    #[inline]
    pub(super) const fn len(&self) -> usize {
        self.bytes.len() + (self.count / BYTE_BITS) as usize
    }

    pub(super) const fn is_aligned(&self) -> bool {
        self.count == 0
    }

    pub(super) const fn mark(&self) -> Mark {
        Mark {
            length: self.bytes.len(),
            buffer: self.buffer,
            count: self.count,
        }
    }

    pub(super) fn rewind(&mut self, mark: Mark) {
        self.bytes.truncate(mark.length);
        self.buffer = mark.buffer;
        self.count = mark.count;
    }

    pub(super) fn flush_bytes(&mut self) {
        while self.count >= BYTE_BITS {
            self.bytes.push(self.buffer as u8);
            self.buffer >>= BYTE_BITS;
            self.count -= BYTE_BITS;
        }
    }

    pub(super) fn align(&mut self) {
        self.put(0, (BYTE_BITS - self.count % BYTE_BITS) % BYTE_BITS);
        self.flush_bytes();
        assert!(self.count == 0);
    }
}
