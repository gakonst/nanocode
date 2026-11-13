use alloc::vec;
use alloc::vec::Vec;

use crate::error::WebpError;
use crate::lossless;

const PREPROCESSING_SHIFT: u8 = 4;
const FILTERING_SHIFT: u8 = 2;
const FIELD_MASK: u8 = 0b11;
const PREPROCESSING_LEVEL_MAX: u8 = 1;
const COMPRESSION_RAW: u8 = 0;
const COMPRESSION_LOSSLESS: u8 = 1;
const RGBA_BYTES_PER_PIXEL: usize = 4;
const ALPHA_OFFSET: usize = 3;
const GREEN_OFFSET: usize = 1;

#[derive(Clone, Copy)]
enum Filtering {
    None,
    Horizontal,
    Vertical,
    Gradient,
}

pub(crate) struct AlphaPlane {
    filtering: Filtering,
    values: Vec<u8>,
}

pub(crate) fn read(stream: &[u8], width: u16, height: u16) -> Result<AlphaPlane, WebpError> {
    let Some((&info, rest)) = stream.split_first() else {
        return Err(WebpError::Truncated);
    };
    if (info >> PREPROCESSING_SHIFT) & FIELD_MASK > PREPROCESSING_LEVEL_MAX {
        return Err(WebpError::AlphaPreprocessing);
    }
    let filtering = match (info >> FILTERING_SHIFT) & FIELD_MASK {
        0 => Filtering::None,
        1 => Filtering::Horizontal,
        2 => Filtering::Vertical,
        _ => Filtering::Gradient,
    };
    let count = usize::from(width) * usize::from(height);
    let values = match info & FIELD_MASK {
        COMPRESSION_RAW => {
            if rest.len() < count {
                return Err(WebpError::Truncated);
            }
            rest[..count].to_vec()
        }
        COMPRESSION_LOSSLESS => {
            let mut rgba = vec![0; count * RGBA_BYTES_PER_PIXEL];
            lossless::decode_implicit(rest, width, height, &mut rgba)?;
            rgba.as_chunks::<RGBA_BYTES_PER_PIXEL>()
                .0
                .iter()
                .map(|pixel| pixel[GREEN_OFFSET])
                .collect()
        }
        _ => return Err(WebpError::AlphaCompression),
    };
    Ok(AlphaPlane { filtering, values })
}

pub(crate) fn apply(
    plane: &AlphaPlane,
    width: usize,
    height: usize,
    rgba: &mut [u8],
) -> Result<(), WebpError> {
    let count = width * height;
    if count > plane.values.len() || count * RGBA_BYTES_PER_PIXEL > rgba.len() {
        return Err(WebpError::FramePixels);
    }
    let pixel = RGBA_BYTES_PER_PIXEL;
    let row_bytes = width * pixel;
    let values = &plane.values[..count];
    let alpha = &mut rgba[..count * pixel];
    if let Filtering::None = plane.filtering {
        for (output, &value) in alpha
            .as_chunks_mut::<RGBA_BYTES_PER_PIXEL>()
            .0
            .iter_mut()
            .zip(values)
        {
            output[ALPHA_OFFSET] = value;
        }
        return Ok(());
    }
    for (index, &value) in values.iter().enumerate() {
        let position = index * pixel + ALPHA_OFFSET;
        let x = index % width;
        let predictor = match (x, index < width, plane.filtering) {
            (0, true, _) => 0,
            (_, true, _) => alpha[position - pixel],
            (0, false, _) => alpha[position - row_bytes],
            (_, false, Filtering::Horizontal) => alpha[position - pixel],
            (_, false, Filtering::Gradient) => {
                let left = i16::from(alpha[position - pixel]);
                let top = i16::from(alpha[position - row_bytes]);
                let top_left = i16::from(alpha[position - row_bytes - pixel]);
                (left + top - top_left).clamp(0, i16::from(u8::MAX)) as u8
            }
            (_, false, _) => alpha[position - row_bytes],
        };
        alpha[position] = predictor.wrapping_add(value);
    }
    Ok(())
}
