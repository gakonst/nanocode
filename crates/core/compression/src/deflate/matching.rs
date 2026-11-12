use super::state::{
    HASH_SIZE, HashVariant, Io, MIN_LOOKAHEAD, ROLL_HASH_CHAIN_ABOVE, STD_MIN_MATCH, State,
};

const STANDARD_HASH_MULTIPLIER: u32 = 2_654_435_761;
const STANDARD_HASH_SHIFT: u32 = 16;
const STANDARD_HASH_MASK: u32 = (1 << u16::BITS) - 1;
const ROLL_HASH_SHIFT: u32 = 5;
const ROLL_HASH_BITS: u32 = 15;
const ROLL_HASH_MASK: u32 = (1 << ROLL_HASH_BITS) - 1;
const ROLL_HASH_OFFSET: usize = STD_MIN_MATCH - 1;
const HASH_INPUT_BYTES: usize = 4;
const WORD_BYTES: usize = 8;
const COMPARE_BYTES: usize = 256;
const EARLY_EXIT_LEVEL_BELOW: u8 = 5;
const GOOD_MATCH_CHAIN_SHIFT: u32 = 2;
pub(super) const KNOWN_PREFIX_BYTES: usize = 2;
const WINDOW_MORE_MIN: usize = 2;

pub(super) fn read_u32(window: &[u8], position: usize) -> u32 {
    let mut bytes = [0; HASH_INPUT_BYTES];
    bytes.copy_from_slice(&window[position..position + HASH_INPUT_BYTES]);
    u32::from_le_bytes(bytes)
}

fn read_u64(window: &[u8], position: usize) -> u64 {
    let mut bytes = [0; WORD_BYTES];
    bytes.copy_from_slice(&window[position..position + WORD_BYTES]);
    u64::from_le_bytes(bytes)
}

pub(super) fn compare256(window: &[u8], first: usize, second: usize) -> usize {
    let first = &window[first..first + COMPARE_BYTES];
    let second = &window[second..second + COMPARE_BYTES];
    for (index, (left, right)) in first
        .as_chunks::<WORD_BYTES>()
        .0
        .iter()
        .zip(second.as_chunks::<WORD_BYTES>().0)
        .enumerate()
    {
        let difference = u64::from_le_bytes(*left) ^ u64::from_le_bytes(*right);
        if difference != 0 {
            return index * WORD_BYTES + (difference.trailing_zeros() / u8::BITS) as usize;
        }
    }
    COMPARE_BYTES
}

pub(super) fn compare256_run(byte: u8, window: &[u8], start: usize) -> usize {
    let expected = u64::from_le_bytes([byte; WORD_BYTES]);
    let run = &window[start..start + COMPARE_BYTES];
    for (index, chunk) in run.as_chunks::<WORD_BYTES>().0.iter().enumerate() {
        let difference = expected ^ u64::from_le_bytes(*chunk);
        if difference != 0 {
            return index * WORD_BYTES + (difference.trailing_zeros() / u8::BITS) as usize;
        }
    }
    COMPARE_BYTES
}

const fn standard_hash(value: u32) -> usize {
    ((value.wrapping_mul(STANDARD_HASH_MULTIPLIER) >> STANDARD_HASH_SHIFT) & STANDARD_HASH_MASK)
        as usize
}

const fn roll_hash(hash: u32, value: u32) -> u32 {
    ((hash << ROLL_HASH_SHIFT) ^ value) & ROLL_HASH_MASK
}

fn link(
    head: &mut [u16; HASH_SIZE],
    previous: &mut [u16],
    mask: usize,
    hash: usize,
    position: usize,
) -> u16 {
    let first = head[hash];
    let index = window_index(position);
    if first != index {
        previous[position & mask] = first;
        head[hash] = index;
    }
    first
}

fn slide_table(table: &mut [u16], size: u16) {
    for entry in table {
        *entry = entry.saturating_sub(size);
    }
}

fn window_index(position: usize) -> u16 {
    u16::try_from(position).expect("a window position fits in 16 bits")
}

impl State {
    pub(super) const fn update_hash(&self, hash: u32, value: u32) -> u32 {
        match self.hash {
            HashVariant::Standard => standard_hash(value) as u32,
            HashVariant::Roll => roll_hash(hash, value),
        }
    }

    fn link(&mut self, hash: usize, position: usize) -> u16 {
        let mask = self.window_mask();
        link(&mut self.head, &mut self.previous, mask, hash, position)
    }

    pub(super) fn standard_insert_value(&mut self, position: usize, value: u32) -> u16 {
        self.link(standard_hash(value), position)
    }

    pub(super) fn standard_insert(&mut self, position: usize) -> u16 {
        let value = read_u32(&self.window, position);
        self.standard_insert_value(position, value)
    }

    pub(super) fn quick_insert_string(&mut self, position: usize) -> u16 {
        match self.hash {
            HashVariant::Standard => self.standard_insert(position),
            HashVariant::Roll => {
                let value = u32::from(self.window[position + ROLL_HASH_OFFSET]);
                self.insert_hash = roll_hash(self.insert_hash, value);
                self.link(self.insert_hash as usize, position)
            }
        }
    }

    pub(super) fn insert_string(&mut self, position: usize, count: usize) {
        match self.hash {
            HashVariant::Standard => {
                let mask = self.window_mask();
                let available = self.window.len() - position;
                let end = position + available.min(count + STD_MIN_MATCH);
                let words = self.window[position..end].windows(HASH_INPUT_BYTES);
                for (offset, word) in words.enumerate() {
                    let value = u32::from_le_bytes([word[0], word[1], word[2], word[3]]);
                    let hash = standard_hash(value);
                    link(
                        &mut self.head,
                        &mut self.previous,
                        mask,
                        hash,
                        position + offset,
                    );
                }
            }
            HashVariant::Roll => {
                let mask = self.window_mask();
                let mut hash = self.insert_hash;
                let bytes = &self.window[position + ROLL_HASH_OFFSET..][..count];
                for (offset, byte) in bytes.iter().enumerate() {
                    hash = roll_hash(hash, u32::from(*byte));
                    link(
                        &mut self.head,
                        &mut self.previous,
                        mask,
                        hash as usize,
                        position + offset,
                    );
                }
                self.insert_hash = hash;
            }
        }
    }

    fn slide_hash(&mut self) {
        let size = window_index(self.window_size);
        slide_table(&mut self.head[..], size);
        slide_table(&mut self.previous, size);
    }

    fn slide_window(&mut self) {
        let size = self.window_size;
        self.window.copy_within(size..2 * size, 0);
        let size_index = window_index(size);
        if self.match_start >= size_index {
            self.match_start -= size_index;
        } else {
            self.match_start = 0;
            self.previous_length = 0;
        }
        self.string_start -= size;
        self.block_start -= super::state::signed(size);
        self.insert = self.insert.min(self.string_start);
        self.slide_hash();
    }

    pub(super) fn fill_window(&mut self, io: &mut Io) {
        assert!(
            self.lookahead < MIN_LOOKAHEAD,
            "the window is filled only when short"
        );
        loop {
            let mut more = self.window_buffer_size() - self.lookahead - self.string_start;
            if self.string_start >= self.window_size + self.max_distance() {
                self.slide_window();
                more += self.window_size;
            }
            if io.input_available() == 0 {
                break;
            }
            assert!(more >= WINDOW_MORE_MIN, "the window has room for input");
            let read = self.read_buffer(io, self.string_start + self.lookahead, more);
            self.lookahead += read;
            if self.lookahead + self.insert >= STD_MIN_MATCH {
                self.insert_pending_strings();
            }
            if !(self.lookahead < MIN_LOOKAHEAD && io.input_available() != 0) {
                break;
            }
        }
        assert!(
            self.string_start <= self.window_buffer_size() - MIN_LOOKAHEAD,
            "the window leaves room for a search"
        );
    }

    fn insert_pending_strings(&mut self) {
        let string = self.string_start - self.insert;
        if self.max_chain_length > ROLL_HASH_CHAIN_ABOVE {
            let first = u32::from(self.window[string]);
            let second = u32::from(self.window[string + 1]);
            self.insert_hash = self.update_hash(first, second);
        } else if string >= 1 {
            self.quick_insert_string(string - 1);
        }
        let mut count = self.insert;
        if self.lookahead == 1 {
            count -= 1;
        }
        if count > 0 {
            self.insert_string(string, count);
            self.insert -= count;
        }
    }

    pub(super) const fn early_exit(&self) -> bool {
        self.level < EARLY_EXIT_LEVEL_BELOW
    }
}

struct Search {
    best_length: usize,
    match_start: u16,
    current: u16,
    chain_length: u16,
    limit: u16,
    limit_base: u16,
    match_offset: u16,
    offset: usize,
}

const fn compare_offset(best_length: usize) -> usize {
    if best_length >= WORD_BYTES {
        best_length - WORD_BYTES + 1
    } else if best_length >= HASH_INPUT_BYTES {
        best_length - HASH_INPUT_BYTES + 1
    } else {
        best_length - 1
    }
}

pub(super) fn longest_match(state: &State, current: u16, slow: bool) -> (usize, u16) {
    let best_length = if state.previous_length > 0 {
        usize::from(state.previous_length)
    } else {
        STD_MIN_MATCH - 1
    };
    let mut chain_length = state.max_chain_length;
    if best_length >= usize::from(state.good_match) {
        chain_length >>= GOOD_MATCH_CHAIN_SHIFT;
    }
    let limit = window_index(state.string_start.saturating_sub(state.max_distance()));
    let mut search = Search {
        best_length,
        match_start: state.match_start,
        current,
        chain_length,
        limit,
        limit_base: if slow { limit } else { 0 },
        match_offset: 0,
        offset: compare_offset(best_length),
    };
    if slow && best_length >= STD_MIN_MATCH && !search.seek_offset_start(state) {
        return break_matching(state, &search);
    }
    assert!(
        state.string_start <= state.window_buffer_size().saturating_sub(MIN_LOOKAHEAD),
        "a search has lookahead"
    );
    search.run(state, slow)
}

fn break_matching(state: &State, search: &Search) -> (usize, u16) {
    (search.best_length.min(state.lookahead), search.match_start)
}

enum Step {
    Found(usize),
    Exhausted,
}

impl Search {
    fn seek_offset_start(&mut self, state: &State) -> bool {
        let scan = &state.window[state.string_start..=state.string_start + self.best_length];
        let mut hash = state.update_hash(0, u32::from(scan[1]));
        hash = state.update_hash(hash, u32::from(scan[2]));
        for (index, byte) in scan[3..].iter().enumerate() {
            hash = state.update_hash(hash, u32::from(*byte));
            let position = state.head[hash as usize];
            if position < self.current {
                self.match_offset = window_index(index + 1);
                self.current = position;
            }
        }
        self.limit = self.limit_base + self.match_offset;
        self.current > self.limit
    }

    fn base(&self) -> usize {
        usize::from(self.current) - usize::from(self.match_offset)
    }

    fn next_in_chain(&mut self, state: &State) -> bool {
        self.chain_length -= 1;
        if self.chain_length > 0 {
            self.current = state.previous[usize::from(self.current) & state.window_mask()];
            if self.current > self.limit {
                return true;
            }
        }
        false
    }

    fn find_candidate(&mut self, state: &State) -> Step {
        let window = &state.window[..];
        let start = state.string_start;
        if self.best_length < WORD_BYTES {
            let scan = read_u64(window, start);
            loop {
                let difference = scan ^ read_u64(window, self.base());
                if difference == 0 {
                    return Step::Found(0);
                }
                let length = (difference.trailing_zeros() / u8::BITS) as usize;
                if length > self.best_length {
                    return Step::Found(length);
                }
                if !self.next_in_chain(state) {
                    return Step::Exhausted;
                }
            }
        }
        let scan_end = read_u64(window, start + self.offset);
        let scan_start = read_u64(window, start);
        loop {
            let base = self.base();
            if read_u64(window, base + self.offset) == scan_end
                && read_u64(window, base) == scan_start
            {
                return Step::Found(0);
            }
            if !self.next_in_chain(state) {
                return Step::Exhausted;
            }
        }
    }

    fn run(&mut self, state: &State, slow: bool) -> (usize, u16) {
        let start = state.string_start;
        let early_exit = !slow && state.early_exit();
        loop {
            if usize::from(self.current) >= start {
                break;
            }
            let length = match self.find_candidate(state) {
                Step::Found(0) => {
                    compare256(
                        &state.window,
                        start + KNOWN_PREFIX_BYTES,
                        self.base() + KNOWN_PREFIX_BYTES,
                    ) + KNOWN_PREFIX_BYTES
                }
                Step::Found(length) => length,
                Step::Exhausted => return (self.best_length, self.match_start),
            };
            assert!(
                start + length < state.window_buffer_size(),
                "a match stays in the window"
            );
            if length > self.best_length {
                self.match_start = self.current - self.match_offset;
                if length >= state.lookahead {
                    return (state.lookahead, self.match_start);
                }
                self.best_length = length;
                if self.best_length >= usize::from(state.nice_match) {
                    return (self.best_length, self.match_start);
                }
                self.offset = compare_offset(self.best_length);
                if slow && length > STD_MIN_MATCH && usize::from(self.match_start) + length < start
                {
                    if !self.rechain(state, length) {
                        return break_matching(state, self);
                    }
                    continue;
                }
            } else if early_exit {
                break;
            }
            if !self.next_in_chain(state) {
                return (self.best_length, self.match_start);
            }
        }
        (self.best_length, self.match_start)
    }

    fn rechain(&mut self, state: &State, length: usize) -> bool {
        self.current -= self.match_offset;
        self.match_offset = 0;
        let mut next = self.current;
        for index in 0..=length - STD_MIN_MATCH {
            let position =
                state.previous[(usize::from(self.current) + index) & state.window_mask()];
            if position < next {
                if position <= self.limit_base + window_index(index) {
                    return false;
                }
                next = position;
                self.match_offset = window_index(index);
            }
        }
        self.current = next;
        let scan = &state.window[state.string_start + length - (STD_MIN_MATCH + 1)..];
        let mut hash = state.update_hash(0, u32::from(scan[0]));
        hash = state.update_hash(hash, u32::from(scan[1]));
        hash = state.update_hash(hash, u32::from(scan[2]));
        let position = state.head[hash as usize];
        if position < self.current {
            self.match_offset = window_index(length - (STD_MIN_MATCH + 1));
            if position <= self.limit_base + self.match_offset {
                return false;
            }
            self.current = position;
        }
        self.limit = self.limit_base + self.match_offset;
        true
    }
}
