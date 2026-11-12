use core::hash::Hasher;

const INITIALIZATION: [u64; 4] = [
    0x736f_6d65_7073_6575,
    0x646f_7261_6e64_6f6d,
    0x6c79_6765_6e65_7261,
    0x7465_6462_7974_6573,
];
const COMPRESSION_ROUNDS: usize = 2;
const FINALIZATION_ROUNDS: usize = 4;
const FINALIZATION_MARKER: u64 = 0xff;
const WORD_BYTES: usize = 8;
const LENGTH_SHIFT_BITS: u32 = 56;

#[derive(Clone, Copy, Debug)]
pub struct SipHasher24 {
    state: [u64; 4],
    tail: u64,
    tail_bytes: usize,
    length_bytes: usize,
}

impl SipHasher24 {
    #[inline]
    #[must_use]
    pub const fn new_with_keys(key0: u64, key1: u64) -> Self {
        Self {
            state: [
                key0 ^ INITIALIZATION[0],
                key1 ^ INITIALIZATION[1],
                key0 ^ INITIALIZATION[2],
                key1 ^ INITIALIZATION[3],
            ],
            tail: 0,
            tail_bytes: 0,
            length_bytes: 0,
        }
    }

    #[inline]
    fn short_write(&mut self, value: u64, size: usize) {
        assert!(self.tail_bytes < WORD_BYTES);
        assert!(size <= WORD_BYTES);
        self.length_bytes = self.length_bytes.wrapping_add(size);
        let needed = WORD_BYTES - self.tail_bytes;
        self.tail |= value << (8 * self.tail_bytes);
        if size < needed {
            self.tail_bytes += size;
            return;
        }
        absorb(&mut self.state, self.tail);
        self.tail_bytes = size - needed;
        self.tail = if needed < WORD_BYTES {
            value >> (8 * needed)
        } else {
            0
        };
    }
}

impl Hasher for SipHasher24 {
    #[inline]
    fn write_u8(&mut self, value: u8) {
        self.short_write(u64::from(value), size_of::<u8>());
    }

    #[inline]
    fn write_u16(&mut self, value: u16) {
        self.short_write(u64::from(value.to_le()), size_of::<u16>());
    }

    #[inline]
    fn write_u32(&mut self, value: u32) {
        self.short_write(u64::from(value.to_le()), size_of::<u32>());
    }

    #[inline]
    fn write_u64(&mut self, value: u64) {
        self.short_write(value.to_le(), size_of::<u64>());
    }

    #[inline]
    fn write_usize(&mut self, value: usize) {
        self.short_write(value.to_le() as u64, size_of::<usize>());
    }

    #[inline]
    fn write(&mut self, mut bytes: &[u8]) {
        assert!(self.tail_bytes < WORD_BYTES);
        self.length_bytes = self.length_bytes.wrapping_add(bytes.len());
        if self.tail_bytes > 0 {
            let taken = bytes.len().min(WORD_BYTES - self.tail_bytes);
            self.tail |= little_endian(&bytes[..taken]) << (8 * self.tail_bytes);
            self.tail_bytes += taken;
            bytes = &bytes[taken..];
            if self.tail_bytes < WORD_BYTES {
                return;
            }
            absorb(&mut self.state, self.tail);
        }
        let (words, rest) = bytes.as_chunks::<WORD_BYTES>();
        for word in words {
            absorb(&mut self.state, u64::from_le_bytes(*word));
        }
        self.tail = little_endian(rest);
        self.tail_bytes = rest.len();
    }

    #[inline]
    fn finish(&self) -> u64 {
        let mut state = self.state;
        let last = ((self.length_bytes as u64) << LENGTH_SHIFT_BITS) | self.tail;
        absorb(&mut state, last);
        state[2] ^= FINALIZATION_MARKER;
        for _ in 0..FINALIZATION_ROUNDS {
            round(&mut state);
        }
        state[0] ^ state[1] ^ state[2] ^ state[3]
    }
}

#[inline]
fn little_endian(bytes: &[u8]) -> u64 {
    assert!(bytes.len() < WORD_BYTES);
    bytes
        .iter()
        .rev()
        .fold(0, |word, byte| (word << 8) | u64::from(*byte))
}

#[inline]
fn absorb(state: &mut [u64; 4], word: u64) {
    state[3] ^= word;
    for _ in 0..COMPRESSION_ROUNDS {
        round(state);
    }
    state[0] ^= word;
}

#[inline]
fn round(state: &mut [u64; 4]) {
    let [mut v0, mut v1, mut v2, mut v3] = *state;
    v0 = v0.wrapping_add(v1);
    v1 = v1.rotate_left(13) ^ v0;
    v0 = v0.rotate_left(32);
    v2 = v2.wrapping_add(v3);
    v3 = v3.rotate_left(16) ^ v2;
    v0 = v0.wrapping_add(v3);
    v3 = v3.rotate_left(21) ^ v0;
    v2 = v2.wrapping_add(v1);
    v1 = v1.rotate_left(17) ^ v2;
    v2 = v2.rotate_left(32);
    *state = [v0, v1, v2, v3];
}
