use super::DeflateFlush;
use super::matching::longest_match;
use super::state::{BlockState, Io, MIN_LOOKAHEAD, STD_MIN_MATCH, State, WANT_MIN_MATCH};

const INSERT_LENGTH_FACTOR: usize = 16;
const FIZZLE_LENGTH_MAX: u16 = 256;

#[derive(Clone, Copy)]
struct Match {
    match_start: u16,
    match_length: u16,
    string_start: u16,
    original_start: u16,
}

impl Match {
    const EMPTY: Self = Self {
        match_start: 0,
        match_length: 0,
        string_start: 0,
        original_start: 0,
    };
}

fn window_index(position: usize) -> u16 {
    u16::try_from(position).expect("a window position fits in 16 bits")
}

fn find_match(state: &mut State, candidate: &mut Match, head: u16) -> bool {
    if state.within_distance(usize::from(head)) && head != 0 {
        let (length, match_start) = longest_match(state, head, false);
        state.match_start = match_start;
        candidate.match_length = window_index(length);
        candidate.match_start = match_start;
        return true;
    }
    candidate.match_start = 0;
    candidate.match_length = 1;
    false
}

fn current_match(state: &mut State) -> Match {
    let mut current = Match::EMPTY;
    let head = if state.lookahead >= WANT_MIN_MATCH {
        state.standard_insert(state.string_start)
    } else {
        0
    };
    current.string_start = window_index(state.string_start);
    current.original_start = current.string_start;
    if find_match(state, &mut current, head) {
        if usize::from(current.match_length) < WANT_MIN_MATCH {
            current.match_length = 1;
        }
        if current.match_start >= current.string_start {
            current.match_length = 1;
        }
    }
    current
}

fn look_ahead(state: &mut State, current: &mut Match, next: &mut Match) {
    state.string_start = usize::from(current.string_start + current.match_length);
    let head = state.standard_insert(state.string_start);
    next.string_start = window_index(state.string_start);
    next.original_start = next.string_start;
    if find_match(state, next, head) {
        if next.match_start >= next.string_start {
            next.match_length = 1;
        }
        if usize::from(next.match_length) < WANT_MIN_MATCH {
            next.match_length = 1;
        } else {
            fizzle_matches(&state.window, state.max_distance(), current, next);
        }
    }
    state.string_start = usize::from(current.string_start);
}

pub(super) fn deflate_medium(state: &mut State, io: &mut Io, flush: DeflateFlush) -> BlockState {
    let early_exit = state.early_exit();
    let mut current;
    let mut next = Match::EMPTY;
    loop {
        if state.lookahead < MIN_LOOKAHEAD {
            state.fill_window(io);
            if state.lookahead < MIN_LOOKAHEAD && flush == DeflateFlush::NoFlush {
                return BlockState::NeedMore;
            }
            if state.lookahead == 0 {
                break;
            }
            next.match_length = 0;
        }
        if !early_exit && next.match_length > 0 {
            current = next;
            next.match_length = 0;
        } else {
            current = current_match(state);
        }
        insert_match(state, current);
        if !early_exit
            && state.lookahead > MIN_LOOKAHEAD
            && usize::from(current.string_start + current.match_length)
                < state.window_buffer_size() - MIN_LOOKAHEAD
        {
            look_ahead(state, &mut current, &mut next);
        } else {
            next.match_length = 0;
        }
        let block_full = emit_match(state, current);
        state.string_start += usize::from(current.match_length);
        if block_full && let Some(stop) = state.flush_block_or_stop(io, false) {
            return stop;
        }
    }
    state.insert = state.string_start.min(STD_MIN_MATCH - 1);
    state.finish_block(io, flush)
}

fn emit_match(state: &mut State, current: Match) -> bool {
    let mut block_full = false;
    let length = usize::from(current.match_length);
    if length < WANT_MIN_MATCH {
        for offset in 0..length {
            let byte = state.window[state.string_start + offset];
            block_full |= state.tally_literal(byte);
        }
    } else {
        block_full |= state.tally_distance(
            usize::from(current.string_start - current.match_start),
            length - STD_MIN_MATCH,
        );
    }
    state.lookahead -= length;
    block_full
}

fn insert_range(state: &mut State, current: Match) {
    if current.string_start >= current.original_start {
        if current.string_start + current.match_length > current.original_start {
            state.insert_string(
                usize::from(current.string_start),
                usize::from(current.match_length),
            );
        } else {
            state.insert_string(
                usize::from(current.string_start),
                usize::from(current.original_start - current.string_start + 1),
            );
        }
    } else if current.original_start < current.string_start + current.match_length {
        state.insert_string(
            usize::from(current.original_start),
            usize::from(current.string_start + current.match_length - current.original_start),
        );
    }
}

fn insert_match(state: &mut State, mut current: Match) {
    if state.lookahead <= usize::from(current.match_length) + WANT_MIN_MATCH {
        return;
    }
    if usize::from(current.match_length) < WANT_MIN_MATCH {
        current.string_start += 1;
        current.match_length -= 1;
        if current.match_length > 0 && current.string_start >= current.original_start {
            insert_range(state, current);
        }
        return;
    }
    if usize::from(current.match_length) <= INSERT_LENGTH_FACTOR * usize::from(state.max_lazy_match)
        && state.lookahead >= WANT_MIN_MATCH
    {
        current.match_length -= 1;
        current.string_start += 1;
        insert_range(state, current);
    } else {
        current.string_start += current.match_length;
        if current.string_start >= 1 {
            state.standard_insert(usize::from(current.string_start) - 1);
        }
    }
}

fn fizzle_matches(window: &[u8], max_distance: usize, current: &mut Match, next: &mut Match) {
    if current.match_length <= 1 {
        return;
    }
    if current.match_length > 1 + next.match_start {
        return;
    }
    if current.match_length > 1 + next.string_start {
        return;
    }
    let candidate = usize::from(next.match_start + 1 - current.match_length);
    let original = usize::from(next.string_start + 1 - current.match_length);
    if window[candidate] != window[original] {
        return;
    }
    let limit = next.string_start.saturating_sub(window_index(max_distance));
    let mut changed_current = *current;
    let mut changed_next = *next;
    let mut candidate_bytes = window[..usize::from(changed_next.match_start)].iter().rev();
    let mut original_bytes = window[..usize::from(changed_next.string_start)]
        .iter()
        .rev();
    let mut changed = 0;
    while candidate_bytes.next() == original_bytes.next() {
        if changed_current.match_length < 1
            || changed_next.string_start <= limit
            || changed_next.match_length >= FIZZLE_LENGTH_MAX
            || changed_next.match_start <= 1
        {
            break;
        }
        changed_next.string_start -= 1;
        changed_next.match_start -= 1;
        changed_next.match_length += 1;
        changed_current.match_length -= 1;
        changed += 1;
    }
    if changed == 0 {
        return;
    }
    if changed_current.match_length <= 1 && changed_next.match_length != 2 {
        changed_next.original_start += 1;
        *current = changed_current;
        *next = changed_next;
    }
}
