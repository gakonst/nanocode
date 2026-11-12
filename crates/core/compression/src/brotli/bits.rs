const BYTE_BITS: u32 = 8;
const WORD_BYTES: usize = 8;
const ACCUMULATOR_BITS: u32 = u64::BITS;
const REFILL_LIMIT_BITS: u32 = ACCUMULATOR_BITS - BYTE_BITS;

pub(super) const fn low_mask(bits: u32) -> u64 {
    if bits >= ACCUMULATOR_BITS {
        u64::MAX
    } else {
        (1 << bits) - 1
    }
}

#[derive(Clone, Copy)]
pub(super) struct BitBuffer {
    value: u64,
    count: u32,
}

impl BitBuffer {
    pub(super) const fn new() -> Self {
        Self { value: 0, count: 0 }
    }
}

pub(super) struct Bits<'a> {
    value: u64,
    count: u32,
    input: &'a [u8],
    position: usize,
}

impl<'a> Bits<'a> {
    pub(super) const fn new(buffer: BitBuffer, input: &'a [u8]) -> Self {
        Self {
            value: buffer.value,
            count: buffer.count,
            input,
            position: 0,
        }
    }

    pub(super) const fn finish(&self) -> BitBuffer {
        BitBuffer {
            value: self.value & low_mask(self.count),
            count: self.count,
        }
    }

    pub(super) const fn position(&self) -> usize {
        self.position
    }

    #[inline(always)]
    pub(super) const fn available(&self) -> u32 {
        self.count
    }

    #[inline(always)]
    pub(super) const fn input_remaining(&self) -> usize {
        self.input.len() - self.position
    }

    #[inline(always)]
    pub(super) const fn has(&self, needed: u32) -> bool {
        let remaining = self.input.len() - self.position;
        remaining >= WORD_BYTES || self.count as usize + remaining * BYTE_BITS as usize >= needed as usize
    }

    #[inline(always)]
    pub(super) fn refill(&mut self) {
        if self.count > REFILL_LIMIT_BITS {
            return;
        }
        if let Some(chunk) = self.input.get(self.position..self.position + WORD_BYTES) {
            let mut word = [0; WORD_BYTES];
            word.copy_from_slice(chunk);
            self.value |= u64::from_le_bytes(word) << self.count;
            let bytes = (ACCUMULATOR_BITS - 1 - self.count) / BYTE_BITS;
            self.position += bytes as usize;
            self.count += bytes * BYTE_BITS;
        } else {
            self.refill_tail();
        }
    }

    #[inline(always)]
    pub(super) fn ensure(&mut self, bits: u32) {
        if self.count < bits {
            self.refill();
        }
    }

    fn refill_tail(&mut self) {
        while self.count <= REFILL_LIMIT_BITS {
            let Some(&byte) = self.input.get(self.position) else {
                break;
            };
            self.value |= u64::from(byte) << self.count;
            self.position += 1;
            self.count += BYTE_BITS;
        }
    }

    #[inline(always)]
    pub(super) const fn peek(&self, bits: u32) -> u32 {
        (self.value & low_mask(bits)) as u32
    }

    #[inline(always)]
    pub(super) const fn word(&self) -> u64 {
        self.value
    }

    #[inline(always)]
    pub(super) fn consume(&mut self, bits: u32) {
        debug_assert!(bits <= self.count);
        self.value >>= bits;
        self.count -= bits;
    }

    #[inline(always)]
    pub(super) fn read(&mut self, bits: u32) -> u32 {
        if self.count < bits {
            self.refill();
        }
        assert!(self.count >= bits);
        let value = self.peek(bits);
        self.consume(bits);
        value
    }

    pub(super) fn try_read(&mut self, bits: u32) -> Option<u32> {
        if self.count < bits {
            self.refill();
            if self.count < bits {
                return None;
            }
        }
        Some(self.read(bits))
    }

    pub(super) fn absorb(&mut self) {
        while let Some(&byte) = self.input.get(self.position) {
            assert!(self.count + BYTE_BITS <= ACCUMULATOR_BITS);
            self.value |= u64::from(byte) << self.count;
            self.position += 1;
            self.count += BYTE_BITS;
        }
    }

    pub(super) fn unload(&mut self) {
        let bytes = self.count / BYTE_BITS;
        assert!(bytes as usize <= self.position);
        self.position -= bytes as usize;
        self.count -= bytes * BYTE_BITS;
    }

    pub(super) fn align(&mut self) -> u32 {
        let padding = self.count % BYTE_BITS;
        let bits = self.peek(padding);
        self.consume(padding);
        bits
    }

    pub(super) fn take_bytes(&mut self, target: &mut [u8]) -> usize {
        assert!(self.count.is_multiple_of(BYTE_BITS));
        let mut taken = 0;
        while taken < target.len() && self.count != 0 {
            target[taken] = self.peek(BYTE_BITS) as u8;
            self.consume(BYTE_BITS);
            taken += 1;
        }
        let direct = (target.len() - taken).min(self.input_remaining());
        target[taken..taken + direct].copy_from_slice(&self.input[self.position..self.position + direct]);
        self.position += direct;
        if self.count == 0 {
            self.value = 0;
        }
        taken + direct
    }

    pub(super) fn skip_bytes(&mut self, count: usize) -> usize {
        assert!(self.count.is_multiple_of(BYTE_BITS));
        let mut skipped = 0;
        while skipped < count && self.count != 0 {
            self.consume(BYTE_BITS);
            skipped += 1;
        }
        let direct = (count - skipped).min(self.input_remaining());
        self.position += direct;
        if self.count == 0 {
            self.value = 0;
        }
        skipped + direct
    }
}
