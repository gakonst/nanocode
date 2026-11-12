use super::sequences::Sequence;

const HASH_MULTIPLIER: u64 = 0xCF1B_BCDC_B7A5_6463;
const SEARCH_STRENGTH: u32 = 8;
const HASH_READ_SIZE: usize = 8;
const LONG_MATCH: u32 = 8;
const REPEAT_OFFSETS_START: [u32; 3] = [1, 4, 8];
pub(crate) const MATCH_MIN: usize = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Strategy {
    Fast,
    DoubleFast,
}

#[inline]
fn read32(data: &[u8], at: usize) -> u32 {
    let word: [u8; 4] = data[at..at + 4].try_into().expect("four bytes");
    u32::from_le_bytes(word)
}

#[inline]
fn read64(data: &[u8], at: usize) -> u64 {
    let word: [u8; 8] = data[at..at + 8].try_into().expect("eight bytes");
    u64::from_le_bytes(word)
}

#[inline]
const fn hash(word: u64, bytes: u32, log: u32) -> usize {
    ((word << (64 - 8 * bytes)).wrapping_mul(HASH_MULTIPLIER) >> (64 - log)) as usize
}

#[inline(always)]
fn common(data: &[u8], earlier: usize, current: usize, end: usize) -> usize {
    let mut length = 0;
    while current + length + 8 <= end {
        let difference = read64(data, earlier + length) ^ read64(data, current + length);
        if difference != 0 {
            return length + (difference.trailing_zeros() / 8) as usize;
        }
        length += 8;
    }
    while current + length < end && data[earlier + length] == data[current + length] {
        length += 1;
    }
    length
}

#[inline]
const fn reaches(distance_max: usize, candidate: usize, position: usize) -> bool {
    candidate < position && position - candidate <= distance_max
}

#[inline]
fn repeat_at(distance_max: usize, data: &[u8], position: usize, offset: u32) -> bool {
    let offset = offset as usize;
    offset <= position
        && offset <= distance_max
        && read32(data, position - offset) == read32(data, position)
}

pub(crate) struct Emitter {
    pub(crate) literals: Vec<u8>,
    pub(crate) sequences: Vec<Sequence>,
    pub(crate) repeats: [u32; 3],
}

impl Emitter {
    pub(crate) fn new(block_max: usize) -> Self {
        Self {
            literals: Vec::with_capacity(block_max),
            sequences: Vec::with_capacity(block_max / MATCH_MIN),
            repeats: REPEAT_OFFSETS_START,
        }
    }

    pub(crate) const fn reset(&mut self) {
        self.repeats = REPEAT_OFFSETS_START;
    }

    #[inline(always)]
    fn emit(&mut self, data: &[u8], anchor: usize, start: usize, offset: usize, length: usize) {
        assert!(anchor <= start, "a match starts after its literals");
        assert!(length >= MATCH_MIN, "a match is at least {MATCH_MIN} bytes");
        assert!(
            offset >= 1 && offset <= start,
            "a match reaches back into the history"
        );
        self.literals.extend_from_slice(&data[anchor..start]);
        let literal_length = (start - anchor) as u32;
        let offset_base = self.offset_base(offset as u32, literal_length == 0);
        self.sequences.push(Sequence {
            literal_length,
            match_length: length as u32,
            offset_base,
        });
    }

    const fn offset_base(&mut self, offset: u32, no_literals: bool) -> u32 {
        let [first, second, third] = self.repeats;
        if no_literals {
            if offset == second {
                self.repeats = [second, first, third];
                return 1;
            }
            if offset == third {
                self.repeats = [third, first, second];
                return 2;
            }
            if first > 1 && offset == first - 1 {
                self.repeats = [offset, first, second];
                return 3;
            }
        } else {
            if offset == first {
                return 1;
            }
            if offset == second {
                self.repeats = [second, first, third];
                return 2;
            }
            if offset == third {
                self.repeats = [third, first, second];
                return 3;
            }
        }
        self.repeats = [offset, first, second];
        offset + 3
    }

    fn tail(&mut self, data: &[u8], anchor: usize, end: usize) {
        self.literals.extend_from_slice(&data[anchor..end]);
    }
}

pub(crate) struct Matcher {
    strategy: Strategy,
    hash_log: u32,
    chain_log: u32,
    match_bytes: u32,
    long: Vec<u32>,
    short: Vec<u32>,
    distance_max: usize,
}

impl Matcher {
    pub(crate) fn new(
        strategy: Strategy,
        hash_log: u32,
        chain_log: u32,
        match_min: u32,
        distance_max: usize,
    ) -> Self {
        let short = match strategy {
            Strategy::Fast => Vec::new(),
            Strategy::DoubleFast => vec![0; 1 << chain_log],
        };
        Self {
            strategy,
            hash_log,
            chain_log,
            match_bytes: match_min.clamp(4, 7),
            long: vec![0; 1 << hash_log],
            short,
            distance_max,
        }
    }

    pub(crate) fn prime(&mut self, data: &[u8], end: usize) {
        let last = end.min(data.len().saturating_sub(HASH_READ_SIZE));
        for position in 0..last {
            let word = read64(data, position);
            match self.strategy {
                Strategy::Fast => {
                    self.long[hash(word, self.match_bytes, self.hash_log)] = position as u32;
                }
                Strategy::DoubleFast => {
                    self.long[hash(word, LONG_MATCH, self.hash_log)] = position as u32;
                    self.short[hash(word, self.match_bytes, self.chain_log)] = position as u32;
                }
            }
        }
    }

    pub(crate) fn block(&mut self, data: &[u8], start: usize, end: usize, emitter: &mut Emitter) {
        assert!(end <= data.len() && start <= end);
        assert!(
            data.len() < u32::MAX as usize,
            "positions fit the hash tables"
        );
        if end - start <= HASH_READ_SIZE + 1 {
            return emitter.tail(data, start, end);
        }
        match self.strategy {
            Strategy::Fast => self.fast(data, start, end, emitter),
            Strategy::DoubleFast => self.double_fast(data, start, end, emitter),
        }
    }

    fn fast(&mut self, data: &[u8], start: usize, end: usize, emitter: &mut Emitter) {
        let limit = end - HASH_READ_SIZE;
        let (hash_log, bytes) = (self.hash_log, self.match_bytes);
        let long = &mut self.long[..];
        assert!(long.len().is_power_of_two());
        let long_mask = long.len() - 1;
        let (mut position, mut anchor) = (start, start);
        while position < limit {
            let searched = position;
            let slot = hash(read64(data, position), bytes, hash_log);
            let candidate = long[slot & long_mask] as usize;
            long[slot & long_mask] = position as u32;
            let first = emitter.repeats[0];
            if repeat_at(self.distance_max, data, position + 1, first) {
                let at = position + 1;
                let earlier = at - first as usize;
                let length = 4 + common(data, earlier + 4, at + 4, end);
                emitter.emit(data, anchor, at, first as usize, length);
                position = at + length;
            } else if reaches(self.distance_max, candidate, position)
                && read32(data, candidate) == read32(data, position)
            {
                let mut length = 4 + common(data, candidate + 4, position + 4, end);
                let (mut earlier, mut at) = (candidate, position);
                while at > anchor && earlier > 0 && data[at - 1] == data[earlier - 1] {
                    at -= 1;
                    earlier -= 1;
                    length += 1;
                }
                emitter.emit(data, anchor, at, at - earlier, length);
                position = at + length;
            } else {
                position += ((position - anchor) >> SEARCH_STRENGTH) + 1;
                continue;
            }
            anchor = position;
            if position <= limit {
                let inserted = searched + 2;
                long[hash(read64(data, inserted), bytes, hash_log) & long_mask] = inserted as u32;
                let behind = position - 2;
                long[hash(read64(data, behind), bytes, hash_log) & long_mask] = behind as u32;
                while position <= limit
                    && repeat_at(self.distance_max, data, position, emitter.repeats[1])
                {
                    let offset = emitter.repeats[1] as usize;
                    let length = 4 + common(data, position - offset + 4, position + 4, end);
                    long[hash(read64(data, position), bytes, hash_log) & long_mask] =
                        position as u32;
                    emitter.emit(data, anchor, position, offset, length);
                    position += length;
                    anchor = position;
                }
            }
        }
        emitter.tail(data, anchor, end);
    }

    fn double_fast(&mut self, data: &[u8], start: usize, end: usize, emitter: &mut Emitter) {
        let limit = end - HASH_READ_SIZE;
        let (mut position, mut anchor) = (start, start);
        let (long_log, short_log, bytes) = (self.hash_log, self.chain_log, self.match_bytes);
        let distance_max = self.distance_max;
        let long = &mut self.long[..];
        let short = &mut self.short[..];
        assert!(long.len().is_power_of_two());
        assert!(short.len().is_power_of_two());
        let (long_mask, short_mask) = (long.len() - 1, short.len() - 1);
        while position < limit {
            let searched = position;
            let word = read64(data, position);
            let long_slot = hash(word, LONG_MATCH, long_log);
            let short_slot = hash(word, bytes, short_log);
            let long_candidate = long[long_slot & long_mask] as usize;
            let short_candidate = short[short_slot & short_mask] as usize;
            long[long_slot & long_mask] = position as u32;
            short[short_slot & short_mask] = position as u32;
            let first = emitter.repeats[0];
            let (at, earlier, length) = if repeat_at(distance_max, data, position + 1, first) {
                let at = position + 1;
                let earlier = at - first as usize;
                (at, earlier, 4 + common(data, earlier + 4, at + 4, end))
            } else if reaches(distance_max, long_candidate, position)
                && read64(data, long_candidate) == word
            {
                (
                    position,
                    long_candidate,
                    8 + common(data, long_candidate + 8, position + 8, end),
                )
            } else if reaches(distance_max, short_candidate, position)
                && read32(data, short_candidate) == read32(data, position)
            {
                let next = position + 1;
                let next_word = read64(data, next);
                let next_slot = hash(next_word, LONG_MATCH, long_log);
                let next_candidate = long[next_slot & long_mask] as usize;
                long[next_slot & long_mask] = next as u32;
                if reaches(distance_max, next_candidate, next)
                    && read64(data, next_candidate) == next_word
                {
                    (
                        next,
                        next_candidate,
                        8 + common(data, next_candidate + 8, next + 8, end),
                    )
                } else {
                    (
                        position,
                        short_candidate,
                        4 + common(data, short_candidate + 4, position + 4, end),
                    )
                }
            } else {
                position += ((position - anchor) >> SEARCH_STRENGTH) + 1;
                continue;
            };
            let (mut at, mut earlier, mut length) = (at, earlier, length);
            while at > anchor && earlier > 0 && data[at - 1] == data[earlier - 1] {
                at -= 1;
                earlier -= 1;
                length += 1;
            }
            emitter.emit(data, anchor, at, at - earlier, length);
            position = at + length;
            anchor = position;
            if position <= limit {
                let inserted = searched + 2;
                let inserted_word = read64(data, inserted);
                long[(hash(inserted_word, LONG_MATCH, long_log)) & long_mask] = inserted as u32;
                short[(hash(inserted_word, bytes, short_log)) & short_mask] = inserted as u32;
                let behind = position - 2;
                long[(hash(read64(data, behind), LONG_MATCH, long_log)) & long_mask] =
                    behind as u32;
                let previous = position - 1;
                short[(hash(read64(data, previous), bytes, short_log)) & short_mask] =
                    previous as u32;
                while position <= limit
                    && repeat_at(distance_max, data, position, emitter.repeats[1])
                {
                    let offset = emitter.repeats[1] as usize;
                    let length = 4 + common(data, position - offset + 4, position + 4, end);
                    let word = read64(data, position);
                    long[(hash(word, LONG_MATCH, long_log)) & long_mask] = position as u32;
                    short[(hash(word, bytes, short_log)) & short_mask] = position as u32;
                    emitter.emit(data, anchor, position, offset, length);
                    position += length;
                    anchor = position;
                }
            }
        }
        emitter.tail(data, anchor, end);
    }
}
