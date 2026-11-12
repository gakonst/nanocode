use super::{Direction, translate, wrapping_position};

pub(super) const UNFILTERED_BYTES_MAX: usize = 4;
pub(super) const ALIGNMENT_BYTES: u32 = 4;
const INSTRUCTION_BYTES: usize = 4;
const OPCODE_SHIFT: u32 = 2;
const OPCODE_BRANCH: u8 = 0x12;
const FLAGS_MASK: u8 = 0x03;
const FLAGS_LINK: u8 = 0x01;
const OPCODE_BRANCH_BYTE: u32 = 0x48;
const ADDRESS_HIGH_MASK: u32 = 0x03;
const ADDRESS_HIGH_SHIFT: u32 = 24;

pub(super) fn code(now_pos: u32, direction: Direction, buffer: &mut [u8]) -> usize {
    let (instructions, _) = buffer.as_chunks_mut::<INSTRUCTION_BYTES>();
    let mut position = 0;
    for instruction in instructions {
        let first = instruction[0];
        let last = instruction[3];
        if first >> OPCODE_SHIFT == OPCODE_BRANCH && last & FLAGS_MASK == FLAGS_LINK {
            let source = u32::from_be_bytes([
                first & FLAGS_MASK,
                instruction[1],
                instruction[2],
                last & !FLAGS_MASK,
            ]);
            let destination = translate(direction, source, wrapping_position(now_pos, position));
            let bytes = destination.to_be_bytes();
            instruction[0] = (OPCODE_BRANCH_BYTE
                | (destination >> ADDRESS_HIGH_SHIFT & ADDRESS_HIGH_MASK))
                as u8;
            instruction[1] = bytes[1];
            instruction[2] = bytes[2];
            instruction[3] = (last & FLAGS_MASK) | bytes[3];
        }
        position += INSTRUCTION_BYTES;
    }
    position
}
