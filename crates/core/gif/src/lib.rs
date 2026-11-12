#![forbid(unsafe_code)]

mod lzw;
mod stream;

use std::fmt;

use stream::{Event, Frame, Reader};

const RGBA_BYTES: usize = 4;
const PALETTE_CHANNELS: usize = 3;
const OPAQUE: u8 = 0xFF;
const TRANSPARENT: u8 = 0;
const INTERLACE_STEPS: [usize; 4] = [8, 8, 4, 2];
const INTERLACE_STARTS: [usize; 4] = [4, 2, 1, 0];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GifError {
    Format(&'static str),
    UnexpectedEnd,
    InvalidCode,
    NoFrame,
    FrameTooLarge { bytes: u64, max: u64 },
}

impl fmt::Display for GifError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Format(message) => formatter.write_str(message),
            Self::UnexpectedEnd => formatter.write_str("unexpected end of file"),
            Self::InvalidCode => formatter.write_str("invalid code in LZW stream"),
            Self::NoFrame => formatter.write_str("the GIF holds no frame"),
            Self::FrameTooLarge { bytes, max } => {
                write!(
                    formatter,
                    "a {bytes}-byte frame exceeds the {max}-byte allocation limit"
                )
            }
        }
    }
}

impl std::error::Error for GifError {}

pub struct Gif<'a> {
    reader: Reader<'a>,
    global_palette: Option<Vec<u8>>,
}

impl<'a> Gif<'a> {
    pub fn new(bytes: &'a [u8]) -> Result<Self, GifError> {
        let mut reader = Reader::new(bytes);
        let mut global_palette = None;
        loop {
            match reader.next(None)? {
                Some(Event::GlobalPalette(palette)) => {
                    global_palette = (!palette.is_empty()).then_some(palette);
                }
                Some(Event::HeaderEnd) => break,
                Some(_) => {}
                None => return Err(GifError::Format("file does not contain any image data")),
            }
        }
        Ok(Self {
            reader,
            global_palette,
        })
    }

    pub const fn width(&self) -> u32 {
        self.reader.width as u32
    }

    pub const fn height(&self) -> u32 {
        self.reader.height as u32
    }

    pub fn first_frame(mut self, frame_bytes_max: u64) -> Result<Vec<u8>, GifError> {
        let frame = loop {
            match self.reader.next(None)? {
                Some(Event::FrameMetadata) => {
                    let frame = self.reader.take_frame();
                    if frame.palette.is_none() && self.global_palette.is_none() {
                        return Err(GifError::Format(
                            "no color table available for current frame",
                        ));
                    }
                    break frame;
                }
                Some(_) => {}
                None => return Err(GifError::NoFrame),
            }
        };
        let (width, height) = (self.width(), self.height());
        let line = width as usize * RGBA_BYTES;
        let mut canvas = vec![0u8; line * height as usize];
        let (left, top) = (u32::from(frame.left), u32::from(frame.top));
        let (frame_width, frame_height) = (u32::from(frame.width), u32::from(frame.height));
        if left == 0
            && frame_width == width
            && u64::from(top) + u64::from(frame_height) <= u64::from(height)
        {
            let start = line * top as usize;
            let end = start + line * frame_height as usize;
            self.read_frame(&frame, &mut canvas[start..end])?;
            return Ok(canvas);
        }
        let frame_bytes = u64::from(frame_width) * u64::from(frame_height) * RGBA_BYTES as u64;
        if frame_bytes > frame_bytes_max {
            return Err(GifError::FrameTooLarge {
                bytes: frame_bytes,
                max: frame_bytes_max,
            });
        }
        let mut pixels = vec![0u8; frame_bytes as usize];
        self.read_frame(&frame, &mut pixels)?;
        for (row, target) in canvas
            .chunks_exact_mut(line.max(1))
            .take(height as usize)
            .enumerate()
        {
            let frame_y = (row as u32).wrapping_sub(top);
            for (column, pixel) in target.as_chunks_mut::<RGBA_BYTES>().0.iter_mut().enumerate() {
                let frame_x = (column as u32).wrapping_sub(left);
                if frame_x < frame_width && frame_y < frame_height {
                    let at =
                        (frame_y as usize * frame_width as usize + frame_x as usize) * RGBA_BYTES;
                    pixel.copy_from_slice(&pixels[at..at + RGBA_BYTES]);
                }
            }
        }
        Ok(canvas)
    }

    fn read_frame(&mut self, frame: &Frame, buffer: &mut [u8]) -> Result<(), GifError> {
        let line = usize::from(frame.width) * RGBA_BYTES;
        let mut indices = Vec::new();
        if frame.interlaced {
            for row in interlaced_rows(usize::from(frame.height)) {
                let target = &mut buffer[row * line..row * line + line];
                if !self.fill(frame, target, &mut indices)? {
                    return Err(GifError::Format("image truncated"));
                }
            }
        } else {
            let target = &mut buffer[..line * usize::from(frame.height)];
            if !self.fill(frame, target, &mut indices)? {
                return Err(GifError::Format("image truncated"));
            }
        }
        Ok(())
    }

    fn fill(
        &mut self,
        frame: &Frame,
        mut target: &mut [u8],
        indices: &mut Vec<u8>,
    ) -> Result<bool, GifError> {
        let palette = frame
            .palette
            .as_deref()
            .or(self.global_palette.as_deref())
            .unwrap_or_default();
        loop {
            let pixels = target.len() / RGBA_BYTES;
            if pixels == 0 {
                return Err(GifError::Format("odd-sized buffer"));
            }
            if indices.len() < pixels {
                indices.resize(pixels, 0);
            }
            let decoded = match self.reader.next(Some(&mut indices[..pixels]))? {
                Some(Event::BytesDecoded(count)) => count,
                Some(Event::DataEnd) => return Ok(false),
                _ => return Err(GifError::Format("unexpected data")),
            };
            let (done, rest) = target.split_at_mut(decoded * RGBA_BYTES);
            target = rest;
            for (pixel, &index) in done.as_chunks_mut::<RGBA_BYTES>().0.iter_mut().zip(&indices[..decoded]) {
                let offset = PALETTE_CHANNELS * usize::from(index);
                if let Some(color) = palette.get(offset..offset + PALETTE_CHANNELS) {
                    pixel[..PALETTE_CHANNELS].copy_from_slice(color);
                    pixel[PALETTE_CHANNELS] = if frame.transparent == Some(index) {
                        TRANSPARENT
                    } else {
                        OPAQUE
                    };
                }
            }
            if target.is_empty() {
                return Ok(true);
            }
        }
    }
}

fn interlaced_rows(height: usize) -> impl Iterator<Item = usize> {
    let mut next = 0usize;
    let mut pass = 0usize;
    std::iter::from_fn(move || {
        if height == 0 {
            return None;
        }
        let mut following = next + INTERLACE_STEPS.get(pass)?;
        while following >= height {
            following = *INTERLACE_STARTS.get(pass)?;
            pass += 1;
        }
        Some(std::mem::replace(&mut next, following))
    })
}
