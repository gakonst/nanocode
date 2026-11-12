const INITIAL_STATE: [u32; 5] = [
    0x6745_2301,
    0xefcd_ab89,
    0x98ba_dcfe,
    0x1032_5476,
    0xc3d2_e1f0,
];
const ROUND_CONSTANTS: [u32; 4] = [0x5a82_7999, 0x6ed9_eba1, 0x8f1b_bcdc, 0xca62_c1d6];
pub(crate) const DIGEST_BYTES: usize = 20;
const BLOCK_BYTES: usize = 64;
const WORD_BYTES: usize = 4;
const LENGTH_BYTES: usize = 8;
const SCHEDULE_WORDS: usize = 80;
const BLOCK_WORDS: usize = 16;
const ROUNDS_PER_FUNCTION: usize = 20;
const PADDING_MARKER: u8 = 0x80;
const BITS_PER_BYTE: u64 = 8;
const SCHEDULE_ROTATION: u32 = 1;
const WORKING_ROTATION: u32 = 5;
const CARRY_ROTATION: u32 = 30;

pub(crate) fn sha1(message: &[u8]) -> [u8; DIGEST_BYTES] {
    let mut state = INITIAL_STATE;
    let (blocks, remainder) = message.as_chunks::<BLOCK_BYTES>();
    for block in blocks {
        compress(&mut state, block);
    }
    let mut tail = [0; 2 * BLOCK_BYTES];
    tail[..remainder.len()].copy_from_slice(remainder);
    tail[remainder.len()] = PADDING_MARKER;
    let tail_bytes = if remainder.len() + 1 + LENGTH_BYTES <= BLOCK_BYTES {
        BLOCK_BYTES
    } else {
        2 * BLOCK_BYTES
    };
    let bits = u64::try_from(message.len())
        .expect("a message length fits in 64 bits")
        .wrapping_mul(BITS_PER_BYTE);
    tail[tail_bytes - LENGTH_BYTES..tail_bytes].copy_from_slice(&bits.to_be_bytes());
    for block in tail[..tail_bytes].as_chunks::<BLOCK_BYTES>().0 {
        compress(&mut state, block);
    }
    let mut digest = [0; DIGEST_BYTES];
    for (bytes, word) in digest.as_chunks_mut::<WORD_BYTES>().0.iter_mut().zip(state) {
        *bytes = word.to_be_bytes();
    }
    digest
}

fn compress(state: &mut [u32; 5], block: &[u8; BLOCK_BYTES]) {
    let mut schedule = [0_u32; SCHEDULE_WORDS];
    for (word, bytes) in schedule.iter_mut().zip(block.as_chunks::<WORD_BYTES>().0) {
        *word = u32::from_be_bytes(*bytes);
    }
    for index in BLOCK_WORDS..SCHEDULE_WORDS {
        schedule[index] = (schedule[index - 3]
            ^ schedule[index - 8]
            ^ schedule[index - 14]
            ^ schedule[index - BLOCK_WORDS])
            .rotate_left(SCHEDULE_ROTATION);
    }
    let [mut a, mut b, mut c, mut d, mut e] = *state;
    for (index, word) in schedule.iter().enumerate() {
        let group = index / ROUNDS_PER_FUNCTION;
        let mixed = match group {
            0 => (b & c) | (!b & d),
            2 => (b & c) | (b & d) | (c & d),
            _ => b ^ c ^ d,
        };
        let next = a
            .rotate_left(WORKING_ROTATION)
            .wrapping_add(mixed)
            .wrapping_add(e)
            .wrapping_add(ROUND_CONSTANTS[group])
            .wrapping_add(*word);
        e = d;
        d = c;
        c = b.rotate_left(CARRY_ROTATION);
        b = a;
        a = next;
    }
    for (word, value) in state.iter_mut().zip([a, b, c, d, e]) {
        *word = word.wrapping_add(value);
    }
}
