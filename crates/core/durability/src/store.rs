use std::{collections::BTreeMap, future::Future, pin::Pin};

#[derive(Clone, Debug, Eq, Hash, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(transparent)]
pub struct OwnerId(String);

impl OwnerId {
    pub fn new() -> Self {
        #[cfg(not(target_family = "wasm"))]
        let id = uuid::Uuid::now_v7();
        #[cfg(target_family = "wasm")]
        let id = nanocode_platform::uuid_v7();
        Self(id.to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<String> for OwnerId {
    fn from(id: String) -> Self {
        Self(id)
    }
}

impl Default for OwnerId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct OwnerToken {
    owner_id: OwnerId,
    fence: u64,
}

impl OwnerToken {
    pub const fn new(owner_id: OwnerId, fence: u64) -> Self {
        Self { owner_id, fence }
    }

    pub const fn owner_id(&self) -> &OwnerId {
        &self.owner_id
    }

    pub const fn fence(&self) -> u64 {
        self.fence
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct StoredState {
    pub revision: u64,

    pub payload: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct OwnedState {
    pub owner: OwnerToken,

    pub state: StoredState,
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum StoreError {
    #[error("durability state owner was fenced")]
    Fenced,

    #[error("durability state revision conflict: expected {expected}, found {actual}")]
    Conflict { expected: u64, actual: u64 },

    #[error("durability store operation was not committed: {0}")]
    NotCommitted(String),

    #[error(
        "{component} storage format {stored} is newer than this release supports (format {known}); upgrade before opening it"
    )]
    UnsupportedFormat {
        component: &'static str,
        stored: u64,
        known: u32,
    },

    #[error("durability store failed: {0}")]
    Backend(String),
}

#[cfg(not(target_family = "wasm"))]
pub type StoreFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

#[cfg(target_family = "wasm")]
pub type StoreFuture<'a, T> = Pin<Box<dyn Future<Output = T> + 'a>>;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct StoreRecord {
    pub key: String,

    pub value: String,
}

pub struct StoreReplacement {
    pub payload: String,
    pub records: Vec<StoreRecord>,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct StoreEvent {
    pub kind: String,
    pub at_ms: u64,
    pub body: String,
    pub text: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct StoredEvent {
    pub seq: u64,
    pub revision: u64,
    pub event: StoreEvent,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct StateSummary {
    pub scope: Option<String>,
    pub parent: Option<String>,
    pub title: Option<String>,
    pub preview: Option<String>,
    pub attributes: BTreeMap<String, String>,
    pub updated_at_ms: u64,
    pub value: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum SummaryUpdate {
    #[default]
    Unchanged,
    Set(StateSummary),
    Remove,
}

#[derive(Clone, Copy, Debug)]
pub struct StateCommit<'a> {
    pub payload: &'a str,
    pub records: &'a [StoreRecord],
    pub events: &'a [StoreEvent],
    pub summary: &'a SummaryUpdate,
}

#[derive(Clone, Debug)]
pub struct OwnedCommit {
    pub state_id: String,
    pub owner: OwnerToken,
    pub expected_revision: u64,
    pub payload: String,
    pub records: Vec<StoreRecord>,
    pub events: Vec<StoreEvent>,
    pub summary: SummaryUpdate,
}

impl OwnedCommit {
    pub fn commit(&self) -> StateCommit<'_> {
        StateCommit {
            payload: &self.payload,
            records: &self.records,
            events: &self.events,
            summary: &self.summary,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Committed {
    pub revision: u64,
    pub first_event: Option<u64>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum EventOrder {
    #[default]
    Ascending,
    Descending,
}

#[derive(Clone, Debug, Default)]
pub struct EventRange {
    pub after: Option<u64>,
    pub before: Option<u64>,
    pub kinds: Vec<String>,
    pub order: EventOrder,
    pub limit: usize,
}

#[derive(Clone, Debug, Default)]
pub struct EventPage {
    pub events: Vec<StoredEvent>,
    pub next: Option<u64>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, serde::Deserialize, serde::Serialize)]
pub struct StateCursor {
    pub updated_at_ms: u64,
    pub state_id: String,
}

#[derive(Clone, Debug, Default)]
pub struct StateQuery {
    pub scope: Option<String>,
    pub parent: Option<String>,
    pub attributes: BTreeMap<String, String>,
    pub text: Vec<String>,
    pub after: Option<StateCursor>,
    pub limit: usize,
}

#[derive(Clone, Debug)]
pub struct IndexedState {
    pub state_id: String,
    pub revision: u64,
    pub summary: StateSummary,
}

#[derive(Clone, Debug, Default)]
pub struct StatePage {
    pub states: Vec<IndexedState>,
    pub next: Option<StateCursor>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, serde::Deserialize, serde::Serialize)]
pub struct EventCursor {
    pub at_ms: u64,
    pub state_id: String,
    pub seq: u64,
}

#[derive(Clone, Debug, Default)]
pub struct EventQuery {
    pub scope: Option<String>,
    pub state_id: Option<String>,
    pub kinds: Vec<String>,
    pub text: Vec<String>,
    pub after: Option<EventCursor>,
    pub limit: usize,
}

#[derive(Clone, Debug)]
pub struct EventHit {
    pub state_id: String,
    pub event: StoredEvent,
}

#[derive(Clone, Debug, Default)]
pub struct EventHits {
    pub hits: Vec<EventHit>,
    pub next: Option<EventCursor>,
}

#[cfg(not(target_family = "wasm"))]
pub type StoreUpdate =
    Box<dyn FnOnce(&StoredState) -> Result<Option<StoreReplacement>, StoreError> + Send>;

#[cfg(target_family = "wasm")]
pub type StoreUpdate =
    Box<dyn FnOnce(&StoredState) -> Result<Option<StoreReplacement>, StoreError>>;

async fn acquire_updated<S: StateStore + ?Sized>(
    store: &mut S,
    state_id: &str,
    owner_id: OwnerId,
    update: StoreUpdate,
) -> Result<OwnedState, StoreError> {
    let mut owned = store.acquire(state_id, owner_id).await?;
    if let Some(replacement) = update(&owned.state).map_err(|error| {
        StoreError::Backend(format!(
            "state update failed after acquiring its owner: {error}"
        ))
    })? {
        let revision = store
            .replace(
                state_id,
                &owned.owner,
                owned.state.revision,
                &replacement.payload,
                &replacement.records,
            )
            .await
            .map_err(|error| {
                StoreError::Backend(format!(
                    "state update failed after acquiring its owner: {error}"
                ))
            })?;
        owned.state = StoredState {
            revision,
            payload: Some(replacement.payload),
        };
    }
    Ok(owned)
}

async fn commit_replaced<S: StateStore + ?Sized>(
    store: &mut S,
    state_id: &str,
    owner: &OwnerToken,
    expected_revision: u64,
    commit: StateCommit<'_>,
) -> Result<Committed, StoreError> {
    if !commit.events.is_empty() {
        return Err(StoreError::NotCommitted(
            "this store does not record state events".to_owned(),
        ));
    }
    let revision = store
        .replace(
            state_id,
            owner,
            expected_revision,
            commit.payload,
            commit.records,
        )
        .await?;
    Ok(Committed {
        revision,
        first_event: None,
    })
}

async fn commit_each<S: StateStore + ?Sized>(
    store: &mut S,
    commits: Vec<OwnedCommit>,
) -> Vec<Result<Committed, StoreError>> {
    let mut results = Vec::with_capacity(commits.len());
    for commit in &commits {
        results.push(
            store
                .commit(
                    &commit.state_id,
                    &commit.owner,
                    commit.expected_revision,
                    commit.commit(),
                )
                .await,
        );
    }
    results
}

async fn acquire_companion<S: StateStore + ?Sized>(
    store: &mut S,
    state_id: &str,
    owner_id: OwnerId,
    update: StoreUpdate,
    companion_id: &str,
    companion_owner: OwnerId,
) -> Result<(OwnedState, OwnedState), StoreError> {
    if companion_id == state_id {
        return Err(StoreError::NotCommitted(
            "a companion state must differ from its state".to_owned(),
        ));
    }
    let companion = store.acquire(companion_id, companion_owner).await?;
    let owned = store
        .acquire_with(state_id, owner_id, update)
        .await
        .map_err(|error| {
            StoreError::Backend(format!(
                "state acquisition failed after acquiring its companion: {error}"
            ))
        })?;
    Ok((owned, companion))
}

fn read_only_unsupported() -> StoreError {
    StoreError::Backend("this store does not support non-fencing reads".to_owned())
}

fn record_listing_unsupported() -> StoreError {
    StoreError::Backend("this store does not list a state's records".to_owned())
}

#[cfg(not(target_family = "wasm"))]
pub trait StateStore: Send {
    fn read_record<'a>(
        &'a mut self,
        state_id: &'a str,
        key: &'a str,
    ) -> StoreFuture<'a, Result<Option<String>, StoreError>>;

    fn read_records<'a>(
        &'a mut self,
        state_id: &'a str,
        keys: &'a [String],
    ) -> StoreFuture<'a, Result<Vec<Option<String>>, StoreError>> {
        Box::pin(async move {
            let mut records = Vec::with_capacity(keys.len());
            for key in keys {
                records.push(self.read_record(state_id, key).await?);
            }
            Ok(records)
        })
    }

    fn peek<'a>(
        &'a mut self,
        state_id: &'a str,
    ) -> StoreFuture<'a, Result<StoredState, StoreError>>;

    fn list_states<'a>(
        &'a mut self,
        _limit: usize,
    ) -> StoreFuture<'a, Result<Vec<String>, StoreError>> {
        Box::pin(async { Err(read_only_unsupported()) })
    }

    fn list_records<'a>(
        &'a mut self,
        _state_id: &'a str,
    ) -> StoreFuture<'a, Result<Vec<String>, StoreError>> {
        Box::pin(async { Err(record_listing_unsupported()) })
    }

    fn owner<'a>(
        &'a mut self,
        _state_id: &'a str,
    ) -> StoreFuture<'a, Result<Option<OwnerToken>, StoreError>> {
        Box::pin(async { Err(read_only_unsupported()) })
    }

    fn acquire<'a>(
        &'a mut self,
        state_id: &'a str,
        owner_id: OwnerId,
    ) -> StoreFuture<'a, Result<OwnedState, StoreError>>;

    fn acquire_with<'a>(
        &'a mut self,
        state_id: &'a str,
        owner_id: OwnerId,
        update: StoreUpdate,
    ) -> StoreFuture<'a, Result<OwnedState, StoreError>> {
        Box::pin(acquire_updated(self, state_id, owner_id, update))
    }

    fn batches_companion_acquisition(&self) -> bool {
        false
    }

    fn supports_events(&self) -> bool {
        false
    }

    fn supports_summaries(&self) -> bool {
        false
    }

    fn acquire_with_companion<'a>(
        &'a mut self,
        state_id: &'a str,
        owner_id: OwnerId,
        update: StoreUpdate,
        companion_id: &'a str,
        companion_owner: OwnerId,
    ) -> StoreFuture<'a, Result<(OwnedState, OwnedState), StoreError>> {
        Box::pin(acquire_companion(
            self,
            state_id,
            owner_id,
            update,
            companion_id,
            companion_owner,
        ))
    }

    fn replace<'a>(
        &'a mut self,
        state_id: &'a str,
        owner: &'a OwnerToken,
        expected_revision: u64,
        payload: &'a str,
        records: &'a [crate::StoreRecord],
    ) -> StoreFuture<'a, Result<u64, StoreError>>;

    fn replace_to<'a>(
        &'a mut self,
        _state_id: &'a str,
        _owner: &'a OwnerToken,
        _expected_revision: u64,
        _revision: u64,
        _payload: &'a str,
        _records: &'a [crate::StoreRecord],
    ) -> StoreFuture<'a, Result<Option<u64>, StoreError>> {
        Box::pin(async { Ok(None) })
    }

    fn put_records<'a>(
        &'a mut self,
        _state_id: &'a str,
        _records: &'a [crate::StoreRecord],
    ) -> StoreFuture<'a, Result<bool, StoreError>> {
        Box::pin(async { Ok(false) })
    }

    fn commit<'a>(
        &'a mut self,
        state_id: &'a str,
        owner: &'a OwnerToken,
        expected_revision: u64,
        commit: StateCommit<'a>,
    ) -> StoreFuture<'a, Result<Committed, StoreError>> {
        Box::pin(commit_replaced(
            self,
            state_id,
            owner,
            expected_revision,
            commit,
        ))
    }

    fn commit_many(
        &mut self,
        commits: Vec<OwnedCommit>,
    ) -> StoreFuture<'_, Vec<Result<Committed, StoreError>>> {
        Box::pin(commit_each(self, commits))
    }

    fn flush(&mut self) -> StoreFuture<'_, Result<(), StoreError>> {
        Box::pin(async { Ok(()) })
    }

    fn concurrent_handle(&self) -> Option<Box<dyn StateStore>> {
        None
    }
}

#[cfg(target_family = "wasm")]
pub trait StateStore {
    fn read_record<'a>(
        &'a mut self,
        state_id: &'a str,
        key: &'a str,
    ) -> StoreFuture<'a, Result<Option<String>, StoreError>>;

    fn read_records<'a>(
        &'a mut self,
        state_id: &'a str,
        keys: &'a [String],
    ) -> StoreFuture<'a, Result<Vec<Option<String>>, StoreError>> {
        Box::pin(async move {
            let mut records = Vec::with_capacity(keys.len());
            for key in keys {
                records.push(self.read_record(state_id, key).await?);
            }
            Ok(records)
        })
    }

    fn peek<'a>(
        &'a mut self,
        state_id: &'a str,
    ) -> StoreFuture<'a, Result<StoredState, StoreError>>;

    fn list_states<'a>(
        &'a mut self,
        _limit: usize,
    ) -> StoreFuture<'a, Result<Vec<String>, StoreError>> {
        Box::pin(async { Err(read_only_unsupported()) })
    }

    fn list_records<'a>(
        &'a mut self,
        _state_id: &'a str,
    ) -> StoreFuture<'a, Result<Vec<String>, StoreError>> {
        Box::pin(async { Err(record_listing_unsupported()) })
    }

    fn owner<'a>(
        &'a mut self,
        _state_id: &'a str,
    ) -> StoreFuture<'a, Result<Option<OwnerToken>, StoreError>> {
        Box::pin(async { Err(read_only_unsupported()) })
    }

    fn acquire<'a>(
        &'a mut self,
        state_id: &'a str,
        owner_id: OwnerId,
    ) -> StoreFuture<'a, Result<OwnedState, StoreError>>;

    fn acquire_with<'a>(
        &'a mut self,
        state_id: &'a str,
        owner_id: OwnerId,
        update: StoreUpdate,
    ) -> StoreFuture<'a, Result<OwnedState, StoreError>> {
        Box::pin(acquire_updated(self, state_id, owner_id, update))
    }

    fn batches_companion_acquisition(&self) -> bool {
        false
    }

    fn supports_events(&self) -> bool {
        false
    }

    fn supports_summaries(&self) -> bool {
        false
    }

    fn acquire_with_companion<'a>(
        &'a mut self,
        state_id: &'a str,
        owner_id: OwnerId,
        update: StoreUpdate,
        companion_id: &'a str,
        companion_owner: OwnerId,
    ) -> StoreFuture<'a, Result<(OwnedState, OwnedState), StoreError>> {
        Box::pin(acquire_companion(
            self,
            state_id,
            owner_id,
            update,
            companion_id,
            companion_owner,
        ))
    }

    fn replace<'a>(
        &'a mut self,
        state_id: &'a str,
        owner: &'a OwnerToken,
        expected_revision: u64,
        payload: &'a str,
        records: &'a [crate::StoreRecord],
    ) -> StoreFuture<'a, Result<u64, StoreError>>;

    fn replace_to<'a>(
        &'a mut self,
        _state_id: &'a str,
        _owner: &'a OwnerToken,
        _expected_revision: u64,
        _revision: u64,
        _payload: &'a str,
        _records: &'a [crate::StoreRecord],
    ) -> StoreFuture<'a, Result<Option<u64>, StoreError>> {
        Box::pin(async { Ok(None) })
    }

    fn put_records<'a>(
        &'a mut self,
        _state_id: &'a str,
        _records: &'a [crate::StoreRecord],
    ) -> StoreFuture<'a, Result<bool, StoreError>> {
        Box::pin(async { Ok(false) })
    }

    fn commit<'a>(
        &'a mut self,
        state_id: &'a str,
        owner: &'a OwnerToken,
        expected_revision: u64,
        commit: StateCommit<'a>,
    ) -> StoreFuture<'a, Result<Committed, StoreError>> {
        Box::pin(commit_replaced(
            self,
            state_id,
            owner,
            expected_revision,
            commit,
        ))
    }

    fn commit_many(
        &mut self,
        commits: Vec<OwnedCommit>,
    ) -> StoreFuture<'_, Vec<Result<Committed, StoreError>>> {
        Box::pin(commit_each(self, commits))
    }

    fn flush(&mut self) -> StoreFuture<'_, Result<(), StoreError>> {
        Box::pin(async { Ok(()) })
    }

    fn concurrent_handle(&self) -> Option<Box<dyn StateStore>> {
        None
    }
}

#[cfg(not(target_family = "wasm"))]
pub trait StateReader: Send {
    fn read_state<'a>(
        &'a mut self,
        state_id: &'a str,
    ) -> StoreFuture<'a, Result<StoredState, StoreError>>;

    fn read_records<'a>(
        &'a mut self,
        state_id: &'a str,
        keys: &'a [String],
    ) -> StoreFuture<'a, Result<Vec<Option<String>>, StoreError>>;

    fn read_events<'a>(
        &'a mut self,
        state_id: &'a str,
        range: &'a EventRange,
    ) -> StoreFuture<'a, Result<EventPage, StoreError>>;

    fn query_states<'a>(
        &'a mut self,
        query: &'a StateQuery,
    ) -> StoreFuture<'a, Result<StatePage, StoreError>>;

    fn query_events<'a>(
        &'a mut self,
        query: &'a EventQuery,
    ) -> StoreFuture<'a, Result<EventHits, StoreError>>;
}

#[cfg(target_family = "wasm")]
pub trait StateReader {
    fn read_state<'a>(
        &'a mut self,
        state_id: &'a str,
    ) -> StoreFuture<'a, Result<StoredState, StoreError>>;

    fn read_records<'a>(
        &'a mut self,
        state_id: &'a str,
        keys: &'a [String],
    ) -> StoreFuture<'a, Result<Vec<Option<String>>, StoreError>>;

    fn read_events<'a>(
        &'a mut self,
        state_id: &'a str,
        range: &'a EventRange,
    ) -> StoreFuture<'a, Result<EventPage, StoreError>>;

    fn query_states<'a>(
        &'a mut self,
        query: &'a StateQuery,
    ) -> StoreFuture<'a, Result<StatePage, StoreError>>;

    fn query_events<'a>(
        &'a mut self,
        query: &'a EventQuery,
    ) -> StoreFuture<'a, Result<EventHits, StoreError>>;
}
