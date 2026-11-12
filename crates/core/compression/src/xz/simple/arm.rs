use super::{Direction, translate, wrapping_position};

pub(super) const UNFILTERED_BYTES_MAX: usize = 4;
pub(super) const ALIGNMENT_BYTES: u32 = 4;
const INSTRUCTION_BYTES: usize = 4;
const OPCODE_BRANCH_LINK: u8 = 0xEB;
const PIPELINE_OFFSET_BYTES: usize = 8;
const ADDRESS_SHIFT: u32 = 2;
const OFFSET_BYTES: usize = 3;

pub(super) fn code(now_pos: u32, direction: Direction, buffer: &mut [u8]) -> usize {
    let (instructions, _) = buffer.as_chunks_mut::<INSTRUCTION_BYTES>();
    let mut position = 0;
    for instruction in instructions {
        if instruction[OFFSET_BYTES] == OPCODE_BRANCH_LINK {
            let mut offset = *instruction;
            offset[OFFSET_BYTES] = 0;
            let source = u32::from_le_bytes(offset) << ADDRESS_SHIFT;
            let program_counter = wrapping_position(now_pos, position + PIPELINE_OFFSET_BYTES);
            let destination = translate(direction, source, program_counter) >> ADDRESS_SHIFT;
            let bytes = destination.to_le_bytes();
            instruction[..OFFSET_BYTES].copy_from_slice(&bytes[..OFFSET_BYTES]);
        }
        position += INSTRUCTION_BYTES;
    }
    position
}
