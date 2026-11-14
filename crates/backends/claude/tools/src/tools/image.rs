const MAX_DECODE_BYTES: u64 = 256 * 1024 * 1024;
const MAX_DIMENSION: u32 = 32768;

pub fn media_type(format: nanocode_image::Format) -> Result<Option<&'static str>, String> {
    match format {
        nanocode_image::Format::Png => Ok(Some("image/png")),
        nanocode_image::Format::Jpeg => Ok(Some("image/jpeg")),
        nanocode_image::Format::Gif => Ok(Some("image/gif")),
        nanocode_image::Format::WebP => Ok(Some("image/webp")),
        nanocode_image::Format::Bmp | nanocode_image::Format::Pnm => Ok(None),
        _ => Err("unsupported image format; use PNG, JPEG, GIF, WebP, BMP or PNM".into()),
    }
}

pub fn decode(
    bytes: &[u8],
    format: nanocode_image::Format,
) -> Result<nanocode_image::Image, String> {
    let limits = nanocode_image::Limits {
        alloc_bytes_max: MAX_DECODE_BYTES,
        dimension_max: Some(MAX_DIMENSION),
    };
    nanocode_image::Decoder::new(bytes, format, limits)
        .and_then(|mut decoder| {
            decoder.reserve()?;
            decoder.decode()
        })
        .map(|decoded| decoded.image)
        .map_err(|e| format!("invalid or oversized image: {e}"))
}
