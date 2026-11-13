use alloc::vec;
use alloc::vec::Vec;
use core::ops::Range;

use crate::alpha;
use crate::container::{
    self, CHUNK_HEADER_BYTES, FOURCC_ALPHA, FOURCC_FRAME, FOURCC_LOSSLESS, FOURCC_LOSSY, Header,
};
use crate::error::WebpError;
use crate::lossless;
use crate::lossy::{self, Expected};
use crate::stream::Cursor;
use crate::yuv;

const FRAME_CHUNK_BYTES_MIN: u64 = 32;
const FRAME_HEADER_BYTES: u64 = 24;
const FRAME_DIMENSION_MAX: u32 = 16384;
const FRAME_OFFSET_SCALE: u32 = 2;
const FLAG_NO_BLENDING: u8 = 0b0000_0010;
const RGB_BYTES_PER_PIXEL: usize = 3;
const RGBA_BYTES_PER_PIXEL: usize = 4;
const ALPHA_OPAQUE: u8 = u8::MAX;

struct FrameRectangle {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    blend: bool,
}

pub(crate) fn first_frame(
    data: &[u8],
    frame: Range<u64>,
    canvas: Header,
    output: &mut [u8],
) -> Result<(), WebpError> {
    let mut cursor = Cursor::new(data);
    cursor.seek(frame.start - CHUNK_HEADER_BYTES);
    let head = container::read_chunk_head(&mut cursor)?;
    if head.fourcc != FOURCC_FRAME || head.size < FRAME_CHUNK_BYTES_MIN {
        return Err(WebpError::ChunkHeader(FOURCC_FRAME));
    }
    let frame_size = head.size;
    let rectangle = read_rectangle(&mut cursor, canvas)?;
    let (pixels, pixels_alpha) = decode_frame(&mut cursor, frame_size, &rectangle)?;
    let canvas_width = canvas.width as usize;
    let canvas_height = canvas.height as usize;
    let canvas_bytes = canvas_width * canvas_height * RGBA_BYTES_PER_PIXEL;
    if canvas.alpha {
        assert_eq!(output.len(), canvas_bytes);
        composite(output, canvas, &pixels, pixels_alpha, &rectangle);
        return Ok(());
    }
    let mut rgba = vec![0; canvas_bytes];
    composite(&mut rgba, canvas, &pixels, pixels_alpha, &rectangle);
    for (rgb, source) in output
        .as_chunks_mut::<RGB_BYTES_PER_PIXEL>()
        .0
        .iter_mut()
        .zip(rgba.as_chunks::<RGBA_BYTES_PER_PIXEL>().0.iter())
    {
        rgb.copy_from_slice(&source[..RGB_BYTES_PER_PIXEL]);
    }
    Ok(())
}

fn read_rectangle(cursor: &mut Cursor<'_>, canvas: Header) -> Result<FrameRectangle, WebpError> {
    let x = cursor.read_u24()? * FRAME_OFFSET_SCALE;
    let y = cursor.read_u24()? * FRAME_OFFSET_SCALE;
    let width = cursor.read_u24()? + 1;
    let height = cursor.read_u24()? + 1;
    if width > FRAME_DIMENSION_MAX || height > FRAME_DIMENSION_MAX {
        return Err(WebpError::ImageTooLarge);
    }
    if x + width > canvas.width || y + height > canvas.height {
        return Err(WebpError::FrameOutsideImage);
    }
    let _duration = cursor.read_u24()?;
    let flags = cursor.read_u8()?;
    Ok(FrameRectangle {
        x,
        y,
        width,
        height,
        blend: flags & FLAG_NO_BLENDING == 0,
    })
}

fn decode_frame(
    cursor: &mut Cursor<'_>,
    frame_size: u64,
    rectangle: &FrameRectangle,
) -> Result<(Vec<u8>, bool), WebpError> {
    let width = rectangle.width;
    let height = rectangle.height;
    let pixel_count = width as usize * height as usize;
    let head = container::read_chunk_head(cursor)?;
    if head.size_rounded + FRAME_HEADER_BYTES > frame_size {
        return Err(WebpError::ChunkHeader(head.fourcc));
    }
    match head.fourcc {
        FOURCC_LOSSY => {
            let image = lossy::decode(cursor.take(head.size), Expected::Size { width, height })?;
            let mut rgb = vec![0; pixel_count * RGB_BYTES_PER_PIXEL];
            yuv::fill_fancy::<RGB_BYTES_PER_PIXEL>(&mut rgb, &image)?;
            Ok((rgb, false))
        }
        FOURCC_LOSSLESS => {
            let mut rgba = vec![0; pixel_count * RGBA_BYTES_PER_PIXEL];
            lossless::decode_explicit(cursor.take(head.size), width, height, &mut rgba)?;
            Ok((rgba, true))
        }
        FOURCC_ALPHA => {
            if head.size_rounded + FRAME_CHUNK_BYTES_MIN > frame_size {
                return Err(WebpError::ChunkHeader(head.fourcc));
            }
            let next_chunk = cursor.position() + head.size_rounded;
            let plane = alpha::read(cursor.take(head.size), width as u16, height as u16)?;
            cursor.seek(next_chunk);
            let image_head = container::read_chunk_head(cursor)?;
            if head.size + image_head.size + FRAME_CHUNK_BYTES_MIN > frame_size {
                return Err(WebpError::ChunkHeader(image_head.fourcc));
            }
            let area_max = pixel_count as u64;
            let image =
                lossy::decode(cursor.take(image_head.size), Expected::AreaAtMost(area_max))?;
            let mut rgba = vec![0; pixel_count * RGBA_BYTES_PER_PIXEL];
            yuv::fill_fancy::<RGBA_BYTES_PER_PIXEL>(&mut rgba, &image)?;
            alpha::apply(
                &plane,
                usize::from(image.width),
                usize::from(image.height),
                &mut rgba,
            )?;
            Ok((rgba, true))
        }
        fourcc => Err(WebpError::ChunkHeader(fourcc)),
    }
}

fn composite(
    canvas: &mut [u8],
    canvas_header: Header,
    pixels: &[u8],
    pixels_alpha: bool,
    rectangle: &FrameRectangle,
) {
    let full_size = rectangle.x == 0
        && rectangle.y == 0
        && rectangle.width == canvas_header.width
        && rectangle.height == canvas_header.height;
    if full_size && !rectangle.blend {
        if pixels_alpha {
            canvas.copy_from_slice(pixels);
        } else {
            for (source, destination) in pixels
                .as_chunks::<RGB_BYTES_PER_PIXEL>()
                .0
                .iter()
                .zip(canvas.as_chunks_mut::<RGBA_BYTES_PER_PIXEL>().0.iter_mut())
            {
                destination[..RGB_BYTES_PER_PIXEL].copy_from_slice(source);
                destination[RGB_BYTES_PER_PIXEL] = ALPHA_OPAQUE;
            }
        }
        return;
    }
    let canvas_width = canvas_header.width as usize;
    let frame_width = rectangle.width as usize;
    let width = rectangle
        .width
        .min(canvas_header.width.saturating_sub(rectangle.x)) as usize;
    let height = rectangle
        .height
        .min(canvas_header.height.saturating_sub(rectangle.y)) as usize;
    let x = rectangle.x as usize;
    let y = rectangle.y as usize;
    for row in 0..height {
        let canvas_start = (x + (y + row) * canvas_width) * RGBA_BYTES_PER_PIXEL;
        let destination = &mut canvas[canvas_start..][..width * RGBA_BYTES_PER_PIXEL];
        if pixels_alpha {
            let source =
                &pixels[row * frame_width * RGBA_BYTES_PER_PIXEL..][..width * RGBA_BYTES_PER_PIXEL];
            if rectangle.blend {
                for (input, output) in source.as_chunks::<RGBA_BYTES_PER_PIXEL>().0.iter().zip(
                    destination
                        .as_chunks_mut::<RGBA_BYTES_PER_PIXEL>()
                        .0
                        .iter_mut(),
                ) {
                    let blended = blend(
                        [input[0], input[1], input[2], input[3]],
                        [output[0], output[1], output[2], output[3]],
                    );
                    output.copy_from_slice(&blended);
                }
            } else {
                destination.copy_from_slice(source);
            }
        } else {
            let source =
                &pixels[row * frame_width * RGB_BYTES_PER_PIXEL..][..width * RGB_BYTES_PER_PIXEL];
            for (input, output) in source.as_chunks::<RGB_BYTES_PER_PIXEL>().0.iter().zip(
                destination
                    .as_chunks_mut::<RGBA_BYTES_PER_PIXEL>()
                    .0
                    .iter_mut(),
            ) {
                output[..RGB_BYTES_PER_PIXEL].copy_from_slice(input);
                output[RGB_BYTES_PER_PIXEL] = ALPHA_OPAQUE;
            }
        }
    }
}

const BLEND_SCALE_BITS: u32 = 24;

const fn divide_by_255(value: u32) -> u32 {
    (((value + 0x80) >> 8) + value + 0x80) >> 8
}

fn blend(source: [u8; 4], destination: [u8; 4]) -> [u8; 4] {
    let source_alpha = u32::from(source[3]);
    if source_alpha == 0 {
        return destination;
    }
    let destination_factor = divide_by_255(u32::from(destination[3]) * (255 - source_alpha));
    let blend_alpha = source_alpha + destination_factor;
    assert!(blend_alpha <= u32::from(u8::MAX));
    let scale = (1u32 << BLEND_SCALE_BITS) / blend_alpha;
    let channel = |index: usize| {
        let mixed = u32::from(source[index]) * source_alpha
            + u32::from(destination[index]) * destination_factor;
        ((mixed * scale) >> BLEND_SCALE_BITS) as u8
    };
    [channel(0), channel(1), channel(2), blend_alpha as u8]
}
