use alloc::vec::Vec;

pub(super) const WINDOW_GAP_BYTES: usize = 16;
const SHORT_COPY_BYTES: usize = WINDOW_GAP_BYTES;

pub(super) struct Window {
    pub buffer: Vec<u8>,
    pub position: usize,
    size_max: usize,
    flushed: usize,
    wrapped: bool,
}

impl Window {
    pub(super) const fn new() -> Self {
        Self { buffer: Vec::new(), position: 0, size_max: 0, flushed: 0, wrapped: false }
    }

    pub(super) const fn is_allocated(&self) -> bool {
        self.size_max != 0
    }

    pub(super) fn allocate(&mut self, size_max: usize, first_length: usize) {
        assert!(!self.is_allocated());
        assert!(size_max.is_power_of_two());
        self.size_max = size_max;
        self.buffer.resize(first_length.next_power_of_two().min(size_max), 0);
    }

    #[inline(always)]
    pub(super) const fn is_full(&self) -> bool {
        self.is_allocated() && self.position == self.size_max
    }

    #[inline(always)]
    pub(super) fn reserve(&mut self) -> bool {
        if self.position < self.buffer.len() {
            return true;
        }
        if self.buffer.len() == self.size_max {
            return false;
        }
        let size = (self.buffer.len() * 2).min(self.size_max);
        self.buffer.resize(size, 0);
        true
    }

    #[inline(always)]
    pub(super) const fn space(&self) -> usize {
        self.buffer.len() - self.position
    }

    #[inline(always)]
    pub(super) fn max_distance(&self) -> usize {
        let limit = self.size_max - WINDOW_GAP_BYTES;
        if self.wrapped { limit } else { self.position.min(limit) }
    }

    pub(super) const fn pending(&self) -> usize {
        self.position - self.flushed
    }

    pub(super) fn flush(&mut self, output: &mut [u8]) -> usize {
        let count = self.pending().min(output.len());
        output[..count].copy_from_slice(&self.buffer[self.flushed..self.flushed + count]);
        self.flushed += count;
        count
    }

    pub(super) fn wrap(&mut self) {
        assert!(self.is_full());
        assert!(self.pending() == 0);
        self.position = 0;
        self.flushed = 0;
        self.wrapped = true;
    }

    #[inline(always)]
    pub(super) fn previous(&self, back: usize) -> u8 {
        self.buffer[self.position.wrapping_sub(back) & (self.buffer.len() - 1)]
    }

    #[inline(always)]
    pub(super) fn copy(&mut self, distance: usize, length: usize) -> usize {
        let size = self.buffer.len();
        let count = length.min(size - self.position);
        let target = self.position;
        let source = target.wrapping_sub(distance) & (size - 1);
        if source < target && count <= SHORT_COPY_BYTES && distance >= SHORT_COPY_BYTES && target + SHORT_COPY_BYTES <= size {
            self.buffer.copy_within(source..source + SHORT_COPY_BYTES, target);
        } else if source < target && count <= SHORT_COPY_BYTES {
            let region = &mut self.buffer[source..target + count];
            for index in 0..count {
                region[distance + index] = region[index];
            }
        } else if source < target && distance >= count {
            self.buffer.copy_within(source..source + count, target);
        } else {
            copy_in_ring(&mut self.buffer, source, target, count);
        }
        self.position += count;
        count
    }

    pub(super) fn write(&mut self, bytes: &[u8]) -> usize {
        let count = bytes.len().min(self.space());
        self.buffer[self.position..self.position + count].copy_from_slice(&bytes[..count]);
        self.position += count;
        count
    }
}

fn copy_in_ring(buffer: &mut [u8], source: usize, target: usize, count: usize) {
    let size = buffer.len();
    let mut source = source;
    let mut target = target;
    let mut left = count;
    while left > 0 {
        let chunk = if source > target {
            let chunk = left.min(size - source);
            buffer.copy_within(source..source + chunk, target);
            source = (source + chunk) & (size - 1);
            chunk
        } else {
            let chunk = left.min(target - source);
            buffer.copy_within(source..source + chunk, target);
            chunk
        };
        target += chunk;
        left -= chunk;
    }
}
