use core::hash::{BuildHasher, Hasher};
use core::sync::atomic::{AtomicU64, Ordering};

const MIX_MULTIPLIER: u64 = 0x9e37_79b9_7f4a_7c15;
const MIX_SECOND: u64 = 0xbf58_476d_1ce4_e5b9;
const WORD_BYTES: usize = 8;

static INSTANCE_COUNTER: AtomicU64 = AtomicU64::new(0);

fn process_seed() -> u64 {
    static SEED: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *SEED.get_or_init(|| std::collections::hash_map::RandomState::new().hash_one(MIX_MULTIPLIER))
}

#[inline]
const fn folded_multiply(left: u64, right: u64) -> u64 {
    let full = (left as u128).wrapping_mul(right as u128);
    (full as u64) ^ ((full >> 64) as u64)
}

#[derive(Clone, Copy, Debug)]
pub struct RandomState {
    seed: u64,
}

impl RandomState {
    #[must_use]
    pub fn new() -> Self {
        let instance = INSTANCE_COUNTER.fetch_add(1, Ordering::Relaxed);
        Self {
            seed: folded_multiply(process_seed() ^ instance, MIX_MULTIPLIER),
        }
    }
}

impl Default for RandomState {
    fn default() -> Self {
        Self::new()
    }
}

impl BuildHasher for RandomState {
    type Hasher = FastHasher;

    #[inline]
    fn build_hasher(&self) -> FastHasher {
        FastHasher { state: self.seed }
    }
}

#[derive(Clone, Debug)]
pub struct FastHasher {
    state: u64,
}

impl FastHasher {
    #[inline]
    const fn mix(&mut self, value: u64) {
        self.state = folded_multiply(self.state ^ value, MIX_MULTIPLIER);
    }
}

impl Hasher for FastHasher {
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        let mut chunks = bytes.chunks_exact(WORD_BYTES);
        for chunk in &mut chunks {
            let mut word = [0u8; WORD_BYTES];
            word.copy_from_slice(chunk);
            self.mix(u64::from_le_bytes(word));
        }
        let rest = chunks.remainder();
        let mut word = [0u8; WORD_BYTES];
        word[..rest.len()].copy_from_slice(rest);
        self.mix(u64::from_le_bytes(word) ^ ((bytes.len() as u64) << 56));
    }

    #[inline]
    fn write_u8(&mut self, value: u8) {
        self.mix(u64::from(value));
    }

    #[inline]
    fn write_u16(&mut self, value: u16) {
        self.mix(u64::from(value));
    }

    #[inline]
    fn write_u32(&mut self, value: u32) {
        self.mix(u64::from(value));
    }

    #[inline]
    fn write_u64(&mut self, value: u64) {
        self.mix(value);
    }

    #[inline]
    fn write_usize(&mut self, value: usize) {
        self.mix(value as u64);
    }

    #[inline]
    fn finish(&self) -> u64 {
        folded_multiply(self.state, MIX_SECOND)
    }
}
