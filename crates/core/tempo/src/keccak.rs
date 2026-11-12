const LANES: usize = 25;
const RATE_BYTES: usize = 136;
const ROUNDS: usize = 24;
const ROW: usize = 5;
const DOMAIN_PADDING: u8 = 0x01;
const FINAL_PADDING: u8 = 0x80;
const OUTPUT_BYTES: usize = 32;
const LANE_BYTES: usize = 8;

const ROUND_CONSTANTS: [u64; ROUNDS] = [
    0x0000000000000001,
    0x0000000000008082,
    0x800000000000808a,
    0x8000000080008000,
    0x000000000000808b,
    0x0000000080000001,
    0x8000000080008081,
    0x8000000000008009,
    0x000000000000008a,
    0x0000000000000088,
    0x0000000080008009,
    0x000000008000000a,
    0x000000008000808b,
    0x800000000000008b,
    0x8000000000008089,
    0x8000000000008003,
    0x8000000000008002,
    0x8000000000000080,
    0x000000000000800a,
    0x800000008000000a,
    0x8000000080008081,
    0x8000000000008080,
    0x0000000080000001,
    0x8000000080008008,
];

const ROTATIONS: [u32; LANES] = [
    0, 1, 62, 28, 27, 36, 44, 6, 55, 20, 3, 10, 43, 25, 39, 41, 45, 15, 21, 8, 18, 2, 61, 56, 14,
];

#[derive(Clone)]
pub struct Keccak256 {
    state: [u64; LANES],
    buffer: [u8; RATE_BYTES],
    filled: usize,
}

impl Default for Keccak256 {
    fn default() -> Self {
        Self::new()
    }
}

impl Keccak256 {
    pub const fn new() -> Self {
        Self {
            state: [0; LANES],
            buffer: [0; RATE_BYTES],
            filled: 0,
        }
    }

    pub fn update(&mut self, mut data: &[u8]) {
        while !data.is_empty() {
            let take = (RATE_BYTES - self.filled).min(data.len());
            self.buffer[self.filled..self.filled + take].copy_from_slice(&data[..take]);
            self.filled += take;
            data = &data[take..];
            if self.filled == RATE_BYTES {
                self.absorb();
            }
        }
    }

    pub fn finalize(mut self) -> [u8; OUTPUT_BYTES] {
        self.buffer[self.filled..].fill(0);
        self.buffer[self.filled] ^= DOMAIN_PADDING;
        self.buffer[RATE_BYTES - 1] ^= FINAL_PADDING;
        self.absorb();
        let mut output = [0u8; OUTPUT_BYTES];
        for (chunk, lane) in output.chunks_mut(LANE_BYTES).zip(self.state) {
            chunk.copy_from_slice(&lane.to_le_bytes());
        }
        output
    }

    fn absorb(&mut self) {
        for (lane, chunk) in self.state.iter_mut().zip(self.buffer.chunks(LANE_BYTES)) {
            let mut bytes = [0u8; LANE_BYTES];
            bytes.copy_from_slice(chunk);
            *lane ^= u64::from_le_bytes(bytes);
        }
        permute(&mut self.state);
        self.filled = 0;
    }
}

pub fn keccak256(data: impl AsRef<[u8]>) -> [u8; OUTPUT_BYTES] {
    let mut hasher = Keccak256::new();
    hasher.update(data.as_ref());
    hasher.finalize()
}

fn permute(state: &mut [u64; LANES]) {
    for constant in ROUND_CONSTANTS {
        let mut columns = [0u64; ROW];
        for (x, column) in columns.iter_mut().enumerate() {
            *column = state[x] ^ state[x + ROW] ^ state[x + 2 * ROW] ^ state[x + 3 * ROW] ^ state[x + 4 * ROW];
        }
        for x in 0..ROW {
            let d = columns[(x + ROW - 1) % ROW] ^ columns[(x + 1) % ROW].rotate_left(1);
            for y in 0..ROW {
                state[x + ROW * y] ^= d;
            }
        }
        let mut moved = [0u64; LANES];
        for x in 0..ROW {
            for y in 0..ROW {
                let index = x + ROW * y;
                let target = y + ROW * ((2 * x + 3 * y) % ROW);
                moved[target] = state[index].rotate_left(ROTATIONS[index]);
            }
        }
        for y in 0..ROW {
            for x in 0..ROW {
                state[x + ROW * y] = moved[x + ROW * y]
                    ^ (!moved[(x + 1) % ROW + ROW * y] & moved[(x + 2) % ROW + ROW * y]);
            }
        }
        state[0] ^= constant;
    }
}
