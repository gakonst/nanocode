const BLOCK_BYTES: usize = 64;
const LENGTH_FIELD_BYTES: usize = 8;
const DIGEST_BYTES: usize = 32;
const STATE_WORDS: usize = 8;
const SCHEDULE_WORDS: usize = 64;
const BLOCK_WORDS: usize = 16;
const WORD_BYTES: usize = 4;
const BYTE_BITS: u64 = 8;
const PADDING_FIRST_BYTE: u8 = 0x80;

const INITIAL_HASH: [u32; STATE_WORDS] = [
    0x6A09_E667,
    0xBB67_AE85,
    0x3C6E_F372,
    0xA54F_F53A,
    0x510E_527F,
    0x9B05_688C,
    0x1F83_D9AB,
    0x5BE0_CD19,
];

const ROUND_CONSTANTS: [u32; SCHEDULE_WORDS] = [
    0x428A_2F98,
    0x7137_4491,
    0xB5C0_FBCF,
    0xE9B5_DBA5,
    0x3956_C25B,
    0x59F1_11F1,
    0x923F_82A4,
    0xAB1C_5ED5,
    0xD807_AA98,
    0x1283_5B01,
    0x2431_85BE,
    0x550C_7DC3,
    0x72BE_5D74,
    0x80DE_B1FE,
    0x9BDC_06A7,
    0xC19B_F174,
    0xE49B_69C1,
    0xEFBE_4786,
    0x0FC1_9DC6,
    0x240C_A1CC,
    0x2DE9_2C6F,
    0x4A74_84AA,
    0x5CB0_A9DC,
    0x76F9_88DA,
    0x983E_5152,
    0xA831_C66D,
    0xB003_27C8,
    0xBF59_7FC7,
    0xC6E0_0BF3,
    0xD5A7_9147,
    0x06CA_6351,
    0x1429_2967,
    0x27B7_0A85,
    0x2E1B_2138,
    0x4D2C_6DFC,
    0x5338_0D13,
    0x650A_7354,
    0x766A_0ABB,
    0x81C2_C92E,
    0x9272_2C85,
    0xA2BF_E8A1,
    0xA81A_664B,
    0xC24B_8B70,
    0xC76C_51A3,
    0xD192_E819,
    0xD699_0624,
    0xF40E_3585,
    0x106A_A070,
    0x19A4_C116,
    0x1E37_6C08,
    0x2748_774C,
    0x34B0_BCB5,
    0x391C_0CB3,
    0x4ED8_AA4A,
    0x5B9C_CA4F,
    0x682E_6FF3,
    0x748F_82EE,
    0x78A5_636F,
    0x84C8_7814,
    0x8CC7_0208,
    0x90BE_FFFA,
    0xA450_6CEB,
    0xBEF9_A3F7,
    0xC671_78F2,
];

const SIGMA0_ROTATIONS: [u32; 3] = [2, 13, 22];
const SIGMA1_ROTATIONS: [u32; 3] = [6, 11, 25];
const SCHEDULE_SIGMA0_ROTATIONS: [u32; 2] = [7, 18];
const SCHEDULE_SIGMA0_SHIFT: u32 = 3;
const SCHEDULE_SIGMA1_ROTATIONS: [u32; 2] = [17, 19];
const SCHEDULE_SIGMA1_SHIFT: u32 = 10;
const SCHEDULE_LAG_SHORT_WORDS: usize = 2;
const SCHEDULE_LAG_SIGMA0_WORDS: usize = 15;
const SCHEDULE_LAG_ADD_WORDS: usize = 7;

pub(crate) struct Sha256 {
    state: [u32; STATE_WORDS],
    block: [u8; BLOCK_BYTES],
    total_bytes: u64,
}

impl Sha256 {
    pub(crate) const fn new() -> Self {
        Self {
            state: INITIAL_HASH,
            block: [0; BLOCK_BYTES],
            total_bytes: 0,
        }
    }

    pub(crate) fn update(&mut self, input: &[u8]) {
        let mut remaining = input;
        while !remaining.is_empty() {
            let start = self.block_position();
            let count = (BLOCK_BYTES - start).min(remaining.len());
            match remaining.split_first_chunk::<BLOCK_BYTES>() {
                Some((block, rest)) if start == 0 => {
                    compress(&mut self.state, block);
                    remaining = rest;
                }
                _ => {
                    let (part, rest) = remaining.split_at(count);
                    self.block[start..start + count].copy_from_slice(part);
                    remaining = rest;
                    if start + count == BLOCK_BYTES {
                        compress(&mut self.state, &self.block);
                    }
                }
            }
            self.total_bytes = self
                .total_bytes
                .wrapping_add(u64::try_from(count).expect("a block part fits in 64 bits"));
        }
    }

    pub(crate) fn finish(mut self) -> [u8; DIGEST_BYTES] {
        let mut position = self.block_position();
        self.block[position] = PADDING_FIRST_BYTE;
        position += 1;
        if position > BLOCK_BYTES - LENGTH_FIELD_BYTES {
            self.block[position..].fill(0);
            compress(&mut self.state, &self.block);
            position = 0;
        }
        self.block[position..BLOCK_BYTES - LENGTH_FIELD_BYTES].fill(0);
        let total_bits = self.total_bytes.wrapping_mul(BYTE_BITS);
        self.block[BLOCK_BYTES - LENGTH_FIELD_BYTES..].copy_from_slice(&total_bits.to_be_bytes());
        compress(&mut self.state, &self.block);
        let mut digest = [0; DIGEST_BYTES];
        for (bytes, word) in digest
            .as_chunks_mut::<WORD_BYTES>()
            .0
            .iter_mut()
            .zip(self.state)
        {
            *bytes = word.to_be_bytes();
        }
        digest
    }

    fn block_position(&self) -> usize {
        usize::try_from(self.total_bytes % BLOCK_BYTES as u64)
            .expect("a position inside a block fits in usize")
    }
}

const fn big_sigma(value: u32, rotations: [u32; 3]) -> u32 {
    value.rotate_right(rotations[0])
        ^ value.rotate_right(rotations[1])
        ^ value.rotate_right(rotations[2])
}

const fn small_sigma(value: u32, rotations: [u32; 2], shift: u32) -> u32 {
    value.rotate_right(rotations[0]) ^ value.rotate_right(rotations[1]) ^ (value >> shift)
}

fn round<const OFFSET: usize>(working: &mut [u32; STATE_WORDS], constant: u32, word: u32) {
    let slot = |index: usize| (index + STATE_WORDS - OFFSET) % STATE_WORDS;
    let [a, b, c, e, f, g, h] = [0, 1, 2, 4, 5, 6, 7].map(|index| working[slot(index)]);
    let choose = (e & f) ^ (!e & g);
    let majority = (a & b) ^ (a & c) ^ (b & c);
    let first = h
        .wrapping_add(big_sigma(e, SIGMA1_ROTATIONS))
        .wrapping_add(choose)
        .wrapping_add(constant)
        .wrapping_add(word);
    working[slot(3)] = working[slot(3)].wrapping_add(first);
    working[slot(7)] = first
        .wrapping_add(big_sigma(a, SIGMA0_ROTATIONS))
        .wrapping_add(majority);
}

fn extend(schedule: &mut [u32; BLOCK_WORDS], index: usize) {
    let at = |lag: usize| schedule[(index + BLOCK_WORDS - lag) % BLOCK_WORDS];
    let extended = small_sigma(
        at(SCHEDULE_LAG_SHORT_WORDS),
        SCHEDULE_SIGMA1_ROTATIONS,
        SCHEDULE_SIGMA1_SHIFT,
    )
    .wrapping_add(at(SCHEDULE_LAG_ADD_WORDS))
    .wrapping_add(small_sigma(
        at(SCHEDULE_LAG_SIGMA0_WORDS),
        SCHEDULE_SIGMA0_ROTATIONS,
        SCHEDULE_SIGMA0_SHIFT,
    ));
    schedule[index] = schedule[index].wrapping_add(extended);
}

fn rounds<const EXTEND: bool, const START: usize>(
    working: &mut [u32; STATE_WORDS],
    schedule: &mut [u32; BLOCK_WORDS],
    group: usize,
) {
    let mut step = |index: usize| {
        if EXTEND {
            extend(schedule, index);
        }
        (
            ROUND_CONSTANTS[group * BLOCK_WORDS + index],
            schedule[index],
        )
    };
    let (constant, word) = step(START);
    round::<0>(working, constant, word);
    let (constant, word) = step(START + 1);
    round::<1>(working, constant, word);
    let (constant, word) = step(START + 2);
    round::<2>(working, constant, word);
    let (constant, word) = step(START + 3);
    round::<3>(working, constant, word);
    let (constant, word) = step(START + 4);
    round::<4>(working, constant, word);
    let (constant, word) = step(START + 5);
    round::<5>(working, constant, word);
    let (constant, word) = step(START + 6);
    round::<6>(working, constant, word);
    let (constant, word) = step(START + 7);
    round::<7>(working, constant, word);
}

fn compress(state: &mut [u32; STATE_WORDS], block: &[u8; BLOCK_BYTES]) {
    let mut schedule = [0u32; BLOCK_WORDS];
    for (word, bytes) in schedule.iter_mut().zip(block.as_chunks::<WORD_BYTES>().0) {
        *word = u32::from_be_bytes(*bytes);
    }
    let mut working = *state;
    rounds::<false, 0>(&mut working, &mut schedule, 0);
    rounds::<false, STATE_WORDS>(&mut working, &mut schedule, 0);
    for group in 1..SCHEDULE_WORDS / BLOCK_WORDS {
        rounds::<true, 0>(&mut working, &mut schedule, group);
        rounds::<true, STATE_WORDS>(&mut working, &mut schedule, group);
    }
    for (word, value) in state.iter_mut().zip(working) {
        *word = word.wrapping_add(value);
    }
}
