use std::sync::Arc;

use nanocode_platform::{MaybeSend, MaybeSync};

use base64::{Engine as _, engine::general_purpose::STANDARD};

use crate::{StoreError, StoreFuture};

const KEY_PREFIX: &str = "b3:";
const URL_SCHEME: &str = "nanocode-blob:";

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct BlobKey(Arc<str>);

impl BlobKey {
    #[must_use]
    pub fn of(bytes: &[u8]) -> Self {
        Self(format!("{KEY_PREFIX}{}", blake3::hash(bytes).to_hex()).into())
    }

    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let hash = text.strip_prefix(KEY_PREFIX)?;
        (hash.len() == 64
            && hash
                .bytes()
                .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f')))
        .then(|| Self(text.into()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn matches(&self, bytes: &[u8]) -> bool {
        *self == Self::of(bytes)
    }
}

impl std::fmt::Display for BlobKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

pub trait BlobStore: MaybeSend + MaybeSync {
    fn put<'a>(
        &'a self,
        key: &'a BlobKey,
        bytes: &'a [u8],
    ) -> StoreFuture<'a, Result<(), StoreError>>;

    fn get<'a>(&'a self, key: &'a BlobKey) -> StoreFuture<'a, Result<Option<Vec<u8>>, StoreError>>;
}

pub type SharedBlobStore = Arc<dyn BlobStore + Send + Sync>;

pub async fn get_blob(store: &dyn BlobStore, key: &BlobKey) -> Result<Vec<u8>, StoreError> {
    let bytes = store
        .get(key)
        .await?
        .ok_or_else(|| StoreError::Backend(format!("blob {key} is missing")))?;
    if !key.matches(&bytes) {
        return Err(StoreError::Backend(format!(
            "blob {key} does not hold the content its key names"
        )));
    }
    Ok(bytes)
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct BlobUrl {
    media_type: String,
    key: BlobKey,
}

impl BlobUrl {
    #[must_use]
    pub fn parse(url: &str) -> Option<Self> {
        let rest = url.strip_prefix(URL_SCHEME)?;
        let (media_type, key) = rest.rsplit_once(';')?;
        if media_type.is_empty() || media_type.contains([',', ';']) {
            return None;
        }
        Some(Self {
            media_type: media_type.to_owned(),
            key: BlobKey::parse(key)?,
        })
    }

    #[must_use]
    pub fn is_blob_url(url: &str) -> bool {
        url.starts_with(URL_SCHEME)
    }

    #[must_use]
    pub const fn key(&self) -> &BlobKey {
        &self.key
    }

    #[must_use]
    pub fn from_data_url(url: &str) -> Option<(Self, Vec<u8>)> {
        let (media_type, data) = base64_data_url(url)?;
        let bytes = STANDARD.decode(data).ok()?;
        let blob = Self {
            media_type: media_type.to_owned(),
            key: BlobKey::of(&bytes),
        };
        Some((blob, bytes))
    }

    pub async fn store_data_url(
        store: &dyn BlobStore,
        url: &str,
    ) -> Result<Option<Self>, StoreError> {
        let Some((blob, bytes)) = Self::from_data_url(url) else {
            return Ok(None);
        };
        store.put(&blob.key, &bytes).await?;
        Ok(Some(blob))
    }

    pub async fn load_data_url(&self, store: &dyn BlobStore) -> Result<String, StoreError> {
        let bytes = get_blob(store, &self.key).await?;
        let mut url = String::with_capacity(
            "data:;base64,".len() + self.media_type.len() + bytes.len().div_ceil(3) * 4,
        );
        url.push_str("data:");
        url.push_str(&self.media_type);
        url.push_str(";base64,");
        STANDARD.encode_string(&bytes, &mut url);
        Ok(url)
    }
}

impl std::fmt::Display for BlobUrl {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{URL_SCHEME}{};{}", self.media_type, self.key)
    }
}

pub fn base64_data_url(url: &str) -> Option<(&str, &str)> {
    let (header, data) = url.strip_prefix("data:")?.split_once(',')?;
    let media_type = header.strip_suffix(";base64")?;
    (!media_type.is_empty() && !media_type.contains(';')).then_some((media_type, data))
}
