use core::fmt;
use nanocode_sys::blake3::Blake3Instructions;

pub const BLAKE3_OUTPUT_BYTES: usize = 32;

const BLOCK_BYTES: usize = 64;
const CHUNK_BYTES: usize = 1024;
const BLOCK_WORDS: usize = 16;
const CHAINING_WORDS: usize = 8;
const CHAINING_BYTES: usize = 32;
const SUBTREE_CHUNKS_MAX: usize = 256;
const PARENTS_PER_PASS: usize = 16;
const ROUNDS: usize = 7;
const HEX_BYTES: usize = 2 * BLAKE3_OUTPUT_BYTES;
const STACK_DEPTH_MAX: usize = (u64::BITS - CHUNK_BYTES.trailing_zeros()) as usize;

const CHUNK_START: u32 = 1 << 0;
const CHUNK_END: u32 = 1 << 1;
const PARENT: u32 = 1 << 2;
const ROOT: u32 = 1 << 3;

const IV: [u32; CHAINING_WORDS] = [
    0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
];

const MESSAGE_PERMUTATION: [usize; BLOCK_WORDS] =
    [2, 6, 3, 10, 7, 0, 4, 13, 1, 11, 12, 5, 9, 14, 15, 8];

const MESSAGE_SCHEDULE: [[usize; BLOCK_WORDS]; ROUNDS] = message_schedule();

const HEX_DIGITS: &[u8; 16] = b"0123456789abcdef";

const fn message_schedule() -> [[usize; BLOCK_WORDS]; ROUNDS] {
    let mut schedule = [[0; BLOCK_WORDS]; ROUNDS];
    let mut index = 0;
    while index < BLOCK_WORDS {
        schedule[0][index] = index;
        index += 1;
    }
    let mut round = 1;
    while round < ROUNDS {
        let mut index = 0;
        while index < BLOCK_WORDS {
            schedule[round][index] = schedule[round - 1][MESSAGE_PERMUTATION[index]];
            index += 1;
        }
        round += 1;
    }
    schedule
}

#[derive(Clone, Copy)]
pub struct Hash([u8; BLAKE3_OUTPUT_BYTES]);

impl Hash {
    pub const fn from_bytes(bytes: [u8; BLAKE3_OUTPUT_BYTES]) -> Self {
        Self(bytes)
    }

    pub const fn as_bytes(&self) -> &[u8; BLAKE3_OUTPUT_BYTES] {
        &self.0
    }

    pub fn to_hex(&self) -> Hex {
        let mut digits = [0u8; HEX_BYTES];
        for (pair, byte) in digits.as_chunks_mut::<2>().0.iter_mut().zip(self.0) {
            pair[0] = HEX_DIGITS[usize::from(byte >> 4)];
            pair[1] = HEX_DIGITS[usize::from(byte & 0x0f)];
        }
        Hex(digits)
    }
}

impl PartialEq for Hash {
    fn eq(&self, other: &Self) -> bool {
        self.0
            .iter()
            .zip(&other.0)
            .fold(0u8, |difference, (left, right)| difference | (left ^ right))
            == 0
    }
}

impl Eq for Hash {}

impl core::hash::Hash for Hash {
    fn hash<H: core::hash::Hasher>(&self, state: &mut H) {
        self.0.hash(state);
    }
}

impl fmt::Debug for Hash {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("Hash")
            .field(&self.to_hex().as_str())
            .finish()
    }
}

impl fmt::Display for Hash {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.to_hex().as_str())
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Hex([u8; HEX_BYTES]);

impl Hex {
    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.0).expect("hex digits are ASCII")
    }
}

impl fmt::Display for Hex {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl fmt::Debug for Hex {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.as_str(), formatter)
    }
}

pub fn blake3(bytes: &[u8]) -> Hash {
    if bytes.len() <= CHUNK_BYTES {
        let mut chunk = ChunkState::new(0);
        chunk.update(bytes);
        return chunk.output().root_hash();
    }
    let mut hasher = Blake3::new();
    hasher.update(bytes);
    hasher.finalize()
}

#[derive(Clone, Debug)]
pub struct Blake3 {
    chunk: ChunkState,
    stack: [[u32; CHAINING_WORDS]; STACK_DEPTH_MAX + 1],
    stack_len: usize,
}

impl Default for Blake3 {
    fn default() -> Self {
        Self::new()
    }
}

impl Blake3 {
    pub const fn new() -> Self {
        Self {
            chunk: ChunkState::new(0),
            stack: [[0; CHAINING_WORDS]; STACK_DEPTH_MAX + 1],
            stack_len: 0,
        }
    }

    pub fn update(&mut self, mut bytes: &[u8]) -> &mut Self {
        if self.chunk.len() > 0 {
            let taken = (CHUNK_BYTES - self.chunk.len()).min(bytes.len());
            let (now, later) = bytes.split_at(taken);
            self.chunk.update(now);
            bytes = later;
            if bytes.is_empty() {
                return self;
            }
            let counter = self.chunk.counter;
            let chaining_value = self.chunk.output().chaining_value();
            self.push_chaining_value(chaining_value, counter);
            self.chunk = ChunkState::new(chunks_after(counter, 1));
        }
        if bytes.len() > CHUNK_BYTES {
            let instructions = Blake3Instructions::detect();
            while bytes.len() > CHUNK_BYTES {
                bytes = self.update_subtree(instructions, bytes);
            }
        }
        if !bytes.is_empty() {
            self.chunk.update(bytes);
            self.merge_stack(self.chunk.counter);
        }
        self
    }

    pub fn finalize(&self) -> Hash {
        let stack = &self.stack[..self.stack_len];
        let (mut output, lefts) = if self.chunk.len() > 0 || stack.is_empty() {
            (self.chunk.output(), stack)
        } else {
            assert!(stack.len() >= 2);
            let (lefts, children) = stack.split_at(stack.len() - 2);
            (parent_output(&children[0], &children[1]), lefts)
        };
        for left in lefts.iter().rev() {
            output = parent_output(left, &output.chaining_value());
        }
        output.root_hash()
    }

    fn update_subtree<'a>(
        &mut self,
        instructions: Option<Blake3Instructions>,
        bytes: &'a [u8],
    ) -> &'a [u8] {
        assert_eq!(self.chunk.len(), 0);
        let counter = self.chunk.counter;
        let chunks_available = bytes.len() / CHUNK_BYTES;
        assert!(chunks_available > 0);
        let subtree_log2 = chunks_available
            .ilog2()
            .min(counter.trailing_zeros())
            .min(SUBTREE_CHUNKS_MAX.ilog2());
        let subtree_chunks = 1usize << subtree_log2;
        let (subtree, rest) = bytes.split_at(subtree_chunks * CHUNK_BYTES);
        let (chunks, remainder) = subtree.as_chunks::<CHUNK_BYTES>();
        assert!(remainder.is_empty());
        if let [chunk] = chunks {
            let chaining_value = chunk_chaining_value(chunk, counter);
            self.push_chaining_value(chaining_value, counter);
        } else {
            let [left, right] = subtree_children(instructions, chunks, counter);
            self.push_chaining_value(left, counter);
            let right_counter = chunks_after(counter, subtree_chunks / 2);
            self.push_chaining_value(right, right_counter);
        }
        self.chunk = ChunkState::new(chunks_after(counter, subtree_chunks));
        rest
    }

    fn push_chaining_value(&mut self, chaining_value: [u32; CHAINING_WORDS], counter: u64) {
        self.merge_stack(counter);
        assert!(self.stack_len < self.stack.len());
        self.stack[self.stack_len] = chaining_value;
        self.stack_len += 1;
    }

    fn merge_stack(&mut self, total_chunks: u64) {
        let kept = total_chunks.count_ones() as usize;
        while self.stack_len > kept {
            assert!(self.stack_len >= 2);
            let right = self.stack[self.stack_len - 1];
            let left = &self.stack[self.stack_len - 2];
            self.stack[self.stack_len - 2] = parent_output(left, &right).chaining_value();
            self.stack_len -= 1;
        }
    }
}

const fn chunks_after(counter: u64, chunks: usize) -> u64 {
    counter
        .checked_add(chunks as u64)
        .expect("BLAKE3 bounds the input below 2^64 chunks")
}

fn subtree_children(
    instructions: Option<Blake3Instructions>,
    chunks: &[[u8; CHUNK_BYTES]],
    counter: u64,
) -> [[u32; CHAINING_WORDS]; 2] {
    assert!(chunks.len() >= 2);
    assert!(chunks.len() <= SUBTREE_CHUNKS_MAX);
    assert!(chunks.len().is_power_of_two());
    if chunks.len() <= PARENTS_PER_PASS {
        let mut level = [[0u8; CHAINING_BYTES]; PARENTS_PER_PASS];
        reduce_subtree(instructions, chunks, counter, &mut level)
    } else {
        let mut level = [[0u8; CHAINING_BYTES]; SUBTREE_CHUNKS_MAX];
        reduce_subtree(instructions, chunks, counter, &mut level)
    }
}

fn reduce_subtree(
    instructions: Option<Blake3Instructions>,
    chunks: &[[u8; CHUNK_BYTES]],
    counter: u64,
    level: &mut [[u8; CHAINING_BYTES]],
) -> [[u32; CHAINING_WORDS]; 2] {
    assert!(chunks.len() <= level.len());
    let mut count = chunks.len();
    hash_chunks(instructions, chunks, counter, &mut level[..count]);
    while count > 2 {
        let mut parents = [[0u8; CHAINING_BYTES]; PARENTS_PER_PASS];
        for pass in 0..(count / 2).div_ceil(PARENTS_PER_PASS) {
            let first = pass * PARENTS_PER_PASS;
            let taken = PARENTS_PER_PASS.min(count / 2 - first);
            let children = &level[2 * first..2 * (first + taken)];
            let (blocks, remainder) = children.as_flattened().as_chunks::<BLOCK_BYTES>();
            assert!(remainder.is_empty());
            hash_parents(instructions, blocks, &mut parents[..taken]);
            level[first..first + taken].copy_from_slice(&parents[..taken]);
        }
        count /= 2;
    }
    [chaining_words(&level[0]), chaining_words(&level[1])]
}

fn hash_chunks(
    instructions: Option<Blake3Instructions>,
    chunks: &[[u8; CHUNK_BYTES]],
    counter: u64,
    chaining_values: &mut [[u8; CHAINING_BYTES]],
) {
    assert_eq!(chunks.len(), chaining_values.len());
    match instructions {
        Some(instructions) => instructions.hash_chunks(chunks, &IV, counter, 0, chaining_values),
        None => {
            for (index, (chunk, chaining_value)) in chunks.iter().zip(chaining_values).enumerate() {
                *chaining_value =
                    chaining_bytes(&chunk_chaining_value(chunk, chunks_after(counter, index)));
            }
        }
    }
}

fn hash_parents(
    instructions: Option<Blake3Instructions>,
    parents: &[[u8; BLOCK_BYTES]],
    chaining_values: &mut [[u8; CHAINING_BYTES]],
) {
    assert_eq!(parents.len(), chaining_values.len());
    match instructions {
        Some(instructions) => instructions.hash_parents(parents, &IV, 0, chaining_values),
        None => {
            for (parent, chaining_value) in parents.iter().zip(chaining_values) {
                let words = compress(&IV, &block_words(parent), 0, BLOCK_BYTES as u32, PARENT);
                *chaining_value = chaining_bytes(&words);
            }
        }
    }
}

fn chunk_chaining_value(chunk: &[u8; CHUNK_BYTES], counter: u64) -> [u32; CHAINING_WORDS] {
    let mut state = ChunkState::new(counter);
    state.update(chunk);
    state.output().chaining_value()
}

fn chaining_words(bytes: &[u8; CHAINING_BYTES]) -> [u32; CHAINING_WORDS] {
    let mut words = [0u32; CHAINING_WORDS];
    for (word, bytes) in words.iter_mut().zip(bytes.as_chunks::<4>().0) {
        *word = u32::from_le_bytes(*bytes);
    }
    words
}

fn chaining_bytes(words: &[u32; CHAINING_WORDS]) -> [u8; CHAINING_BYTES] {
    let mut bytes = [0u8; CHAINING_BYTES];
    for (slot, word) in bytes.as_chunks_mut::<4>().0.iter_mut().zip(words) {
        *slot = word.to_le_bytes();
    }
    bytes
}

#[derive(Clone, Debug)]
struct ChunkState {
    chaining_value: [u32; CHAINING_WORDS],
    counter: u64,
    block: [u8; BLOCK_BYTES],
    block_len: usize,
    blocks_compressed: usize,
}

impl ChunkState {
    const fn new(counter: u64) -> Self {
        Self {
            chaining_value: IV,
            counter,
            block: [0; BLOCK_BYTES],
            block_len: 0,
            blocks_compressed: 0,
        }
    }

    const fn len(&self) -> usize {
        self.blocks_compressed * BLOCK_BYTES + self.block_len
    }

    const fn start_flag(&self) -> u32 {
        if self.blocks_compressed == 0 {
            CHUNK_START
        } else {
            0
        }
    }

    fn compress_block(&mut self, block: &[u8; BLOCK_BYTES]) {
        let words = block_words(block);
        self.chaining_value = compress(
            &self.chaining_value,
            &words,
            self.counter,
            BLOCK_BYTES as u32,
            self.start_flag(),
        );
        self.blocks_compressed += 1;
    }

    fn update(&mut self, mut bytes: &[u8]) {
        assert!(self.len() + bytes.len() <= CHUNK_BYTES);
        while !bytes.is_empty() {
            if self.block_len == BLOCK_BYTES {
                let block = self.block;
                self.compress_block(&block);
                self.block_len = 0;
            }
            if self.block_len == 0 {
                while bytes.len() > BLOCK_BYTES
                    && let Some((block, rest)) = bytes.split_first_chunk::<BLOCK_BYTES>()
                {
                    self.compress_block(block);
                    bytes = rest;
                }
            }
            let taken = (BLOCK_BYTES - self.block_len).min(bytes.len());
            let (now, later) = bytes.split_at(taken);
            self.block[self.block_len..self.block_len + taken].copy_from_slice(now);
            self.block_len += taken;
            bytes = later;
        }
    }

    fn output(&self) -> Output {
        let mut block = [0u8; BLOCK_BYTES];
        block[..self.block_len].copy_from_slice(&self.block[..self.block_len]);
        Output {
            input_chaining_value: self.chaining_value,
            block_words: block_words(&block),
            counter: self.counter,
            block_len: self.block_len as u32,
            flags: self.start_flag() | CHUNK_END,
        }
    }
}

struct Output {
    input_chaining_value: [u32; CHAINING_WORDS],
    block_words: [u32; BLOCK_WORDS],
    counter: u64,
    block_len: u32,
    flags: u32,
}

impl Output {
    fn chaining_value(&self) -> [u32; CHAINING_WORDS] {
        compress(
            &self.input_chaining_value,
            &self.block_words,
            self.counter,
            self.block_len,
            self.flags,
        )
    }

    fn root_hash(&self) -> Hash {
        let words = compress(
            &self.input_chaining_value,
            &self.block_words,
            0,
            self.block_len,
            self.flags | ROOT,
        );
        let mut bytes = [0u8; BLAKE3_OUTPUT_BYTES];
        for (slot, word) in bytes.as_chunks_mut::<4>().0.iter_mut().zip(words) {
            slot.copy_from_slice(&word.to_le_bytes());
        }
        Hash(bytes)
    }
}

fn parent_output(left: &[u32; CHAINING_WORDS], right: &[u32; CHAINING_WORDS]) -> Output {
    let mut block_words = [0u32; BLOCK_WORDS];
    block_words[..CHAINING_WORDS].copy_from_slice(left);
    block_words[CHAINING_WORDS..].copy_from_slice(right);
    Output {
        input_chaining_value: IV,
        block_words,
        counter: 0,
        block_len: BLOCK_BYTES as u32,
        flags: PARENT,
    }
}

#[inline(always)]
fn block_words(block: &[u8; BLOCK_BYTES]) -> [u32; BLOCK_WORDS] {
    let mut words = [0u32; BLOCK_WORDS];
    for (word, bytes) in words.iter_mut().zip(block.as_chunks::<4>().0) {
        *word = u32::from_le_bytes(*bytes);
    }
    words
}

#[inline(always)]
const fn mix(
    state: &mut [u32; BLOCK_WORDS],
    a: usize,
    b: usize,
    c: usize,
    d: usize,
    x: u32,
    y: u32,
) {
    state[a] = state[a].wrapping_add(state[b]).wrapping_add(x);
    state[d] = (state[d] ^ state[a]).rotate_right(16);
    state[c] = state[c].wrapping_add(state[d]);
    state[b] = (state[b] ^ state[c]).rotate_right(12);
    state[a] = state[a].wrapping_add(state[b]).wrapping_add(y);
    state[d] = (state[d] ^ state[a]).rotate_right(8);
    state[c] = state[c].wrapping_add(state[d]);
    state[b] = (state[b] ^ state[c]).rotate_right(7);
}

#[inline(always)]
const fn round(
    state: &mut [u32; BLOCK_WORDS],
    words: &[u32; BLOCK_WORDS],
    schedule: &[usize; BLOCK_WORDS],
) {
    mix(state, 0, 4, 8, 12, words[schedule[0]], words[schedule[1]]);
    mix(state, 1, 5, 9, 13, words[schedule[2]], words[schedule[3]]);
    mix(state, 2, 6, 10, 14, words[schedule[4]], words[schedule[5]]);
    mix(state, 3, 7, 11, 15, words[schedule[6]], words[schedule[7]]);
    mix(state, 0, 5, 10, 15, words[schedule[8]], words[schedule[9]]);
    mix(
        state,
        1,
        6,
        11,
        12,
        words[schedule[10]],
        words[schedule[11]],
    );
    mix(state, 2, 7, 8, 13, words[schedule[12]], words[schedule[13]]);
    mix(state, 3, 4, 9, 14, words[schedule[14]], words[schedule[15]]);
}

#[inline(always)]
fn compress(
    chaining_value: &[u32; CHAINING_WORDS],
    words: &[u32; BLOCK_WORDS],
    counter: u64,
    block_len: u32,
    flags: u32,
) -> [u32; CHAINING_WORDS] {
    let mut state = [
        chaining_value[0],
        chaining_value[1],
        chaining_value[2],
        chaining_value[3],
        chaining_value[4],
        chaining_value[5],
        chaining_value[6],
        chaining_value[7],
        IV[0],
        IV[1],
        IV[2],
        IV[3],
        counter as u32,
        (counter >> u32::BITS) as u32,
        block_len,
        flags,
    ];
    for schedule in &MESSAGE_SCHEDULE {
        round(&mut state, words, schedule);
    }
    let mut output = [0u32; CHAINING_WORDS];
    for (index, word) in output.iter_mut().enumerate() {
        *word = state[index] ^ state[index + CHAINING_WORDS];
    }
    output
}
