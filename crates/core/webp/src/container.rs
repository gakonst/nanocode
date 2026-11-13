use core::ops::Range;

use crate::error::WebpError;
use crate::stream::Cursor;

pub(crate) const FOURCC_RIFF: [u8; 4] = *b"RIFF";
pub(crate) const FOURCC_WEBP: [u8; 4] = *b"WEBP";
pub(crate) const FOURCC_LOSSY: [u8; 4] = *b"VP8 ";
pub(crate) const FOURCC_LOSSLESS: [u8; 4] = *b"VP8L";
pub(crate) const FOURCC_EXTENDED: [u8; 4] = *b"VP8X";
pub(crate) const FOURCC_ANIMATION: [u8; 4] = *b"ANIM";
pub(crate) const FOURCC_FRAME: [u8; 4] = *b"ANMF";
pub(crate) const FOURCC_ALPHA: [u8; 4] = *b"ALPH";
pub(crate) const FOURCC_ICC_PROFILE: [u8; 4] = *b"ICCP";
pub(crate) const FOURCC_EXIF: [u8; 4] = *b"EXIF";
pub(crate) const FOURCC_XMP: [u8; 4] = *b"XMP ";

pub(crate) const CHUNK_HEADER_BYTES: u64 = 8;
pub(crate) const LOSSY_MAGIC: [u8; 3] = [0x9d, 0x01, 0x2a];
pub(crate) const LOSSY_DIMENSION_MASK: u16 = 0x3fff;
pub(crate) const LOSSLESS_SIGNATURE: u8 = 0x2f;

const RIFF_HEADER_BYTES: u64 = 12;
const LOSSLESS_DIMENSION_MASK: u32 = 0x3fff;
const LOSSLESS_DIMENSION_BITS: u32 = 14;
const LOSSLESS_ALPHA_BIT: u32 = 28;
const LOSSLESS_VERSION_SHIFT: u32 = 29;
const FLAG_ALPHA: u8 = 0b0001_0000;
const FLAG_EXIF: u8 = 0b0000_1000;
const FLAG_XMP: u8 = 0b0000_0100;
const FLAG_ANIMATION: u8 = 0b0000_0010;
const FRAME_CHUNK_BYTES_MIN: u64 = 24;
const FRAME_DURATION_OFFSET_BYTES: i64 = 12;
const FRAME_DATA_OFFSET_BYTES: u64 = 16;
const FRAME_SUBCHUNKS_RECORDED: usize = 2;
const ANIMATION_CHUNK_BYTES: u64 = 6;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    pub width: u32,
    pub height: u32,
    pub alpha: bool,
}

#[derive(Clone, Default)]
pub(crate) struct Chunks {
    pub(crate) lossy: Option<Range<u64>>,
    pub(crate) lossless: Option<Range<u64>>,
    pub(crate) alpha: Option<Range<u64>>,
    pub(crate) animation: Option<Range<u64>>,
    pub(crate) frame: Option<Range<u64>>,
    pub(crate) icc_profile: Option<Range<u64>>,
    pub(crate) exif: Option<Range<u64>>,
    pub(crate) xmp: Option<Range<u64>>,
}

impl Chunks {
    const fn insert(&mut self, fourcc: [u8; 4], range: Range<u64>) {
        let slot = match fourcc {
            FOURCC_LOSSY => &mut self.lossy,
            FOURCC_LOSSLESS => &mut self.lossless,
            FOURCC_ALPHA => &mut self.alpha,
            FOURCC_ANIMATION => &mut self.animation,
            FOURCC_FRAME => &mut self.frame,
            FOURCC_ICC_PROFILE => &mut self.icc_profile,
            FOURCC_EXIF => &mut self.exif,
            FOURCC_XMP => &mut self.xmp,
            _ => return,
        };
        if slot.is_none() {
            *slot = Some(range);
        }
    }
}

pub(crate) struct Container<'a> {
    pub(crate) data: &'a [u8],
    pub(crate) header: Header,
    pub(crate) animated: bool,
    pub(crate) chunks: Chunks,
}

pub(crate) struct ChunkHead {
    pub(crate) fourcc: [u8; 4],
    pub(crate) size: u64,
    pub(crate) size_rounded: u64,
}

pub(crate) fn read_chunk_head(cursor: &mut Cursor<'_>) -> Result<ChunkHead, WebpError> {
    let fourcc = cursor.read_array::<4>()?;
    let size = cursor.read_u32()?;
    let size_rounded = size.saturating_add(size & 1);
    Ok(ChunkHead {
        fourcc,
        size: u64::from(size),
        size_rounded: u64::from(size_rounded),
    })
}

pub(crate) fn parse(data: &[u8]) -> Result<Container<'_>, WebpError> {
    let mut cursor = Cursor::new(data);
    let riff = read_chunk_head(&mut cursor)?;
    if riff.fourcc != FOURCC_RIFF {
        return Err(WebpError::ChunkHeader(FOURCC_RIFF));
    }
    let signature = cursor.read_array::<4>()?;
    if signature != FOURCC_WEBP {
        return Err(WebpError::WebpSignature(signature));
    }
    let first = read_chunk_head(&mut cursor)?;
    let start = cursor.position();
    let mut chunks = Chunks::default();
    let header = match first.fourcc {
        FOURCC_LOSSY => {
            let header = parse_lossy(&mut cursor)?;
            chunks.lossy = Some(start..start + first.size);
            header
        }
        FOURCC_LOSSLESS => {
            let header = parse_lossless(&mut cursor)?;
            chunks.lossless = Some(start..start + first.size);
            header
        }
        FOURCC_EXTENDED => return parse_extended(data, cursor, riff.size, &first),
        fourcc => return Err(WebpError::ChunkHeader(fourcc)),
    };
    Ok(Container {
        data,
        header,
        animated: false,
        chunks,
    })
}

fn parse_lossy(cursor: &mut Cursor<'_>) -> Result<Header, WebpError> {
    let tag = cursor.read_u24()?;
    if tag & 1 != 0 {
        return Err(WebpError::NonKeyframe);
    }
    let magic = cursor.read_array::<3>()?;
    if magic != LOSSY_MAGIC {
        return Err(WebpError::LossyMagic(magic));
    }
    let width = u32::from(cursor.read_u16()? & LOSSY_DIMENSION_MASK);
    let height = u32::from(cursor.read_u16()? & LOSSY_DIMENSION_MASK);
    if width == 0 || height == 0 {
        return Err(WebpError::ImageSizes);
    }
    Ok(Header {
        width,
        height,
        alpha: false,
    })
}

fn parse_lossless(cursor: &mut Cursor<'_>) -> Result<Header, WebpError> {
    let signature = cursor.read_u8()?;
    if signature != LOSSLESS_SIGNATURE {
        return Err(WebpError::LosslessSignature(signature));
    }
    let bits = cursor.read_u32()?;
    let version = bits >> LOSSLESS_VERSION_SHIFT;
    if version != 0 {
        return Err(WebpError::LosslessVersion(version as u8));
    }
    Ok(Header {
        width: (1 + bits) & LOSSLESS_DIMENSION_MASK,
        height: (1 + (bits >> LOSSLESS_DIMENSION_BITS)) & LOSSLESS_DIMENSION_MASK,
        alpha: (bits >> LOSSLESS_ALPHA_BIT) & 1 != 0,
    })
}

fn parse_extended<'a>(
    data: &'a [u8],
    mut cursor: Cursor<'a>,
    riff_size: u64,
    first: &ChunkHead,
) -> Result<Container<'a>, WebpError> {
    let start = cursor.position();
    let flags = cursor.read_u8()?;
    let _reserved = cursor.read_u24()?;
    let width = cursor.read_u24()? + 1;
    let height = cursor.read_u24()? + 1;
    if width.checked_mul(height).is_none() {
        return Err(WebpError::ImageTooLarge);
    }
    let animated = flags & FLAG_ANIMATION != 0;
    let mut chunks = Chunks::default();
    let mut position = start + first.size_rounded;
    let position_end = position + riff_size.saturating_sub(RIFF_HEADER_BYTES);
    cursor.seek(position);
    let mut lossy_frames = false;
    while position < position_end {
        let Ok(head) = read_chunk_head(&mut cursor) else {
            break;
        };
        let range = position + CHUNK_HEADER_BYTES..position + CHUNK_HEADER_BYTES + head.size;
        position += CHUNK_HEADER_BYTES + head.size_rounded;
        chunks.insert(head.fourcc, range);
        if head.fourcc == FOURCC_FRAME {
            if head.size < FRAME_CHUNK_BYTES_MIN {
                return Err(WebpError::ChunkSize);
            }
            cursor.seek_relative(FRAME_DURATION_OFFSET_BYTES)?;
            let _duration = cursor.read_u32()?;
            let rounded = head.size_rounded as i64;
            if lossy_frames {
                cursor.seek_relative(rounded - FRAME_DATA_OFFSET_BYTES as i64)?;
            } else {
                let frame_data = read_chunk_head(&mut cursor)?;
                lossy_frames = matches!(frame_data.fourcc, FOURCC_LOSSY | FOURCC_ALPHA);
                cursor.seek_relative(rounded - FRAME_CHUNK_BYTES_MIN as i64)?;
            }
            continue;
        }
        cursor.seek_relative(head.size_rounded as i64)?;
    }
    let animation_missing = animated && (chunks.animation.is_none() || chunks.frame.is_none());
    let exif_missing = flags & FLAG_EXIF != 0 && chunks.exif.is_none();
    let xmp_missing = flags & FLAG_XMP != 0 && chunks.xmp.is_none();
    let image_ambiguous = !animated && chunks.lossy.is_some() == chunks.lossless.is_some();
    if animation_missing || exif_missing || xmp_missing || image_ambiguous {
        return Err(WebpError::ChunkMissing);
    }
    if animated {
        read_animation(&mut cursor, &chunks)?;
    }
    if let Some(frame) = chunks.frame.clone() {
        record_frame_chunks(&mut cursor, &mut chunks, &frame)?;
    }
    Ok(Container {
        data,
        header: Header {
            width,
            height,
            alpha: flags & FLAG_ALPHA != 0,
        },
        animated,
        chunks,
    })
}

fn read_animation(cursor: &mut Cursor<'_>, chunks: &Chunks) -> Result<(), WebpError> {
    let Some(range) = chunks.animation.clone() else {
        return Err(WebpError::ChunkMissing);
    };
    let length = range.end - range.start;
    if length > ANIMATION_CHUNK_BYTES {
        return Err(WebpError::ChunkSize);
    }
    cursor.seek(range.start);
    let bytes = cursor.read(length as usize)?;
    if (bytes.len() as u64) < ANIMATION_CHUNK_BYTES {
        return Err(WebpError::Truncated);
    }
    Ok(())
}

fn record_frame_chunks(
    cursor: &mut Cursor<'_>,
    chunks: &mut Chunks,
    frame: &Range<u64>,
) -> Result<(), WebpError> {
    let mut position = frame.start + FRAME_DATA_OFFSET_BYTES;
    cursor.seek(position);
    for _ in 0..FRAME_SUBCHUNKS_RECORDED {
        let head = read_chunk_head(cursor)?;
        chunks.insert(
            head.fourcc,
            position + CHUNK_HEADER_BYTES..position + CHUNK_HEADER_BYTES + head.size,
        );
        position += CHUNK_HEADER_BYTES + head.size_rounded;
        if position + CHUNK_HEADER_BYTES > frame.end {
            break;
        }
    }
    Ok(())
}
