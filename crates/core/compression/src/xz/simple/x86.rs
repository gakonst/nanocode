use super::{Direction, translate, wrapping_position};

pub(super) const UNFILTERED_BYTES_MAX: usize = 5;
pub(super) const ALIGNMENT_BYTES: u32 = 1;
const INSTRUCTION_BYTES: usize = 5;
const INSTRUCTION_BYTES_U32: u32 = INSTRUCTION_BYTES as u32;
const OPCODE_CALL: u8 = 0xE8;
const OPCODE_JUMP: u8 = 0xE9;
const OPCODE_CALL_OR_JUMP_MASK: u8 = !(OPCODE_CALL ^ OPCODE_JUMP);
const OPERAND_OFFSET_BYTES: usize = 1;
const OPERAND_HIGH_BYTE_OFFSET: usize = 4;
const OPERAND_BYTES: usize = INSTRUCTION_BYTES - OPERAND_OFFSET_BYTES;
const BYTE_ALL_ZEROS: u8 = 0x00;
const BYTE_ALL_ONES: u8 = 0xFF;
const SCAN_CHUNK_BYTES: usize = 16;
const MASK_TO_BIT_NUMBER: [u32; 5] = [0, 1, 2, 2, 3];
const PREVIOUS_MASK_KEEP: u32 = 0x77;
const PREVIOUS_MASK_SEEN: u32 = 0x01;
const PREVIOUS_MASK_SEEN_FAR: u32 = 0x10;
const PREVIOUS_INDEX_MAX: u32 = 4;
const PREVIOUS_INDEX_REJECTED: u32 = 3;
const BYTE_BITS: u32 = 8;
const WORD_BITS: u32 = 32;
const TOP_BYTE_SHIFT: u32 = 24;
const ADDRESS_SIGN_SHIFT: u32 = 24;

pub(super) struct State {
    previous_mask: u32,
    previous_position: u32,
}

impl State {
    pub(super) const fn new() -> Self {
        Self {
            previous_mask: 0,
            previous_position: 0u32.wrapping_sub(INSTRUCTION_BYTES_U32),
        }
    }
}

const fn is_opcode(byte: u8) -> bool {
    byte & OPCODE_CALL_OR_JUMP_MASK == OPCODE_CALL
}

fn find_opcode(bytes: &[u8]) -> Option<usize> {
    let (chunks, remainder) = bytes.as_chunks::<SCAN_CHUNK_BYTES>();
    for (index, chunk) in chunks.iter().enumerate() {
        if chunk
            .iter()
            .fold(false, |found, &byte| found | is_opcode(byte))
        {
            let offset = chunk
                .iter()
                .position(|&byte| is_opcode(byte))
                .expect("a chunk with an opcode has its position");
            return Some(index * SCAN_CHUNK_BYTES + offset);
        }
    }
    let remainder_start = chunks.len() * SCAN_CHUNK_BYTES;
    remainder
        .iter()
        .position(|&byte| is_opcode(byte))
        .map(|offset| remainder_start + offset)
}

const fn is_far_byte(byte: u8) -> bool {
    byte == BYTE_ALL_ZEROS || byte == BYTE_ALL_ONES
}

fn convert(
    direction: Direction,
    previous_mask: u32,
    operand: &mut [u8; OPERAND_BYTES],
    position: u32,
) {
    let source = u32::from_le_bytes(*operand);
    let mut destination = translate(direction, source, position);
    if previous_mask != 0 {
        let index = MASK_TO_BIT_NUMBER[(previous_mask >> 1) as usize];
        let checked_shift = TOP_BYTE_SHIFT - index * BYTE_BITS;
        if is_far_byte((destination >> checked_shift) as u8) {
            let flip_mask = (1u32 << (WORD_BITS - index * BYTE_BITS)) - 1;
            destination = translate(direction, destination ^ flip_mask, position);
            assert!(!is_far_byte((destination >> checked_shift) as u8));
        }
    }
    let sign = (destination >> ADDRESS_SIGN_SHIFT) & 1;
    let mut converted = destination.to_le_bytes();
    converted[OPERAND_BYTES - 1] = (!(sign.wrapping_sub(1))) as u8;
    *operand = converted;
}

pub(super) fn code(
    state: &mut State,
    now_pos: u32,
    direction: Direction,
    buffer: &mut [u8],
) -> usize {
    if buffer.len() < INSTRUCTION_BYTES {
        return 0;
    }
    let mut previous_mask = state.previous_mask;
    let mut previous_position = state.previous_position;
    if now_pos.wrapping_sub(previous_position) > INSTRUCTION_BYTES_U32 {
        previous_position = now_pos.wrapping_sub(INSTRUCTION_BYTES_U32);
    }
    let limit = buffer.len() - INSTRUCTION_BYTES;
    let mut position = 0;
    while position <= limit {
        let Some(found) = find_opcode(&buffer[position..=limit]) else {
            position = limit + 1;
            break;
        };
        position += found;
        let current = wrapping_position(now_pos, position);
        let offset = current.wrapping_sub(previous_position);
        previous_position = current;
        if offset > INSTRUCTION_BYTES_U32 {
            previous_mask = 0;
        } else {
            for _ in 0..offset {
                previous_mask &= PREVIOUS_MASK_KEEP;
                previous_mask <<= 1;
            }
        }
        let high = buffer[position + OPERAND_HIGH_BYTE_OFFSET];
        let index = previous_mask >> 1;
        if is_far_byte(high) && index <= PREVIOUS_INDEX_MAX && index != PREVIOUS_INDEX_REJECTED {
            let operand_start = position + OPERAND_OFFSET_BYTES;
            let operand = (&mut buffer[operand_start..position + INSTRUCTION_BYTES])
                .try_into()
                .expect("an x86 operand is four bytes");
            convert(
                direction,
                previous_mask,
                operand,
                wrapping_position(current, INSTRUCTION_BYTES),
            );
            position += INSTRUCTION_BYTES;
            previous_mask = 0;
        } else {
            position += 1;
            previous_mask |= PREVIOUS_MASK_SEEN;
            if is_far_byte(high) {
                previous_mask |= PREVIOUS_MASK_SEEN_FAR;
            }
        }
    }
    state.previous_mask = previous_mask;
    state.previous_position = previous_position;
    position
}
