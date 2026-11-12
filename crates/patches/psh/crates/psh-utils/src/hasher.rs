use std::hash::{BuildHasher, Hasher, RandomState};

const FIBONACCI_MULTIPLIER: u64 = 0x9E37_79B9_7F4A_7C15;
const WORD_BYTES: usize = 8;
const PAIR_BYTES: usize = 2 * WORD_BYTES;
const HALF_WORD_BYTES: usize = WORD_BYTES / 2;
const LENGTH_SHIFT_BITS: usize = 56;

const fn folded_multiply(left: u64, right: u64) -> u64 {
    let full = (left as u128) * (right as u128);
    (full as u64) ^ ((full >> u64::BITS) as u64)
}

#[derive(Clone, Debug)]
pub(crate) struct SeededState {
    seed: u64,
    key: u64,
}

impl Default for SeededState {
    fn default() -> Self {
        let keys = RandomState::new();
        Self {
            seed: keys.hash_one(0_u8),
            key: keys.hash_one(1_u8) | 1,
        }
    }
}

impl BuildHasher for SeededState {
    type Hasher = FoldedHasher;

    fn build_hasher(&self) -> FoldedHasher {
        FoldedHasher {
            accumulator: self.seed,
            key: self.key,
            sponge: 0,
            sponge_bits: 0,
        }
    }
}

pub(crate) struct FoldedHasher {
    accumulator: u64,
    key: u64,
    sponge: u128,
    sponge_bits: u32,
}

impl FoldedHasher {
    const fn absorb(&mut self, low: u64, high: u64) {
        self.accumulator = folded_multiply(
            self.accumulator ^ low,
            self.key ^ high ^ FIBONACCI_MULTIPLIER,
        );
    }

    fn absorb_number(&mut self, value: u64, bits: u32) {
        assert!(bits <= u64::BITS);
        assert!(bits == u64::BITS || value >> bits == 0);
        if self.sponge_bits + bits > u128::BITS {
            self.absorb(self.sponge as u64, (self.sponge >> u64::BITS) as u64);
            self.sponge = 0;
            self.sponge_bits = 0;
        }
        self.sponge |= u128::from(value) << self.sponge_bits;
        self.sponge_bits += bits;
    }
}

fn words(pair: &[u8; PAIR_BYTES]) -> (u64, u64) {
    let (words, rest) = pair.as_chunks::<WORD_BYTES>();
    assert!(words.len() == 2 && rest.is_empty());
    (u64::from_le_bytes(words[0]), u64::from_le_bytes(words[1]))
}

fn tail_words(rest: &[u8]) -> (u64, u64) {
    assert!(rest.len() < PAIR_BYTES);
    let length = rest.len();
    if length >= WORD_BYTES {
        let (first, _) = rest.split_first_chunk::<WORD_BYTES>().expect("eight bytes");
        let (_, last) = rest.split_last_chunk::<WORD_BYTES>().expect("eight bytes");
        (u64::from_le_bytes(*first), u64::from_le_bytes(*last))
    } else if length >= HALF_WORD_BYTES {
        let (first, _) = rest
            .split_first_chunk::<HALF_WORD_BYTES>()
            .expect("four bytes");
        let (_, last) = rest
            .split_last_chunk::<HALF_WORD_BYTES>()
            .expect("four bytes");
        (
            u64::from(u32::from_le_bytes(*first)),
            u64::from(u32::from_le_bytes(*last)),
        )
    } else if length > 0 {
        let low = u64::from(rest[0]) | (u64::from(rest[length / 2]) << u8::BITS);
        (low, u64::from(rest[length - 1]))
    } else {
        (0, 0)
    }
}

impl Hasher for FoldedHasher {
    fn write(&mut self, bytes: &[u8]) {
        let (pairs, rest) = bytes.as_chunks::<PAIR_BYTES>();
        for pair in pairs {
            let (low, high) = words(pair);
            self.absorb(low, high);
        }
        let (low, high) = tail_words(rest);
        self.absorb(low, high ^ ((rest.len() as u64) << LENGTH_SHIFT_BITS));
    }

    fn write_u8(&mut self, value: u8) {
        self.absorb_number(u64::from(value), u8::BITS);
    }

    fn write_u16(&mut self, value: u16) {
        self.absorb_number(u64::from(value), u16::BITS);
    }

    fn write_u32(&mut self, value: u32) {
        self.absorb_number(u64::from(value), u32::BITS);
    }

    fn write_u64(&mut self, value: u64) {
        self.absorb_number(value, u64::BITS);
    }

    fn write_usize(&mut self, value: usize) {
        self.absorb_number(value as u64, usize::BITS);
    }

    fn finish(&self) -> u64 {
        folded_multiply(
            self.accumulator ^ self.sponge as u64,
            self.key ^ (self.sponge >> u64::BITS) as u64 ^ FIBONACCI_MULTIPLIER,
        )
    }
}
