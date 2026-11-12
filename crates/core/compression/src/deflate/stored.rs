use super::DeflateFlush;
use super::state::{BlockState, Io, MAX_STORED, State, signed};
use super::trees::stored_block;

const BLOCK_HEADER_BITS: u8 = 3;
const ALIGN_BITS_MAX: u8 = 7;
const STORED_LENGTH_FIELDS_BITS: u8 = 32;
const STORED_HEADER_BITS: u8 = BLOCK_HEADER_BITS + ALIGN_BITS_MAX + STORED_LENGTH_FIELDS_BITS;
const STORED_BLOCK_HEADER_BYTES: usize = 5;
const STORED_LENGTH_FIELDS_BYTES: usize = 4;

fn header_bytes(state: &State) -> usize {
    usize::from((state.bits.bits_valid + STORED_HEADER_BITS) / u8::BITS as u8)
}

fn unsigned(position: isize) -> usize {
    usize::try_from(position).expect("a window position is not negative")
}

pub(super) fn deflate_stored(state: &mut State, io: &mut Io, flush: DeflateFlush) -> BlockState {
    let min_block =
        (state.bits.pending.capacity() - STORED_BLOCK_HEADER_BYTES).min(state.window_size);
    let available_before = io.input_available();
    let last = copy_direct(state, io, flush, min_block);
    let used = available_before - io.input_available();
    if used > 0 {
        remember_direct(state, io, used);
    }
    if last {
        return BlockState::FinishDone;
    }
    if flush != DeflateFlush::NoFlush
        && flush != DeflateFlush::Finish
        && io.input_available() == 0
        && signed(state.string_start) == state.block_start
    {
        return BlockState::BlockDone;
    }
    fill_stored_window(state, io);
    store_pending(state, io, flush)
}

fn copy_direct(state: &mut State, io: &mut Io, flush: DeflateFlush, min_block: usize) -> bool {
    loop {
        let header = header_bytes(state);
        if io.output_available() < header {
            return false;
        }
        let left = unsigned((signed(state.string_start) - state.block_start).max(0));
        let have = io.output_available() - header;
        let mut length = MAX_STORED.min(left + io.input_available()).min(have);
        let everything = length == left + io.input_available();
        if length < min_block
            && ((length == 0 && flush != DeflateFlush::Finish)
                || flush == DeflateFlush::NoFlush
                || !everything)
        {
            return false;
        }
        let last = flush == DeflateFlush::Finish && everything;
        stored_block(&mut state.bits, &[], last);
        state.bits.pending.rewind(STORED_LENGTH_FIELDS_BYTES);
        let stored = u16::try_from(length).expect("a stored block is at most 65535 bytes");
        state.bits.pending.extend(&stored.to_le_bytes());
        state.bits.pending.extend(&(!stored).to_le_bytes());
        state.flush_pending(io);
        if left > 0 {
            let left = left.min(length);
            let start = unsigned(state.block_start);
            io.write(&state.window[start..start + left]);
            state.total_out += u64::try_from(left).expect("a buffer length fits in 64 bits");
            state.block_start += signed(left);
            length -= left;
        }
        if length > 0 {
            copy_input(state, io, length);
        }
        if last {
            return true;
        }
    }
}

fn copy_input(state: &mut State, io: &mut Io, size: usize) {
    let length = io.input_available().min(size);
    if length == 0 {
        return;
    }
    let data = io.take_input(length);
    io.write(data);
    state.update_checksum(data);
    let length = u64::try_from(length).expect("a buffer length fits in 64 bits");
    state.total_in += length;
    state.total_out += length;
}

fn slide_stored_window(state: &mut State) {
    state.string_start -= state.window_size;
    let copy = state
        .string_start
        .min(state.window_buffer_size() - state.window_size);
    state
        .window
        .copy_within(state.window_size..state.window_size + copy, 0);
    state.insert = state.insert.min(state.string_start);
}

fn remember_direct(state: &mut State, io: &Io, used: usize) {
    let size = state.window_size;
    if used >= size {
        state.window[..size].copy_from_slice(io.consumed_tail(size));
        state.string_start = size;
        state.insert = state.string_start;
    } else {
        if state.window_buffer_size() - state.string_start <= used {
            slide_stored_window(state);
        }
        let start = state.string_start;
        state.window[start..start + used].copy_from_slice(io.consumed_tail(used));
        state.string_start += used;
        state.insert += used.min(size - state.insert);
    }
    state.block_start = signed(state.string_start);
}

fn fill_stored_window(state: &mut State, io: &mut Io) {
    let mut have = state.window_buffer_size() - state.string_start;
    if io.input_available() > have && state.block_start >= signed(state.window_size) {
        state.block_start -= signed(state.window_size);
        slide_stored_window(state);
        have += state.window_size;
    }
    let have = have.min(io.input_available());
    if have > 0 {
        state.read_buffer(io, state.string_start, have);
        state.string_start += have;
        state.insert += have.min(state.window_size - state.insert);
    }
}

fn store_pending(state: &mut State, io: &mut Io, flush: DeflateFlush) -> BlockState {
    let header = header_bytes(state);
    let have = (state.bits.pending.capacity() - header).min(MAX_STORED);
    let min_block = have.min(state.window_size);
    let left = unsigned(signed(state.string_start) - state.block_start);
    let mut last = false;
    if left >= min_block
        || ((left > 0 || flush == DeflateFlush::Finish)
            && flush != DeflateFlush::NoFlush
            && io.input_available() == 0
            && left <= have)
    {
        let length = left.min(have);
        last = flush == DeflateFlush::Finish && io.input_available() == 0 && length == left;
        let start = unsigned(state.block_start);
        stored_block(&mut state.bits, &state.window[start..start + length], last);
        state.block_start += signed(length);
        state.flush_pending(io);
    }
    if last {
        BlockState::FinishStarted
    } else {
        BlockState::NeedMore
    }
}
