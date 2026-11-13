use std::fmt;

use crate::bmp::{Bmp, BmpError};
use crate::pixels::{Image, Pixels};
use crate::pnm::{Pnm, PnmError, Samples};

const MASKED_SIGNATURE_FILL: u8 = 0xFF;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Format {
    Png,
    Jpeg,
    Gif,
    WebP,
    Pnm,
    Tiff,
    Tga,
    Dds,
    Bmp,
    Ico,
    Hdr,
    OpenExr,
    Farbfeld,
    Avif,
    Qoi,
    Pcx,
}

impl Format {
    pub const fn mime_type(self) -> &'static str {
        match self {
            Self::Avif => "image/avif",
            Self::Jpeg => "image/jpeg",
            Self::Png => "image/png",
            Self::Gif => "image/gif",
            Self::WebP => "image/webp",
            Self::Tiff => "image/tiff",
            Self::Tga => "image/x-targa",
            Self::Dds => "image/vnd-ms.dds",
            Self::Bmp => "image/bmp",
            Self::Ico => "image/x-icon",
            Self::Hdr => "image/vnd.radiance",
            Self::OpenExr => "image/x-exr",
            Self::Pnm => "image/x-portable-anymap",
            Self::Qoi => "image/x-qoi",
            Self::Farbfeld => "application/octet-stream",
            Self::Pcx => "image/vnd.zbrush.pcx",
        }
    }
}

const SIGNATURES: [(&[u8], &[u8], Format); 23] = [
    (b"\x89PNG\r\n\x1a\n", b"", Format::Png),
    (&[0xff, 0xd8, 0xff], b"", Format::Jpeg),
    (b"GIF89a", b"", Format::Gif),
    (b"GIF87a", b"", Format::Gif),
    (
        b"RIFF\0\0\0\0WEBP",
        b"\xFF\xFF\xFF\xFF\0\0\0\0",
        Format::WebP,
    ),
    (b"MM\x00*", b"", Format::Tiff),
    (b"II*\x00", b"", Format::Tiff),
    (b"DDS ", b"", Format::Dds),
    (b"BM", b"", Format::Bmp),
    (&[0, 0, 1, 0], b"", Format::Ico),
    (b"#?RADIANCE", b"", Format::Hdr),
    (b"\0\0\0\0ftypavif", b"\xFF\xFF\0\0", Format::Avif),
    (&[0x76, 0x2f, 0x31, 0x01], b"", Format::OpenExr),
    (b"qoif", b"", Format::Qoi),
    (b"P1", b"", Format::Pnm),
    (b"P2", b"", Format::Pnm),
    (b"P3", b"", Format::Pnm),
    (b"P4", b"", Format::Pnm),
    (b"P5", b"", Format::Pnm),
    (b"P6", b"", Format::Pnm),
    (b"P7", b"", Format::Pnm),
    (b"farbfeld", b"", Format::Farbfeld),
    (&[0x0a, 0x0], b"\xFF\xF8", Format::Pcx),
];

pub fn guess_format(bytes: &[u8]) -> Option<Format> {
    SIGNATURES.iter().find_map(|&(signature, mask, format)| {
        let matched = if mask.is_empty() {
            bytes.starts_with(signature)
        } else {
            bytes.len() >= signature.len()
                && bytes
                    .iter()
                    .zip(signature)
                    .zip(mask.iter().chain(std::iter::repeat(&MASKED_SIGNATURE_FILL)))
                    .all(|((&byte, &expected), &mask)| byte & mask == expected)
        };
        matched.then_some(format)
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    pub alloc_bytes_max: u64,
    pub dimension_max: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeError {
    UnknownFormat,
    Unsupported(Format),
    Decoding { format: Format, message: String },
    DimensionsTooLarge { width: u32, height: u32 },
    AllocationTooLarge { bytes: u64, max: u64 },
}

impl fmt::Display for DecodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownFormat => formatter.write_str("The image format could not be determined"),
            Self::Unsupported(format) => {
                write!(formatter, "The image format {format:?} is not supported")
            }
            Self::Decoding { format, message } => {
                write!(formatter, "Format error decoding {format:?}: {message}")
            }
            Self::DimensionsTooLarge { width, height } => {
                write!(
                    formatter,
                    "The image dimensions {width}x{height} exceed the limit"
                )
            }
            Self::AllocationTooLarge { bytes, max } => {
                write!(
                    formatter,
                    "Memory limit exceeded: {bytes} bytes against a limit of {max}"
                )
            }
        }
    }
}

impl std::error::Error for DecodeError {}

fn failed(format: Format) -> impl Fn(&dyn fmt::Display) -> DecodeError {
    move |error| DecodeError::Decoding {
        format,
        message: error.to_string(),
    }
}

enum Source<'a> {
    Png(nanocode_png::Png<'a>),
    Jpeg(&'a [u8]),
    Gif(nanocode_gif::Gif<'a>),
    WebP(nanocode_webp::Decoder<'a>),
    Bmp(&'a [u8], Bmp),
    Pnm(Pnm<'a>),
}

pub struct Decoder<'a> {
    source: Source<'a>,
    width: u32,
    height: u32,
    bytes_per_pixel: u64,
    limits: Limits,
}

pub struct Decoded {
    pub image: Image,
    pub icc_profile: Option<Vec<u8>>,
    pub exif: Option<Vec<u8>>,
}

impl<'a> Decoder<'a> {
    pub fn new(bytes: &'a [u8], format: Format, limits: Limits) -> Result<Self, DecodeError> {
        let (width, height, bytes_per_pixel, source) = match format {
            Format::Png => {
                let memory = usize::try_from(limits.alloc_bytes_max).unwrap_or(usize::MAX);
                let png = nanocode_png::Png::new(bytes, memory)
                    .map_err(|error| failed(format)(&error))?;
                let pixel = png.color().bytes_per_pixel() as u64;
                (png.width(), png.height(), pixel, Source::Png(png))
            }
            Format::Jpeg => {
                let header =
                    nanocode_jpeg::header(bytes).map_err(|error| failed(format)(&error))?;
                let pixel = header.color.channels() as u64;
                (header.width, header.height, pixel, Source::Jpeg(bytes))
            }
            Format::Gif => {
                let gif = nanocode_gif::Gif::new(bytes).map_err(|error| failed(format)(&error))?;
                (gif.width(), gif.height(), 4, Source::Gif(gif))
            }
            Format::WebP => {
                let decoder =
                    nanocode_webp::Decoder::new(bytes).map_err(|error| failed(format)(&error))?;
                let header = decoder.header();
                let pixel = if header.alpha { 4 } else { 3 };
                (header.width, header.height, pixel, Source::WebP(decoder))
            }
            Format::Bmp => {
                let bmp = Bmp::new(bytes).map_err(|error: BmpError| failed(format)(&error))?;
                let pixel = if bmp.alpha() { 4 } else { 3 };
                (bmp.width(), bmp.height(), pixel, Source::Bmp(bytes, bmp))
            }
            Format::Pnm => {
                let pnm = Pnm::new(bytes).map_err(|error: PnmError| failed(format)(&error))?;
                let pixel = (pnm.tuple().channels() * pnm.tuple().sample_bytes()) as u64;
                (pnm.width(), pnm.height(), pixel, Source::Pnm(pnm))
            }
            other => return Err(DecodeError::Unsupported(other)),
        };
        if let Some(max) = limits.dimension_max
            && (width > max || height > max)
        {
            return Err(DecodeError::DimensionsTooLarge { width, height });
        }
        Ok(Self {
            source,
            width,
            height,
            bytes_per_pixel,
            limits,
        })
    }

    pub const fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    pub const fn total_bytes(&self) -> u64 {
        (self.width as u64 * self.height as u64).saturating_mul(self.bytes_per_pixel)
    }

    pub fn icc_profile(&self) -> Option<Vec<u8>> {
        match &self.source {
            Source::Png(png) => png.icc_profile().map(<[u8]>::to_vec),
            _ => None,
        }
    }

    pub fn reserve(&mut self) -> Result<(), DecodeError> {
        let bytes = self.total_bytes();
        let max = self.limits.alloc_bytes_max;
        self.limits.alloc_bytes_max = max
            .checked_sub(bytes)
            .ok_or(DecodeError::AllocationTooLarge { bytes, max })?;
        Ok(())
    }

    pub fn decode(self) -> Result<Decoded, DecodeError> {
        let total = self.total_bytes();
        if usize::try_from(total).is_err() || total > isize::MAX as u64 {
            return Err(DecodeError::AllocationTooLarge {
                bytes: total,
                max: isize::MAX as u64,
            });
        }
        let (width, height) = (self.width, self.height);
        let image = |pixels: Pixels| {
            Image::new(width, height, pixels).expect("decoders fill width * height pixels")
        };
        match self.source {
            Source::Png(png) => {
                let icc_profile = png.icc_profile().map(<[u8]>::to_vec);
                let color = png.color();
                let samples = png.decode().map_err(|error| failed(Format::Png)(&error))?;
                let pixels = match (color, samples) {
                    (nanocode_png::Color::L8, nanocode_png::Samples::Eight(samples)) => {
                        Pixels::Luma8(samples)
                    }
                    (nanocode_png::Color::La8, nanocode_png::Samples::Eight(samples)) => {
                        Pixels::LumaAlpha8(samples)
                    }
                    (nanocode_png::Color::Rgb8, nanocode_png::Samples::Eight(samples)) => {
                        Pixels::Rgb8(samples)
                    }
                    (nanocode_png::Color::Rgba8, nanocode_png::Samples::Eight(samples)) => {
                        Pixels::Rgba8(samples)
                    }
                    (nanocode_png::Color::L16, nanocode_png::Samples::Sixteen(samples)) => {
                        Pixels::Luma16(samples)
                    }
                    (nanocode_png::Color::La16, nanocode_png::Samples::Sixteen(samples)) => {
                        Pixels::LumaAlpha16(samples)
                    }
                    (nanocode_png::Color::Rgb16, nanocode_png::Samples::Sixteen(samples)) => {
                        Pixels::Rgb16(samples)
                    }
                    (nanocode_png::Color::Rgba16, nanocode_png::Samples::Sixteen(samples)) => {
                        Pixels::Rgba16(samples)
                    }
                    _ => unreachable!("PNG sample width follows its color type"),
                };
                Ok(Decoded {
                    image: image(pixels),
                    icc_profile,
                    exif: None,
                })
            }
            Source::Jpeg(bytes) => {
                let decoded = nanocode_jpeg::decode(bytes, u64::MAX)
                    .map_err(|error| failed(Format::Jpeg)(&error))?;
                let pixels = match decoded.color {
                    nanocode_jpeg::Color::Luma => Pixels::Luma8(decoded.pixels),
                    nanocode_jpeg::Color::Rgb => Pixels::Rgb8(decoded.pixels),
                };
                Ok(Decoded {
                    image: image(pixels),
                    icc_profile: decoded.icc_profile,
                    exif: decoded.exif,
                })
            }
            Source::Gif(gif) => {
                let pixels = gif
                    .first_frame(self.limits.alloc_bytes_max)
                    .map_err(|error| failed(Format::Gif)(&error))?;
                Ok(Decoded {
                    image: image(Pixels::Rgba8(pixels)),
                    icc_profile: None,
                    exif: None,
                })
            }
            Source::WebP(decoder) => {
                let icc_profile = decoder.icc_profile().ok().flatten().map(<[u8]>::to_vec);
                let exif = decoder.exif().ok().flatten().map(<[u8]>::to_vec);
                let alpha = decoder.header().alpha;
                let samples = decoder
                    .decode(u64::MAX)
                    .map_err(|error| failed(Format::WebP)(&error))?;
                let pixels = if alpha {
                    Pixels::Rgba8(samples)
                } else {
                    Pixels::Rgb8(samples)
                };
                Ok(Decoded {
                    image: image(pixels),
                    icc_profile,
                    exif,
                })
            }
            Source::Bmp(bytes, bmp) => {
                let samples = bmp
                    .decode(bytes)
                    .map_err(|error| failed(Format::Bmp)(&error))?;
                let pixels = if bmp.alpha() {
                    Pixels::Rgba8(samples)
                } else {
                    Pixels::Rgb8(samples)
                };
                Ok(Decoded {
                    image: image(pixels),
                    icc_profile: None,
                    exif: None,
                })
            }
            Source::Pnm(pnm) => {
                let tuple = pnm.tuple();
                let samples = pnm.decode().map_err(|error| failed(Format::Pnm)(&error))?;
                let pixels = match (tuple.channels(), samples) {
                    (1, Samples::Eight(samples)) => Pixels::Luma8(samples),
                    (_, Samples::Eight(samples)) => Pixels::Rgb8(samples),
                    (1, Samples::Sixteen(samples)) => Pixels::Luma16(samples),
                    (_, Samples::Sixteen(samples)) => Pixels::Rgb16(samples),
                };
                Ok(Decoded {
                    image: image(pixels),
                    icc_profile: None,
                    exif: None,
                })
            }
        }
    }
}

pub fn dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    let format = guess_format(bytes.get(..bytes.len().min(16))?)?;
    let limits = Limits {
        alloc_bytes_max: DEFAULT_ALLOC_BYTES_MAX,
        dimension_max: None,
    };
    Decoder::new(bytes, format, limits)
        .ok()
        .map(|decoder| decoder.dimensions())
}

pub const DEFAULT_ALLOC_BYTES_MAX: u64 = 512 * 1024 * 1024;
