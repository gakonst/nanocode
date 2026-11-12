use super::DeflateFlush;
use super::matching::{longest_match, read_u32};
use super::state::{BlockState, Io, MIN_LOOKAHEAD, STD_MIN_MATCH, State, WANT_MIN_MATCH};

fn try_match(state: &mut State) -> Option<bool> {
    let start = state.string_start;
    let value = read_u32(&state.window, start);
    let head = state.standard_insert_value(start, value);
    if !state.within_distance(usize::from(head)) || head == 0 {
        return None;
    }
    let (mut length, match_start) = longest_match(state, head, false);
    state.match_start = match_start;
    if length < WANT_MIN_MATCH {
        return None;
    }
    let flush = state.tally_distance(start - usize::from(match_start), length - STD_MIN_MATCH);
    state.lookahead -= length;
    if length <= usize::from(state.max_lazy_match) && state.lookahead >= WANT_MIN_MATCH {
        length -= 1;
        state.string_start += 1;
        state.insert_string(state.string_start, length);
        state.string_start += length;
    } else {
        state.string_start += length;
        state.standard_insert(state.string_start - 1);
    }
    Some(flush)
}

pub(super) fn deflate_fast(state: &mut State, io: &mut Io, flush: DeflateFlush) -> BlockState {
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
        let block_full = if state.lookahead >= WANT_MIN_MATCH
            && let Some(block_full) = try_match(state)
        {
            block_full
        } else {
            let byte = state.window[state.string_start];
            let block_full = state.tally_literal(byte);
            state.lookahead -= 1;
            state.string_start += 1;
            block_full
        };
        if block_full && let Some(stop) = state.flush_block_or_stop(io, false) {
            return stop;
        }
    }
    state.insert = state.string_start.min(STD_MIN_MATCH - 1);
    state.finish_block(io, flush)
}
