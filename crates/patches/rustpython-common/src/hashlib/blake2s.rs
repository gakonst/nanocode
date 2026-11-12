use super::blake2::{COLUMNS_THEN_DIAGONALS, Layout, Parameters, SIGMA};

pub(super) const BLOCK_BYTES: usize = 64;
pub(super) const OUTPUT_BYTES: usize = 32;
const WORD_BYTES: usize = 4;
const PARAMETER_BYTES: usize = 8 * WORD_BYTES;
const ROUNDS: usize = 10;
const ROTATIONS: [u32; 4] = [16, 12, 8, 7];

pub(super) const LAYOUT: Layout = Layout {
    output_bytes: OUTPUT_BYTES,
    key_bytes: 32,
    node_offset_bytes: 6,
    reserved_bytes: 0,
    salt_bytes: 8,
};

const INITIAL_VECTOR: [u32; 8] = [
    0x6a09_e667,
    0xbb67_ae85,
    0x3c6e_f372,
    0xa54f_f53a,
    0x510e_527f,
    0x9b05_688c,
    0x1f83_d9ab,
    0x5be0_cd19,
];

#[derive(Clone)]
pub(super) struct Blake2s {
    state: [u32; 8],
    block: [u8; BLOCK_BYTES],
    filled: usize,
    counter: u64,
    output_bytes: usize,
    last_node: bool,
}

impl Blake2s {
    pub(super) fn new(parameters: &Parameters<'_>) -> Self {
        let parameter_block: [u8; PARAMETER_BYTES] = parameters.block(&LAYOUT);
        let mut state = INITIAL_VECTOR;
        for (word, chunk) in state
            .iter_mut()
            .zip(parameter_block.as_chunks::<WORD_BYTES>().0)
        {
            *word ^= u32::from_le_bytes(*chunk);
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
                self.counter = self.counter.wrapping_add(BLOCK_BYTES as u64);
                compress(&mut self.state, &self.block, self.counter, [0, 0]);
                self.filled = 0;
            }
            if self.filled == 0
                && let Some((block, rest)) = data.split_first_chunk::<BLOCK_BYTES>()
                && !rest.is_empty()
            {
                self.counter = self.counter.wrapping_add(BLOCK_BYTES as u64);
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
        self.counter = self.counter.wrapping_add(self.filled as u64);
        self.block[self.filled..].fill(0);
        let last_node = if self.last_node { u32::MAX } else { 0 };
        compress(
            &mut self.state,
            &self.block,
            self.counter,
            [u32::MAX, last_node],
        );
        let mut output = Vec::with_capacity(OUTPUT_BYTES);
        for word in self.state {
            output.extend_from_slice(&word.to_le_bytes());
        }
        output.truncate(self.output_bytes);
        output
    }
}

fn compress(state: &mut [u32; 8], block: &[u8; BLOCK_BYTES], counter: u64, finalization: [u32; 2]) {
    let mut message = [0; 16];
    for (word, chunk) in message.iter_mut().zip(block.as_chunks::<WORD_BYTES>().0) {
        *word = u32::from_le_bytes(*chunk);
    }
    let mut work = [0; 16];
    work[..8].copy_from_slice(state);
    work[8..].copy_from_slice(&INITIAL_VECTOR);
    work[12] ^= counter as u32;
    work[13] ^= (counter >> u32::BITS) as u32;
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
