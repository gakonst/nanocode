use std::time::Duration;

use http::{HeaderMap, HeaderName};

const SHOULD_RETRY: HeaderName = HeaderName::from_static("x-should-retry");

const RETRY_AFTER_MS: HeaderName = HeaderName::from_static("retry-after-ms");

#[must_use]
pub fn should_retry(headers: &HeaderMap) -> Option<bool> {
    match headers.get(SHOULD_RETRY)?.as_bytes() {
        b"true" => Some(true),
        b"false" => Some(false),
        _ => None,
    }
}

pub const PATIENCE: Duration = Duration::from_secs(600);

#[must_use]
pub const fn retryable_status(status: u16) -> bool {
    matches!(status, 408 | 409 | 429 | 500..=599)
}

#[must_use]
pub const fn patient_status(status: u16) -> bool {
    matches!(status, 429 | 502 | 503 | 504 | 529)
}

#[must_use]
pub fn retry_after(headers: &HeaderMap) -> Option<Duration> {
    retry_after_ms(headers).or_else(|| {
        let seconds = headers
            .get(http::header::RETRY_AFTER)?
            .to_str()
            .ok()?
            .trim()
            .parse()
            .ok()?;
        Some(Duration::from_secs(seconds))
    })
}

#[must_use]
pub fn retry_after_ms(headers: &HeaderMap) -> Option<Duration> {
    let milliseconds: f64 = headers
        .get(RETRY_AFTER_MS)?
        .to_str()
        .ok()?
        .trim()
        .parse()
        .ok()?;
    Duration::try_from_secs_f64(milliseconds / 1000.0).ok()
}
