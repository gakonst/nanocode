use crate::pending::{Pending, words_be};

const BLOCK_BYTES: usize = 64;
const OUTPUT_BYTES: usize = 20;
const MESSAGE_WORDS: usize = 16;
const ROUNDS_PER_FUNCTION: usize = 20;

const INITIAL: [u32; 5] = [0x67452301, 0xefcdab89, 0x98badcfe, 0x10325476, 0xc3d2e1f0];
const ROUND_CONSTANTS: [u32; 4] = [0x5a827999, 0x6ed9eba1, 0x8f1bbcdc, 0xca62c1d6];

#[derive(Clone, Debug)]
pub struct Sha1 {
    state: [u32; 5],
    pending: Pending<BLOCK_BYTES>,
}

impl Default for Sha1 {
    fn default() -> Self {
        Self::new()
    }
}

impl Sha1 {
    pub const OUTPUT_BYTES: usize = OUTPUT_BYTES;

    pub const fn new() -> Self {
        Self {
            state: INITIAL,
            pending: Pending::new(),
        }
    }

    pub fn update(&mut self, bytes: &[u8]) {
        self.pending
            .absorb(bytes, |blocks| compress(&mut self.state, blocks));
    }

    pub fn finalize(&self) -> [u8; OUTPUT_BYTES] {
        let mut state = self.state;
        let bits = u64::try_from(self.pending.absorbed_bits())
            .expect("FIPS 180-4 bounds a SHA-1 message below 2^64 bits");
        self.pending
            .pad(&bits.to_be_bytes(), |blocks| compress(&mut state, blocks));
        words_be(state.map(u32::to_be_bytes))
    }

    pub fn digest(bytes: &[u8]) -> [u8; OUTPUT_BYTES] {
        let mut hasher = Self::new();
        hasher.update(bytes);
        hasher.finalize()
    }
}

fn compress(state: &mut [u32; 5], blocks: &[[u8; BLOCK_BYTES]]) {
    for block in blocks {
        let mut words = [0u32; MESSAGE_WORDS];
        for (word, bytes) in words.iter_mut().zip(block.as_chunks::<4>().0) {
            *word = u32::from_be_bytes(*bytes);
        }
        let [mut a, mut b, mut c, mut d, mut e] = *state;
        rounds_80!(index {
            let word = if index < MESSAGE_WORDS {
                words[index]
            } else {
                words[index % MESSAGE_WORDS] = (words[(index + 13) % MESSAGE_WORDS]
                    ^ words[(index + 8) % MESSAGE_WORDS]
                    ^ words[(index + 2) % MESSAGE_WORDS]
                    ^ words[index % MESSAGE_WORDS])
                    .rotate_left(1);
                words[index % MESSAGE_WORDS]
            };
            let (function, constant) = match index / ROUNDS_PER_FUNCTION {
                0 => ((b & c) | (!b & d), ROUND_CONSTANTS[0]),
                1 => (b ^ c ^ d, ROUND_CONSTANTS[1]),
                2 => ((b & c) | (b & d) | (c & d), ROUND_CONSTANTS[2]),
                _ => (b ^ c ^ d, ROUND_CONSTANTS[3]),
            };
            let next = a
                .rotate_left(5)
                .wrapping_add(function)
                .wrapping_add(e)
                .wrapping_add(constant)
                .wrapping_add(word);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = next;
        });
        for (slot, value) in state.iter_mut().zip([a, b, c, d, e]) {
            *slot = slot.wrapping_add(value);
        }
    }
}
