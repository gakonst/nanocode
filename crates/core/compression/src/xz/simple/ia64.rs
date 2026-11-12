use super::{Direction, translate, wrapping_position};

pub(super) const UNFILTERED_BYTES_MAX: usize = 16;
pub(super) const ALIGNMENT_BYTES: u32 = 16;
const BUNDLE_BYTES: usize = 16;
const TEMPLATE_MASK: u8 = 0x1F;
const BRANCH_TABLE: [u32; 32] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 4, 4, 6, 6, 0, 0, 7, 7, 4, 4, 0, 0, 4, 4, 0, 0,
];
const SLOTS: u32 = 3;
const TEMPLATE_BITS: u32 = 5;
const SLOT_BITS: u32 = 41;
const BYTE_BITS: u32 = 8;
const BIT_IN_BYTE_MASK: u32 = 0x07;
const INSTRUCTION_BYTES: usize = 6;
const WORD_BYTES: usize = 8;
const OPCODE_SHIFT: u32 = 37;
const OPCODE_MASK: u64 = 0x0F;
const OPCODE_BRANCH: u64 = 0x05;
const BTYPE_SHIFT: u32 = 9;
const BTYPE_MASK: u64 = 0x07;
const IMMEDIATE_SHIFT: u32 = 13;
const IMMEDIATE_MASK: u64 = 0x000F_FFFF;
const SIGN_SHIFT: u32 = 36;
const SIGN_POSITION: u32 = 20;
const ADDRESS_SHIFT: u32 = 4;
const IMMEDIATE_CLEAR_MASK: u64 = 0x008F_FFFF;
const DESTINATION_IMMEDIATE_MASK: u32 = 0x000F_FFFF;
const DESTINATION_SIGN_MASK: u32 = 0x0010_0000;

fn convert_slot(direction: Direction, bundle: &mut [u8], bit_position: u32, position: u32) {
    let byte_position = (bit_position / BYTE_BITS) as usize;
    let bit_shift = bit_position & BIT_IN_BYTE_MASK;
    let field = &mut bundle[byte_position..byte_position + INSTRUCTION_BYTES];
    let mut word = [0u8; WORD_BYTES];
    word[..INSTRUCTION_BYTES].copy_from_slice(field);
    let mut instruction = u64::from_le_bytes(word);
    let mut normalized = instruction >> bit_shift;
    if normalized >> OPCODE_SHIFT & OPCODE_MASK != OPCODE_BRANCH
        || normalized >> BTYPE_SHIFT & BTYPE_MASK != 0
    {
        return;
    }
    let source = (((normalized >> IMMEDIATE_SHIFT & IMMEDIATE_MASK)
        | (normalized >> SIGN_SHIFT & 1) << SIGN_POSITION) as u32)
        << ADDRESS_SHIFT;
    let destination = translate(direction, source, position) >> ADDRESS_SHIFT;
    normalized &= !(IMMEDIATE_CLEAR_MASK << IMMEDIATE_SHIFT);
    normalized |= u64::from(destination & DESTINATION_IMMEDIATE_MASK) << IMMEDIATE_SHIFT;
    normalized |= u64::from(destination & DESTINATION_SIGN_MASK) << (SIGN_SHIFT - SIGN_POSITION);
    instruction &= (1u64 << bit_shift) - 1;
    instruction |= normalized << bit_shift;
    field.copy_from_slice(&instruction.to_le_bytes()[..INSTRUCTION_BYTES]);
}

pub(super) fn code(now_pos: u32, direction: Direction, buffer: &mut [u8]) -> usize {
    let (bundles, _) = buffer.as_chunks_mut::<BUNDLE_BYTES>();
    let mut position = 0;
    for bundle in bundles {
        let mask = BRANCH_TABLE[usize::from(bundle[0] & TEMPLATE_MASK)];
        let bundle_position = wrapping_position(now_pos, position);
        for slot in 0..SLOTS {
            if mask >> slot & 1 != 0 {
                let bit_position = TEMPLATE_BITS + slot * SLOT_BITS;
                convert_slot(direction, bundle, bit_position, bundle_position);
            }
        }
        position += BUNDLE_BYTES;
    }
    position
}
