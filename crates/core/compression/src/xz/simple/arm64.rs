use super::{Direction, translate, wrapping_position};

pub(super) const UNFILTERED_BYTES_MAX: usize = 4;
pub(super) const ALIGNMENT_BYTES: u32 = 4;
const INSTRUCTION_BYTES: usize = 4;
const OPCODE_SHIFT: u32 = 26;
const OPCODE_BRANCH_LINK: u32 = 0x25;
const BRANCH_LINK_BASE: u32 = 0x9400_0000;
const BRANCH_OFFSET_MASK: u32 = 0x03FF_FFFF;
const BRANCH_PC_SHIFT: u32 = 2;
const ADRP_MASK: u32 = 0x9F00_0000;
const ADRP_OPCODE: u32 = 0x9000_0000;
const ADRP_IMMEDIATE_LOW_SHIFT: u32 = 29;
const ADRP_IMMEDIATE_LOW_MASK: u32 = 0x03;
const ADRP_IMMEDIATE_HIGH_SHIFT: u32 = 3;
const ADRP_IMMEDIATE_HIGH_MASK: u32 = 0x001F_FFFC;
const ADRP_RANGE_BIAS: u32 = 0x0002_0000;
const ADRP_RANGE_MASK: u32 = 0x001C_0000;
const ADRP_KEEP_MASK: u32 = 0x9000_001F;
const ADRP_PC_SHIFT: u32 = 12;
const ADRP_CONVERTED_HIGH_MASK: u32 = 0x0003_FFFC;
const ADRP_SIGN_BIT: u32 = 0x0002_0000;
const ADRP_SIGN_EXTENSION_MASK: u32 = 0x00E0_0000;

const fn convert_branch_link(direction: Direction, instruction: u32, position: u32) -> u32 {
    let program_counter = position >> BRANCH_PC_SHIFT;
    BRANCH_LINK_BASE | (translate(direction, instruction, program_counter) & BRANCH_OFFSET_MASK)
}

const fn convert_adrp(direction: Direction, instruction: u32, position: u32) -> Option<u32> {
    let source = (instruction >> ADRP_IMMEDIATE_LOW_SHIFT & ADRP_IMMEDIATE_LOW_MASK)
        | (instruction >> ADRP_IMMEDIATE_HIGH_SHIFT & ADRP_IMMEDIATE_HIGH_MASK);
    if source.wrapping_add(ADRP_RANGE_BIAS) & ADRP_RANGE_MASK != 0 {
        return None;
    }
    let destination = translate(direction, source, position >> ADRP_PC_SHIFT);
    Some(
        (instruction & ADRP_KEEP_MASK)
            | (destination & ADRP_IMMEDIATE_LOW_MASK) << ADRP_IMMEDIATE_LOW_SHIFT
            | (destination & ADRP_CONVERTED_HIGH_MASK) << ADRP_IMMEDIATE_HIGH_SHIFT
            | (0u32.wrapping_sub(destination & ADRP_SIGN_BIT) & ADRP_SIGN_EXTENSION_MASK),
    )
}

pub(super) fn code(now_pos: u32, direction: Direction, buffer: &mut [u8]) -> usize {
    let (instructions, _) = buffer.as_chunks_mut::<INSTRUCTION_BYTES>();
    let mut position = 0;
    for instruction in instructions {
        let word = u32::from_le_bytes(*instruction);
        let program_counter = wrapping_position(now_pos, position);
        if word >> OPCODE_SHIFT == OPCODE_BRANCH_LINK {
            *instruction = convert_branch_link(direction, word, program_counter).to_le_bytes();
        } else if word & ADRP_MASK == ADRP_OPCODE
            && let Some(converted) = convert_adrp(direction, word, program_counter)
        {
            *instruction = converted.to_le_bytes();
        }
        position += INSTRUCTION_BYTES;
    }
    position
}
