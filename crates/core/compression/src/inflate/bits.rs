use super::Stop;

const HOLD_BITS: u32 = u64::BITS;
const REFILL_BYTES: usize = 8;
const PEEK_BITS_MAX: u32 = u32::BITS;

#[derive(Clone, Copy)]
pub(super) struct Input<'a> {
    bytes: &'a [u8],
    position: usize,
    hold: u64,
    bits: u32,
}

impl<'a> Input<'a> {
    #[inline(always)]
    pub(super) const fn new(bytes: &'a [u8], hold: u64, bits: u32) -> Self {
        Self {
            bytes,
            position: 0,
            hold,
            bits,
        }
    }

    #[inline(always)]
    pub(super) const fn position(&self) -> usize {
        self.position
    }

    #[inline(always)]
    pub(super) const fn buffer(&self) -> (u64, u32) {
        (self.hold, self.bits)
    }

    #[inline(always)]
    pub(super) const fn hold(&self) -> u64 {
        self.hold
    }

    #[inline(always)]
    pub(super) const fn bits(&self) -> u32 {
        self.bits
    }

    #[inline(always)]
    pub(super) const fn remaining(&self) -> usize {
        self.bytes.len() - self.position
    }

    #[inline(always)]
    pub(super) fn remaining_with_buffer(&self) -> usize {
        self.remaining() + bytes_of(self.bits / u8::BITS)
    }

    #[inline(always)]
    pub(super) fn rest(&self) -> &'a [u8] {
        &self.bytes[self.position..]
    }

    #[inline(always)]
    pub(super) fn advance(&mut self, count: usize) {
        assert!(count <= self.remaining());
        self.position += count;
    }

    #[inline(always)]
    pub(super) fn pull_byte(&mut self) -> Result<(), Stop> {
        let Some(&byte) = self.bytes.get(self.position) else {
            return Err(Stop::Suspend);
        };
        self.position += 1;
        self.hold |= u64::from(byte) << self.bits;
        self.bits += u8::BITS;
        Ok(())
    }

    #[inline(always)]
    pub(super) fn need(&mut self, count: u32) -> Result<(), Stop> {
        while self.bits < count {
            self.pull_byte()?;
        }
        Ok(())
    }

    #[inline(always)]
    pub(super) fn peek(&self, count: u32) -> usize {
        debug_assert!(count <= PEEK_BITS_MAX);
        let value = self.hold & ((1_u64 << count) - 1);
        usize::try_from(value).expect("at most 32 bits fit in usize")
    }

    #[inline(always)]
    pub(super) const fn low_word(&self) -> u32 {
        low_u32(self.hold)
    }

    #[inline(always)]
    pub(super) fn consume(&mut self, count: u32) {
        debug_assert!(count <= self.bits);
        self.hold >>= count;
        self.bits -= count;
    }

    #[inline(always)]
    pub(super) fn byte_boundary(&mut self) {
        self.consume(self.bits % u8::BITS);
    }

    #[inline(always)]
    pub(super) const fn clear(&mut self) {
        self.hold = 0;
        self.bits = 0;
    }

    #[inline(always)]
    pub(super) fn refill(&mut self) {
        let word = self.bytes[self.position..]
            .first_chunk::<REFILL_BYTES>()
            .expect("the fast decoder keeps at least 8 input bytes ahead");
        self.hold |= u64::from_le_bytes(*word) << self.bits;
        let count = (HOLD_BITS - 1 - self.bits) / u8::BITS;
        self.position += bytes_of(count);
        self.bits += count * u8::BITS;
    }

    #[inline(always)]
    pub(super) fn return_unused(&mut self) {
        let count = self.bits / u8::BITS;
        self.position -= bytes_of(count);
        self.bits -= count * u8::BITS;
        self.hold &= (1_u64 << self.bits) - 1;
    }
}

#[inline(always)]
fn bytes_of(count: u32) -> usize {
    usize::try_from(count).expect("a byte count under 8 fits in usize")
}

#[inline(always)]
pub(super) const fn low_u32(value: u64) -> u32 {
    let bytes = value.to_le_bytes();
    u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}
