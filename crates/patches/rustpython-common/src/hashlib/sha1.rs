use nanocode_sys::sha1::Sha1Instructions;

use super::block::BlockBuffer;

pub(super) const BLOCK_BYTES: usize = 64;
pub(super) const OUTPUT_BYTES: usize = 20;
const SCHEDULE_WORDS: usize = 80;
const STEPS_PER_ROUND: usize = 20;

const INITIAL_STATE: [u32; 5] = [
    0x6745_2301,
    0xefcd_ab89,
    0x98ba_dcfe,
    0x1032_5476,
    0xc3d2_e1f0,
];

const ROUND_CONSTANTS: [u32; 4] = [0x5a82_7999, 0x6ed9_eba1, 0x8f1b_bcdc, 0xca62_c1d6];

#[derive(Clone)]
pub(super) struct Sha1 {
    state: [u32; 5],
    buffer: BlockBuffer<BLOCK_BYTES>,
    length_bytes: u64,
}

impl Sha1 {
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
        self.buffer.pad(&length_bits.to_be_bytes(), |block| {
            compress(&mut self.state, block)
        });
        let mut output = [0; OUTPUT_BYTES];
        for (chunk, word) in output.as_chunks_mut::<4>().0.iter_mut().zip(self.state) {
            *chunk = word.to_be_bytes();
        }
        output
    }
}

fn compress(state: &mut [u32; 5], block: &[u8; BLOCK_BYTES]) {
    match Sha1Instructions::detect() {
        Some(instructions) => instructions.compress(state, core::slice::from_ref(block)),
        None => compress_portable(state, block),
    }
}

fn compress_portable(state: &mut [u32; 5], block: &[u8; BLOCK_BYTES]) {
    let mut schedule = [0u32; SCHEDULE_WORDS];
    for (word, chunk) in schedule.iter_mut().zip(block.as_chunks::<4>().0) {
        *word = u32::from_be_bytes(*chunk);
    }
    for index in 16..SCHEDULE_WORDS {
        schedule[index] = (schedule[index - 3]
            ^ schedule[index - 8]
            ^ schedule[index - 14]
            ^ schedule[index - 16])
            .rotate_left(1);
    }
    let mut registers = *state;
    for word in &schedule[0..STEPS_PER_ROUND] {
        let [_, b, c, d, _] = registers;
        mix(&mut registers, (b & c) | (!b & d), 0, *word);
    }
    for word in &schedule[STEPS_PER_ROUND..2 * STEPS_PER_ROUND] {
        let [_, b, c, d, _] = registers;
        mix(&mut registers, b ^ c ^ d, 1, *word);
    }
    for word in &schedule[2 * STEPS_PER_ROUND..3 * STEPS_PER_ROUND] {
        let [_, b, c, d, _] = registers;
        mix(&mut registers, (b & c) | (b & d) | (c & d), 2, *word);
    }
    for word in &schedule[3 * STEPS_PER_ROUND..4 * STEPS_PER_ROUND] {
        let [_, b, c, d, _] = registers;
        mix(&mut registers, b ^ c ^ d, 3, *word);
    }
    for (word, register) in state.iter_mut().zip(registers) {
        *word = word.wrapping_add(register);
    }
}

#[inline(always)]
fn mix(registers: &mut [u32; 5], mixed: u32, round: usize, word: u32) {
    let [a, b, c, d, e] = *registers;
    let temporary = a
        .rotate_left(5)
        .wrapping_add(mixed)
        .wrapping_add(e)
        .wrapping_add(ROUND_CONSTANTS[round])
        .wrapping_add(word);
    *registers = [temporary, a, b.rotate_left(30), c, d];
}
