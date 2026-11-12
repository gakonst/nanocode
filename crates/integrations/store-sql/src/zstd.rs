use nanocode_durability_core::{PageCodec, StoreError};
use nanocode_zstd::StreamingDecoder;

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
        nanocode_zstd::compress(page.as_bytes(), self.level)
            .map_err(|error| StoreError::NotCommitted(format!("packing a page: {error}")))
    }

    fn decode(&self, packed: &[u8]) -> Result<String, StoreError> {
        let unreadable = |error: &dyn std::fmt::Display| {
            StoreError::Backend(format!("a packed page does not read: {error}"))
        };
        let mut decoder = StreamingDecoder::default();
        decoder.push(packed).map_err(|error| unreadable(&error))?;
        decoder.finish().map_err(|error| unreadable(&error))?;
        String::from_utf8(decoder.take_output()).map_err(|error| unreadable(&error))
    }
}
