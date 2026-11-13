use super::{Image, ImagePreparationError, MAX_PROMPT_IMAGE_DECODE_BYTES, Pixels};

pub(super) fn decode_alpha(bytes: &[u8]) -> Result<Option<Image>, ImagePreparationError> {
    if !bytes.starts_with(b"P7\n") && !bytes.starts_with(b"P7\r\n") {
        return Ok(None);
    }
    let invalid = || {
        ImagePreparationError::Processing(
            "unable to decode PAM alpha image: invalid header or pixels".into(),
        )
    };
    let mut offset = 0usize;
    let mut width = None;
    let mut height = None;
    let mut depth = None;
    let mut maxval = None;
    let mut tuple = None;
    loop {
        let remaining = bytes.get(offset..).ok_or_else(invalid)?;
        let length = memchr::memchr(b'\n', remaining).ok_or_else(invalid)?;
        let line = std::str::from_utf8(&remaining[..length])
            .map_err(|_| invalid())?
            .trim();
        offset += length + 1;
        if line == "ENDHDR" {
            break;
        }
        if line.is_empty() || line.starts_with('#') || line == "P7" {
            continue;
        }
        let Some((key, value)) = line.split_once(char::is_whitespace) else {
            return Err(invalid());
        };
        let value = value.trim();
        match key {
            "WIDTH" => width = Some(value.parse::<u32>().map_err(|_| invalid())?),
            "HEIGHT" => height = Some(value.parse::<u32>().map_err(|_| invalid())?),
            "DEPTH" => depth = Some(value.parse::<u32>().map_err(|_| invalid())?),
            "MAXVAL" => maxval = Some(value.parse::<u32>().map_err(|_| invalid())?),
            "TUPLTYPE" => tuple = Some(value),
            _ => return Err(invalid()),
        }
    }
    let channels = match tuple {
        Some("BLACKANDWHITE_ALPHA" | "GRAYSCALE_ALPHA") => 2,
        Some("RGB_ALPHA") => 4,
        _ => return Ok(None),
    };
    let width = width.ok_or_else(invalid)?;
    let height = height.ok_or_else(invalid)?;
    let maxval = maxval.ok_or_else(invalid)?;
    if depth != Some(channels)
        || maxval == 0
        || maxval > u16::MAX.into()
        || (tuple == Some("BLACKANDWHITE_ALPHA") && maxval != 1)
    {
        return Err(invalid());
    }
    let sample_bytes = if maxval <= u8::MAX.into() { 1 } else { 2 };
    let size = u64::from(width) * u64::from(height);
    let size = size
        .checked_mul(u64::from(channels) * sample_bytes)
        .ok_or_else(invalid)?;
    if size > MAX_PROMPT_IMAGE_DECODE_BYTES {
        return Err(ImagePreparationError::ImageTooLarge {
            representation: "decoded pixels",
            size: usize::try_from(size).unwrap_or(usize::MAX),
            max: MAX_PROMPT_IMAGE_DECODE_BYTES as usize,
        });
    }
    let pixels = bytes
        .get(offset..offset.checked_add(size as usize).ok_or_else(invalid)?)
        .ok_or_else(invalid)?;
    let dynamic = if sample_bytes == 1 {
        let factor = f32::from(u8::MAX) / maxval as f32;
        let pixels = if maxval == u32::from(u8::MAX) {
            pixels.to_vec()
        } else {
            pixels
                .iter()
                .map(|value| (f32::from(*value) * factor).round() as u8)
                .collect()
        };
        let pixels = if channels == 2 {
            Pixels::LumaAlpha8(pixels)
        } else {
            Pixels::Rgba8(pixels)
        };
        Image::new(width, height, pixels).ok_or_else(invalid)?
    } else {
        let factor = f32::from(u16::MAX) / maxval as f32;
        let pixels = if maxval == u32::from(u16::MAX) {
            pixels
                .as_chunks::<2>()
                .0
                .iter()
                .map(|value| u16::from_be_bytes(*value))
                .collect()
        } else {
            pixels
                .as_chunks::<2>()
                .0
                .iter()
                .map(|value| (f32::from(u16::from_be_bytes(*value)) * factor).round() as u16)
                .collect()
        };
        let pixels = if channels == 2 {
            Pixels::LumaAlpha16(pixels)
        } else {
            Pixels::Rgba16(pixels)
        };
        Image::new(width, height, pixels).ok_or_else(invalid)?
    };
    Ok(Some(dynamic))
}
