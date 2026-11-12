use super::matching::longest_match;
use super::state::{
    BlockState, Io, MIN_LOOKAHEAD, ROLL_HASH_CHAIN_ABOVE, STD_MIN_MATCH, State, WANT_MIN_MATCH,
};
use super::{DeflateFlush, Strategy};

const FILTERED_LENGTH_MAX: usize = 5;

fn search(state: &mut State, slow_search: bool) -> usize {
    let head = if state.lookahead >= WANT_MIN_MATCH {
        state.quick_insert_string(state.string_start)
    } else {
        0
    };
    state.previous_match = state.match_start;
    let mut length = STD_MIN_MATCH - 1;
    if state.within_distance(usize::from(head))
        && state.previous_length < state.max_lazy_match
        && head != 0
    {
        let (found, match_start) = longest_match(state, head, slow_search);
        length = found;
        state.match_start = match_start;
        if length <= FILTERED_LENGTH_MAX && state.strategy == Strategy::Filtered {
            length = STD_MIN_MATCH - 1;
        }
    }
    length
}

fn emit_previous_match(state: &mut State) -> bool {
    let max_insert = state.string_start + state.lookahead - STD_MIN_MATCH;
    let block_full = state.tally_distance(
        state.string_start - 1 - usize::from(state.previous_match),
        usize::from(state.previous_length) - STD_MIN_MATCH,
    );
    state.previous_length -= 1;
    state.lookahead -= usize::from(state.previous_length);
    let move_forward = usize::from(state.previous_length) - 1;
    if max_insert > state.string_start {
        let count = move_forward.min(max_insert - state.string_start);
        state.insert_string(state.string_start + 1, count);
    }
    state.previous_length = 0;
    state.match_available = false;
    state.string_start += move_forward + 1;
    block_full
}

pub(super) fn deflate_slow(state: &mut State, io: &mut Io, flush: DeflateFlush) -> BlockState {
    let slow_search = state.max_chain_length > ROLL_HASH_CHAIN_ABOVE;
    let mut match_available = state.match_available;
    loop {
        if state.lookahead < MIN_LOOKAHEAD {
            state.fill_window(io);
            if state.lookahead < MIN_LOOKAHEAD && flush == DeflateFlush::NoFlush {
                return BlockState::NeedMore;
            }
            if state.lookahead == 0 {
                break;
            }
        }
        let length = search(state, slow_search);
        let previous_length = usize::from(state.previous_length);
        if previous_length >= STD_MIN_MATCH && length <= previous_length {
            let block_full = emit_previous_match(state);
            match_available = false;
            if block_full && let Some(stop) = state.flush_block_or_stop(io, false) {
                return stop;
            }
        } else if match_available {
            let byte = state.window[state.string_start - 1];
            if state.tally_literal(byte) {
                state.flush_block_only(io, false);
            }
            state.previous_length = u16::try_from(length).expect("a match length fits in 16 bits");
            state.string_start += 1;
            state.lookahead -= 1;
            if io.output_available() == 0 {
                return BlockState::NeedMore;
            }
        } else {
            state.previous_length = u16::try_from(length).expect("a match length fits in 16 bits");
            state.match_available = true;
            match_available = true;
            state.string_start += 1;
            state.lookahead -= 1;
        }
    }
    assert!(
        flush != DeflateFlush::NoFlush,
        "the slow loop ends only when flushing"
    );
    if state.match_available {
        let byte = state.window[state.string_start - 1];
        state.tally_literal(byte);
        state.match_available = false;
    }
    state.insert = state.string_start.min(STD_MIN_MATCH - 1);
    state.finish_block(io, flush)
}
