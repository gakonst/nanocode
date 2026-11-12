const LANES: usize = 25;
const LANE_BYTES: usize = 8;
const STATE_BYTES: usize = LANES * LANE_BYTES;
const ROWS: usize = 5;
const PADDING_LAST_BYTE: u8 = 0x80;
pub(super) const SHA3_SUFFIX: u8 = 0x06;
pub(super) const SHAKE_SUFFIX: u8 = 0x1f;

const ROUND_CONSTANTS: [u64; 24] = [
    0x0000_0000_0000_0001,
    0x0000_0000_0000_8082,
    0x8000_0000_0000_808a,
    0x8000_0000_8000_8000,
    0x0000_0000_0000_808b,
    0x0000_0000_8000_0001,
    0x8000_0000_8000_8081,
    0x8000_0000_0000_8009,
    0x0000_0000_0000_008a,
    0x0000_0000_0000_0088,
    0x0000_0000_8000_8009,
    0x0000_0000_8000_000a,
    0x0000_0000_8000_808b,
    0x8000_0000_0000_008b,
    0x8000_0000_0000_8089,
    0x8000_0000_0000_8003,
    0x8000_0000_0000_8002,
    0x8000_0000_0000_0080,
    0x0000_0000_0000_800a,
    0x8000_0000_8000_000a,
    0x8000_0000_8000_8081,
    0x8000_0000_0000_8080,
    0x0000_0000_8000_0001,
    0x8000_0000_8000_8008,
];

const PI_ORDER: [usize; LANES - 1] = [
    10, 7, 11, 17, 18, 3, 5, 16, 8, 21, 24, 4, 15, 23, 19, 13, 12, 2, 20, 14, 22, 9, 6, 1,
];

const RHO_OFFSETS: [u32; LANES - 1] = [
    1, 3, 6, 10, 15, 21, 28, 36, 45, 55, 2, 14, 27, 41, 56, 8, 25, 43, 62, 18, 39, 61, 20, 44,
];

#[derive(Clone)]
pub(super) struct Keccak {
    lanes: [u64; LANES],
    rate_bytes: usize,
    position: usize,
    suffix: u8,
}

impl Keccak {
    pub(super) const fn new(rate_bytes: usize, suffix: u8) -> Self {
        assert!(rate_bytes > 0);
        assert!(rate_bytes < STATE_BYTES);
        assert!(rate_bytes.is_multiple_of(LANE_BYTES));
        Self {
            lanes: [0; LANES],
            rate_bytes,
            position: 0,
            suffix,
        }
    }

    pub(super) const fn sha3(output_bytes: usize) -> Self {
        Self::new(STATE_BYTES - 2 * output_bytes, SHA3_SUFFIX)
    }

    pub(super) fn update(&mut self, mut data: &[u8]) {
        assert!(self.position < self.rate_bytes);
        while self.position > 0 && !data.is_empty() {
            self.xor_byte(self.position, data[0]);
            data = &data[1..];
            self.position += 1;
            if self.position == self.rate_bytes {
                permute(&mut self.lanes);
                self.position = 0;
            }
        }
        if self.position > 0 {
            return;
        }
        let mut blocks = data.chunks_exact(self.rate_bytes);
        for block in &mut blocks {
            for (lane, chunk) in self.lanes.iter_mut().zip(block.as_chunks::<LANE_BYTES>().0) {
                *lane ^= u64::from_le_bytes(*chunk);
            }
            permute(&mut self.lanes);
        }
        for (index, byte) in blocks.remainder().iter().enumerate() {
            self.xor_byte(index, *byte);
        }
        self.position = blocks.remainder().len();
    }

    pub(super) fn finalize(mut self, output: &mut [u8]) {
        assert!(self.position < self.rate_bytes);
        self.xor_byte(self.position, self.suffix);
        self.xor_byte(self.rate_bytes - 1, PADDING_LAST_BYTE);
        permute(&mut self.lanes);
        let mut chunks = output.chunks_mut(self.rate_bytes);
        if let Some(first) = chunks.next() {
            self.squeeze(first);
        }
        for chunk in chunks {
            permute(&mut self.lanes);
            self.squeeze(chunk);
        }
    }

    fn squeeze(&self, output: &mut [u8]) {
        assert!(output.len() <= self.rate_bytes);
        for (chunk, lane) in output.chunks_mut(LANE_BYTES).zip(self.lanes) {
            chunk.copy_from_slice(&lane.to_le_bytes()[..chunk.len()]);
        }
    }

    fn xor_byte(&mut self, index: usize, byte: u8) {
        self.lanes[index / LANE_BYTES] ^= u64::from(byte) << (8 * (index % LANE_BYTES));
    }
}

fn permute(lanes: &mut [u64; LANES]) {
    for round_constant in ROUND_CONSTANTS {
        let mut parity = [0u64; ROWS];
        for (column, sum) in parity.iter_mut().enumerate() {
            *sum = lanes[column]
                ^ lanes[column + ROWS]
                ^ lanes[column + 2 * ROWS]
                ^ lanes[column + 3 * ROWS]
                ^ lanes[column + 4 * ROWS];
        }
        for column in 0..ROWS {
            let effect =
                parity[(column + ROWS - 1) % ROWS] ^ parity[(column + 1) % ROWS].rotate_left(1);
            for row in 0..ROWS {
                lanes[row * ROWS + column] ^= effect;
            }
        }
        let mut carried = lanes[1];
        for (target, offset) in PI_ORDER.iter().zip(RHO_OFFSETS) {
            let displaced = lanes[*target];
            lanes[*target] = carried.rotate_left(offset);
            carried = displaced;
        }
        for row in 0..ROWS {
            let start = row * ROWS;
            let plane = [
                lanes[start],
                lanes[start + 1],
                lanes[start + 2],
                lanes[start + 3],
                lanes[start + 4],
            ];
            for column in 0..ROWS {
                lanes[start + column] =
                    plane[column] ^ (!plane[(column + 1) % ROWS] & plane[(column + 2) % ROWS]);
            }
        }
        lanes[0] ^= round_constant;
    }
}
