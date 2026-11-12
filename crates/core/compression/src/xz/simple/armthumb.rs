use super::{Direction, translate, wrapping_position};

pub(super) const UNFILTERED_BYTES_MAX: usize = 4;
pub(super) const ALIGNMENT_BYTES: u32 = 2;
const INSTRUCTION_BYTES: usize = 4;
const HALFWORD_BYTES: usize = 2;
const PREFIX_MASK: u8 = 0xF8;
const PREFIX_HIGH: u8 = 0xF0;
const PREFIX_LOW: u8 = 0xF8;
const OFFSET_PART_MASK: u32 = 0x07;
const HIGH_PART_SHIFT: u32 = 19;
const MIDDLE_BYTE_SHIFT: u32 = 11;
const LOW_PART_SHIFT: u32 = 8;
const ADDRESS_SHIFT: u32 = 1;
const PIPELINE_OFFSET_BYTES: usize = 4;

pub(super) fn code(now_pos: u32, direction: Direction, buffer: &mut [u8]) -> usize {
    if buffer.len() < INSTRUCTION_BYTES {
        return 0;
    }
    let limit = buffer.len() - INSTRUCTION_BYTES;
    let mut position = 0;
    while position <= limit {
        let instruction = &mut buffer[position..position + INSTRUCTION_BYTES];
        if instruction[1] & PREFIX_MASK == PREFIX_HIGH && instruction[3] & PREFIX_MASK == PREFIX_LOW
        {
            let source = ((u32::from(instruction[1]) & OFFSET_PART_MASK) << HIGH_PART_SHIFT
                | u32::from(instruction[0]) << MIDDLE_BYTE_SHIFT
                | (u32::from(instruction[3]) & OFFSET_PART_MASK) << LOW_PART_SHIFT
                | u32::from(instruction[2]))
                << ADDRESS_SHIFT;
            let program_counter = wrapping_position(now_pos, position + PIPELINE_OFFSET_BYTES);
            let destination = translate(direction, source, program_counter) >> ADDRESS_SHIFT;
            instruction[1] = (u32::from(PREFIX_HIGH)
                | (destination >> HIGH_PART_SHIFT & OFFSET_PART_MASK))
                as u8;
            instruction[0] = (destination >> MIDDLE_BYTE_SHIFT) as u8;
            instruction[3] =
                (u32::from(PREFIX_LOW) | (destination >> LOW_PART_SHIFT & OFFSET_PART_MASK)) as u8;
            instruction[2] = destination as u8;
            position += HALFWORD_BYTES;
        }
        position += HALFWORD_BYTES;
    }
    position
}
