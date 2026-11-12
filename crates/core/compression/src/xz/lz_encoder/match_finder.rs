mod binary_tree;
mod hash_chain;

use alloc::vec::Vec;

use super::super::coder::Coder;
use super::super::crc32::TABLE;
use super::super::filter_chain::FILTERS_MAX;
use super::super::lzma_common::DICT_SIZE_MIN;
use super::super::options::MatchFinder as MatchFinderId;
use super::super::stream::{Action, Error, Status};

pub(super) const DICT_SIZE_MAX: u32 = (1 << 30) + (1 << 29);
const HASH_2_SIZE: u32 = 1 << 10;
const HASH_3_SIZE: u32 = 1 << 16;
const HASH_2_MASK: u32 = HASH_2_SIZE - 1;
const HASH_3_MASK: u32 = HASH_3_SIZE - 1;
const FIX_3_HASH_SIZE: u32 = HASH_2_SIZE;
const FIX_4_HASH_SIZE: u32 = HASH_2_SIZE + HASH_3_SIZE;
const HASH_TWO_BYTES_MASK: u32 = 0xFFFF;
const HASH_MASK_MIN: u32 = 0xFFFF;
const HASH_MASK_LIMIT: u32 = 1 << 24;
const HASH_3_BYTES_MASK_MAX: u32 = (1 << 24) - 1;
const HASH_4_TABLE_SHIFT_BITS: u32 = 5;
const RESERVE_HALVING_LIMIT_BYTES: u32 = 1 << 30;
const RESERVE_EXTRA_BYTES: u32 = 1 << 19;
const HASH_CHAIN_DEPTH_BASE: u32 = 4;
const HASH_CHAIN_DEPTH_NICE_LEN_DIVISOR: u32 = 4;
const BINARY_TREE_DEPTH_BASE: u32 = 16;
const BINARY_TREE_DEPTH_NICE_LEN_DIVISOR: u32 = 2;
const MOVE_ALIGNMENT_MASK: u32 = !15;
const EMPTY_HASH_VALUE: u32 = 0;
const MUST_NORMALIZE_POS: u32 = u32::MAX;
const FILTER_BUFFERED_BYTES_MAX: usize = 32;
const NEXT_FILTERS_BUFFERED_BYTES_MAX: usize = FILTER_BUFFERED_BYTES_MAX * (FILTERS_MAX - 1);
const WORD_BYTES: usize = 8;
const BYTE_BITS: u32 = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Match {
    pub(super) len: u32,
    pub(super) dist: u32,
}

impl Match {
    pub(super) const EMPTY: Self = Self { len: 0, dist: 0 };
}

pub(super) struct Settings {
    pub(super) before_size: u32,
    pub(super) dict_size: u32,
    pub(super) after_size: u32,
    pub(super) match_len_max: u32,
    pub(super) nice_len: u32,
    pub(super) match_finder: MatchFinderId,
    pub(super) depth: u32,
}

pub(super) const fn hash_bytes(match_finder: MatchFinderId) -> u32 {
    match match_finder {
        MatchFinderId::BinaryTree2 => 2,
        MatchFinderId::HashChain3 | MatchFinderId::BinaryTree3 => 3,
        MatchFinderId::HashChain4 | MatchFinderId::BinaryTree4 => 4,
    }
}

const fn is_binary_tree(match_finder: MatchFinderId) -> bool {
    matches!(
        match_finder,
        MatchFinderId::BinaryTree2 | MatchFinderId::BinaryTree3 | MatchFinderId::BinaryTree4
    )
}

pub(super) fn memcmplen(buffer: &[u8], first: usize, second: usize, len: u32, limit: u32) -> u32 {
    if len >= limit {
        return limit;
    }
    let end = limit as usize;
    let left = &buffer[first..first + end];
    let right = &buffer[second..second + end];
    let mut index = len as usize;
    while index + WORD_BYTES <= end {
        let difference = read_word(left, index) ^ read_word(right, index);
        if difference != 0 {
            let equal_bytes = (difference.trailing_zeros() / BYTE_BITS) as usize;
            return u32::try_from(index + equal_bytes).expect("a match length fits in 32 bits");
        }
        index += WORD_BYTES;
    }
    while index < end && left[index] == right[index] {
        index += 1;
    }
    u32::try_from(index).expect("a match length fits in 32 bits")
}

pub(super) const fn not_equal_16(buffer: &[u8], first: usize, second: usize) -> bool {
    buffer[first] != buffer[second] || buffer[first + 1] != buffer[second + 1]
}

fn read_word(bytes: &[u8], position: usize) -> u64 {
    let word: [u8; WORD_BYTES] = bytes[position..position + WORD_BYTES]
        .try_into()
        .expect("a word-sized slice converts to an array");
    u64::from_le_bytes(word)
}

fn zeroed(count: u32) -> Result<Vec<u32>, Error> {
    drop(reserve::<u32>(count)?);
    Ok(alloc::vec![EMPTY_HASH_VALUE; count as usize])
}

fn reserve<T>(count: u32) -> Result<Vec<T>, Error> {
    let mut vector = Vec::new();
    vector
        .try_reserve_exact(count as usize)
        .map_err(|_| Error::Memory)?;
    Ok(vector)
}

pub(super) struct MatchFinder {
    buffer: Vec<u8>,
    size: u32,
    keep_size_before: u32,
    keep_size_after: u32,
    offset: u32,
    pub(super) read_pos: u32,
    pub(super) read_ahead: u32,
    pub(super) read_limit: u32,
    pub(super) write_pos: u32,
    pending: u32,
    kind: MatchFinderId,
    hash: Vec<u32>,
    son: Vec<u32>,
    sons_per_position: usize,
    cyclic_pos: u32,
    cyclic_size: u32,
    hash_mask: u32,
    depth: u32,
    pub(super) nice_len: u32,
    pub(super) match_len_max: u32,
    pub(super) action: Action,
}

impl MatchFinder {
    pub(super) fn new(settings: &Settings) -> Result<Self, Error> {
        if settings.dict_size < DICT_SIZE_MIN
            || settings.dict_size > DICT_SIZE_MAX
            || settings.nice_len > settings.match_len_max
        {
            return Err(Error::Options);
        }
        let keep_size_before = settings.before_size + settings.dict_size;
        let keep_size_after = settings.after_size + settings.match_len_max;
        let mut reserve_bytes = settings.dict_size / 2;
        if reserve_bytes > RESERVE_HALVING_LIMIT_BYTES {
            reserve_bytes /= 2;
        }
        reserve_bytes += (settings.before_size + settings.match_len_max + settings.after_size) / 2
            + RESERVE_EXTRA_BYTES;
        let size = keep_size_before + reserve_bytes + keep_size_after;
        let cyclic_size = settings.dict_size + 1;
        let hash_bytes = hash_bytes(settings.match_finder);
        let is_binary_tree = is_binary_tree(settings.match_finder);
        let hash_mask = hash_mask(hash_bytes, settings.dict_size);
        let mut hash_count = hash_mask + 1;
        if hash_bytes > 2 {
            hash_count += HASH_2_SIZE;
        }
        if hash_bytes > 3 {
            hash_count += HASH_3_SIZE;
        }
        let sons_per_position = if is_binary_tree { 2 } else { 1 };
        let sons_count = cyclic_size * sons_per_position;
        let depth = match (settings.depth, is_binary_tree) {
            (0, true) => {
                BINARY_TREE_DEPTH_BASE + settings.nice_len / BINARY_TREE_DEPTH_NICE_LEN_DIVISOR
            }
            (0, false) => {
                HASH_CHAIN_DEPTH_BASE + settings.nice_len / HASH_CHAIN_DEPTH_NICE_LEN_DIVISOR
            }
            (depth, _) => depth,
        };
        let buffer = reserve(size)?;
        let hash = zeroed(hash_count)?;
        let son = reserve(sons_count)?;
        Ok(Self {
            buffer,
            size,
            keep_size_before,
            keep_size_after,
            offset: cyclic_size,
            read_pos: 0,
            read_ahead: 0,
            read_limit: 0,
            write_pos: 0,
            pending: 0,
            kind: settings.match_finder,
            hash,
            son,
            sons_per_position: sons_per_position as usize,
            cyclic_pos: 0,
            cyclic_size,
            hash_mask,
            depth,
            nice_len: settings.nice_len,
            match_len_max: settings.match_len_max,
            action: Action::Run,
        })
    }

    pub(super) fn buffer(&self) -> &[u8] {
        &self.buffer
    }

    pub(super) const fn avail(&self) -> u32 {
        self.write_pos - self.read_pos
    }

    pub(super) const fn unencoded(&self) -> u32 {
        self.write_pos - self.read_pos + self.read_ahead
    }

    fn move_window(&mut self) {
        assert!(self.read_pos > self.keep_size_before);
        let move_offset = (self.read_pos - self.keep_size_before) & MOVE_ALIGNMENT_MASK;
        assert!(self.write_pos > move_offset);
        self.buffer
            .copy_within(move_offset as usize..self.write_pos as usize, 0);
        self.offset += move_offset;
        self.read_pos -= move_offset;
        self.read_limit -= move_offset;
        self.write_pos -= move_offset;
    }

    pub(super) fn fill_window(
        &mut self,
        next: Option<&mut Coder>,
        input: &[u8],
        input_position: &mut usize,
        action: Action,
    ) -> Result<Status, Error> {
        assert!(self.read_pos <= self.write_pos);
        if self.read_pos >= self.size - self.keep_size_after {
            self.move_window();
        }
        let result = match next {
            None => {
                self.copy_input(input, input_position);
                if action != Action::Run && *input_position == input.len() {
                    Ok(Status::StreamEnd)
                } else {
                    Ok(Status::Ok)
                }
            }
            Some(next) => self.code_next(next, input, input_position, action),
        };
        let result = match result {
            Ok(Status::StreamEnd) => {
                self.action = action;
                self.read_limit = self.write_pos;
                Ok(Status::Ok)
            }
            other => {
                if self.write_pos > self.keep_size_after {
                    self.read_limit = self.write_pos - self.keep_size_after;
                }
                other
            }
        };
        if self.pending > 0 && self.read_pos < self.read_limit {
            let pending = self.pending;
            self.pending = 0;
            assert!(self.read_pos >= pending);
            self.read_pos -= pending;
            self.skip_positions(pending);
        }
        result
    }

    fn copy_input(&mut self, input: &[u8], input_position: &mut usize) {
        let write_pos = self.write_pos as usize;
        assert!(self.buffer.len() >= write_pos);
        let count = (input.len() - *input_position).min(self.size as usize - write_pos);
        let source = &input[*input_position..*input_position + count];
        let end = write_pos + count;
        let overlap = self.buffer.len().min(end) - write_pos;
        self.buffer[write_pos..write_pos + overlap].copy_from_slice(&source[..overlap]);
        self.buffer.extend_from_slice(&source[overlap..]);
        *input_position += count;
        self.write_pos = u32::try_from(end).expect("the window size fits in 32 bits");
    }

    fn code_next(
        &mut self,
        next: &mut Coder,
        input: &[u8],
        input_position: &mut usize,
        action: Action,
    ) -> Result<Status, Error> {
        let size = self.size as usize;
        let mut write_pos = self.write_pos as usize;
        let wanted = write_pos + (input.len() - *input_position) + NEXT_FILTERS_BUFFERED_BYTES_MAX;
        let length = wanted.min(size);
        if self.buffer.len() < length {
            self.buffer.resize(length, 0);
        }
        let result = next.code(
            input,
            input_position,
            &mut self.buffer[..length],
            &mut write_pos,
            action,
        );
        assert!(
            write_pos < length || length == size,
            "a filter before LZMA produced more than its input and buffered bytes"
        );
        self.write_pos = u32::try_from(write_pos).expect("the window size fits in 32 bits");
        result
    }

    pub(super) fn find(&mut self, matches: &mut [Match]) -> (u32, u32) {
        let count = match self.kind {
            MatchFinderId::HashChain3 => self.hc3_find(matches),
            MatchFinderId::HashChain4 => self.hc4_find(matches),
            MatchFinderId::BinaryTree2 => self.bt2_find(matches),
            MatchFinderId::BinaryTree3 => self.bt3_find(matches),
            MatchFinderId::BinaryTree4 => self.bt4_find(matches),
        };
        let mut len_best = 0;
        if count > 0 {
            let longest = matches[count - 1];
            len_best = longest.len;
            if len_best == self.nice_len {
                let limit = (self.avail() + 1).min(self.match_len_max);
                let first = self.read_pos as usize - 1;
                let second = first - longest.dist as usize - 1;
                len_best = memcmplen(&self.buffer, first, second, len_best, limit);
            }
        }
        self.read_ahead += 1;
        (
            u32::try_from(count).expect("match counts fit in 32 bits"),
            len_best,
        )
    }

    pub(super) fn skip(&mut self, amount: u32) {
        if amount != 0 {
            self.skip_positions(amount);
            self.read_ahead += amount;
        }
    }

    fn skip_positions(&mut self, amount: u32) {
        assert!(amount > 0);
        for _ in 0..amount {
            match self.kind {
                MatchFinderId::HashChain3 => self.hc3_skip(),
                MatchFinderId::HashChain4 => self.hc4_skip(),
                MatchFinderId::BinaryTree2 => self.bt2_skip(),
                MatchFinderId::BinaryTree3 => self.bt3_skip(),
                MatchFinderId::BinaryTree4 => self.bt4_skip(),
            }
        }
    }

    const fn position(&self) -> u32 {
        self.read_pos.wrapping_add(self.offset)
    }

    const fn move_pending(&mut self) {
        self.read_pos += 1;
        self.pending += 1;
    }

    fn move_pos(&mut self) {
        self.cyclic_pos += 1;
        if self.cyclic_pos == self.cyclic_size {
            self.cyclic_pos = 0;
        }
        self.read_pos += 1;
        if self.position() == MUST_NORMALIZE_POS {
            self.normalize();
        }
    }

    #[cold]
    fn normalize(&mut self) {
        let subvalue = MUST_NORMALIZE_POS - self.cyclic_size;
        for value in self.hash.iter_mut().chain(self.son.iter_mut()) {
            *value = if *value <= subvalue {
                EMPTY_HASH_VALUE
            } else {
                *value - subvalue
            };
        }
        self.offset -= subvalue;
    }

    fn grow_son(&mut self) {
        let needed = (self.cyclic_pos as usize + 1) * self.sons_per_position;
        if self.son.len() < needed {
            self.son.resize(needed, EMPTY_HASH_VALUE);
        }
    }

    const fn len_limit(&self, len_min: u32) -> Option<u32> {
        let avail = self.avail();
        if self.nice_len <= avail {
            Some(self.nice_len)
        } else if avail < len_min {
            None
        } else {
            Some(avail)
        }
    }

    fn update_hash_2(&mut self, cur: usize, pos: u32) -> u32 {
        let hash_value =
            usize::from(self.buffer[cur]) | (usize::from(self.buffer[cur + 1]) << BYTE_BITS);
        let cur_match = self.hash[hash_value];
        self.hash[hash_value] = pos;
        cur_match
    }

    fn update_hash_3(&mut self, cur: usize, pos: u32) -> (u32, u32) {
        let bytes = &self.buffer[cur..cur + 3];
        let temp = TABLE[usize::from(bytes[0])] ^ u32::from(bytes[1]);
        let hash_2_value = (temp & HASH_2_MASK) as usize;
        let hash_value = (temp ^ (u32::from(bytes[2]) << BYTE_BITS)) & self.hash_mask;
        let hash_3_index = (FIX_3_HASH_SIZE + hash_value) as usize;
        let delta2 = pos.wrapping_sub(self.hash[hash_2_value]);
        let cur_match = self.hash[hash_3_index];
        self.hash[hash_2_value] = pos;
        self.hash[hash_3_index] = pos;
        (delta2, cur_match)
    }

    fn update_hash_4(&mut self, cur: usize, pos: u32) -> (u32, u32, u32) {
        let bytes = &self.buffer[cur..cur + 4];
        let temp = TABLE[usize::from(bytes[0])] ^ u32::from(bytes[1]);
        let temp_3 = temp ^ (u32::from(bytes[2]) << BYTE_BITS);
        let hash_2_value = (temp & HASH_2_MASK) as usize;
        let hash_3_index = (FIX_3_HASH_SIZE + (temp_3 & HASH_3_MASK)) as usize;
        let hash_value =
            (temp_3 ^ (TABLE[usize::from(bytes[3])] << HASH_4_TABLE_SHIFT_BITS)) & self.hash_mask;
        let hash_4_index = (FIX_4_HASH_SIZE + hash_value) as usize;
        let delta2 = pos.wrapping_sub(self.hash[hash_2_value]);
        let delta3 = pos.wrapping_sub(self.hash[hash_3_index]);
        let cur_match = self.hash[hash_4_index];
        self.hash[hash_2_value] = pos;
        self.hash[hash_3_index] = pos;
        self.hash[hash_4_index] = pos;
        (delta2, delta3, cur_match)
    }

    const fn cursor(&self, len_limit: u32, pos: u32, cur: usize) -> Cursor {
        Cursor {
            len_limit,
            pos,
            cur,
            depth: self.depth,
            cyclic_pos: self.cyclic_pos,
            cyclic_size: self.cyclic_size,
        }
    }
}

#[derive(Clone, Copy)]
struct Cursor {
    len_limit: u32,
    pos: u32,
    cur: usize,
    depth: u32,
    cyclic_pos: u32,
    cyclic_size: u32,
}

impl Cursor {
    const fn slot(&self, delta: u32) -> usize {
        if delta > self.cyclic_pos {
            (self.cyclic_pos + self.cyclic_size - delta) as usize
        } else {
            (self.cyclic_pos - delta) as usize
        }
    }
}

const fn hash_mask(hash_bytes: u32, dict_size: u32) -> u32 {
    if hash_bytes == 2 {
        return HASH_TWO_BYTES_MASK;
    }
    let mut mask = dict_size - 1;
    mask |= mask >> 1;
    mask |= mask >> 2;
    mask |= mask >> 4;
    mask |= mask >> 8;
    mask >>= 1;
    mask |= HASH_MASK_MIN;
    if mask > HASH_MASK_LIMIT {
        if hash_bytes == 3 {
            mask = HASH_3_BYTES_MASK_MAX;
        } else {
            mask >>= 1;
        }
    }
    mask
}
