const WORD_BYTES: usize = 8;

pub(super) struct Output<'a> {
    bytes: &'a mut [u8],
    filled: usize,
    counted_from: usize,
}

impl<'a> Output<'a> {
    pub(super) const fn new(bytes: &'a mut [u8]) -> Self {
        Self {
            bytes,
            filled: 0,
            counted_from: 0,
        }
    }

    pub(super) const fn len(&self) -> usize {
        self.filled
    }

    pub(super) const fn remaining(&self) -> usize {
        self.bytes.len() - self.filled
    }

    pub(super) const fn is_full(&self) -> bool {
        self.filled == self.bytes.len()
    }

    pub(super) fn filled(&self) -> &[u8] {
        &self.bytes[..self.filled]
    }

    pub(super) fn counted(&self) -> &[u8] {
        &self.bytes[self.counted_from..self.filled]
    }

    pub(super) const fn restart_count(&mut self) {
        self.counted_from = self.filled;
    }

    pub(super) const fn push(&mut self, byte: u8) {
        self.bytes[self.filled] = byte;
        self.filled += 1;
    }

    pub(super) fn extend(&mut self, data: &[u8]) {
        self.bytes[self.filled..self.filled + data.len()].copy_from_slice(data);
        self.filled += data.len();
    }

    pub(super) fn copy_match_wide(&mut self, distance: usize, count: usize) {
        if distance < WORD_BYTES || self.remaining() < count + WORD_BYTES {
            self.copy_match(distance, count);
            return;
        }
        assert!(distance <= self.filled);
        let start = self.filled - distance;
        let mut copied = 0;
        while copied < count {
            let word = *self.bytes[start + copied..]
                .first_chunk::<WORD_BYTES>()
                .expect("the source lies before the end of the output");
            self.bytes[self.filled + copied..self.filled + copied + WORD_BYTES]
                .copy_from_slice(&word);
            copied += WORD_BYTES;
        }
        self.filled += count;
    }

    pub(super) fn copy_match(&mut self, distance: usize, count: usize) {
        assert!(distance > 0);
        assert!(distance <= self.filled);
        assert!(count <= self.remaining());
        let start = self.filled - distance;
        if count <= distance {
            self.bytes.copy_within(start..start + count, self.filled);
        } else {
            let mut copied = 0;
            while copied < count {
                let chunk = (self.filled + copied - start).min(count - copied);
                self.bytes
                    .copy_within(start..start + chunk, self.filled + copied);
                copied += chunk;
            }
        }
        self.filled += count;
    }
}
