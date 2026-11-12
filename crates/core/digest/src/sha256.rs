use crate::pending::{Pending, words_be};

const BLOCK_BYTES: usize = 64;
const MESSAGE_WORDS: usize = 16;
const ROUNDS: usize = 64;
const SHA224_OUTPUT_BYTES: usize = 28;
const SHA256_OUTPUT_BYTES: usize = 32;

const SHA224_INITIAL: [u32; 8] = [
    0xc1059ed8, 0x367cd507, 0x3070dd17, 0xf70e5939, 0xffc00b31, 0x68581511, 0x64f98fa7, 0xbefa4fa4,
];

const SHA256_INITIAL: [u32; 8] = [
    0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
];

const ROUND_CONSTANTS: [u32; ROUNDS] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

#[derive(Clone, Debug)]
struct Engine {
    state: [u32; 8],
    pending: Pending<BLOCK_BYTES>,
}

impl Engine {
    const fn new(initial: [u32; 8]) -> Self {
        Self {
            state: initial,
            pending: Pending::new(),
        }
    }

    fn update(&mut self, bytes: &[u8]) {
        self.pending
            .absorb(bytes, |blocks| compress(&mut self.state, blocks));
    }

    fn finalize(&self) -> [u32; 8] {
        let mut state = self.state;
        let bits = u64::try_from(self.pending.absorbed_bits())
            .expect("FIPS 180-4 bounds a SHA-256 message below 2^64 bits");
        self.pending
            .pad(&bits.to_be_bytes(), |blocks| compress(&mut state, blocks));
        state
    }
}

#[derive(Clone, Debug)]
pub struct Sha224(Engine);

impl Default for Sha224 {
    fn default() -> Self {
        Self::new()
    }
}

impl Sha224 {
    pub const OUTPUT_BYTES: usize = SHA224_OUTPUT_BYTES;

    pub const fn new() -> Self {
        Self(Engine::new(SHA224_INITIAL))
    }

    pub fn update(&mut self, bytes: &[u8]) {
        self.0.update(bytes);
    }

    pub fn finalize(&self) -> [u8; SHA224_OUTPUT_BYTES] {
        words_be(self.0.finalize().map(u32::to_be_bytes))
    }

    pub fn digest(bytes: &[u8]) -> [u8; SHA224_OUTPUT_BYTES] {
        let mut hasher = Self::new();
        hasher.update(bytes);
        hasher.finalize()
    }
}

#[derive(Clone, Debug)]
pub struct Sha256(Engine);

impl Default for Sha256 {
    fn default() -> Self {
        Self::new()
    }
}

impl Sha256 {
    pub const OUTPUT_BYTES: usize = SHA256_OUTPUT_BYTES;

    pub const fn new() -> Self {
        Self(Engine::new(SHA256_INITIAL))
    }

    pub fn update(&mut self, bytes: &[u8]) {
        self.0.update(bytes);
    }

    pub fn finalize(&self) -> [u8; SHA256_OUTPUT_BYTES] {
        words_be(self.0.finalize().map(u32::to_be_bytes))
    }

    pub fn digest(bytes: &[u8]) -> [u8; SHA256_OUTPUT_BYTES] {
        let mut hasher = Self::new();
        hasher.update(bytes);
        hasher.finalize()
    }
}

fn compress(state: &mut [u32; 8], blocks: &[[u8; BLOCK_BYTES]]) {
    for block in blocks {
        let mut words = [0u32; MESSAGE_WORDS];
        for (word, bytes) in words.iter_mut().zip(block.as_chunks::<4>().0) {
            *word = u32::from_be_bytes(*bytes);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = *state;
        rounds_64!(index {
            let word = if index < MESSAGE_WORDS {
                words[index]
            } else {
                let early = words[(index + 1) % MESSAGE_WORDS];
                let late = words[(index + 14) % MESSAGE_WORDS];
                let s0 = early.rotate_right(7) ^ early.rotate_right(18) ^ (early >> 3);
                let s1 = late.rotate_right(17) ^ late.rotate_right(19) ^ (late >> 10);
                words[index % MESSAGE_WORDS] = words[index % MESSAGE_WORDS]
                    .wrapping_add(s0)
                    .wrapping_add(words[(index + 9) % MESSAGE_WORDS])
                    .wrapping_add(s1);
                words[index % MESSAGE_WORDS]
            };
            let first = h
                .wrapping_add(e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25))
                .wrapping_add((e & f) ^ (!e & g))
                .wrapping_add(ROUND_CONSTANTS[index])
                .wrapping_add(word);
            let second = (a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22))
                .wrapping_add((a & b) ^ (a & c) ^ (b & c));
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(first);
            d = c;
            c = b;
            b = a;
            a = first.wrapping_add(second);
        });
        for (slot, value) in state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
            *slot = slot.wrapping_add(value);
        }
    }
}
