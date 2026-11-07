use std::{collections::HashMap, sync::Arc};

use nanocode_durability_core::{BlobKey, BlobStore, StoreError, StoreFuture};
use nanocode_sync::Swap;

#[derive(Clone, Default)]
pub struct MemoryBlobStore {
    blobs: Arc<Swap<HashMap<BlobKey, Arc<[u8]>>>>,
}

impl MemoryBlobStore {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.blobs.load().len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.blobs.load().is_empty()
    }

    #[must_use]
    pub fn bytes(&self) -> usize {
        self.blobs.load().values().map(|bytes| bytes.len()).sum()
    }
}

impl BlobStore for MemoryBlobStore {
    fn put<'a>(
        &'a self,
        key: &'a BlobKey,
        bytes: &'a [u8],
    ) -> StoreFuture<'a, Result<(), StoreError>> {
        let stored = if key.matches(bytes) {
            if !self.blobs.load().contains_key(key) {
                let bytes: Arc<[u8]> = bytes.into();
                self.blobs.rcu(|blobs| {
                    let mut blobs = HashMap::clone(blobs);
                    blobs
                        .entry(key.clone())
                        .or_insert_with(|| Arc::clone(&bytes));
                    blobs
                });
            }
            Ok(())
        } else {
            Err(StoreError::NotCommitted(format!(
                "blob {key} does not hold the content its key names"
            )))
        };
        Box::pin(async move { stored })
    }

    fn get<'a>(&'a self, key: &'a BlobKey) -> StoreFuture<'a, Result<Option<Vec<u8>>, StoreError>> {
        let bytes = self.blobs.load().get(key).map(|bytes| bytes.to_vec());
        Box::pin(async move { Ok(bytes) })
    }
}
