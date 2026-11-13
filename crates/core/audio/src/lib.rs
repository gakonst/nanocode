mod bytes;
mod matroska;
mod mp4;
mod mpeg;
mod ogg;
mod wav;

use nanocode_digest::Sha1;
use std::collections::VecDeque;
use std::sync::LazyLock;

use nanocode_sync::Swap;

const BASE64_ERROR: &str = "Base64Error";
const AUDIO_TOKEN_ESTIMATE_CACHE_SIZE: usize = 32;
const AUDIO_TOKENS_PER_SECOND: f64 = 10.0;
const APPROX_BYTES_PER_TOKEN: usize = 4;

type AudioEstimates = VecDeque<([u8; 20], usize)>;
static AUDIO_TOKEN_ESTIMATE_CACHE: LazyLock<Swap<AudioEstimates>> = LazyLock::new(Swap::default);

fn remember(key: [u8; 20], tokens: usize) {
    AUDIO_TOKEN_ESTIMATE_CACHE.rcu(|cache| {
        let mut cache = VecDeque::clone(cache);
        cache.retain(|(cached, _)| *cached != key);
        if cache.len() >= AUDIO_TOKEN_ESTIMATE_CACHE_SIZE {
            cache.pop_front();
        }
        cache.push_back((key, tokens));
        cache
    });
}

#[must_use]
pub fn estimate_audio_token_count(audio_url: &str) -> usize {
    let key: [u8; 20] = Sha1::digest(audio_url.as_bytes());
    let cached = AUDIO_TOKEN_ESTIMATE_CACHE
        .load()
        .iter()
        .find(|(cached, _)| *cached == key)
        .map(|(_, tokens)| *tokens);
    let tokens = cached.unwrap_or_else(|| uncached_audio_token_count(audio_url));
    remember(key, tokens);
    tokens
}

fn uncached_audio_token_count(audio_url: &str) -> usize {
    let Some(duration_seconds) = audio_duration_seconds(audio_url) else {
        return approx_token_count(audio_url);
    };
    let token_count = (duration_seconds * AUDIO_TOKENS_PER_SECOND).ceil();
    if token_count >= usize::MAX as f64 {
        usize::MAX
    } else {
        token_count as usize
    }
}

const fn approx_token_count(text: &str) -> usize {
    text.len()
        .saturating_add(APPROX_BYTES_PER_TOKEN.saturating_sub(1))
        / APPROX_BYTES_PER_TOKEN
}

const fn is_supported_audio_mime(mime: &str) -> bool {
    mime.eq_ignore_ascii_case("audio/wav")
        || mime.eq_ignore_ascii_case("audio/x-wav")
        || mime.eq_ignore_ascii_case("audio/wave")
        || mime.eq_ignore_ascii_case("audio/vnd.wave")
        || mime.eq_ignore_ascii_case("audio/mpeg")
        || mime.eq_ignore_ascii_case("audio/mp3")
        || mime.eq_ignore_ascii_case("audio/mp4")
        || mime.eq_ignore_ascii_case("audio/m4a")
        || mime.eq_ignore_ascii_case("audio/x-m4a")
        || mime.eq_ignore_ascii_case("audio/webm")
        || mime.eq_ignore_ascii_case("audio/ogg")
}

fn audio_duration_seconds(audio_url: &str) -> Option<f64> {
    let (metadata, payload) = audio_url.split_once(',')?;
    let metadata = metadata.get("data:".len()..)?;
    let mut metadata_parts = metadata.split(';');
    if !is_supported_audio_mime(metadata_parts.next()?) {
        return None;
    }
    if !metadata_parts.any(|part| part.eq_ignore_ascii_case("base64")) {
        return None;
    }

    let bytes = match nanocode_sys::base64::STANDARD.decode(payload) {
        Ok(bytes) => bytes,
        Err(_) => {
            tracing::trace!(
                error = BASE64_ERROR,
                "failed to decode audio payload for token estimation"
            );
            return None;
        }
    };
    let Some(duration_seconds) = container_duration_seconds(&bytes) else {
        tracing::trace!("failed to read audio duration for token estimation");
        return None;
    };
    duration_seconds.is_finite().then_some(duration_seconds)
}

fn container_duration_seconds(bytes: &[u8]) -> Option<f64> {
    let start = bytes.windows(4).position(|magic| {
        magic == b"RIFF"
            || magic == b"OggS"
            || magic == matroska::EBML_MAGIC
            || magic == b"ftyp"
            || magic[0] == 0xff && magic[1] & 0xe0 == 0xe0
            || magic.starts_with(b"ID3")
    })?;
    let start = if bytes.get(start..start + 4) == Some(b"ftyp") {
        start.checked_sub(4)?
    } else {
        start
    };
    let bytes = &bytes[start..];
    if bytes.starts_with(b"RIFF") {
        wav::duration_seconds(bytes)
    } else if bytes.starts_with(b"OggS") {
        ogg::duration_seconds(bytes)
    } else if bytes.starts_with(&matroska::EBML_MAGIC) {
        matroska::duration_seconds(bytes)
    } else if bytes.get(4..8) == Some(b"ftyp") {
        mp4::duration_seconds(bytes)
    } else {
        mpeg::duration_seconds(bytes)
    }
}

fn ticks_to_seconds(ticks: u64, numer: u64, denom: u64) -> Option<f64> {
    (denom != 0).then(|| ticks as f64 * numer as f64 / denom as f64)
}
