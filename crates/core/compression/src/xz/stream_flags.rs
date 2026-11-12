use super::check::CHECK_ID_MAX;
use super::crc32;
use super::stream::Error;

pub(crate) const STREAM_HEADER_BYTES: usize = 12;
pub(crate) const HEADER_MAGIC: [u8; 6] = [0xFD, b'7', b'z', b'X', b'Z', 0];
const FOOTER_MAGIC: [u8; 2] = *b"YZ";
const MAGIC_BYTES: usize = 6;
const FLAGS_BYTES: usize = 2;
const CRC_BYTES: usize = 4;
const BACKWARD_SIZE_OFFSET: usize = 4;
const FOOTER_FLAGS_OFFSET: usize = 8;
const FOOTER_MAGIC_OFFSET: usize = 10;
const FLAGS_RESERVED_MASK: u8 = 0xF0;
const CHECK_ID_MASK: u8 = 0x0F;
const BACKWARD_SIZE_UNIT_BYTES: u64 = 4;
pub(crate) const BACKWARD_SIZE_MIN: u64 = 4;
pub(crate) const BACKWARD_SIZE_MAX: u64 = 1 << 34;

pub(crate) struct StreamFlags {
    pub(crate) check_id: u32,
    pub(crate) backward_size: u64,
}

fn decode_flags(flags: [u8; FLAGS_BYTES]) -> Result<u32, Error> {
    if flags[0] != 0 || flags[1] & FLAGS_RESERVED_MASK != 0 {
        return Err(Error::Options);
    }
    Ok(u32::from(flags[1] & CHECK_ID_MASK))
}

fn read_u32(buffer: &[u8], offset: usize) -> u32 {
    let mut bytes = [0; CRC_BYTES];
    bytes.copy_from_slice(&buffer[offset..offset + CRC_BYTES]);
    u32::from_le_bytes(bytes)
}

pub(crate) fn decode_header(buffer: &[u8; STREAM_HEADER_BYTES]) -> Result<u32, Error> {
    if buffer[..MAGIC_BYTES] != HEADER_MAGIC {
        return Err(Error::Format);
    }
    let flags = &buffer[MAGIC_BYTES..MAGIC_BYTES + FLAGS_BYTES];
    if crc32::update(0, flags) != read_u32(buffer, MAGIC_BYTES + FLAGS_BYTES) {
        return Err(Error::Data);
    }
    decode_flags([flags[0], flags[1]])
}

pub(crate) fn decode_footer(buffer: &[u8; STREAM_HEADER_BYTES]) -> Result<StreamFlags, Error> {
    if buffer[FOOTER_MAGIC_OFFSET..] != FOOTER_MAGIC {
        return Err(Error::Format);
    }
    if crc32::update(0, &buffer[BACKWARD_SIZE_OFFSET..FOOTER_MAGIC_OFFSET]) != read_u32(buffer, 0) {
        return Err(Error::Data);
    }
    let check_id = decode_flags([buffer[FOOTER_FLAGS_OFFSET], buffer[FOOTER_FLAGS_OFFSET + 1]])?;
    let stored = u64::from(read_u32(buffer, BACKWARD_SIZE_OFFSET));
    Ok(StreamFlags {
        check_id,
        backward_size: (stored + 1) * BACKWARD_SIZE_UNIT_BYTES,
    })
}

pub(crate) fn encode_header(check_id: u32) -> [u8; STREAM_HEADER_BYTES] {
    assert!(check_id <= CHECK_ID_MAX);
    let mut buffer = [0; STREAM_HEADER_BYTES];
    buffer[..MAGIC_BYTES].copy_from_slice(&HEADER_MAGIC);
    buffer[MAGIC_BYTES + 1] = check_id as u8;
    let crc = crc32::update(0, &buffer[MAGIC_BYTES..MAGIC_BYTES + FLAGS_BYTES]);
    buffer[MAGIC_BYTES + FLAGS_BYTES..].copy_from_slice(&crc.to_le_bytes());
    buffer
}

pub(crate) fn encode_footer(check_id: u32, backward_size: u64) -> [u8; STREAM_HEADER_BYTES] {
    assert!(check_id <= CHECK_ID_MAX);
    assert!(backward_size >= BACKWARD_SIZE_MIN);
    assert!(backward_size <= BACKWARD_SIZE_MAX);
    assert!(backward_size.is_multiple_of(BACKWARD_SIZE_UNIT_BYTES));
    let stored = u32::try_from(backward_size / BACKWARD_SIZE_UNIT_BYTES - 1)
        .expect("a valid backward size fits in 32 bits");
    let mut buffer = [0; STREAM_HEADER_BYTES];
    buffer[BACKWARD_SIZE_OFFSET..FOOTER_FLAGS_OFFSET].copy_from_slice(&stored.to_le_bytes());
    buffer[FOOTER_FLAGS_OFFSET + 1] = check_id as u8;
    let crc = crc32::update(0, &buffer[BACKWARD_SIZE_OFFSET..FOOTER_MAGIC_OFFSET]);
    buffer[..CRC_BYTES].copy_from_slice(&crc.to_le_bytes());
    buffer[FOOTER_MAGIC_OFFSET..].copy_from_slice(&FOOTER_MAGIC);
    buffer
}
