use std::sync::Arc;

use futures_util::future::try_join_all;
use nanocode_platform::{MaybeSend, MaybeSync};

use crate::{
    OwnedState, OwnerId, OwnerToken, StateStore, StoreError, StoreFuture, StoreRecord, StoredState,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Versioned {
    pub version: String,
    pub value: String,
}

pub trait KeyValue: MaybeSend + MaybeSync {
    fn get<'a>(&'a self, key: &'a str) -> StoreFuture<'a, Result<Option<Versioned>, StoreError>>;

    fn get_values<'a>(
        &'a self,
        keys: &'a [String],
    ) -> StoreFuture<'a, Result<Vec<Option<String>>, StoreError>> {
        Box::pin(get_each(self, keys))
    }

    fn put_values<'a>(
        &'a self,
        entries: &'a [(String, String)],
    ) -> StoreFuture<'a, Result<(), StoreError>>;

    fn compare_and_swap<'a>(
        &'a self,
        key: &'a str,
        expected_version: Option<&'a str>,
        version: &'a str,
        value: &'a str,
    ) -> StoreFuture<'a, Result<bool, StoreError>>;

    fn list_keys<'a>(
        &'a self,
        _prefix: &'a str,
    ) -> StoreFuture<'a, Result<Vec<String>, StoreError>> {
        Box::pin(async { Err(listing_unsupported()) })
    }
}

async fn get_each<B: KeyValue + ?Sized>(
    backend: &B,
    keys: &[String],
) -> Result<Vec<Option<String>>, StoreError> {
    let mut values = Vec::with_capacity(keys.len());
    for key in keys {
        values.push(backend.get(key).await?.map(|entry| entry.value));
    }
    Ok(values)
}

fn listing_unsupported() -> StoreError {
    StoreError::Backend("this key-value store does not list keys".to_owned())
}

pub struct KeyValueStore<B: ?Sized> {
    backend: Arc<B>,
    prefix: Arc<str>,
}

impl<B: ?Sized> Clone for KeyValueStore<B> {
    fn clone(&self) -> Self {
        Self {
            backend: Arc::clone(&self.backend),
            prefix: Arc::clone(&self.prefix),
        }
    }
}

struct Head<'a> {
    fence: u64,
    revision: u64,
    owner: &'a str,
}

impl<'a> Head<'a> {
    fn parse(version: &'a str) -> Result<Self, StoreError> {
        let invalid =
            || StoreError::Backend(format!("invalid key-value state version `{version}`"));
        let mut parts = version.splitn(3, '.');
        let (Some(fence), Some(revision), Some(owner)) = (parts.next(), parts.next(), parts.next())
        else {
            return Err(invalid());
        };
        let fence = fence.parse().map_err(|_| invalid())?;
        let revision = revision.parse().map_err(|_| invalid())?;
        if fence == 0 || owner.is_empty() {
            return Err(invalid());
        }
        Ok(Self {
            fence,
            revision,
            owner,
        })
    }

    fn state(&self, value: String) -> StoredState {
        StoredState {
            revision: self.revision,
            payload: (self.revision > 0).then_some(value),
        }
    }
}

const RECORD_VERSION: &str = "record";

fn records_not_stored(error: StoreError) -> StoreError {
    StoreError::NotCommitted(format!("durability records were not stored: {error}"))
}

fn conflicting_record(key: &str) -> StoreError {
    StoreError::NotCommitted(format!("durability record `{key}` has conflicting content"))
}

fn version(fence: u64, revision: u64, owner: &str) -> String {
    assert!(
        fence > 0,
        "a key-value head is written with a granted fence"
    );
    assert!(
        !owner.is_empty(),
        "a key-value head is written with its owner"
    );
    let version = format!("{fence}.{revision}.{owner}");
    let head = Head::parse(&version);
    assert!(
        head.as_ref().is_ok_and(|head| head.fence == fence),
        "a key-value head version parses back to its fence"
    );
    assert!(
        head.as_ref().is_ok_and(|head| head.revision == revision),
        "a key-value head version parses back to its revision"
    );
    assert!(
        head.as_ref().is_ok_and(|head| head.owner == owner),
        "a key-value head version parses back to its owner"
    );
    version
}

fn unnamed_owner() -> StoreError {
    StoreError::NotCommitted("a key-value durability owner needs a non-empty id".to_owned())
}

impl<B: KeyValue + ?Sized> KeyValueStore<B> {
    pub fn new(backend: B) -> Self
    where
        B: Sized,
    {
        Self::shared(Arc::new(backend))
    }

    pub fn shared(backend: Arc<B>) -> Self {
        Self {
            backend,
            prefix: Arc::from(""),
        }
    }

    #[must_use]
    pub fn with_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.prefix = Arc::from(prefix.into());
        self
    }

    pub fn backend(&self) -> &B {
        &self.backend
    }

    pub fn prefix(&self) -> &str {
        &self.prefix
    }

    fn states(&self) -> String {
        format!("{}state/", self.prefix)
    }

    fn head_key(&self, state_id: &str) -> String {
        format!("{}state/{state_id}", self.prefix)
    }

    fn record_key(&self, state_id: &str, key: &str) -> String {
        format!("{}record/{}/{state_id}/{key}", self.prefix, state_id.len())
    }

    fn records_of(&self, state_id: &str) -> String {
        self.record_key(state_id, "")
    }

    async fn acquire_owner(
        &self,
        state_id: &str,
        owner_id: OwnerId,
    ) -> Result<OwnedState, StoreError> {
        if owner_id.as_str().is_empty() {
            return Err(unnamed_owner());
        }
        let key = self.head_key(state_id);
        loop {
            let current = self.backend.get(&key).await?;
            let (fence, state) = match &current {
                None => (0, StoredState::default()),
                Some(entry) => {
                    let head = Head::parse(&entry.version)?;
                    (head.fence, head.state(entry.value.clone()))
                }
            };
            let fence = fence.checked_add(1).ok_or_else(|| {
                StoreError::NotCommitted("key-value durability owner fence overflow".to_owned())
            })?;
            let next = version(fence, state.revision, owner_id.as_str());
            let swapped = self
                .backend
                .compare_and_swap(
                    &key,
                    current.as_ref().map(|entry| entry.version.as_str()),
                    &next,
                    state.payload.as_deref().unwrap_or_default(),
                )
                .await?;
            if swapped {
                return Ok(OwnedState {
                    owner: OwnerToken::new(owner_id, fence),
                    state,
                });
            }
        }
    }

    async fn replace_state(
        &self,
        state_id: &str,
        owner: &OwnerToken,
        expected_revision: u64,
        payload: &str,
        records: &[StoreRecord],
    ) -> Result<u64, StoreError> {
        if owner.fence() == 0 || owner.owner_id().as_str().is_empty() {
            return Err(StoreError::Fenced);
        }
        let revision = expected_revision.checked_add(1).ok_or_else(|| {
            StoreError::NotCommitted("key-value durability revision overflow".to_owned())
        })?;
        try_join_all(
            records
                .iter()
                .filter(|record| !record.value.is_empty())
                .map(|record| self.insert_record(state_id, record)),
        )
        .await?;
        let owner_id = owner.owner_id().as_str();
        let key = self.head_key(state_id);
        let expected = version(owner.fence(), expected_revision, owner_id);
        let next = version(owner.fence(), revision, owner_id);
        if self
            .backend
            .compare_and_swap(&key, Some(&expected), &next, payload)
            .await?
        {
            return Ok(revision);
        }
        let Some(current) = self.backend.get(&key).await? else {
            return Err(StoreError::Fenced);
        };
        let head = Head::parse(&current.version)?;
        if head.fence != owner.fence() || head.owner != owner_id {
            return Err(StoreError::Fenced);
        }
        if head.revision != expected_revision {
            return Err(StoreError::Conflict {
                expected: expected_revision,
                actual: head.revision,
            });
        }
        Err(StoreError::Backend(format!(
            "key-value store refused to replace version `{expected}` of state {state_id} while reporting it as current"
        )))
    }

    async fn insert_record(&self, state_id: &str, record: &StoreRecord) -> Result<(), StoreError> {
        let key = self.record_key(state_id, &record.key);
        if self.swap_record(&key, None, record).await? {
            return Ok(());
        }
        let current = self.stored_record(&key).await?;
        if current.value == record.value {
            return Ok(());
        }
        if !current.value.is_empty() {
            return Err(conflicting_record(&record.key));
        }
        if self
            .swap_record(&key, Some(&current.version), record)
            .await?
        {
            return Ok(());
        }
        if self.stored_record(&key).await?.value == record.value {
            return Ok(());
        }
        Err(conflicting_record(&record.key))
    }

    async fn swap_record(
        &self,
        key: &str,
        expected_version: Option<&str>,
        record: &StoreRecord,
    ) -> Result<bool, StoreError> {
        self.backend
            .compare_and_swap(key, expected_version, RECORD_VERSION, &record.value)
            .await
            .map_err(records_not_stored)
    }

    async fn stored_record(&self, key: &str) -> Result<Versioned, StoreError> {
        self.backend
            .get(key)
            .await
            .map_err(records_not_stored)?
            .ok_or_else(|| {
                StoreError::NotCommitted(format!(
                    "key-value store refused to insert record `{key}` while reporting it absent"
                ))
            })
    }

    async fn peek_state(&self, state_id: &str) -> Result<StoredState, StoreError> {
        match self.backend.get(&self.head_key(state_id)).await? {
            None => Ok(StoredState::default()),
            Some(entry) => Ok(Head::parse(&entry.version)?.state(entry.value)),
        }
    }

    async fn current_owner(&self, state_id: &str) -> Result<Option<OwnerToken>, StoreError> {
        match self.backend.get(&self.head_key(state_id)).await? {
            None => Ok(None),
            Some(entry) => {
                let head = Head::parse(&entry.version)?;
                Ok(Some(OwnerToken::new(
                    OwnerId::from(head.owner.to_owned()),
                    head.fence,
                )))
            }
        }
    }

    async fn records(
        &self,
        state_id: &str,
        keys: &[String],
    ) -> Result<Vec<Option<String>>, StoreError> {
        let keys: Vec<_> = keys
            .iter()
            .map(|key| self.record_key(state_id, key))
            .collect();
        let values = self.backend.get_values(&keys).await?;
        if values.len() != keys.len() {
            return Err(StoreError::Backend(format!(
                "key-value store answered {} of {} record reads",
                values.len(),
                keys.len()
            )));
        }
        Ok(values)
    }

    async fn state_ids(&self, limit: usize) -> Result<Vec<String>, StoreError> {
        self.listed(&self.states(), limit).await
    }

    async fn record_keys(&self, state_id: &str) -> Result<Vec<String>, StoreError> {
        self.listed(&self.records_of(state_id), usize::MAX).await
    }

    async fn listed(&self, prefix: &str, limit: usize) -> Result<Vec<String>, StoreError> {
        let keys = self.backend.list_keys(prefix).await?;
        keys.into_iter()
            .take(limit)
            .map(|key| {
                key.strip_prefix(prefix).map(str::to_owned).ok_or_else(|| {
                    StoreError::Backend(format!(
                        "key-value store listed `{key}` outside the prefix `{prefix}`"
                    ))
                })
            })
            .collect()
    }
}

impl<B: KeyValue + ?Sized + 'static> StateStore for KeyValueStore<B> {
    fn read_record<'a>(
        &'a mut self,
        state_id: &'a str,
        key: &'a str,
    ) -> StoreFuture<'a, Result<Option<String>, StoreError>> {
        Box::pin(async move {
            Ok(self
                .records(state_id, std::slice::from_ref(&key.to_owned()))
                .await?
                .pop()
                .flatten())
        })
    }

    fn read_records<'a>(
        &'a mut self,
        state_id: &'a str,
        keys: &'a [String],
    ) -> StoreFuture<'a, Result<Vec<Option<String>>, StoreError>> {
        Box::pin(self.records(state_id, keys))
    }

    fn peek<'a>(
        &'a mut self,
        state_id: &'a str,
    ) -> StoreFuture<'a, Result<StoredState, StoreError>> {
        Box::pin(self.peek_state(state_id))
    }

    fn list_states<'a>(
        &'a mut self,
        limit: usize,
    ) -> StoreFuture<'a, Result<Vec<String>, StoreError>> {
        Box::pin(self.state_ids(limit))
    }

    fn list_records<'a>(
        &'a mut self,
        state_id: &'a str,
    ) -> StoreFuture<'a, Result<Vec<String>, StoreError>> {
        Box::pin(self.record_keys(state_id))
    }

    fn owner<'a>(
        &'a mut self,
        state_id: &'a str,
    ) -> StoreFuture<'a, Result<Option<OwnerToken>, StoreError>> {
        Box::pin(self.current_owner(state_id))
    }

    fn acquire<'a>(
        &'a mut self,
        state_id: &'a str,
        owner_id: OwnerId,
    ) -> StoreFuture<'a, Result<OwnedState, StoreError>> {
        Box::pin(self.acquire_owner(state_id, owner_id))
    }

    fn replace<'a>(
        &'a mut self,
        state_id: &'a str,
        owner: &'a OwnerToken,
        expected_revision: u64,
        payload: &'a str,
        records: &'a [StoreRecord],
    ) -> StoreFuture<'a, Result<u64, StoreError>> {
        Box::pin(self.replace_state(state_id, owner, expected_revision, payload, records))
    }

    fn concurrent_handle(&self) -> Option<Box<dyn StateStore>> {
        Some(Box::new(self.clone()))
    }
}
