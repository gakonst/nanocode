use super::{Direction, translate, wrapping_position};

pub(super) const UNFILTERED_BYTES_MAX: usize = 4;
pub(super) const ALIGNMENT_BYTES: u32 = 4;
const INSTRUCTION_BYTES: usize = 4;
const CALL_POSITIVE_BYTE: u8 = 0x40;
const CALL_NEGATIVE_BYTE: u8 = 0x7F;
const DISPLACEMENT_TOP_MASK: u8 = 0xC0;
const ADDRESS_SHIFT: u32 = 2;
const SIGN_BIT_SHIFT: u32 = 22;
const SIGN_EXTENSION_MASK: u32 = 0x3FFF_FFFF;
const DISPLACEMENT_MASK: u32 = 0x003F_FFFF;
const OPCODE_CALL: u32 = 0x4000_0000;

pub(super) fn code(now_pos: u32, direction: Direction, buffer: &mut [u8]) -> usize {
    let (instructions, _) = buffer.as_chunks_mut::<INSTRUCTION_BYTES>();
    let mut position = 0;
    for instruction in instructions {
        let first = instruction[0];
        let second = instruction[1];
        if (first == CALL_POSITIVE_BYTE && second & DISPLACEMENT_TOP_MASK == 0)
            || (first == CALL_NEGATIVE_BYTE
                && second & DISPLACEMENT_TOP_MASK == DISPLACEMENT_TOP_MASK)
        {
            let source = u32::from_be_bytes(*instruction) << ADDRESS_SHIFT;
            let destination =
                translate(direction, source, wrapping_position(now_pos, position)) >> ADDRESS_SHIFT;
            let sign = 0u32.wrapping_sub(destination >> SIGN_BIT_SHIFT & 1);
            let converted = (sign << SIGN_BIT_SHIFT & SIGN_EXTENSION_MASK)
                | (destination & DISPLACEMENT_MASK)
                | OPCODE_CALL;
            *instruction = converted.to_be_bytes();
        }
        position += INSTRUCTION_BYTES;
    }
    position
}
