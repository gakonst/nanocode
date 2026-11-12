use super::block::BlockBuffer;

pub(super) const BLOCK_BYTES: usize = 64;
pub(super) const OUTPUT_BYTES: usize = 16;
const STEPS_PER_ROUND: usize = 16;
const SHIFT_CYCLE: usize = 4;

const INITIAL_STATE: [u32; 4] = [0x6745_2301, 0xefcd_ab89, 0x98ba_dcfe, 0x1032_5476];

const SINE_CONSTANTS: [u32; 64] = [
    0xd76a_a478,
    0xe8c7_b756,
    0x2420_70db,
    0xc1bd_ceee,
    0xf57c_0faf,
    0x4787_c62a,
    0xa830_4613,
    0xfd46_9501,
    0x6980_98d8,
    0x8b44_f7af,
    0xffff_5bb1,
    0x895c_d7be,
    0x6b90_1122,
    0xfd98_7193,
    0xa679_438e,
    0x49b4_0821,
    0xf61e_2562,
    0xc040_b340,
    0x265e_5a51,
    0xe9b6_c7aa,
    0xd62f_105d,
    0x0244_1453,
    0xd8a1_e681,
    0xe7d3_fbc8,
    0x21e1_cde6,
    0xc337_07d6,
    0xf4d5_0d87,
    0x455a_14ed,
    0xa9e3_e905,
    0xfcef_a3f8,
    0x676f_02d9,
    0x8d2a_4c8a,
    0xfffa_3942,
    0x8771_f681,
    0x6d9d_6122,
    0xfde5_380c,
    0xa4be_ea44,
    0x4bde_cfa9,
    0xf6bb_4b60,
    0xbebf_bc70,
    0x289b_7ec6,
    0xeaa1_27fa,
    0xd4ef_3085,
    0x0488_1d05,
    0xd9d4_d039,
    0xe6db_99e5,
    0x1fa2_7cf8,
    0xc4ac_5665,
    0xf429_2244,
    0x432a_ff97,
    0xab94_23a7,
    0xfc93_a039,
    0x655b_59c3,
    0x8f0c_cc92,
    0xffef_f47d,
    0x8584_5dd1,
    0x6fa8_7e4f,
    0xfe2c_e6e0,
    0xa301_4314,
    0x4e08_11a1,
    0xf753_7e82,
    0xbd3a_f235,
    0x2ad7_d2bb,
    0xeb86_d391,
];

const MESSAGE_ORDER: [usize; 64] = [
    0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 1, 6, 11, 0, 5, 10, 15, 4, 9, 14, 3, 8,
    13, 2, 7, 12, 5, 8, 11, 14, 1, 4, 7, 10, 13, 0, 3, 6, 9, 12, 15, 2, 0, 7, 14, 5, 12, 3, 10, 1,
    8, 15, 6, 13, 4, 11, 2, 9,
];

const ROUND_SHIFTS: [[u32; SHIFT_CYCLE]; 4] = [
    [7, 12, 17, 22],
    [5, 9, 14, 20],
    [4, 11, 16, 23],
    [6, 10, 15, 21],
];

#[derive(Clone)]
pub(super) struct Md5 {
    state: [u32; 4],
    buffer: BlockBuffer<BLOCK_BYTES>,
    length_bytes: u64,
}

impl Md5 {
    pub(super) const fn new() -> Self {
        Self {
            state: INITIAL_STATE,
            buffer: BlockBuffer::new(),
            length_bytes: 0,
        }
    }

    pub(super) fn update(&mut self, data: &[u8]) {
        self.length_bytes = self.length_bytes.wrapping_add(data.len() as u64);
        self.buffer
            .absorb(data, |block| compress(&mut self.state, block));
    }

    pub(super) fn finalize(mut self) -> [u8; OUTPUT_BYTES] {
        let length_bits = self.length_bytes.wrapping_mul(8);
        self.buffer.pad(&length_bits.to_le_bytes(), |block| {
            compress(&mut self.state, block)
        });
        let mut output = [0; OUTPUT_BYTES];
        for (chunk, word) in output.as_chunks_mut::<4>().0.iter_mut().zip(self.state) {
            *chunk = word.to_le_bytes();
        }
        output
    }
}

fn compress(state: &mut [u32; 4], block: &[u8; BLOCK_BYTES]) {
    let mut words = [0u32; 16];
    for (word, chunk) in words.iter_mut().zip(block.as_chunks::<4>().0) {
        *word = u32::from_le_bytes(*chunk);
    }
    let mut registers = *state;
    for step in 0..STEPS_PER_ROUND {
        let [_, b, c, d] = registers;
        mix(&mut registers, (b & c) | (!b & d), step, &words);
    }
    for step in STEPS_PER_ROUND..2 * STEPS_PER_ROUND {
        let [_, b, c, d] = registers;
        mix(&mut registers, (d & b) | (!d & c), step, &words);
    }
    for step in 2 * STEPS_PER_ROUND..3 * STEPS_PER_ROUND {
        let [_, b, c, d] = registers;
        mix(&mut registers, b ^ c ^ d, step, &words);
    }
    for step in 3 * STEPS_PER_ROUND..4 * STEPS_PER_ROUND {
        let [_, b, c, d] = registers;
        mix(&mut registers, c ^ (b | !d), step, &words);
    }
    for (word, register) in state.iter_mut().zip(registers) {
        *word = word.wrapping_add(register);
    }
}

#[inline(always)]
fn mix(registers: &mut [u32; 4], mixed: u32, step: usize, words: &[u32; 16]) {
    let [a, b, c, d] = *registers;
    let sum = a
        .wrapping_add(mixed)
        .wrapping_add(SINE_CONSTANTS[step])
        .wrapping_add(words[MESSAGE_ORDER[step]]);
    let shift = ROUND_SHIFTS[step / STEPS_PER_ROUND][step % SHIFT_CYCLE];
    *registers = [d, b.wrapping_add(sum.rotate_left(shift)), b, c];
}
