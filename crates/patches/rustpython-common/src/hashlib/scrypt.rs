use super::compute_pbkdf2_hmac;

const SALSA_WORDS: usize = 16;
const SALSA_DOUBLE_ROUNDS: usize = 4;
const WORD_BYTES: usize = 4;
const SALSA_BYTES: usize = SALSA_WORDS * WORD_BYTES;
const BLOCK_SIZE_PRODUCT_LIMIT: u64 = 1 << 30;
const PBKDF2_PRF: &str = "sha256";
const PBKDF2_ITERATIONS: usize = 1;
const PBKDF2_OUTPUT_BYTES: usize = 32;
const PBKDF2_BLOCKS_MAX: usize = u32::MAX as usize;

const COLUMNS_THEN_ROWS: [[usize; 4]; 8] = [
    [0, 4, 8, 12],
    [5, 9, 13, 1],
    [10, 14, 2, 6],
    [15, 3, 7, 11],
    [0, 1, 2, 3],
    [5, 6, 7, 4],
    [10, 11, 8, 9],
    [15, 12, 13, 14],
];

const QUARTER_ROTATIONS: [u32; 4] = [7, 9, 13, 18];

pub(super) fn scrypt(
    password: &[u8],
    salt: &[u8],
    log_n: u8,
    r: u32,
    p: u32,
    output_bytes: usize,
) -> Option<Vec<u8>> {
    if r == 0 || p == 0 || u32::from(log_n) >= usize::BITS {
        return None;
    }
    let cost = 1usize << log_n;
    let block_bytes = usize::try_from(r).ok()?.checked_mul(2 * SALSA_BYTES)?;
    block_bytes.checked_mul(cost)?;
    let blocks_bytes = block_bytes.checked_mul(usize::try_from(p).ok()?)?;
    if u64::from(r) * u64::from(p) >= BLOCK_SIZE_PRODUCT_LIMIT {
        return None;
    }
    if output_bytes == 0 || output_bytes / PBKDF2_OUTPUT_BYTES > PBKDF2_BLOCKS_MAX {
        return None;
    }
    let derived = compute_pbkdf2_hmac(PBKDF2_PRF, password, salt, PBKDF2_ITERATIONS, blocks_bytes)?;
    let mut words: Vec<u32> = derived
        .as_chunks::<WORD_BYTES>()
        .0
        .iter()
        .map(|chunk| u32::from_le_bytes(*chunk))
        .collect();
    let block_words = block_bytes / WORD_BYTES;
    let mut memory = vec![0u32; block_words * cost];
    let mut scratch = vec![0u32; block_words];
    for block in words.chunks_exact_mut(block_words) {
        romix(block, &mut memory, &mut scratch);
    }
    let mixed: Vec<u8> = words.iter().flat_map(|word| word.to_le_bytes()).collect();
    compute_pbkdf2_hmac(
        PBKDF2_PRF,
        password,
        &mixed,
        PBKDF2_ITERATIONS,
        output_bytes,
    )
}

fn romix(block: &mut [u32], memory: &mut [u32], scratch: &mut [u32]) {
    let block_words = block.len();
    assert_eq!(scratch.len(), block_words);
    assert!(memory.len().is_multiple_of(block_words));
    let cost = memory.len() / block_words;
    assert!(cost.is_power_of_two());
    for slot in memory.chunks_exact_mut(block_words) {
        slot.copy_from_slice(block);
        block_mix(slot, block);
    }
    for _ in 0..cost {
        let last = &block[block_words - SALSA_WORDS..];
        let integer = u64::from(last[0]) | (u64::from(last[1]) << u32::BITS);
        let index = (integer & (cost as u64 - 1)) as usize;
        let slot = &memory[index * block_words..(index + 1) * block_words];
        for ((mixed, word), stored) in scratch.iter_mut().zip(block.iter()).zip(slot) {
            *mixed = word ^ stored;
        }
        block_mix(scratch, block);
    }
}

fn block_mix(input: &[u32], output: &mut [u32]) {
    assert_eq!(input.len(), output.len());
    let (chunks, rest) = input.as_chunks::<SALSA_WORDS>();
    assert!(rest.is_empty());
    assert!(chunks.len().is_multiple_of(2));
    let half = chunks.len() / 2;
    let mut state = *chunks.last().expect("a block has salsa chunks");
    for (index, chunk) in chunks.iter().enumerate() {
        for (word, input_word) in state.iter_mut().zip(chunk) {
            *word ^= input_word;
        }
        salsa20_8(&mut state);
        let target = index / 2 + if index.is_multiple_of(2) { 0 } else { half };
        output[target * SALSA_WORDS..(target + 1) * SALSA_WORDS].copy_from_slice(&state);
    }
}

fn salsa20_8(block: &mut [u32; SALSA_WORDS]) {
    let mut words = *block;
    for _ in 0..SALSA_DOUBLE_ROUNDS {
        for [a, b, c, d] in COLUMNS_THEN_ROWS {
            words[b] ^= words[a]
                .wrapping_add(words[d])
                .rotate_left(QUARTER_ROTATIONS[0]);
            words[c] ^= words[b]
                .wrapping_add(words[a])
                .rotate_left(QUARTER_ROTATIONS[1]);
            words[d] ^= words[c]
                .wrapping_add(words[b])
                .rotate_left(QUARTER_ROTATIONS[2]);
            words[a] ^= words[d]
                .wrapping_add(words[c])
                .rotate_left(QUARTER_ROTATIONS[3]);
        }
    }
    for (word, mixed) in block.iter_mut().zip(words) {
        *word = word.wrapping_add(mixed);
    }
}
