const PRIME64_1: u64 = 0x9E37_79B1_85EB_CA87;
const PRIME64_2: u64 = 0xC2B2_AE3D_27D4_EB4F;
const PRIME64_3: u64 = 0x1656_67B1_9E37_79F9;
const PRIME64_4: u64 = 0x85EB_CA77_C2B2_AE63;
const PRIME64_5: u64 = 0x27D4_EB2F_1656_67C5;
#[derive(Debug, Clone)]
pub(crate) struct Xxh64State {
    seed: u64,
    total_len: u64,
    v1: u64,
    v2: u64,
    v3: u64,
    v4: u64,
    buffered: [u8; 32],
    buffered_len: usize,
}
impl Xxh64State {
    pub(crate) const fn new(seed: u64) -> Self {
        Self {
            seed,
            total_len: 0,
            v1: seed.wrapping_add(PRIME64_1).wrapping_add(PRIME64_2),
            v2: seed.wrapping_add(PRIME64_2),
            v3: seed,
            v4: seed.wrapping_sub(PRIME64_1),
            buffered: [0; 32],
            buffered_len: 0,
        }
    }
    pub(crate) fn update(&mut self, mut input: &[u8]) {
        self.total_len = self.total_len.wrapping_add(input.len() as u64);
        if self.buffered_len + input.len() < 32 {
            self.buffered[self.buffered_len..self.buffered_len + input.len()]
                .copy_from_slice(input);
            self.buffered_len += input.len();
            return;
        }
        if self.buffered_len != 0 {
            let fill = 32 - self.buffered_len;
            self.buffered[self.buffered_len..].copy_from_slice(&input[..fill]);
            let buffered = self.buffered;
            self.consume_chunk(&buffered);
            self.buffered_len = 0;
            input = &input[fill..];
        }
        while input.len() >= 32 {
            self.consume_chunk(&input[..32]);
            input = &input[32..];
        }
        self.buffered[..input.len()].copy_from_slice(input);
        self.buffered_len = input.len();
    }
    pub(crate) fn digest(&self) -> u64 {
        let mut hash = if self.total_len >= 32 {
            let mut value = self
                .v1
                .rotate_left(1)
                .wrapping_add(self.v2.rotate_left(7))
                .wrapping_add(self.v3.rotate_left(12))
                .wrapping_add(self.v4.rotate_left(18));
            value = merge_round(value, self.v1);
            value = merge_round(value, self.v2);
            value = merge_round(value, self.v3);
            merge_round(value, self.v4)
        } else {
            self.seed.wrapping_add(PRIME64_5)
        };
        hash = hash.wrapping_add(self.total_len);
        let mut index = 0usize;
        let input = &self.buffered[..self.buffered_len];
        while index + 8 <= input.len() {
            let k1 = round(0, read_u64_le(&input[index..index + 8]));
            hash ^= k1;
            hash = hash
                .rotate_left(27)
                .wrapping_mul(PRIME64_1)
                .wrapping_add(PRIME64_4);
            index += 8;
        }
        if index + 4 <= input.len() {
            hash ^= (read_u32_le(&input[index..index + 4]) as u64).wrapping_mul(PRIME64_1);
            hash = hash
                .rotate_left(23)
                .wrapping_mul(PRIME64_2)
                .wrapping_add(PRIME64_3);
            index += 4;
        }
        while index < input.len() {
            hash ^= (input[index] as u64).wrapping_mul(PRIME64_5);
            hash = hash.rotate_left(11).wrapping_mul(PRIME64_1);
            index += 1;
        }
        avalanche(hash)
    }
    fn consume_chunk(&mut self, input: &[u8]) {
        debug_assert!(input.len() == 32);
        self.v1 = round(self.v1, read_u64_le(&input[..8]));
        self.v2 = round(self.v2, read_u64_le(&input[8..16]));
        self.v3 = round(self.v3, read_u64_le(&input[16..24]));
        self.v4 = round(self.v4, read_u64_le(&input[24..32]));
    }
}
const fn round(acc: u64, lane: u64) -> u64 {
    acc.wrapping_add(lane.wrapping_mul(PRIME64_2))
        .rotate_left(31)
        .wrapping_mul(PRIME64_1)
}
const fn merge_round(acc: u64, value: u64) -> u64 {
    (acc ^ round(0, value))
        .wrapping_mul(PRIME64_1)
        .wrapping_add(PRIME64_4)
}
const fn avalanche(mut hash: u64) -> u64 {
    hash ^= hash >> 33;
    hash = hash.wrapping_mul(PRIME64_2);
    hash ^= hash >> 29;
    hash = hash.wrapping_mul(PRIME64_3);
    hash ^= hash >> 32;
    hash
}
const fn read_u32_le(src: &[u8]) -> u32 {
    u32::from_le_bytes([src[0], src[1], src[2], src[3]])
}
const fn read_u64_le(src: &[u8]) -> u64 {
    u64::from_le_bytes([
        src[0], src[1], src[2], src[3], src[4], src[5], src[6], src[7],
    ])
}
