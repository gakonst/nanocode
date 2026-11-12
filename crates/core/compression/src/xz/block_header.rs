use alloc::vec::Vec;

use super::check::check_bytes;
use super::crc32;
use super::filter_chain::FILTERS_MAX;
use super::filter_flags;
use super::options::Filter;
use super::stream::Error;
use super::vli::{self, VLI_MAX};

pub(crate) const BLOCK_HEADER_BYTES_MAX: usize = 1024;
const BLOCK_HEADER_BYTES_MIN: usize = 8;
pub(crate) const BLOCK_HEADER_UNIT_BYTES: usize = 4;
const CRC_BYTES: usize = 4;
const FLAGS_OFFSET: usize = 1;
const FIELDS_OFFSET: usize = 2;
const FLAGS_RESERVED_MASK: u8 = 0x3C;
const FLAG_COMPRESSED_SIZE: u8 = 0x40;
const FLAG_UNCOMPRESSED_SIZE: u8 = 0x80;
const FLAG_FILTER_COUNT_MASK: u8 = 0x03;
const SIZE_BYTE_BYTES: usize = 1;
const FLAGS_BYTES: usize = 1;
pub(crate) const UNPADDED_SIZE_MIN: u64 = 5;
pub(crate) const UNPADDED_SIZE_MAX: u64 = VLI_MAX & !3;

pub(crate) struct BlockHeader {
    pub(crate) header_size: usize,
    pub(crate) compressed_size: Option<u64>,
    pub(crate) uncompressed_size: Option<u64>,
    pub(crate) filters: Vec<Filter>,
}

pub(crate) const fn header_size_from_byte(byte: u8) -> usize {
    (byte as usize + 1) * BLOCK_HEADER_UNIT_BYTES
}

pub(crate) fn unpadded_size(
    header_size: usize,
    compressed_size: u64,
    check_id: u32,
) -> Option<u64> {
    assert!(header_size >= BLOCK_HEADER_BYTES_MIN);
    assert!(header_size <= BLOCK_HEADER_BYTES_MAX);
    if compressed_size == 0 || compressed_size > VLI_MAX {
        return None;
    }
    let unpadded = compressed_size + header_size as u64 + check_bytes(check_id) as u64;
    if unpadded > UNPADDED_SIZE_MAX {
        return None;
    }
    Some(unpadded)
}

fn read_u32(bytes: &[u8]) -> u32 {
    let mut word = [0; CRC_BYTES];
    word.copy_from_slice(&bytes[..CRC_BYTES]);
    u32::from_le_bytes(word)
}

pub(crate) fn decode(buffer: &[u8], check_id: u32) -> Result<BlockHeader, Error> {
    let header_size = buffer.len();
    assert_eq!(header_size_from_byte(buffer[0]), header_size);
    let fields_end = header_size - CRC_BYTES;
    if crc32::update(0, &buffer[..fields_end]) != read_u32(&buffer[fields_end..]) {
        return Err(Error::Data);
    }
    let flags = buffer[FLAGS_OFFSET];
    if flags & FLAGS_RESERVED_MASK != 0 {
        return Err(Error::Options);
    }
    let fields = &buffer[..fields_end];
    let mut position = FIELDS_OFFSET;
    let compressed_size = if flags & FLAG_COMPRESSED_SIZE != 0 {
        let size = vli::decode_whole(fields, &mut position)?;
        if unpadded_size(header_size, size, check_id).is_none() {
            return Err(Error::Data);
        }
        Some(size)
    } else {
        None
    };
    let uncompressed_size = if flags & FLAG_UNCOMPRESSED_SIZE != 0 {
        Some(vli::decode_whole(fields, &mut position)?)
    } else {
        None
    };
    let filter_count = usize::from(flags & FLAG_FILTER_COUNT_MASK) + 1;
    let mut filters = Vec::with_capacity(filter_count);
    for _ in 0..filter_count {
        filters.push(filter_flags::decode(fields, &mut position)?);
    }
    if fields[position..].iter().any(|&byte| byte != 0) {
        return Err(Error::Options);
    }
    Ok(BlockHeader {
        header_size,
        compressed_size,
        uncompressed_size,
        filters,
    })
}

pub(crate) fn encoded_size(filters: &[Filter]) -> Result<usize, Error> {
    if filters.is_empty() || filters.len() > FILTERS_MAX {
        return Err(Error::Program);
    }
    let mut fields = 0;
    for filter in filters {
        fields += filter_flags::size(filter)?;
    }
    let size = SIZE_BYTE_BYTES + FLAGS_BYTES + CRC_BYTES + fields;
    Ok(size.next_multiple_of(BLOCK_HEADER_UNIT_BYTES))
}

pub(crate) fn encode(filters: &[Filter], header_size: usize, output: &mut [u8]) {
    assert!(!filters.is_empty());
    assert!(filters.len() <= FILTERS_MAX);
    assert!(header_size <= BLOCK_HEADER_BYTES_MAX);
    let fields_end = header_size - CRC_BYTES;
    output[0] = u8::try_from(fields_end / BLOCK_HEADER_UNIT_BYTES)
        .expect("a block header size byte fits in a byte");
    output[FLAGS_OFFSET] = u8::try_from(filters.len() - 1).expect("a filter count fits in a byte");
    let mut position = FIELDS_OFFSET;
    for filter in filters {
        filter_flags::encode(filter, &mut output[..fields_end], &mut position);
    }
    output[position..fields_end].fill(0);
    let crc = crc32::update(0, &output[..fields_end]);
    output[fields_end..header_size].copy_from_slice(&crc.to_le_bytes());
}
