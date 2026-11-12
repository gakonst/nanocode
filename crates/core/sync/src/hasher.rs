use std::hash::{BuildHasher, Hasher, RandomState};

const FIBONACCI_MULTIPLIER: u64 = 0x9E37_79B9_7F4A_7C15;
const WORD_BYTES: usize = 8;

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
        }
    }
}

pub(crate) struct FoldedHasher {
    accumulator: u64,
    key: u64,
}

impl FoldedHasher {
    const fn absorb(&mut self, word: u64) {
        self.accumulator =
            folded_multiply(self.accumulator ^ word, self.key ^ FIBONACCI_MULTIPLIER);
    }
}

impl Hasher for FoldedHasher {
    fn write(&mut self, bytes: &[u8]) {
        let (words, rest) = bytes.as_chunks::<WORD_BYTES>();
        for word in words {
            self.absorb(u64::from_le_bytes(*word));
        }
        let mut last = [0; WORD_BYTES];
        last[..rest.len()].copy_from_slice(rest);
        self.absorb(u64::from_le_bytes(last) ^ ((rest.len() as u64) << 56));
    }

    fn write_u8(&mut self, value: u8) {
        self.absorb(u64::from(value));
    }

    fn write_u16(&mut self, value: u16) {
        self.absorb(u64::from(value));
    }

    fn write_u32(&mut self, value: u32) {
        self.absorb(u64::from(value));
    }

    fn write_u64(&mut self, value: u64) {
        self.absorb(value);
    }

    fn write_usize(&mut self, value: usize) {
        self.absorb(value as u64);
    }

    fn finish(&self) -> u64 {
        folded_multiply(self.accumulator, self.key)
    }
}
