use nanocode_sys::sha512::{ROUND_CONSTANTS, Sha512Instructions};

use crate::pending::{Pending, words_be};

const BLOCK_BYTES: usize = 128;
const MESSAGE_WORDS: usize = 16;
const SHA384_OUTPUT_BYTES: usize = 48;
const SHA512_OUTPUT_BYTES: usize = 64;

const SHA384_INITIAL: [u64; 8] = [
    0xcbbb9d5dc1059ed8,
    0x629a292a367cd507,
    0x9159015a3070dd17,
    0x152fecd8f70e5939,
    0x67332667ffc00b31,
    0x8eb44a8768581511,
    0xdb0c2e0d64f98fa7,
    0x47b5481dbefa4fa4,
];

const SHA512_INITIAL: [u64; 8] = [
    0x6a09e667f3bcc908,
    0xbb67ae8584caa73b,
    0x3c6ef372fe94f82b,
    0xa54ff53a5f1d36f1,
    0x510e527fade682d1,
    0x9b05688c2b3e6c1f,
    0x1f83d9abfb41bd6b,
    0x5be0cd19137e2179,
];

#[derive(Clone, Debug)]
struct Engine {
    state: [u64; 8],
    pending: Pending<BLOCK_BYTES>,
}

impl Engine {
    const fn new(initial: [u64; 8]) -> Self {
        Self {
            state: initial,
            pending: Pending::new(),
        }
    }

    fn update(&mut self, bytes: &[u8]) {
        self.pending
            .absorb(bytes, |blocks| compress(&mut self.state, blocks));
    }

    fn finalize(&self) -> [u64; 8] {
        let mut state = self.state;
        let bits = self.pending.absorbed_bits();
        self.pending
            .pad(&bits.to_be_bytes(), |blocks| compress(&mut state, blocks));
        state
    }
}

#[derive(Clone, Debug)]
pub struct Sha384(Engine);

impl Default for Sha384 {
    fn default() -> Self {
        Self::new()
    }
}

impl Sha384 {
    pub const OUTPUT_BYTES: usize = SHA384_OUTPUT_BYTES;

    pub const fn new() -> Self {
        Self(Engine::new(SHA384_INITIAL))
    }

    pub fn update(&mut self, bytes: &[u8]) {
        self.0.update(bytes);
    }

    pub fn finalize(&self) -> [u8; SHA384_OUTPUT_BYTES] {
        words_be(self.0.finalize().map(u64::to_be_bytes))
    }

    pub fn digest(bytes: &[u8]) -> [u8; SHA384_OUTPUT_BYTES] {
        let mut hasher = Self::new();
        hasher.update(bytes);
        hasher.finalize()
    }
}

#[derive(Clone, Debug)]
pub struct Sha512(Engine);

impl Default for Sha512 {
    fn default() -> Self {
        Self::new()
    }
}

impl Sha512 {
    pub const OUTPUT_BYTES: usize = SHA512_OUTPUT_BYTES;

    pub const fn new() -> Self {
        Self(Engine::new(SHA512_INITIAL))
    }

    pub fn update(&mut self, bytes: &[u8]) {
        self.0.update(bytes);
    }

    pub fn finalize(&self) -> [u8; SHA512_OUTPUT_BYTES] {
        words_be(self.0.finalize().map(u64::to_be_bytes))
    }

    pub fn digest(bytes: &[u8]) -> [u8; SHA512_OUTPUT_BYTES] {
        let mut hasher = Self::new();
        hasher.update(bytes);
        hasher.finalize()
    }
}

fn compress(state: &mut [u64; 8], blocks: &[[u8; BLOCK_BYTES]]) {
    match Sha512Instructions::detect() {
        Some(instructions) => instructions.compress(state, blocks),
        None => compress_portable(state, blocks),
    }
}

fn compress_portable(state: &mut [u64; 8], blocks: &[[u8; BLOCK_BYTES]]) {
    for block in blocks {
        let mut words = [0u64; MESSAGE_WORDS];
        for (word, bytes) in words.iter_mut().zip(block.as_chunks::<8>().0) {
            *word = u64::from_be_bytes(*bytes);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = *state;
        rounds_80!(index {
            let word = if index < MESSAGE_WORDS {
                words[index]
            } else {
                let early = words[(index + 1) % MESSAGE_WORDS];
                let late = words[(index + 14) % MESSAGE_WORDS];
                let s0 = early.rotate_right(1) ^ early.rotate_right(8) ^ (early >> 7);
                let s1 = late.rotate_right(19) ^ late.rotate_right(61) ^ (late >> 6);
                words[index % MESSAGE_WORDS] = words[index % MESSAGE_WORDS]
                    .wrapping_add(s0)
                    .wrapping_add(words[(index + 9) % MESSAGE_WORDS])
                    .wrapping_add(s1);
                words[index % MESSAGE_WORDS]
            };
            let first = h
                .wrapping_add(e.rotate_right(14) ^ e.rotate_right(18) ^ e.rotate_right(41))
                .wrapping_add((e & f) ^ (!e & g))
                .wrapping_add(ROUND_CONSTANTS[index])
                .wrapping_add(word);
            let second = (a.rotate_right(28) ^ a.rotate_right(34) ^ a.rotate_right(39))
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
