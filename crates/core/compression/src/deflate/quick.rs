use super::DeflateFlush;
use super::bits::BlockType;
use super::matching::{KNOWN_PREFIX_BYTES, compare256, read_u32};
use super::state::{
    BlockState, Io, MIN_LOOKAHEAD, QuickBlock, STD_MAX_MATCH, STD_MIN_MATCH, State, WANT_MIN_MATCH,
    signed,
};
use super::trees::STATIC_LITERAL_TREE;

const BIT_BUFFER_BYTES: usize = 8;

fn start_block(state: &mut State, last: bool) {
    state.bits.emit_tree(BlockType::StaticTrees, last);
    state.quick_block = if last {
        QuickBlock::OpenLast
    } else {
        QuickBlock::Open
    };
    state.block_start = signed(state.string_start);
}

fn end_block(state: &mut State, io: &mut Io, last: bool) -> Option<BlockState> {
    if state.quick_block == QuickBlock::Closed {
        return None;
    }
    state
        .bits
        .emit_end_block_and_align(&STATIC_LITERAL_TREE, last);
    state.quick_block = QuickBlock::Closed;
    state.block_start = signed(state.string_start);
    state.flush_pending(io);
    if io.output_available() != 0 {
        return None;
    }
    Some(if last {
        BlockState::FinishStarted
    } else {
        BlockState::NeedMore
    })
}

fn find_match(state: &mut State) -> Option<usize> {
    let start = state.string_start;
    let value = read_u32(&state.window, start);
    let head = usize::from(state.standard_insert_value(start, value));
    if !state.within_distance(head) {
        return None;
    }
    let distance = start - head;
    if value != read_u32(&state.window, head) {
        return None;
    }
    let length = compare256(
        &state.window,
        start + KNOWN_PREFIX_BYTES,
        head + KNOWN_PREFIX_BYTES,
    ) + KNOWN_PREFIX_BYTES;
    if length < WANT_MIN_MATCH {
        return None;
    }
    let length = length.min(state.lookahead).min(STD_MAX_MATCH);
    let distance = u16::try_from(distance).expect("a match distance fits in 16 bits");
    let length_code = u8::try_from(length - STD_MIN_MATCH).expect("a length code fits in a byte");
    state.bits.emit_distance(
        &STATIC_LITERAL_TREE,
        &super::trees::STATIC_DISTANCE_TREE,
        length_code,
        distance,
    );
    Some(length)
}

pub(super) fn deflate_quick(state: &mut State, io: &mut Io, flush: DeflateFlush) -> BlockState {
    let last = flush == DeflateFlush::Finish;
    if last && state.quick_block != QuickBlock::OpenLast {
        if let Some(stop) = end_block(state, io, false) {
            return stop;
        }
        start_block(state, last);
    } else if state.quick_block == QuickBlock::Closed && state.lookahead > 0 {
        start_block(state, last);
    }
    loop {
        if state.bits.pending.length() + BIT_BUFFER_BYTES >= state.bits.pending.capacity() {
            state.flush_pending(io);
            if io.output_available() == 0 {
                let finished = last
                    && io.input_available() == 0
                    && state.bits.bits_valid == 0
                    && state.quick_block == QuickBlock::Closed;
                return if finished {
                    BlockState::FinishStarted
                } else {
                    BlockState::NeedMore
                };
            }
        }
        if state.lookahead < MIN_LOOKAHEAD {
            state.fill_window(io);
            if state.lookahead < MIN_LOOKAHEAD && flush == DeflateFlush::NoFlush {
                return BlockState::NeedMore;
            }
            if state.lookahead == 0 {
                break;
            }
            if state.quick_block == QuickBlock::Closed {
                start_block(state, last);
            }
        }
        if state.lookahead >= WANT_MIN_MATCH
            && let Some(length) = find_match(state)
        {
            state.lookahead -= length;
            state.string_start += length;
            continue;
        }
        let byte = state.window[state.string_start];
        state.bits.emit_literal(&STATIC_LITERAL_TREE, byte);
        state.string_start += 1;
        state.lookahead -= 1;
    }
    state.insert = state.string_start.min(STD_MIN_MATCH - 1);
    if let Some(stop) = end_block(state, io, last) {
        return stop;
    }
    if last {
        BlockState::FinishDone
    } else {
        BlockState::BlockDone
    }
}
