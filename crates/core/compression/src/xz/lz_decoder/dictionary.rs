use alloc::vec::Vec;

use super::super::lzma_common::DICT_SIZE_MIN;
use super::super::stream::Error;

pub(super) const REPEAT_MAX_BYTES: usize = 288;
pub(super) const REPEAT_AREA_BYTES: usize = 2 * REPEAT_MAX_BYTES;
const INITIAL_POSITION: usize = REPEAT_AREA_BYTES;
const SIZE_ALIGNMENT_BYTES: usize = 16;

pub(super) struct Dictionary {
    buffer: Vec<u8>,
    pub(super) position: usize,
    full: usize,
    pub(super) limit: usize,
    size: usize,
    has_wrapped: bool,
    need_reset: bool,
}

impl Dictionary {
    pub(super) fn new(dict_size: u32) -> Result<Self, Error> {
        let dict_size = usize::try_from(dict_size).map_err(|_| Error::Memory)?;
        let dict_size_min =
            usize::try_from(DICT_SIZE_MIN).expect("the minimum dictionary size fits in usize");
        let dict_size = dict_size.max(dict_size_min);
        if dict_size > usize::MAX - (SIZE_ALIGNMENT_BYTES - 1) - REPEAT_AREA_BYTES {
            return Err(Error::Memory);
        }
        let size = dict_size.next_multiple_of(SIZE_ALIGNMENT_BYTES) + REPEAT_AREA_BYTES;
        let mut buffer = Vec::new();
        buffer.try_reserve_exact(size).map_err(|_| Error::Memory)?;
        buffer.resize(INITIAL_POSITION, 0);
        let mut dictionary = Self {
            buffer,
            position: INITIAL_POSITION,
            full: 0,
            limit: INITIAL_POSITION,
            size,
            has_wrapped: false,
            need_reset: false,
        };
        dictionary.reset();
        Ok(dictionary)
    }

    pub(super) fn reset(&mut self) {
        self.position = INITIAL_POSITION;
        self.full = 0;
        self.buffer[INITIAL_POSITION - 1] = 0;
        self.has_wrapped = false;
        self.need_reset = false;
    }

    pub(super) const fn request_reset(&mut self) {
        self.need_reset = true;
    }

    pub(super) const fn take_reset_request(&mut self) -> bool {
        let need_reset = self.need_reset;
        self.need_reset = false;
        need_reset
    }

    pub(super) const fn is_full(&self) -> bool {
        self.position == self.size
    }

    pub(super) fn wrap_if_full(&mut self) {
        if self.is_full() {
            assert!(self.buffer.len() == self.size);
            self.position = REPEAT_MAX_BYTES;
            self.has_wrapped = true;
            self.buffer
                .copy_within(self.size - REPEAT_MAX_BYTES..self.size, 0);
        }
    }

    pub(super) fn set_limit(&mut self, output_available: usize) {
        assert!(self.position < self.size);
        self.limit = self.position + output_available.min(self.size - self.position);
    }

    pub(super) fn copy_since(&self, start: usize, output: &mut [u8]) -> usize {
        assert!(start <= self.position);
        let count = self.position - start;
        output[..count].copy_from_slice(&self.buffer[start..self.position]);
        count
    }

    #[inline(always)]
    pub(super) fn get(&self, distance: u32) -> u8 {
        let distance = distance as usize;
        let index = if distance < self.position {
            self.position - distance - 1
        } else {
            self.position + (self.size - REPEAT_MAX_BYTES) - distance - 1
        };
        self.buffer[index]
    }

    #[inline(always)]
    pub(super) fn get_previous(&self) -> u8 {
        self.buffer[self.position - 1]
    }

    #[inline(always)]
    pub(super) const fn is_distance_valid(&self, distance: u32) -> bool {
        self.full > distance as usize
    }

    #[inline(always)]
    pub(super) const fn is_at_limit(&self) -> bool {
        self.position == self.limit
    }

    #[inline(always)]
    const fn update_full(&mut self) {
        if !self.has_wrapped {
            self.full = self.position - INITIAL_POSITION;
        }
    }

    #[inline(always)]
    pub(super) fn put(&mut self, byte: u8) {
        if self.position < self.buffer.len() {
            self.buffer[self.position] = byte;
        } else {
            debug_assert!(self.position == self.buffer.len());
            self.buffer.push(byte);
        }
        self.position += 1;
        self.update_full();
    }

    #[inline(always)]
    pub(super) fn try_put(&mut self, byte: u8) -> bool {
        if self.is_at_limit() {
            return false;
        }
        self.put(byte);
        true
    }

    fn initialize_to(&mut self, end: usize) {
        assert!(end <= self.size);
        if end > self.buffer.len() {
            self.buffer.resize(end, 0);
        }
    }

    #[inline(always)]
    pub(super) fn repeat(&mut self, distance: u32, len: &mut u32) -> bool {
        let left = (self.limit - self.position).min(*len as usize);
        *len -= u32::try_from(left).expect("a repeat is at most one match long");
        let distance = distance as usize;
        let back = if distance < self.position {
            self.position - distance - 1
        } else {
            self.position + (self.size - REPEAT_MAX_BYTES) - distance - 1
        };
        let end = self.position + left;
        if end <= self.buffer.len() {
            if distance < left {
                for index in 0..left {
                    self.buffer[self.position + index] = self.buffer[back + index];
                }
            } else {
                self.buffer.copy_within(back..back + left, self.position);
            }
        } else {
            self.repeat_growing(back, left, distance < left);
        }
        self.position = end;
        self.update_full();
        *len != 0
    }

    fn repeat_growing(&mut self, back: usize, left: usize, overlapping: bool) {
        let initialized = self.buffer.len() - self.position;
        for index in 0..initialized {
            self.buffer[self.position + index] = self.buffer[back + index];
        }
        assert!(self.buffer.len() == self.position + initialized);
        if overlapping {
            for index in initialized..left {
                let byte = self.buffer[back + index];
                self.buffer.push(byte);
            }
        } else {
            self.buffer
                .extend_from_within(back + initialized..back + left);
        }
    }

    pub(super) fn write(&mut self, input: &[u8], input_position: &mut usize, left: &mut usize) {
        assert!(*input_position <= input.len());
        let count = (input.len() - *input_position)
            .min(*left)
            .min(self.limit - self.position);
        let end = self.position + count;
        self.initialize_to(end);
        self.buffer[self.position..end]
            .copy_from_slice(&input[*input_position..*input_position + count]);
        *input_position += count;
        *left -= count;
        self.position = end;
        self.update_full();
    }
}
