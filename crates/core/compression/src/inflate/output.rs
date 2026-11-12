const WORD_BYTES: usize = 8;
const CHUNK_BYTES: usize = 16;

pub(super) struct Output<'a> {
    bytes: &'a mut [u8],
    filled: usize,
    counted_from: usize,
}

impl<'a> Output<'a> {
    #[inline(always)]
    pub(super) const fn new(bytes: &'a mut [u8]) -> Self {
        Self {
            bytes,
            filled: 0,
            counted_from: 0,
        }
    }

    #[inline(always)]
    pub(super) const fn reborrow(&mut self) -> Output<'_> {
        Output {
            bytes: self.bytes,
            filled: self.filled,
            counted_from: self.counted_from,
        }
    }

    #[inline(always)]
    pub(super) const fn resume(&mut self, filled: usize) {
        assert!(filled >= self.filled);
        assert!(filled <= self.bytes.len());
        self.filled = filled;
    }

    #[inline(always)]
    pub(super) const fn len(&self) -> usize {
        self.filled
    }

    #[inline(always)]
    pub(super) const fn remaining(&self) -> usize {
        self.bytes.len() - self.filled
    }

    #[inline(always)]
    pub(super) const fn is_full(&self) -> bool {
        self.filled == self.bytes.len()
    }

    #[inline(always)]
    pub(super) fn filled(&self) -> &[u8] {
        &self.bytes[..self.filled]
    }

    #[inline(always)]
    pub(super) fn counted(&self) -> &[u8] {
        &self.bytes[self.counted_from..self.filled]
    }

    #[inline(always)]
    pub(super) const fn restart_count(&mut self) {
        self.counted_from = self.filled;
    }

    #[inline(always)]
    pub(super) const fn push(&mut self, byte: u8) {
        self.bytes[self.filled] = byte;
        self.filled += 1;
    }

    #[inline(always)]
    pub(super) fn push_literals<const BYTES: usize>(&mut self, bytes: [u8; BYTES], count: usize) {
        assert!(count <= BYTES);
        *self.bytes[self.filled..]
            .first_chunk_mut::<BYTES>()
            .expect("the fast decoder keeps room for a literal entry") = bytes;
        self.filled += count;
    }

    #[inline(always)]
    pub(super) fn extend(&mut self, data: &[u8]) {
        self.bytes[self.filled..self.filled + data.len()].copy_from_slice(data);
        self.filled += data.len();
    }

    #[inline(always)]
    pub(super) fn copy_match_wide(&mut self, distance: usize, count: usize) {
        assert!(distance > 0);
        assert!(distance <= self.filled);
        if self.remaining() >= count + CHUNK_BYTES {
            if distance >= CHUNK_BYTES {
                self.copy_chunks::<CHUNK_BYTES>(distance, count);
                return;
            }
            if distance >= WORD_BYTES {
                self.copy_chunks::<WORD_BYTES>(distance, count);
                return;
            }
        }
        if distance == 1 {
            let byte = self.bytes[self.filled - 1];
            self.bytes[self.filled..self.filled + count].fill(byte);
            self.filled += count;
            return;
        }
        self.copy_match(distance, count);
    }

    #[inline(always)]
    fn copy_chunks<const BYTES: usize>(&mut self, distance: usize, count: usize) {
        assert!(distance >= BYTES);
        assert!(self.remaining() >= count + BYTES);
        let start = self.filled - distance;
        let mut copied = 0;
        while copied < count {
            let chunk = *self.bytes[start + copied..]
                .first_chunk::<BYTES>()
                .expect("the source lies before the end of the output");
            *self.bytes[self.filled + copied..]
                .first_chunk_mut::<BYTES>()
                .expect("the output keeps a chunk of room past the match") = chunk;
            copied += BYTES;
        }
        self.filled += count;
    }

    #[inline(always)]
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
