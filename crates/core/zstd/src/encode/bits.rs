pub(crate) struct BitWriter<'a> {
    out: &'a mut Vec<u8>,
    container: u64,
    count: u32,
}

pub(crate) const BITS_PER_ADD_MAX: u32 = 56;

impl<'a> BitWriter<'a> {
    pub(crate) const fn new(out: &'a mut Vec<u8>) -> Self {
        Self {
            out,
            container: 0,
            count: 0,
        }
    }

    #[inline]
    pub(crate) fn add(&mut self, value: u64, bits: u32) {
        debug_assert!(bits <= BITS_PER_ADD_MAX, "{bits} bits added at once");
        if self.count + bits >= u64::BITS {
            self.flush();
        }
        let mask = (1_u64 << bits).wrapping_sub(1);
        self.container |= (value & mask) << self.count;
        self.count += bits;
    }

    #[inline]
    fn flush(&mut self) {
        let bytes = (self.count / 8) as usize;
        let length = self.out.len();
        self.out.extend_from_slice(&self.container.to_le_bytes());
        self.out.truncate(length + bytes);
        self.container = self.container.checked_shr(bytes as u32 * 8).unwrap_or(0);
        self.count -= bytes as u32 * 8;
    }

    pub(crate) fn close(mut self) {
        self.add(1, 1);
        self.flush();
        if self.count > 0 {
            self.out.push(self.container as u8);
        }
    }
}
