use std::{fmt, future::Future, time::Duration};

use super::RemoteCatalog;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CatalogError {
    NoCatalog,
    Outdated,
    Invalid(String),
}

impl fmt::Display for CatalogError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoCatalog => f.write_str(super::NO_CATALOG),
            Self::Outdated => f.write_str("first frame is not a remote code mode catalog"),
            Self::Invalid(error) => f.write_str(error),
        }
    }
}

impl std::error::Error for CatalogError {}

pub async fn read_catalog(
    first: impl Future<Output = Option<Option<String>>>,
    timeout: Duration,
    max_frame_bytes: usize,
) -> Result<RemoteCatalog, CatalogError> {
    let Ok(Some(frame)) = nanocode_platform::timeout(timeout, first).await else {
        return Err(CatalogError::NoCatalog);
    };
    let text = frame.ok_or(CatalogError::Outdated)?;
    if !serde_json::from_str::<serde_json::Value>(&text)
        .is_ok_and(|frame| frame.get("definitions").is_some())
    {
        return Err(CatalogError::Outdated);
    }
    RemoteCatalog::decode(&text, max_frame_bytes)
        .map_err(|error| CatalogError::Invalid(error.to_string()))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Admission {
    Take { replaces: bool },
    Wait,
    Retry,
    Fence,
}

#[must_use]
pub fn admit_runtime(
    previous: Option<&str>,
    connected_before: bool,
    live: Option<(impl AsRef<str>, bool)>,
    advertised: &str,
) -> Admission {
    let replaces = previous.is_some_and(|previous| previous != advertised);
    match live {
        _ if replaces && connected_before => Admission::Fence,
        Some((current, _)) if current.as_ref() != advertised => Admission::Wait,
        Some((_, true)) => Admission::Retry,
        Some((_, false)) | None => Admission::Take { replaces },
    }
}
