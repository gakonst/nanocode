use super::DeflateFlush;
use super::matching::{KNOWN_PREFIX_BYTES, compare256_run};
use super::state::{BlockState, Io, MIN_LOOKAHEAD, STD_MAX_MATCH, STD_MIN_MATCH, State};

pub(super) fn deflate_huffman(state: &mut State, io: &mut Io, flush: DeflateFlush) -> BlockState {
    loop {
        if state.lookahead == 0 {
            state.fill_window(io);
            if state.lookahead == 0 {
                if flush == DeflateFlush::NoFlush {
                    return BlockState::NeedMore;
                }
                break;
            }
        }
        let byte = state.window[state.string_start];
        let block_full = state.tally_literal(byte);
        state.lookahead -= 1;
        state.string_start += 1;
        if block_full && let Some(stop) = state.flush_block_or_stop(io, false) {
            return stop;
        }
    }
    state.insert = 0;
    state.finish_block(io, flush)
}

fn run_length(state: &State) -> usize {
    if state.lookahead < STD_MIN_MATCH || state.string_start == 0 {
        return 0;
    }
    let start = state.string_start;
    let byte = state.window[start - 1];
    if byte != state.window[start] || byte != state.window[start + 1] {
        return 0;
    }
    let length =
        compare256_run(byte, &state.window, start + KNOWN_PREFIX_BYTES) + KNOWN_PREFIX_BYTES;
    length.min(state.lookahead).min(STD_MAX_MATCH)
}

pub(super) fn deflate_rle(state: &mut State, io: &mut Io, flush: DeflateFlush) -> BlockState {
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
        let length = run_length(state);
        let block_full = if length >= STD_MIN_MATCH {
            let block_full = state.tally_distance(1, length - STD_MIN_MATCH);
            state.lookahead -= length;
            state.string_start += length;
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
    state.insert = 0;
    state.finish_block(io, flush)
}
