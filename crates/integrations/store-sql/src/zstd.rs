use std::io::Read as _;

use nanocode_durability_core::{PageCodec, StoreError};

pub struct Zstd {
    level: i32,
}

impl Zstd {
    #[must_use]
    pub const fn level(level: i32) -> Self {
        Self { level }
    }
}

impl PageCodec for Zstd {
    fn encode(&self, page: &str) -> Result<Vec<u8>, StoreError> {
        zstd::bulk::compress(page.as_bytes(), self.level)
            .map_err(|error| StoreError::NotCommitted(format!("packing a page: {error}")))
    }

    fn decode(&self, packed: &[u8]) -> Result<String, StoreError> {
        let mut page = String::new();
        zstd::stream::read::Decoder::new(packed)
            .and_then(|mut decoder| decoder.read_to_string(&mut page))
            .map_err(|error| {
                StoreError::Backend(format!("a packed page does not read: {error}"))
            })?;
        Ok(page)
    }
}
