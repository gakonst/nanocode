#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

mod bitstream;
mod color;
mod decoder;
mod encoder;
mod error;
mod headers;
mod huffman;
mod idct;
mod marker;
mod process;
mod progressive;
mod reader;
mod sequential;
mod upsample;

use alloc::vec::Vec;

pub use encoder::encode;
pub use error::{ColorSpace, ComponentPosition, JpegError, TableClass};
pub use marker::Marker;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Color {
    Luma,
    Rgb,
}

impl Color {
    #[must_use]
    pub const fn channels(self) -> usize {
        match self {
            Self::Luma => 1,
            Self::Rgb => 3,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    pub width: u32,
    pub height: u32,
    pub color: Color,
}

impl Header {
    #[must_use]
    pub fn pixel_bytes(&self) -> u64 {
        u64::from(self.width) * u64::from(self.height) * self.color.channels() as u64
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Decoded {
    pub width: u32,
    pub height: u32,
    pub color: Color,
    pub pixels: Vec<u8>,
    pub icc_profile: Option<Vec<u8>>,
    pub exif: Option<Vec<u8>>,
}

pub fn header(bytes: &[u8]) -> Result<Header, JpegError> {
    let mut decoder = decoder::Decoder::new(bytes);
    decoder.parse_headers()?;
    Ok(decoder.header())
}

pub fn decode(bytes: &[u8], pixel_bytes_max: u64) -> Result<Decoded, JpegError> {
    let mut decoder = decoder::Decoder::new(bytes);
    decoder.parse_headers()?;
    let header = decoder.header();
    let pixel_bytes = header.pixel_bytes();
    if pixel_bytes > pixel_bytes_max {
        return Err(JpegError::PixelBytes {
            bytes: pixel_bytes,
            max: pixel_bytes_max,
        });
    }
    let length = usize::try_from(pixel_bytes).map_err(|_| JpegError::PixelBytes {
        bytes: pixel_bytes,
        max: pixel_bytes_max,
    })?;
    let icc_profile = decoder.icc_profile();
    let exif = decoder.exif();
    let mut pixels = Vec::new();
    pixels
        .try_reserve_exact(length)
        .map_err(|_| JpegError::Allocation)?;
    pixels.resize(length, 0);
    decoder.decode_into(&mut pixels)?;
    Ok(Decoded {
        width: header.width,
        height: header.height,
        color: header.color,
        pixels,
        icc_profile,
        exif,
    })
}
