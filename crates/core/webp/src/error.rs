use core::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WebpError {
    Truncated,
    SeekBeforeStart,
    WebpSignature([u8; 4]),
    ChunkHeader([u8; 4]),
    ChunkMissing,
    ChunkSize,
    ImageTooLarge,
    ImageSizes,
    FrameOutsideImage,
    FramePixels,
    AlphaPreprocessing,
    AlphaCompression,
    LosslessSignature(u8),
    LosslessVersion(u8),
    ColorCacheBits(u8),
    HuffmanCode,
    Bitstream,
    Transform,
    LossyMagic([u8; 3]),
    LossyColorSpace(u8),
    NonKeyframe,
    PixelBytes { bytes: u64, max: u64 },
    PixelLength { expected: u64, actual: u64 },
    Dimensions { width: u32, height: u32 },
}

impl fmt::Display for WebpError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated => formatter.write_str("WebP data ends early"),
            Self::SeekBeforeStart => formatter.write_str("WebP chunk offset is before the start"),
            Self::WebpSignature(fourcc) => write!(formatter, "invalid WebP signature: {fourcc:x?}"),
            Self::ChunkHeader(fourcc) => {
                write!(formatter, "invalid WebP chunk header: {fourcc:x?}")
            }
            Self::ChunkMissing => formatter.write_str("an expected WebP chunk is missing"),
            Self::ChunkSize => formatter.write_str("invalid WebP chunk size"),
            Self::ImageTooLarge => formatter.write_str("WebP image too large"),
            Self::ImageSizes => formatter.write_str("inconsistent WebP image sizes"),
            Self::FrameOutsideImage => formatter.write_str("WebP frame outside the canvas"),
            Self::FramePixels => {
                formatter.write_str("WebP frame pixels do not match the frame size")
            }
            Self::AlphaPreprocessing => formatter.write_str("invalid WebP alpha preprocessing"),
            Self::AlphaCompression => formatter.write_str("invalid WebP alpha compression"),
            Self::LosslessSignature(byte) => {
                write!(formatter, "invalid lossless WebP signature: {byte:x?}")
            }
            Self::LosslessVersion(version) => {
                write!(formatter, "invalid lossless WebP version: {version}")
            }
            Self::ColorCacheBits(bits) => write!(formatter, "invalid color cache bits: {bits}"),
            Self::HuffmanCode => formatter.write_str("invalid Huffman code"),
            Self::Bitstream => formatter.write_str("corrupt WebP bitstream"),
            Self::Transform => formatter.write_str("invalid lossless WebP transform"),
            Self::LossyMagic(magic) => write!(formatter, "invalid VP8 magic: {magic:x?}"),
            Self::LossyColorSpace(space) => write!(formatter, "invalid VP8 color space: {space}"),
            Self::NonKeyframe => formatter.write_str("VP8 non-keyframe frames are not supported"),
            Self::PixelBytes { bytes, max } => {
                write!(
                    formatter,
                    "decoded WebP pixels take {bytes} bytes, above {max}"
                )
            }
            Self::PixelLength { expected, actual } => {
                write!(
                    formatter,
                    "WebP pixels take {actual} bytes, expected {expected}"
                )
            }
            Self::Dimensions { width, height } => {
                write!(formatter, "invalid WebP dimensions {width}x{height}")
            }
        }
    }
}

impl core::error::Error for WebpError {}
