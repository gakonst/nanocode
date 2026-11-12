use std::fmt;

use nanocode_compression::inflate::InflateError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PngError {
    Format(FormatError),
    UnexpectedEnd,
    LimitsExceeded,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormatError {
    InvalidSignature,
    ChunkBeforeHeader { chunk: [u8; 4] },
    CrcMismatch { chunk: [u8; 4] },
    DuplicateChunk { chunk: [u8; 4] },
    BeforePalette { chunk: [u8; 4] },
    ShortPalette { expected: usize, length: usize },
    PaletteTooLong { length: usize },
    PaletteRequired,
    InvalidColorBitDepth { color_type: u8, bit_depth: u8 },
    TransparencyForColor { color_type: u8 },
    InvalidDimensions,
    InvalidBitDepth(u8),
    InvalidColorType(u8),
    InvalidDisposeOp(u8),
    InvalidBlendOp(u8),
    InvalidUnit(u8),
    InvalidRenderingIntent(u8),
    UnknownCompressionMethod(u8),
    UnknownFilterMethod(u8),
    UnknownInterlaceMethod(u8),
    BadSubFrameBounds,
    CorruptFlateStream(&'static str),
    NoMoreImageData,
    MissingImageData,
    MissingFrameControl,
    SequenceOrder { expected: u32, present: u32 },
    FrameDataShorterThanFourBytes,
    UnexpectedRestartOfDataChunkSequence { chunk: [u8; 4] },
    ChunkTooShort { chunk: [u8; 4] },
}

impl From<FormatError> for PngError {
    fn from(error: FormatError) -> Self {
        Self::Format(error)
    }
}

impl FormatError {
    pub(crate) const fn inflate(error: InflateError) -> Self {
        Self::CorruptFlateStream(error.as_str())
    }
}

struct Chunk<'a>(&'a [u8; 4]);

impl fmt::Display for Chunk<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for &byte in self.0 {
            write!(formatter, "{}", char::from(byte))?;
        }
        Ok(())
    }
}

impl fmt::Display for PngError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Format(error) => write!(formatter, "{error}"),
            Self::UnexpectedEnd => formatter.write_str("unexpected end of file"),
            Self::LimitsExceeded => formatter.write_str("limits are exceeded"),
        }
    }
}

impl fmt::Display for FormatError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSignature => formatter.write_str("Invalid PNG signature."),
            Self::ChunkBeforeHeader { chunk } => {
                write!(
                    formatter,
                    "{} chunk appeared before IHDR chunk",
                    Chunk(chunk)
                )
            }
            Self::CrcMismatch { chunk } => {
                write!(
                    formatter,
                    "CRC error while decoding {} chunk.",
                    Chunk(chunk)
                )
            }
            Self::DuplicateChunk { chunk } => {
                write!(
                    formatter,
                    "Chunk {} must appear at most once.",
                    Chunk(chunk)
                )
            }
            Self::BeforePalette { chunk } => {
                write!(
                    formatter,
                    "Chunk {} is invalid before PLTE chunk.",
                    Chunk(chunk)
                )
            }
            Self::ShortPalette { expected, length } => write!(
                formatter,
                "Not enough palette entries, expect {expected} got {length}."
            ),
            Self::PaletteTooLong { length } => write!(
                formatter,
                "Palette of {length} bytes is not a whole number of at most 256 entries."
            ),
            Self::PaletteRequired => formatter.write_str("Missing palette of indexed image."),
            Self::InvalidColorBitDepth {
                color_type,
                bit_depth,
            } => write!(
                formatter,
                "Invalid color/depth combination in header: {color_type}/{bit_depth}"
            ),
            Self::TransparencyForColor { color_type } => write!(
                formatter,
                "Transparency chunk found for color type {color_type}."
            ),
            Self::InvalidDimensions => formatter.write_str("Invalid image dimensions"),
            Self::InvalidBitDepth(value) => write!(formatter, "Invalid bit depth {value}."),
            Self::InvalidColorType(value) => write!(formatter, "Invalid color type {value}."),
            Self::InvalidDisposeOp(value) => write!(formatter, "Invalid dispose op {value}."),
            Self::InvalidBlendOp(value) => write!(formatter, "Invalid blend op {value}."),
            Self::InvalidUnit(value) => {
                write!(formatter, "Invalid physical pixel size unit {value}.")
            }
            Self::InvalidRenderingIntent(value) => {
                write!(formatter, "Invalid sRGB rendering intent {value}.")
            }
            Self::UnknownCompressionMethod(value) => {
                write!(formatter, "Unknown compression method {value}.")
            }
            Self::UnknownFilterMethod(value) => write!(formatter, "Unknown filter method {value}."),
            Self::UnknownInterlaceMethod(value) => {
                write!(formatter, "Unknown interlace method {value}.")
            }
            Self::BadSubFrameBounds => formatter.write_str("Sub frame is out-of-bounds."),
            Self::CorruptFlateStream(message) => {
                write!(formatter, "Corrupt deflate stream. {message}")
            }
            Self::NoMoreImageData => {
                formatter.write_str("IDAT or fDAT chunk does not have enough data for image.")
            }
            Self::MissingImageData => formatter.write_str("IDAT or fdAT chunk is missing."),
            Self::MissingFrameControl => {
                formatter.write_str("fcTL chunk missing before fdAT chunk.")
            }
            Self::SequenceOrder { expected, present } => write!(
                formatter,
                "Sequence is not in order, expected #{expected} got #{present}."
            ),
            Self::FrameDataShorterThanFourBytes => {
                formatter.write_str("fdAT chunk shorter than 4 bytes")
            }
            Self::UnexpectedRestartOfDataChunkSequence { chunk } => write!(
                formatter,
                "Unexpected restart of {} chunk sequence",
                Chunk(chunk)
            ),
            Self::ChunkTooShort { chunk } => {
                write!(formatter, "Chunk is too short: {}", Chunk(chunk))
            }
        }
    }
}

impl std::error::Error for PngError {}
