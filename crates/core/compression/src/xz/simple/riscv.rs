use super::wrapping_position;

pub(super) const UNFILTERED_BYTES_MAX: usize = 8;
pub(super) const ALIGNMENT_BYTES: u32 = 2;
const LOOKAHEAD_BYTES: usize = 8;
const STEP_BYTES: usize = 2;
const JAL_BYTES: usize = 4;
const PAIR_BYTES: usize = 8;
const REJECTED_PAIR_BYTES: usize = 6;
const AUIPC_BYTES: usize = 4;
const OPCODE_JAL_BYTE: u8 = 0xEF;
const JAL_RD_CHECK_MASK: u8 = 0x0D;
const OPCODE_MASK: u32 = 0x7F;
const OPCODE_AUIPC: u32 = 0x17;
const AUIPC_RD_MASK: u32 = 0x0E80;
const PAIR_CHECK_SHIFT: u32 = 8;
const PAIR_CHECK_BIAS: u32 = 3;
const PAIR_CHECK_MASK: u32 = 0x000F_8003;
const UPPER_IMMEDIATE_MASK: u32 = 0xFFFF_F000;
const LOWER_IMMEDIATE_SHIFT: u32 = 20;
const LOWER_SIGN_SHIFT: u32 = 19;
const LOWER_SIGN_ADJUST: u32 = 0x1000;
const SPECIAL_RD: u32 = 2;
const RD_SHIFT: u32 = 7;
const SECOND_INSTRUCTION_SHIFT: u32 = 12;
const FAKE_RS1_SHIFT: u32 = 27;
const FAKE_AUIPC_BASE: u32 = 0x3117;
const FAKE_CHECK_SHIFT: u32 = 18;
const FAKE_RS1_MASK: u32 = 0x1D;
const ROUNDING_BIAS: u32 = 0x800;
const JAL_RD_KEEP_MASK: u32 = 0x0F;
const JAL_HIGH_NIBBLE_MASK: u32 = 0xF0;
const JAL_ENCODE_BYTE1_SHIFT: u32 = 8;
const JAL_ENCODE_BYTE2_LOW_MASK: u32 = 0x0F;
const JAL_ENCODE_BYTE2_LOW_SHIFT: u32 = 16;
const JAL_ENCODE_BYTE2_BIT_MASK: u32 = 0x10;
const JAL_ENCODE_BYTE2_BIT_SHIFT: u32 = 7;
const JAL_ENCODE_BYTE2_HIGH_MASK: u32 = 0xE0;
const JAL_ENCODE_BYTE2_HIGH_SHIFT: u32 = 4;
const JAL_ENCODE_BYTE3_LOW_MASK: u32 = 0x7F;
const JAL_ENCODE_BYTE3_LOW_SHIFT: u32 = 4;
const JAL_ENCODE_BYTE3_SIGN_MASK: u32 = 0x80;
const JAL_ENCODE_BYTE3_SIGN_SHIFT: u32 = 13;
const JAL_ADDRESS_BYTE1_SHIFT: u32 = 13;
const JAL_ADDRESS_BYTE2_SHIFT: u32 = 9;
const JAL_ADDRESS_BYTE3_SHIFT: u32 = 1;

fn read_le(buffer: &[u8], position: usize) -> u32 {
    u32::from_le_bytes(
        buffer[position..position + AUIPC_BYTES]
            .try_into()
            .expect("a RISC-V word is four bytes"),
    )
}

fn read_be(buffer: &[u8], position: usize) -> u32 {
    u32::from_be_bytes(
        buffer[position..position + AUIPC_BYTES]
            .try_into()
            .expect("a RISC-V word is four bytes"),
    )
}

fn write(buffer: &mut [u8], position: usize, bytes: [u8; AUIPC_BYTES]) {
    buffer[position..position + AUIPC_BYTES].copy_from_slice(&bytes);
}

fn encode_jal(buffer: &mut [u8], position: usize, program_counter: u32) {
    let byte1 = u32::from(buffer[position + 1]);
    let byte2 = u32::from(buffer[position + 2]);
    let byte3 = u32::from(buffer[position + 3]);
    let address = ((byte1 & JAL_HIGH_NIBBLE_MASK) << JAL_ENCODE_BYTE1_SHIFT
        | (byte2 & JAL_ENCODE_BYTE2_LOW_MASK) << JAL_ENCODE_BYTE2_LOW_SHIFT
        | (byte2 & JAL_ENCODE_BYTE2_BIT_MASK) << JAL_ENCODE_BYTE2_BIT_SHIFT
        | (byte2 & JAL_ENCODE_BYTE2_HIGH_MASK) >> JAL_ENCODE_BYTE2_HIGH_SHIFT
        | (byte3 & JAL_ENCODE_BYTE3_LOW_MASK) << JAL_ENCODE_BYTE3_LOW_SHIFT
        | (byte3 & JAL_ENCODE_BYTE3_SIGN_MASK) << JAL_ENCODE_BYTE3_SIGN_SHIFT)
        .wrapping_add(program_counter);
    buffer[position + 1] = (byte1 & JAL_RD_KEEP_MASK
        | (address >> JAL_ADDRESS_BYTE1_SHIFT & JAL_HIGH_NIBBLE_MASK))
        as u8;
    buffer[position + 2] = (address >> JAL_ADDRESS_BYTE2_SHIFT) as u8;
    buffer[position + 3] = (address >> JAL_ADDRESS_BYTE3_SHIFT) as u8;
}

fn decode_jal(buffer: &mut [u8], position: usize, program_counter: u32) {
    let byte1 = u32::from(buffer[position + 1]);
    let byte2 = u32::from(buffer[position + 2]);
    let byte3 = u32::from(buffer[position + 3]);
    let address = ((byte1 & JAL_HIGH_NIBBLE_MASK) << JAL_ADDRESS_BYTE1_SHIFT
        | byte2 << JAL_ADDRESS_BYTE2_SHIFT
        | byte3 << JAL_ADDRESS_BYTE3_SHIFT)
        .wrapping_sub(program_counter);
    buffer[position + 1] = (byte1 & JAL_RD_KEEP_MASK
        | (address >> JAL_ENCODE_BYTE1_SHIFT & JAL_HIGH_NIBBLE_MASK))
        as u8;
    buffer[position + 2] = (address >> JAL_ENCODE_BYTE2_LOW_SHIFT & JAL_ENCODE_BYTE2_LOW_MASK
        | address >> JAL_ENCODE_BYTE2_BIT_SHIFT & JAL_ENCODE_BYTE2_BIT_MASK
        | address << JAL_ENCODE_BYTE2_HIGH_SHIFT & JAL_ENCODE_BYTE2_HIGH_MASK)
        as u8;
    buffer[position + 3] = (address >> JAL_ENCODE_BYTE3_LOW_SHIFT & JAL_ENCODE_BYTE3_LOW_MASK
        | address >> JAL_ENCODE_BYTE3_SIGN_SHIFT & JAL_ENCODE_BYTE3_SIGN_MASK)
        as u8;
}

const fn pair_matches(instruction: u32, second: u32) -> bool {
    ((instruction << PAIR_CHECK_SHIFT) ^ second.wrapping_sub(PAIR_CHECK_BIAS)) & PAIR_CHECK_MASK
        == 0
}

const fn fake_pair(instruction: u32) -> Option<u32> {
    let fake_rs1 = instruction >> FAKE_RS1_SHIFT;
    if instruction.wrapping_sub(FAKE_AUIPC_BASE) << FAKE_CHECK_SHIFT >= fake_rs1 & FAKE_RS1_MASK {
        None
    } else {
        Some(fake_rs1)
    }
}

fn encode_auipc(buffer: &mut [u8], position: usize, now_pos: u32) -> usize {
    let instruction = read_le(buffer, position);
    let second = read_le(buffer, position + AUIPC_BYTES);
    if instruction & AUIPC_RD_MASK != 0 {
        if !pair_matches(instruction, second) {
            return REJECTED_PAIR_BYTES;
        }
        let address = (instruction & UPPER_IMMEDIATE_MASK)
            .wrapping_add(
                (second >> LOWER_IMMEDIATE_SHIFT)
                    .wrapping_sub(second >> LOWER_SIGN_SHIFT & LOWER_SIGN_ADJUST),
            )
            .wrapping_add(wrapping_position(now_pos, position));
        let converted = OPCODE_AUIPC | SPECIAL_RD << RD_SHIFT | second << SECOND_INSTRUCTION_SHIFT;
        write(buffer, position, converted.to_le_bytes());
        write(buffer, position + AUIPC_BYTES, address.to_be_bytes());
        return PAIR_BYTES;
    }
    let Some(fake_rs1) = fake_pair(instruction) else {
        return AUIPC_BYTES;
    };
    let fake_address = second;
    let fake_second =
        instruction >> SECOND_INSTRUCTION_SHIFT | fake_address << LOWER_IMMEDIATE_SHIFT;
    let converted = OPCODE_AUIPC | fake_rs1 << RD_SHIFT | (fake_address & UPPER_IMMEDIATE_MASK);
    write(buffer, position, converted.to_le_bytes());
    write(buffer, position + AUIPC_BYTES, fake_second.to_le_bytes());
    PAIR_BYTES
}

fn decode_auipc(buffer: &mut [u8], position: usize, now_pos: u32) -> usize {
    let instruction = read_le(buffer, position);
    if instruction & AUIPC_RD_MASK != 0 {
        let second = read_le(buffer, position + AUIPC_BYTES);
        if !pair_matches(instruction, second) {
            return REJECTED_PAIR_BYTES;
        }
        let address =
            (instruction & UPPER_IMMEDIATE_MASK).wrapping_add(second >> LOWER_IMMEDIATE_SHIFT);
        let converted = OPCODE_AUIPC | SPECIAL_RD << RD_SHIFT | second << SECOND_INSTRUCTION_SHIFT;
        write(buffer, position, converted.to_le_bytes());
        write(buffer, position + AUIPC_BYTES, address.to_le_bytes());
        return PAIR_BYTES;
    }
    let Some(second_rs1) = fake_pair(instruction) else {
        return AUIPC_BYTES;
    };
    let address =
        read_be(buffer, position + AUIPC_BYTES).wrapping_sub(wrapping_position(now_pos, position));
    let second = instruction >> SECOND_INSTRUCTION_SHIFT | address << LOWER_IMMEDIATE_SHIFT;
    let converted = OPCODE_AUIPC
        | second_rs1 << RD_SHIFT
        | (address.wrapping_add(ROUNDING_BIAS) & UPPER_IMMEDIATE_MASK);
    write(buffer, position, converted.to_le_bytes());
    write(buffer, position + AUIPC_BYTES, second.to_le_bytes());
    PAIR_BYTES
}

fn code(
    now_pos: u32,
    buffer: &mut [u8],
    jal: impl Fn(&mut [u8], usize, u32),
    auipc: impl Fn(&mut [u8], usize, u32) -> usize,
) -> usize {
    if buffer.len() < LOOKAHEAD_BYTES {
        return 0;
    }
    let limit = buffer.len() - LOOKAHEAD_BYTES;
    let mut position = 0;
    while position <= limit {
        let first = buffer[position];
        if first == OPCODE_JAL_BYTE {
            if buffer[position + 1] & JAL_RD_CHECK_MASK == 0 {
                jal(buffer, position, wrapping_position(now_pos, position));
                position += JAL_BYTES;
            } else {
                position += STEP_BYTES;
            }
        } else if u32::from(first) & OPCODE_MASK == OPCODE_AUIPC {
            position += auipc(buffer, position, now_pos);
        } else {
            position += STEP_BYTES;
        }
    }
    position
}

pub(super) fn encode(now_pos: u32, buffer: &mut [u8]) -> usize {
    code(now_pos, buffer, encode_jal, encode_auipc)
}

pub(super) fn decode(now_pos: u32, buffer: &mut [u8]) -> usize {
    code(now_pos, buffer, decode_jal, decode_auipc)
}
