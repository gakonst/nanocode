use alloc::vec;
use alloc::vec::Vec;
use core::ops::Range;

use crate::alpha;
use crate::animation;
use crate::container::{self, Container, Header};
use crate::error::WebpError;
use crate::lossless;
use crate::lossy::{self, Expected};
use crate::stream::Cursor;
use crate::yuv;

const RGB_BYTES_PER_PIXEL: usize = 3;
const RGBA_BYTES_PER_PIXEL: usize = 4;

pub struct Decoder<'a> {
    container: Container<'a>,
}

pub fn header(bytes: &[u8]) -> Result<Header, WebpError> {
    Ok(container::parse(bytes)?.header)
}

impl<'a> Decoder<'a> {
    pub fn new(bytes: &'a [u8]) -> Result<Self, WebpError> {
        Ok(Self {
            container: container::parse(bytes)?,
        })
    }

    pub const fn header(&self) -> Header {
        self.container.header
    }

    pub fn icc_profile(&self) -> Result<Option<&'a [u8]>, WebpError> {
        chunk_bytes(
            self.container.data,
            self.container.chunks.icc_profile.as_ref(),
        )
    }

    pub fn exif(&self) -> Result<Option<&'a [u8]>, WebpError> {
        chunk_bytes(self.container.data, self.container.chunks.exif.as_ref())
    }

    pub fn decode(&self, pixel_bytes_max: u64) -> Result<Vec<u8>, WebpError> {
        let Header {
            width,
            height,
            alpha,
        } = self.container.header;
        let bytes_per_pixel = if alpha {
            RGBA_BYTES_PER_PIXEL
        } else {
            RGB_BYTES_PER_PIXEL
        };
        let pixel_bytes = u64::from(width) * u64::from(height) * bytes_per_pixel as u64;
        if pixel_bytes > pixel_bytes_max {
            return Err(WebpError::PixelBytes {
                bytes: pixel_bytes,
                max: pixel_bytes_max,
            });
        }
        let length = usize::try_from(pixel_bytes).map_err(|_| WebpError::ImageTooLarge)?;
        let mut pixels = vec![0; length];
        let chunks = &self.container.chunks;
        if self.container.animated {
            let Some(frame) = chunks.frame.clone() else {
                return Err(WebpError::ChunkMissing);
            };
            animation::first_frame(
                self.container.data,
                frame,
                self.container.header,
                &mut pixels,
            )?;
        } else if let Some(range) = chunks.lossless.clone() {
            self.decode_lossless(range, &mut pixels)?;
        } else {
            self.decode_lossy(&mut pixels)?;
        }
        Ok(pixels)
    }

    fn decode_lossless(&self, range: Range<u64>, pixels: &mut [u8]) -> Result<(), WebpError> {
        let Header {
            width,
            height,
            alpha,
        } = self.container.header;
        let stream = Cursor::new(self.container.data).range(range.start, range.end);
        if alpha {
            return lossless::decode_explicit(stream, width, height, pixels);
        }
        let mut rgba = vec![0; pixels.len() / RGB_BYTES_PER_PIXEL * RGBA_BYTES_PER_PIXEL];
        lossless::decode_explicit(stream, width, height, &mut rgba)?;
        for (rgb, source) in pixels
            .as_chunks_mut::<RGB_BYTES_PER_PIXEL>()
            .0
            .iter_mut()
            .zip(rgba.as_chunks::<RGBA_BYTES_PER_PIXEL>().0.iter())
        {
            rgb.copy_from_slice(&source[..RGB_BYTES_PER_PIXEL]);
        }
        Ok(())
    }

    fn decode_lossy(&self, pixels: &mut [u8]) -> Result<(), WebpError> {
        let Header {
            width,
            height,
            alpha,
        } = self.container.header;
        let chunks = &self.container.chunks;
        let mut cursor = Cursor::new(self.container.data);
        let range = chunks.lossy.clone().ok_or(WebpError::ChunkMissing)?;
        let stream = cursor.range(range.start, range.end);
        let frame = lossy::decode(stream, Expected::Size { width, height })?;
        if !alpha {
            return yuv::fill_fancy::<RGB_BYTES_PER_PIXEL>(pixels, &frame);
        }
        yuv::fill_fancy::<RGBA_BYTES_PER_PIXEL>(pixels, &frame)?;
        let range = chunks.alpha.clone().ok_or(WebpError::ChunkMissing)?;
        let stream = cursor.range(range.start, range.end);
        let plane = alpha::read(stream, frame.width, frame.height)?;
        alpha::apply(
            &plane,
            usize::from(frame.width),
            usize::from(frame.height),
            pixels,
        )
    }
}

fn chunk_bytes<'a>(
    data: &'a [u8],
    range: Option<&Range<u64>>,
) -> Result<Option<&'a [u8]>, WebpError> {
    let Some(range) = range else {
        return Ok(None);
    };
    if range.start == range.end {
        return Ok(Some(&[]));
    }
    let start = usize::try_from(range.start).map_err(|_| WebpError::Truncated)?;
    let end = usize::try_from(range.end).map_err(|_| WebpError::Truncated)?;
    if end > data.len() {
        return Err(WebpError::Truncated);
    }
    Ok(Some(&data[start..end]))
}
