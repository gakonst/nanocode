use std::sync::Arc;

use serde::{Serialize, de::DeserializeOwned};

use crate::{Error, Result};

const INLINE_BYTES: usize = 16 * 1024;

const CHUNK_MIN: usize = 2 * 1024;
const CHUNK_AVG: usize = 8 * 1024;
const CHUNK_MAX: usize = 32 * 1024;

const CHUNK_MASK_SMALL: u64 = !(u64::MAX >> 15);
const CHUNK_MASK_LARGE: u64 = !(u64::MAX >> 11);

const MANIFEST_PREFIX: char = '#';

const GEAR: [u64; 256] = gear_table();

const fn gear_table() -> [u64; 256] {
    let mut table = [0_u64; 256];
    let mut state: u64 = 0x6e61_6e6f_636f_6465;
    let mut index = 0;
    while index < 256 {
        state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut value = state;
        value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        table[index] = value ^ (value >> 31);
        index += 1;
    }
    table
}

fn chunk_end(content: &str) -> usize {
    let bytes = content.as_bytes();
    let length = bytes.len();

    if length <= CHUNK_MIN * 2 {
        return length;
    }
    let limit = length.min(CHUNK_MAX);
    let normal = limit.min(CHUNK_AVG);
    let mut hash = 0_u64;
    let mut index = CHUNK_MIN;
    let mut cut = limit;
    while index < limit {
        hash = (hash << 1).wrapping_add(GEAR[bytes[index] as usize]);
        let mask = if index < normal {
            CHUNK_MASK_SMALL
        } else {
            CHUNK_MASK_LARGE
        };
        if hash & mask == 0 {
            cut = index + 1;
            break;
        }
        index += 1;
    }
    while cut < length && !content.is_char_boundary(cut) {
        cut += 1;
    }
    cut
}

fn chunk_key(hash: &str) -> String {
    format!("c:{hash}")
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct EncodedPayload {
    key: Arc<str>,
    #[serde(skip)]
    content: Option<Arc<str>>,
    #[serde(skip)]
    pending: Vec<crate::StoreRecord>,
}

impl EncodedPayload {
    pub fn key(&self) -> &str {
        &self.key
    }

    pub fn encode<T: Serialize + ?Sized>(value: &T) -> Result<Self> {
        let json = serde_json::to_string(value).map_err(Error::InvalidPayload)?;
        Ok(Self {
            key: record_key(&json).into(),
            content: Some(json.into()),
            pending: Vec::new(),
        })
    }

    pub fn from_json(json: Arc<str>) -> Self {
        Self {
            key: record_key(&json).into(),
            content: Some(json),
            pending: Vec::new(),
        }
    }

    pub fn by_key(key: &str) -> Self {
        Self {
            key: key.into(),
            content: None,
            pending: Vec::new(),
        }
    }

    pub fn decode<T: DeserializeOwned>(&self) -> Result<T> {
        serde_json::from_str(self.json()?).map_err(Error::InvalidPayload)
    }

    pub fn json(&self) -> Result<&str> {
        self.content.as_deref().ok_or_else(|| {
            Error::InvalidState(
                "resolve the payload through its durable session before reading it".into(),
            )
        })
    }

    #[cfg_attr(target_family = "wasm", allow(dead_code))]
    pub fn from_key(key: &str) -> Self {
        Self {
            key: key.into(),
            content: None,
            pending: Vec::new(),
        }
    }

    pub fn reference(&self) -> Self {
        Self {
            key: self.key.clone(),
            content: None,
            pending: Vec::new(),
        }
    }

    pub fn with_records(mut self, records: Vec<crate::StoreRecord>) -> Self {
        self.pending = records;
        self
    }

    pub fn stage(&mut self, records: &mut Vec<crate::StoreRecord>) {
        records.append(&mut self.pending);
        let Some(content) = self.content.take() else {
            return;
        };
        if content.len() < INLINE_BYTES {
            records.push(crate::StoreRecord {
                key: self.key.to_string(),
                value: format!("={content}"),
            });
            return;
        }

        let mut manifest = String::with_capacity(1 + content.len() / CHUNK_MIN * 64);
        manifest.push(MANIFEST_PREFIX);
        let mut offset = 0;
        while offset < content.len() {
            let end = offset + chunk_end(&content[offset..]);
            let chunk = &content[offset..end];
            let hash = record_key(chunk);
            records.push(crate::StoreRecord {
                key: chunk_key(&hash),
                value: chunk.to_owned(),
            });
            manifest.push_str(&hash);
            offset = end;
        }
        records.push(crate::StoreRecord {
            key: self.key.to_string(),
            value: manifest,
        });
    }

    pub async fn load(&self, store: &mut dyn crate::StateStore, state_id: &str) -> Result<Self> {
        if self.content.is_some() {
            return Ok(self.clone());
        }
        let record = store
            .read_record(state_id, &self.key)
            .await?
            .ok_or_else(|| Error::InvalidState(format!("missing payload record {}", self.key)))?;
        self.load_record(store, state_id, record).await
    }

    pub async fn read(&self, reader: &mut dyn crate::StateReader, state_id: &str) -> Result<Self> {
        if self.content.is_some() {
            return Ok(self.clone());
        }
        let record = reader
            .read_records(state_id, std::slice::from_ref(&self.key.to_string()))
            .await?
            .pop()
            .flatten()
            .ok_or_else(|| Error::InvalidState(format!("missing payload record {}", self.key)))?;
        self.load_record(reader, state_id, record).await
    }

    pub async fn load_many(
        values: &[Self],
        store: &mut dyn crate::StateStore,
        state_id: &str,
    ) -> Result<Vec<Self>> {
        let mut result = Vec::with_capacity(values.len());
        for page in values.chunks(16) {
            let keys: Vec<_> = page.iter().map(|value| value.key.to_string()).collect();
            let records = store.read_records(state_id, &keys).await?;
            if records.len() != page.len() {
                return Err(Error::InvalidState("record batch length mismatch".into()));
            }
            for (value, record) in page.iter().zip(records) {
                let record = record.ok_or_else(|| {
                    Error::InvalidState(format!("missing payload record {}", value.key))
                })?;
                result.push(value.load_record(store, state_id, record).await?);
            }
        }
        Ok(result)
    }

    async fn load_record<S: RecordSource + ?Sized>(
        &self,
        store: &mut S,
        state_id: &str,
        record: String,
    ) -> Result<Self> {
        let content = if let Some(content) = record.strip_prefix('=') {
            content.to_owned()
        } else if let Some(hashes) = record.strip_prefix(MANIFEST_PREFIX) {
            if hashes.is_empty()
                || hashes.len() % 64 != 0
                || !hashes.bytes().all(|byte| byte.is_ascii_hexdigit())
            {
                return Err(Error::InvalidState("invalid payload chunk manifest".into()));
            }
            let keys: Vec<String> = (0..hashes.len() / 64)
                .map(|index| chunk_key(&hashes[index * 64..(index + 1) * 64]))
                .collect();
            let mut content = String::new();
            for page in keys.chunks(16) {
                let chunks = store.records(state_id, page).await?;
                if chunks.len() != page.len() {
                    return Err(Error::InvalidState("record batch length mismatch".into()));
                }
                for (key, chunk) in page.iter().zip(chunks) {
                    let chunk = chunk.ok_or_else(|| {
                        Error::InvalidState(format!("missing payload chunk {key} of {}", self.key))
                    })?;
                    content.push_str(&chunk);
                }
            }
            content
        } else {
            let count: usize = record
                .strip_prefix('+')
                .ok_or_else(|| Error::InvalidState("invalid payload record".into()))?
                .parse()
                .map_err(|_| Error::InvalidState("invalid payload record count".into()))?;
            let mut content = String::new();
            for index in 0..count {
                let chunk = store
                    .records(state_id, &[format!("{}/{index}", self.key)])
                    .await?
                    .pop()
                    .flatten()
                    .ok_or_else(|| {
                        Error::InvalidState(format!("missing payload chunk {}/{index}", self.key))
                    })?;
                content.push_str(&chunk);
            }
            content
        };
        if record_key(&content) != self.key.as_ref() {
            return Err(Error::InvalidState(format!(
                "payload record checksum mismatch {}",
                self.key
            )));
        }
        Ok(Self {
            key: self.key.clone(),
            content: Some(content.into()),
            pending: Vec::new(),
        })
    }
}

trait RecordSource {
    fn records<'a>(
        &'a mut self,
        state_id: &'a str,
        keys: &'a [String],
    ) -> crate::StoreFuture<'a, std::result::Result<Vec<Option<String>>, crate::StoreError>>;
}

impl RecordSource for dyn crate::StateStore + '_ {
    fn records<'a>(
        &'a mut self,
        state_id: &'a str,
        keys: &'a [String],
    ) -> crate::StoreFuture<'a, std::result::Result<Vec<Option<String>>, crate::StoreError>> {
        self.read_records(state_id, keys)
    }
}

impl RecordSource for dyn crate::StateReader + '_ {
    fn records<'a>(
        &'a mut self,
        state_id: &'a str,
        keys: &'a [String],
    ) -> crate::StoreFuture<'a, std::result::Result<Vec<Option<String>>, crate::StoreError>> {
        self.read_records(state_id, keys)
    }
}

fn record_key(value: &str) -> String {
    use sha2::{Digest, Sha256};
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(64);
    for byte in Sha256::digest(value.as_bytes()) {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 15) as usize] as char);
    }
    encoded
}

impl PartialEq for EncodedPayload {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key
    }
}
impl Eq for EncodedPayload {}
