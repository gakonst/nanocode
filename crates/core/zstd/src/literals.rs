use crate::{
    entropy::huff0::{
        HuffmanDTable, decompress_1x_using_dtable, decompress_4x_using_dtable,
        prefers_double_symbol_decoder, read_dtable_x1, read_dtable_x2,
    },
    error::{Error, Result},
    frame::{read_u24_le, read_u32_le},
};
#[derive(Default)]
pub(crate) struct LiteralsState {
    huffman_table: Option<HuffmanDTable>,
}
impl LiteralsState {}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CompressedLiteralsHeader {
    header_size: usize,
    regenerated_size: usize,
    compressed_size: usize,
    four_streams: bool,
}
#[derive(Clone, Copy)]
pub(crate) struct DecodedLiterals<'a> {
    padded: &'a [u8],
    len: usize,
}
impl<'a> DecodedLiterals<'a> {
    fn new(padded: &'a [u8], len: usize) -> Self {
        debug_assert!(
            padded.len() >= len + LITERAL_OVERREAD_HEADROOM,
            "literals must carry {LITERAL_OVERREAD_HEADROOM} bytes of over-read headroom"
        );
        Self { padded, len }
    }
    pub(crate) const fn padded(&self) -> &'a [u8] {
        self.padded
    }
    pub(crate) fn as_slice(&self) -> &'a [u8] {
        &self.padded[..self.len]
    }
    pub(crate) const fn len(&self) -> usize {
        self.len
    }
}
pub(crate) fn decode_literals_section_into<'a>(
    src: &'a [u8],
    state: &mut LiteralsState,
    block_size_max: usize,
    scratch: &'a mut Vec<u8>,
) -> Result<(usize, DecodedLiterals<'a>)> {
    let header0 = *src.first().ok_or(Error::UnexpectedEof)?;
    let block_type = header0 & 0x3;
    let size_format = (header0 >> 2) & 0x3;
    match block_type {
        0 => decode_raw_literals_into(src, size_format, block_size_max, scratch),
        1 => decode_rle_literals_into(src, size_format, block_size_max, scratch),
        2 | 3 => decode_huffman_literals_into(
            src,
            block_type,
            size_format,
            state,
            block_size_max,
            scratch,
        ),
        _ => unreachable!(),
    }
}
const LITERAL_OVERREAD_HEADROOM: usize = 16;
fn decode_raw_literals_into<'a>(
    src: &'a [u8],
    size_format: u8,
    block_size_max: usize,
    scratch: &'a mut Vec<u8>,
) -> Result<(usize, DecodedLiterals<'a>)> {
    let (header_size, regenerated_size) = decode_raw_or_rle_size(src, size_format)?;
    ensure_regenerated_size(regenerated_size, block_size_max)?;
    let end = header_size
        .checked_add(regenerated_size)
        .ok_or(Error::OutputSizeOverflow)?;
    if end > src.len() {
        return Err(Error::UnexpectedEof);
    }
    if end + LITERAL_OVERREAD_HEADROOM <= src.len() {
        return Ok((
            end,
            DecodedLiterals::new(
                &src[header_size..end + LITERAL_OVERREAD_HEADROOM],
                regenerated_size,
            ),
        ));
    }
    let raw_literals = &src[header_size..end];
    scratch.clear();
    scratch.extend_from_slice(raw_literals);
    scratch.resize(regenerated_size + LITERAL_OVERREAD_HEADROOM, 0);
    Ok((end, DecodedLiterals::new(scratch, regenerated_size)))
}
fn decode_rle_literals_into<'a>(
    src: &[u8],
    size_format: u8,
    block_size_max: usize,
    scratch: &'a mut Vec<u8>,
) -> Result<(usize, DecodedLiterals<'a>)> {
    let (header_size, regenerated_size) = decode_raw_or_rle_size(src, size_format)?;
    ensure_regenerated_size(regenerated_size, block_size_max)?;
    let byte = *src.get(header_size).ok_or(Error::UnexpectedEof)?;
    scratch.clear();
    scratch.resize(regenerated_size, byte);
    scratch.resize(regenerated_size + LITERAL_OVERREAD_HEADROOM, 0);
    Ok((
        header_size + 1,
        DecodedLiterals::new(scratch, regenerated_size),
    ))
}
fn decode_huffman_literals_into<'a>(
    src: &[u8],
    block_type: u8,
    size_format: u8,
    state: &mut LiteralsState,
    block_size_max: usize,
    scratch: &'a mut Vec<u8>,
) -> Result<(usize, DecodedLiterals<'a>)> {
    let header = decode_compressed_literals_header(src, size_format)?;
    ensure_regenerated_size(header.regenerated_size, block_size_max)?;
    let payload_end = header
        .header_size
        .checked_add(header.compressed_size)
        .ok_or(Error::OutputSizeOverflow)?;
    let payload = src
        .get(header.header_size..payload_end)
        .ok_or(Error::UnexpectedEof)?;
    let table_size = if block_type == 2 {
        let result = if header.four_streams
            && prefers_double_symbol_decoder(header.regenerated_size, payload.len())
        {
            read_dtable_x2(
                payload,
                HuffmanDTable::double_slot(&mut state.huffman_table),
            )
        } else {
            read_dtable_x1(
                payload,
                HuffmanDTable::single_slot(&mut state.huffman_table),
            )
        };
        match result {
            Ok(table_size) => table_size,
            Err(error) => {
                state.huffman_table = None;
                return Err(error);
            }
        }
    } else {
        0
    };
    let dtable = state.huffman_table.as_ref().ok_or(Error::Corruption(
        "treeless literals require a prior Huff0 table",
    ))?;
    let streams = payload.get(table_size..).ok_or(Error::UnexpectedEof)?;
    if streams.is_empty() {
        return Err(Error::Corruption(
            "compressed literals payload is missing Huff0 streams",
        ));
    }
    let padded_size = header
        .regenerated_size
        .checked_add(LITERAL_OVERREAD_HEADROOM)
        .ok_or(Error::OutputSizeOverflow)?;
    if scratch.len() < padded_size {
        scratch.resize(padded_size, 0);
    }
    let destination = &mut scratch[..header.regenerated_size];
    if header.four_streams {
        decompress_4x_using_dtable(destination, streams, dtable)?;
    } else {
        decompress_1x_using_dtable(destination, streams, dtable)?;
    }
    Ok((
        payload_end,
        DecodedLiterals::new(scratch, header.regenerated_size),
    ))
}
fn decode_raw_or_rle_size(src: &[u8], size_format: u8) -> Result<(usize, usize)> {
    match size_format {
        0 | 2 => {
            let header0 = *src.first().ok_or(Error::UnexpectedEof)?;
            Ok((1, (header0 >> 3) as usize))
        }
        1 => {
            let header = src.get(..2).ok_or(Error::UnexpectedEof)?;
            let regenerated_size = ((header[0] as usize) >> 4) | ((header[1] as usize) << 4);
            Ok((2, regenerated_size))
        }
        3 => {
            let header = src.get(..3).ok_or(Error::UnexpectedEof)?;
            let regenerated_size = (read_u24_le(header) as usize) >> 4;
            Ok((3, regenerated_size))
        }
        _ => unreachable!(),
    }
}
fn decode_compressed_literals_header(
    src: &[u8],
    size_format: u8,
) -> Result<CompressedLiteralsHeader> {
    let (header_size, regenerated_size, compressed_size, four_streams) = match size_format {
        0 | 1 => {
            let header = src.get(..3).ok_or(Error::UnexpectedEof)?;
            let value = read_u24_le(header) as usize;
            (
                3,
                (value >> 4) & 0x03ff,
                (value >> 14) & 0x03ff,
                size_format == 1,
            )
        }
        2 => {
            let header = src.get(..4).ok_or(Error::UnexpectedEof)?;
            let value = read_u32_le(header) as usize;
            (4, (value >> 4) & 0x3fff, (value >> 18) & 0x3fff, true)
        }
        3 => {
            let header = src.get(..5).ok_or(Error::UnexpectedEof)?;
            let value = (header[0] as u64)
                | ((header[1] as u64) << 8)
                | ((header[2] as u64) << 16)
                | ((header[3] as u64) << 24)
                | ((header[4] as u64) << 32);
            (
                5,
                ((value >> 4) & 0x3ffff) as usize,
                ((value >> 22) & 0x3ffff) as usize,
                true,
            )
        }
        _ => unreachable!(),
    };
    if compressed_size == 0 {
        return Err(Error::Corruption(
            "compressed literals section has an empty payload",
        ));
    }
    if four_streams && compressed_size < 10 {
        return Err(Error::Corruption(
            "4-stream literals section is smaller than the jump-table minimum",
        ));
    }
    Ok(CompressedLiteralsHeader {
        header_size,
        regenerated_size,
        compressed_size,
        four_streams,
    })
}
const fn ensure_regenerated_size(regenerated_size: usize, block_size_max: usize) -> Result<()> {
    if regenerated_size > block_size_max {
        return Err(Error::Corruption(
            "literals section exceeds the frame block size limit",
        ));
    }
    Ok(())
}
