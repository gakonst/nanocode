use super::blake2::{COLUMNS_THEN_DIAGONALS, Layout, Parameters, SIGMA};

pub(super) const BLOCK_BYTES: usize = 128;
pub(super) const OUTPUT_BYTES: usize = 64;
const WORD_BYTES: usize = 8;
const PARAMETER_BYTES: usize = 8 * WORD_BYTES;
const ROUNDS: usize = 12;
const ROTATIONS: [u32; 4] = [32, 24, 16, 63];

pub(super) const LAYOUT: Layout = Layout {
    output_bytes: OUTPUT_BYTES,
    key_bytes: 64,
    node_offset_bytes: 8,
    reserved_bytes: 14,
    salt_bytes: 16,
};

const INITIAL_VECTOR: [u64; 8] = [
    0x6a09_e667_f3bc_c908,
    0xbb67_ae85_84ca_a73b,
    0x3c6e_f372_fe94_f82b,
    0xa54f_f53a_5f1d_36f1,
    0x510e_527f_ade6_82d1,
    0x9b05_688c_2b3e_6c1f,
    0x1f83_d9ab_fb41_bd6b,
    0x5be0_cd19_137e_2179,
];

#[derive(Clone)]
pub(super) struct Blake2b {
    state: [u64; 8],
    block: [u8; BLOCK_BYTES],
    filled: usize,
    counter: u128,
    output_bytes: usize,
    last_node: bool,
}

impl Blake2b {
    pub(super) fn new(parameters: &Parameters<'_>) -> Self {
        let parameter_block: [u8; PARAMETER_BYTES] = parameters.block(&LAYOUT);
        let mut state = INITIAL_VECTOR;
        for (word, chunk) in state
            .iter_mut()
            .zip(parameter_block.as_chunks::<WORD_BYTES>().0)
        {
            *word ^= u64::from_le_bytes(*chunk);
        }
        let mut block = [0; BLOCK_BYTES];
        block[..parameters.key.len()].copy_from_slice(parameters.key);
        let filled = if parameters.key.is_empty() {
            0
        } else {
            BLOCK_BYTES
        };
        Self {
            state,
            block,
            filled,
            counter: 0,
            output_bytes: parameters.digest_size,
            last_node: parameters.last_node,
        }
    }

    pub(super) fn update(&mut self, mut data: &[u8]) {
        while !data.is_empty() {
            if self.filled == BLOCK_BYTES {
                self.counter = self.counter.wrapping_add(BLOCK_BYTES as u128);
                compress(&mut self.state, &self.block, self.counter, [0, 0]);
                self.filled = 0;
            }
            if self.filled == 0
                && let Some((block, rest)) = data.split_first_chunk::<BLOCK_BYTES>()
                && !rest.is_empty()
            {
                self.counter = self.counter.wrapping_add(BLOCK_BYTES as u128);
                compress(&mut self.state, block, self.counter, [0, 0]);
                data = rest;
                continue;
            }
            let taken = data.len().min(BLOCK_BYTES - self.filled);
            self.block[self.filled..self.filled + taken].copy_from_slice(&data[..taken]);
            self.filled += taken;
            data = &data[taken..];
        }
    }

    pub(super) fn finalize(mut self) -> Vec<u8> {
        self.counter = self.counter.wrapping_add(self.filled as u128);
        self.block[self.filled..].fill(0);
        let last_node = if self.last_node { u64::MAX } else { 0 };
        compress(
            &mut self.state,
            &self.block,
            self.counter,
            [u64::MAX, last_node],
        );
        let mut output = Vec::with_capacity(OUTPUT_BYTES);
        for word in self.state {
            output.extend_from_slice(&word.to_le_bytes());
        }
        output.truncate(self.output_bytes);
        output
    }
}

fn compress(
    state: &mut [u64; 8],
    block: &[u8; BLOCK_BYTES],
    counter: u128,
    finalization: [u64; 2],
) {
    let mut message = [0; 16];
    for (word, chunk) in message.iter_mut().zip(block.as_chunks::<WORD_BYTES>().0) {
        *word = u64::from_le_bytes(*chunk);
    }
    let mut work = [0; 16];
    work[..8].copy_from_slice(state);
    work[8..].copy_from_slice(&INITIAL_VECTOR);
    work[12] ^= counter as u64;
    work[13] ^= (counter >> u64::BITS) as u64;
    work[14] ^= finalization[0];
    work[15] ^= finalization[1];
    for round in 0..ROUNDS {
        let permuted = SIGMA[round % SIGMA.len()].map(|index| message[index]);
        for (index, [a, b, c, d]) in COLUMNS_THEN_DIAGONALS.into_iter().enumerate() {
            let x = permuted[2 * index];
            let y = permuted[2 * index + 1];
            work[a] = work[a].wrapping_add(work[b]).wrapping_add(x);
            work[d] = (work[d] ^ work[a]).rotate_right(ROTATIONS[0]);
            work[c] = work[c].wrapping_add(work[d]);
            work[b] = (work[b] ^ work[c]).rotate_right(ROTATIONS[1]);
            work[a] = work[a].wrapping_add(work[b]).wrapping_add(y);
            work[d] = (work[d] ^ work[a]).rotate_right(ROTATIONS[2]);
            work[c] = work[c].wrapping_add(work[d]);
            work[b] = (work[b] ^ work[c]).rotate_right(ROTATIONS[3]);
        }
    }
    for (index, word) in state.iter_mut().enumerate() {
        *word ^= work[index] ^ work[index + 8];
    }
}
