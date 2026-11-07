use crate::{
    block::BLOCK_SIZE_MAX,
    error::{Error, Result},
};
pub const ZSTD_MAGIC_NUMBER: u32 = 0xFD2F_B528;
pub const SKIPPABLE_MAGIC_BASE: u32 = 0x184D_2A50;
const RESERVED_BIT: u8 = 1 << 3;
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Format {
    #[default]
    Zstd1,
    Zstd1Magicless,
}
impl Format {
    pub(crate) const fn magic_size(self) -> usize {
        match self {
            Self::Zstd1 => 4,
            Self::Zstd1Magicless => 0,
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameHeader {
    Zstandard(ZstandardFrameHeader),
    Skippable(SkippableFrame),
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ZstandardFrameHeader {
    pub header_size: usize,
    pub window_size: u64,
    pub block_size_max: u32,
    pub content_size: Option<u64>,
    pub dictionary_id: Option<u32>,
    pub checksum: bool,
    pub single_segment: bool,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SkippableFrame {
    pub header_size: usize,
    pub magic_variant: u8,
    pub size: u32,
}
pub fn parse_frame_header(src: &[u8]) -> Result<FrameHeader> {
    parse_frame_header_with_format(src, Format::Zstd1)
}
pub fn parse_frame_header_with_format(src: &[u8], format: Format) -> Result<FrameHeader> {
    if format == Format::Zstd1Magicless {
        return parse_zstandard_frame_header(src, format);
    }
    if src.len() < 4 {
        return Err(Error::UnexpectedEof);
    }
    let magic = read_u32_le(&src[..4]);
    if magic == ZSTD_MAGIC_NUMBER {
        parse_zstandard_frame_header(src, format)
    } else if is_skippable_magic(magic) {
        parse_skippable_frame_header(src, magic)
    } else {
        Err(Error::BadMagic(magic))
    }
}
pub(crate) fn read_u24_le(src: &[u8]) -> u32 {
    debug_assert!(src.len() >= 3);
    (src[0] as u32) | ((src[1] as u32) << 8) | ((src[2] as u32) << 16)
}
pub(crate) fn read_u32_le(src: &[u8]) -> u32 {
    debug_assert!(src.len() >= 4);
    u32::from_le_bytes([src[0], src[1], src[2], src[3]])
}
fn parse_zstandard_frame_header(src: &[u8], format: Format) -> Result<FrameHeader> {
    let magic_size = format.magic_size();
    let Some(&descriptor) = src.get(magic_size) else {
        return Err(Error::UnexpectedEof);
    };
    if descriptor & RESERVED_BIT != 0 {
        return Err(Error::Corruption("frame header reserved bit is set"));
    }
    let single_segment = descriptor & (1 << 5) != 0;
    let checksum = descriptor & (1 << 2) != 0;
    let dictionary_id_size = match descriptor & 0x3 {
        0 => 0usize,
        1 => 1,
        2 => 2,
        _ => 4,
    };
    let fcs_field_size = frame_content_size_field_size(descriptor >> 6, single_segment);
    let mut pos = magic_size + 1;
    let window_size = if single_segment {
        0
    } else {
        let window_descriptor = *src.get(pos).ok_or(Error::UnexpectedEof)?;
        pos += 1;
        decode_window_descriptor(window_descriptor)
    };
    let dictionary_id = if dictionary_id_size == 0 {
        None
    } else {
        let end = pos
            .checked_add(dictionary_id_size)
            .ok_or(Error::UnexpectedEof)?;
        let field = src.get(pos..end).ok_or(Error::UnexpectedEof)?;
        pos = end;
        let value = match field.len() {
            1 => field[0] as u32,
            2 => u16::from_le_bytes([field[0], field[1]]) as u32,
            4 => u32::from_le_bytes([field[0], field[1], field[2], field[3]]),
            _ => unreachable!(),
        };
        (value != 0).then_some(value)
    };
    let content_size = if fcs_field_size == 0 {
        None
    } else {
        let end = pos
            .checked_add(fcs_field_size)
            .ok_or(Error::UnexpectedEof)?;
        let field = src.get(pos..end).ok_or(Error::UnexpectedEof)?;
        pos = end;
        Some(decode_frame_content_size(field)?)
    };
    let window_size = if single_segment {
        content_size.ok_or(Error::Corruption(
            "single-segment frames must include a content size",
        ))?
    } else {
        window_size
    };
    let block_size_max = window_size.min(BLOCK_SIZE_MAX as u64) as u32;
    Ok(FrameHeader::Zstandard(ZstandardFrameHeader {
        header_size: pos,
        window_size,
        block_size_max,
        content_size,
        dictionary_id,
        checksum,
        single_segment,
    }))
}
fn parse_skippable_frame_header(src: &[u8], magic: u32) -> Result<FrameHeader> {
    if src.len() < 8 {
        return Err(Error::UnexpectedEof);
    }
    let size = read_u32_le(&src[4..8]);
    let magic_variant = (magic - SKIPPABLE_MAGIC_BASE) as u8;
    Ok(FrameHeader::Skippable(SkippableFrame {
        header_size: 8,
        magic_variant,
        size,
    }))
}
const fn is_skippable_magic(magic: u32) -> bool {
    (magic & 0xFFFF_FFF0) == SKIPPABLE_MAGIC_BASE
}
fn frame_content_size_field_size(flag: u8, single_segment: bool) -> usize {
    match flag {
        0 => usize::from(single_segment),
        1 => 2,
        2 => 4,
        3 => 8,
        _ => unreachable!(),
    }
}
fn decode_window_descriptor(descriptor: u8) -> u64 {
    let exponent = descriptor >> 3;
    let mantissa = descriptor & 0x7;
    let window_log = 10u64 + u64::from(exponent);
    let window_base = 1u64 << window_log;
    let window_add = (window_base / 8) * u64::from(mantissa);
    window_base + window_add
}
const fn decode_frame_content_size(field: &[u8]) -> Result<u64> {
    let value = match field.len() {
        1 => field[0] as u64,
        2 => u16::from_le_bytes([field[0], field[1]]) as u64 + 256,
        4 => u32::from_le_bytes([field[0], field[1], field[2], field[3]]) as u64,
        8 => u64::from_le_bytes([
            field[0], field[1], field[2], field[3], field[4], field[5], field[6], field[7],
        ]),
        _ => return Err(Error::Corruption("invalid frame content size field length")),
    };
    Ok(value)
}
