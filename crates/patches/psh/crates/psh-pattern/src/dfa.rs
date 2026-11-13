use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::vec;
use alloc::vec::Vec;
use core::mem::size_of;

use crate::hir::{Look, LookSet};
use crate::literal::Prefilter;
use crate::look::is_word_byte;
use crate::nfa::{Nfa, State, StateId};

const TAG_MATCH: u32 = 1;
const TAG_SPECIAL: u32 = 2;
const TAG_START: u32 = 4;
const TAG_MASK: u32 = TAG_MATCH | TAG_SPECIAL | TAG_START;
const UNKNOWN: u32 = TAG_SPECIAL;
const DEAD: u32 = TAG_SPECIAL | 8;
const QUIT: u32 = TAG_SPECIAL | 16;
const STRIDE_MIN: usize = 8;
const UNROLL_BYTES: usize = 4;

const EDGE: u8 = 1;
const LINE_TERMINATOR: u8 = 2;
const LINE_FEED: u8 = 4;
const CARRIAGE_RETURN: u8 = 8;
const WORD: u8 = 16;
const NON_ASCII: u8 = 32;
const CONTEXTS: usize = 64;

const MINIMUM_CACHE_CLEAR_COUNT: usize = 3;
const MINIMUM_BYTES_PER_STATE: usize = 10;
const MINIMUM_CACHE_STATES: usize = 2;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Kind {
    LeftmostFirst,
    All,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Fallback {
    Quit,
    GaveUp,
}

#[derive(Clone, Debug)]
pub struct Dfa {
    classes: [u8; 256],
    representatives: Vec<u8>,
    alphabet_len: usize,
    stride: usize,
    kind: Kind,
    reverse: bool,
    line_terminator: u8,
    context_mask: u8,
    start_anchored: StateId,
    start_unanchored: StateId,
    specialize_start: bool,
    capacity: usize,
}

#[derive(Clone, Copy, Debug)]
struct Record {
    seeds_start: u32,
    seeds_len: u32,
    context: u8,
    matched: bool,
}

#[derive(Clone, Debug)]
pub struct DfaCache {
    transitions: Vec<u32>,
    records: Vec<Record>,
    seeds: Vec<StateId>,
    map: BTreeMap<Box<[u32]>, u32>,
    starts: Vec<u32>,
    memory: usize,
    clears: usize,
    last_clear_at: usize,
    set: Vec<u32>,
    set_dense: Vec<StateId>,
    seen: Vec<u32>,
    seen_dense: Vec<StateId>,
    stack: Vec<StateId>,
    closure: Vec<StateId>,
    key: Vec<u32>,
    next_seeds: Vec<StateId>,
    ready: bool,
}

impl DfaCache {
    pub fn new() -> DfaCache {
        DfaCache {
            transitions: Vec::new(),
            records: Vec::new(),
            seeds: Vec::new(),
            map: BTreeMap::new(),
            starts: Vec::new(),
            memory: 0,
            clears: 0,
            last_clear_at: 0,
            set: Vec::new(),
            set_dense: Vec::new(),
            seen: Vec::new(),
            seen_dense: Vec::new(),
            stack: Vec::new(),
            closure: Vec::new(),
            key: Vec::new(),
            next_seeds: Vec::new(),
            ready: false,
        }
    }
}

impl Default for DfaCache {
    fn default() -> DfaCache {
        DfaCache::new()
    }
}

fn byte_context(b: u8, line_terminator: u8) -> u8 {
    let mut context = 0;
    if b == line_terminator {
        context |= LINE_TERMINATOR;
    }
    if b == b'\n' {
        context |= LINE_FEED;
    }
    if b == b'\r' {
        context |= CARRIAGE_RETURN;
    }
    if is_word_byte(b) {
        context |= WORD;
    }
    if b >= 0x80 {
        context |= NON_ASCII;
    }
    context
}

fn context_mask(looks: LookSet) -> u8 {
    let mut mask = 0;
    if looks.contains(Look::Start) || looks.contains(Look::End) {
        mask |= EDGE;
    }
    if looks.contains(Look::StartLf) || looks.contains(Look::EndLf) {
        mask |= EDGE | LINE_TERMINATOR;
    }
    if looks.contains(Look::StartCrlf) || looks.contains(Look::EndCrlf) {
        mask |= EDGE | LINE_FEED | CARRIAGE_RETURN;
    }
    if looks.contains_word() {
        mask |= WORD;
    }
    if looks.contains_word_unicode() {
        mask |= NON_ASCII;
    }
    mask
}

fn look_holds(look: Look, left: u8, right: u8) -> Option<bool> {
    let word_left = left & WORD != 0;
    let word_right = right & WORD != 0;
    let unknown_left = left & NON_ASCII != 0;
    let unknown_right = right & NON_ASCII != 0;
    Some(match look {
        Look::Start => left & EDGE != 0,
        Look::End => right & EDGE != 0,
        Look::StartLf => left & (EDGE | LINE_TERMINATOR) != 0,
        Look::EndLf => right & (EDGE | LINE_TERMINATOR) != 0,
        Look::StartCrlf => {
            left & (EDGE | LINE_FEED) != 0
                || (left & CARRIAGE_RETURN != 0 && right & LINE_FEED == 0)
        }
        Look::EndCrlf => {
            right & (EDGE | CARRIAGE_RETURN) != 0
                || (right & LINE_FEED != 0 && left & CARRIAGE_RETURN == 0)
        }
        Look::WordAscii => word_left != word_right,
        Look::WordAsciiNegate => word_left == word_right,
        Look::WordStartAscii => !word_left && word_right,
        Look::WordEndAscii => word_left && !word_right,
        Look::WordStartHalfAscii => !word_left,
        Look::WordEndHalfAscii => !word_right,
        Look::WordUnicode
        | Look::WordUnicodeNegate
        | Look::WordStartUnicode
        | Look::WordEndUnicode => {
            if unknown_left || unknown_right {
                return None;
            }
            match look {
                Look::WordUnicode => word_left != word_right,
                Look::WordUnicodeNegate => word_left == word_right,
                Look::WordStartUnicode => !word_left && word_right,
                _ => word_left && !word_right,
            }
        }
        Look::WordStartHalfUnicode => {
            if unknown_left {
                return None;
            }
            !word_left
        }
        Look::WordEndHalfUnicode => {
            if unknown_right {
                return None;
            }
            !word_right
        }
    })
}

impl Dfa {
    pub fn new(
        nfa: &Nfa,
        kind: Kind,
        line_terminator: u8,
        capacity: usize,
        specialize_start: bool,
    ) -> Dfa {
        let context_mask = context_mask(nfa.look_set);
        let mut boundaries = [false; 256];
        let mut mark = |start: u8, end: u8| {
            boundaries[start as usize] = true;
            if end < 0xFF {
                boundaries[end as usize + 1] = true;
            }
        };
        for state in &nfa.states {
            match state {
                State::ByteRange { start, end, .. } => mark(*start, *end),
                State::Sparse(transitions) => {
                    for t in transitions.iter() {
                        mark(t.start, t.end);
                    }
                }
                _ => {}
            }
        }
        for b in 1..=255u8 {
            if byte_context(b, line_terminator) & context_mask
                != byte_context(b - 1, line_terminator) & context_mask
            {
                boundaries[b as usize] = true;
            }
        }
        let mut classes = [0u8; 256];
        let mut representatives = vec![0u8];
        let mut class = 0usize;
        for b in 1..256usize {
            if boundaries[b] {
                class += 1;
                representatives.push(b as u8);
            }
            classes[b] = class as u8;
        }
        let alphabet_len = class + 1;
        let stride = (alphabet_len + 1).next_power_of_two().max(STRIDE_MIN);
        Dfa {
            classes,
            representatives,
            alphabet_len,
            stride,
            kind,
            reverse: nfa.reverse,
            line_terminator,
            context_mask,
            start_anchored: nfa.start_anchored,
            start_unanchored: nfa.start_unanchored,
            specialize_start,
            capacity,
        }
    }

    fn state_cost(&self, seeds: usize) -> usize {
        self.stride * size_of::<u32>()
            + seeds * size_of::<StateId>() * 2
            + size_of::<Record>()
            + size_of::<u32>() * 4
    }

    fn reset(&self, cache: &mut DfaCache, states: usize) {
        cache.transitions.clear();
        cache.records.clear();
        cache.seeds.clear();
        cache.map.clear();
        cache.starts.clear();
        cache.starts.resize(CONTEXTS * 2, UNKNOWN);
        cache.memory = 0;
        if cache.set.len() != states {
            cache.set = vec![0; states];
            cache.seen = vec![0; states];
        }
        cache.ready = true;
    }

    pub fn prepare(&self, nfa: &Nfa, cache: &mut DfaCache) {
        if !cache.ready || cache.set.len() != nfa.states.len() {
            self.reset(cache, nfa.states.len());
        }
        cache.clears = 0;
    }

    fn intern(&self, cache: &mut DfaCache, context: u8, matched: bool) -> Option<u32> {
        let seeds_len = cache.next_seeds.len();
        cache.key.clear();
        cache
            .key
            .push(u32::from(context) | (u32::from(matched) << 8));
        cache.key.extend_from_slice(&cache.next_seeds);
        if let Some(&id) = cache.map.get(&cache.key[..]) {
            return Some(id);
        }
        let cost = self.state_cost(seeds_len);
        if cache.memory + cost > self.capacity {
            return None;
        }
        let index = cache.records.len();
        let id = u32::try_from(index * self.stride).expect("DFA state identifiers fit in 32 bits");
        let seeds_start = cache.seeds.len() as u32;
        cache.seeds.extend_from_slice(&cache.next_seeds);
        cache.records.push(Record {
            seeds_start,
            seeds_len: seeds_len as u32,
            context,
            matched,
        });
        cache
            .transitions
            .resize(cache.transitions.len() + self.stride, UNKNOWN);
        let key = cache.key.clone().into_boxed_slice();
        cache.map.insert(key, id);
        cache.memory += cost;
        Some(id)
    }

    fn tag(&self, cache: &DfaCache, id: u32) -> u32 {
        let record = cache.records[id as usize / self.stride];
        let mut tagged = id;
        if record.matched {
            tagged |= TAG_MATCH;
        }
        if self.specialize_start
            && !record.matched
            && record.seeds_len == 1
            && cache.seeds[record.seeds_start as usize] == self.start_unanchored
            && self.start_unanchored != self.start_anchored
        {
            tagged |= TAG_START;
        }
        tagged
    }

    fn add_state(
        &self,
        cache: &mut DfaCache,
        context: u8,
        matched: bool,
        at: usize,
    ) -> Result<u32, Fallback> {
        if let Some(id) = self.intern(cache, context, matched) {
            return Ok(self.tag(cache, id));
        }
        self.clear(cache, at)?;
        match self.intern(cache, context, matched) {
            Some(id) => Ok(self.tag(cache, id)),
            None => Err(Fallback::GaveUp),
        }
    }

    fn clear(&self, cache: &mut DfaCache, at: usize) -> Result<(), Fallback> {
        let states = cache.records.len();
        if states < MINIMUM_CACHE_STATES {
            return Err(Fallback::GaveUp);
        }
        if cache.clears >= MINIMUM_CACHE_CLEAR_COUNT {
            let progress = at.abs_diff(cache.last_clear_at);
            if progress < MINIMUM_BYTES_PER_STATE * states {
                return Err(Fallback::GaveUp);
            }
        }
        cache.clears += 1;
        cache.last_clear_at = at;
        let set_len = cache.set.len();
        self.reset(cache, set_len);
        Ok(())
    }

    fn start(
        &self,
        cache: &mut DfaCache,
        anchored: bool,
        context: u8,
        at: usize,
    ) -> Result<u32, Fallback> {
        let index = usize::from(anchored) * CONTEXTS + usize::from(context);
        let known = cache.starts[index];
        if known != UNKNOWN {
            return Ok(known);
        }
        cache.next_seeds.clear();
        cache.next_seeds.push(if anchored {
            self.start_anchored
        } else {
            self.start_unanchored
        });
        let id = self.add_state(cache, context, false, at)?;
        cache.starts[index] = id;
        Ok(id)
    }

    fn compute(
        &self,
        nfa: &Nfa,
        cache: &mut DfaCache,
        current: u32,
        class: usize,
        at: usize,
    ) -> Result<u32, Fallback> {
        let record = cache.records[(current & !TAG_MASK) as usize / self.stride];
        let byte = (class < self.alphabet_len).then(|| self.representatives[class]);
        let neighbor = match byte {
            Some(b) => byte_context(b, self.line_terminator) & self.context_mask,
            None => EDGE & self.context_mask,
        };
        let (left, right) = if self.reverse {
            (neighbor, record.context)
        } else {
            (record.context, neighbor)
        };
        cache.set_dense.clear();
        cache.closure.clear();
        let seeds_start = record.seeds_start as usize;
        for index in 0..record.seeds_len as usize {
            let seed = cache.seeds[seeds_start + index];
            cache.stack.push(seed);
            while let Some(mut id) = cache.stack.pop() {
                loop {
                    let slot = cache.set[id as usize] as usize;
                    if slot < cache.set_dense.len() && cache.set_dense[slot] == id {
                        break;
                    }
                    cache.set[id as usize] = cache.set_dense.len() as u32;
                    cache.set_dense.push(id);
                    match &nfa.states[id as usize] {
                        State::Union(alternates) => {
                            id = alternates[0];
                            cache.stack.extend(alternates[1..].iter().rev().copied());
                        }
                        State::Capture { next, .. } => id = *next,
                        State::Look { look, next } => match look_holds(*look, left, right) {
                            None => {
                                cache.stack.clear();
                                return Ok(QUIT);
                            }
                            Some(false) => break,
                            Some(true) => id = *next,
                        },
                        State::Fail => break,
                        State::ByteRange { .. } | State::Sparse(_) | State::Match => {
                            cache.closure.push(id);
                            break;
                        }
                    }
                }
            }
        }
        let mut matched = false;
        cache.next_seeds.clear();
        cache.seen_dense.clear();
        for index in 0..cache.closure.len() {
            let id = cache.closure[index];
            let target = match (&nfa.states[id as usize], byte) {
                (State::Match, _) => {
                    matched = true;
                    if self.kind == Kind::LeftmostFirst {
                        break;
                    }
                    continue;
                }
                (State::ByteRange { start, end, next }, Some(b)) if *start <= b && b <= *end => {
                    *next
                }
                (State::Sparse(transitions), Some(b)) => {
                    match transitions.iter().find(|t| t.start <= b && b <= t.end) {
                        Some(t) => t.next,
                        None => continue,
                    }
                }
                _ => continue,
            };
            let slot = cache.seen[target as usize] as usize;
            if slot < cache.seen_dense.len() && cache.seen_dense[slot] == target {
                continue;
            }
            cache.seen[target as usize] = cache.seen_dense.len() as u32;
            cache.seen_dense.push(target);
            cache.next_seeds.push(target);
        }
        if cache.next_seeds.is_empty() && !matched {
            self.store(cache, current, class, DEAD);
            return Ok(DEAD);
        }
        let before = cache.clears;
        let next = self.add_state(cache, neighbor, matched, at)?;
        if cache.clears == before {
            self.store(cache, current, class, next);
        }
        Ok(next)
    }

    fn store(&self, cache: &mut DfaCache, current: u32, class: usize, next: u32) {
        let index = (current & !TAG_MASK) as usize + class;
        if index < cache.transitions.len() {
            cache.transitions[index] = next;
        }
    }

    fn context_before(&self, haystack: &[u8], at: usize) -> u8 {
        if at == 0 {
            EDGE & self.context_mask
        } else {
            byte_context(haystack[at - 1], self.line_terminator) & self.context_mask
        }
    }

    fn context_after(&self, haystack: &[u8], at: usize) -> u8 {
        if at >= haystack.len() {
            EDGE & self.context_mask
        } else {
            byte_context(haystack[at], self.line_terminator) & self.context_mask
        }
    }

    #[inline(always)]
    fn step(&self, transitions: &[u32], current: u32, byte: u8) -> u32 {
        transitions[current as usize + usize::from(self.classes[usize::from(byte)])]
    }

    fn skip_untagged(
        &self,
        transitions: &[u32],
        haystack: &[u8],
        start: usize,
        end: usize,
        state: u32,
    ) -> (usize, u32) {
        assert_eq!(
            state & TAG_MASK,
            0,
            "the skipped run starts in an untagged state"
        );
        let haystack = &haystack[..end];
        let mut at = start;
        let mut current = state;
        while at + UNROLL_BYTES <= end {
            let first = self.step(transitions, current, haystack[at]);
            if first & TAG_MASK != 0 {
                return (at, current);
            }
            let second = self.step(transitions, first, haystack[at + 1]);
            if second & TAG_MASK != 0 {
                return (at + 1, first);
            }
            let third = self.step(transitions, second, haystack[at + 2]);
            if third & TAG_MASK != 0 {
                return (at + 2, second);
            }
            let fourth = self.step(transitions, third, haystack[at + 3]);
            if fourth & TAG_MASK != 0 {
                return (at + 3, third);
            }
            current = fourth;
            at += UNROLL_BYTES;
        }
        while at < end {
            let next = self.step(transitions, current, haystack[at]);
            if next & TAG_MASK != 0 {
                return (at, current);
            }
            current = next;
            at += 1;
        }
        (at, current)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn search_forward(
        &self,
        nfa: &Nfa,
        cache: &mut DfaCache,
        haystack: &[u8],
        start: usize,
        end: usize,
        anchored: bool,
        earliest: bool,
        prefilter: Option<&Prefilter>,
    ) -> Result<Option<usize>, Fallback> {
        assert!(!self.reverse);
        self.prepare(nfa, cache);
        let mut at = start;
        let mut last_match = None;
        let mut current = self.start(cache, anchored, self.context_before(haystack, at), at)?;
        if current & TAG_START != 0
            && let Some(prefilter) = prefilter
        {
            match prefilter.find(haystack, at, end) {
                None => return Ok(None),
                Some(found) => {
                    at = found;
                    current = self.start(cache, anchored, self.context_before(haystack, at), at)?;
                }
            }
        }
        while at < end {
            if current & TAG_MASK == 0 {
                (at, current) = self.skip_untagged(&cache.transitions, haystack, at, end, current);
                if at == end {
                    break;
                }
            }
            let class = self.classes[haystack[at] as usize] as usize;
            let mut next = cache.transitions[(current & !TAG_MASK) as usize + class];
            if next & TAG_MASK == 0 {
                current = next;
                at += 1;
                continue;
            }
            if next == UNKNOWN {
                next = self.compute(nfa, cache, current, class, at)?;
            }
            if next & TAG_SPECIAL != 0 {
                if next == DEAD {
                    return Ok(last_match);
                }
                return Err(Fallback::Quit);
            }
            if next & TAG_MATCH != 0 {
                last_match = Some(at);
                if earliest {
                    return Ok(last_match);
                }
            }
            current = next;
            at += 1;
            if current & TAG_START != 0
                && let Some(prefilter) = prefilter
            {
                match prefilter.find(haystack, at, end) {
                    None => return Ok(last_match),
                    Some(found) => {
                        if found != at {
                            at = found;
                            current =
                                self.start(cache, anchored, self.context_before(haystack, at), at)?;
                        }
                    }
                }
            }
        }
        let next = if end < haystack.len() {
            let class = self.classes[haystack[end] as usize] as usize;
            let next = cache.transitions[(current & !TAG_MASK) as usize + class];
            if next == UNKNOWN {
                self.compute(nfa, cache, current, class, at)?
            } else {
                next
            }
        } else {
            let next = cache.transitions[(current & !TAG_MASK) as usize + self.alphabet_len];
            if next == UNKNOWN {
                self.compute(nfa, cache, current, self.alphabet_len, at)?
            } else {
                next
            }
        };
        if next == QUIT {
            return Err(Fallback::Quit);
        }
        if next & TAG_SPECIAL == 0 && next & TAG_MATCH != 0 {
            last_match = Some(end);
        }
        Ok(last_match)
    }

    pub fn search_reverse(
        &self,
        nfa: &Nfa,
        cache: &mut DfaCache,
        haystack: &[u8],
        start: usize,
        end: usize,
    ) -> Result<Option<usize>, Fallback> {
        assert!(self.reverse);
        self.prepare(nfa, cache);
        let mut at = end;
        let mut last_match = None;
        let mut current = self.start(cache, true, self.context_after(haystack, at), at)?;
        while at > start {
            let class = self.classes[haystack[at - 1] as usize] as usize;
            let mut next = cache.transitions[(current & !TAG_MASK) as usize + class];
            if next == UNKNOWN {
                next = self.compute(nfa, cache, current, class, at)?;
            }
            if next & TAG_SPECIAL != 0 {
                if next == DEAD {
                    return Ok(last_match);
                }
                return Err(Fallback::Quit);
            }
            if next & TAG_MATCH != 0 {
                last_match = Some(at);
            }
            current = next;
            at -= 1;
        }
        let next = if start > 0 {
            let class = self.classes[haystack[start - 1] as usize] as usize;
            let next = cache.transitions[(current & !TAG_MASK) as usize + class];
            if next == UNKNOWN {
                self.compute(nfa, cache, current, class, at)?
            } else {
                next
            }
        } else {
            let next = cache.transitions[(current & !TAG_MASK) as usize + self.alphabet_len];
            if next == UNKNOWN {
                self.compute(nfa, cache, current, self.alphabet_len, at)?
            } else {
                next
            }
        };
        if next == QUIT {
            return Err(Fallback::Quit);
        }
        if next & TAG_SPECIAL == 0 && next & TAG_MATCH != 0 {
            last_match = Some(start);
        }
        Ok(last_match)
    }
}
