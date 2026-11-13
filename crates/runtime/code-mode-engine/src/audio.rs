
const MAX_PROMPT_AUDIO_INPUT_BYTES: usize = 50 * 1024 * 1024;

pub(super) fn wav_duration_seconds(audio_url: &str) -> Option<f64> {
    let (metadata, payload) = audio_url.split_once(',')?;
    if !metadata
        .split(';')
        .skip(1)
        .any(|part| part.eq_ignore_ascii_case("base64"))
        || payload.len() > MAX_PROMPT_AUDIO_INPUT_BYTES.div_ceil(3) * 4
    {
        return None;
    }
    let bytes = nanocode_sys::base64::STANDARD
        .decode(payload)
        .ok()?;
    if bytes.get(..4)? != b"RIFF" || bytes.get(8..12)? != b"WAVE" {
        return None;
    }

    let mut chunks = bytes.get(12..)?;
    let mut format = None;
    while chunks.len() >= 8 {
        let chunk_id = &chunks[..4];
        let size = u32::from_le_bytes(chunks[4..8].try_into().ok()?) as usize;
        let remaining = &chunks[8..];

        let chunk = &remaining[..size.min(remaining.len())];
        match chunk_id {
            b"fmt " => {
                let mut encoding = u16::from_le_bytes(chunk.get(..2)?.try_into().ok()?);
                if encoding == 0xfffe {
                    if chunk.get(26..40)?
                        != [0, 0, 0, 0, 0x10, 0, 0x80, 0, 0, 0xaa, 0, 0x38, 0x9b, 0x71]
                    {
                        return None;
                    }
                    encoding = u16::from_le_bytes(chunk.get(24..26)?.try_into().ok()?);
                }
                if !matches!(encoding, 1 | 3) {
                    return None;
                }
                let sample_rate = u32::from_le_bytes(chunk.get(4..8)?.try_into().ok()?);
                let block_align = u16::from_le_bytes(chunk.get(12..14)?.try_into().ok()?);
                if sample_rate == 0 || block_align == 0 {
                    return None;
                }
                format = Some((sample_rate, block_align));
            }
            b"data" => {
                let (sample_rate, block_align) = format?;
                let frames = chunk.len() / usize::from(block_align);
                return Some(frames as f64 / f64::from(sample_rate));
            }
            _ => {}
        }
        chunks = remaining.get(size.checked_add(size % 2)?..)?;
    }
    None
}
