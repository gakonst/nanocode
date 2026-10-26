use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use nanocode_sync::Atom;

use crate::{
    Committed, OwnedState, OwnerId, OwnerToken, StateCommit, StateStore, StoreError, StoreFuture,
    StoreRecord, StoredState, SummaryUpdate,
};

#[derive(Clone, Debug)]
struct Staged {
    base_fence: u64,
    base_revision: u64,
    owner: OwnerToken,
    revision: u64,
    payload: Option<Arc<str>>,
    payloads: Vec<Arc<str>>,
    records: BTreeMap<String, Arc<str>>,
    stored: BTreeSet<String>,
}

type States = BTreeMap<String, Arc<Staged>>;

#[derive(Clone, Debug, Default)]
pub struct StagedWrites(Arc<Atom<States>>);

impl StagedWrites {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.read(|states| {
            states
                .values()
                .all(|staged| staged.revision == staged.base_revision)
        })
    }

    pub fn discard(&self) {
        self.0.set(BTreeMap::new());
    }

    pub async fn stage_records<T: StateStore + ?Sized>(
        &self,
        store: &mut T,
    ) -> Result<(), StoreError> {
        let states = self.0.get();
        for (state_id, staged) in states
            .iter()
            .filter(|(_, staged)| staged.revision != staged.base_revision)
        {
            let records = staged
                .records
                .iter()
                .filter(|(key, _)| !staged.stored.contains(*key))
                .map(|(key, value)| StoreRecord {
                    key: key.clone(),
                    value: value.to_string(),
                })
                .collect::<Vec<_>>();
            if records.is_empty() {
                continue;
            }
            if !store.put_records(state_id, &records).await? {
                return Ok(());
            }
            self.0.update(|states| {
                if let Some(staged) = states.get_mut(state_id) {
                    Arc::make_mut(staged)
                        .stored
                        .extend(records.iter().map(|record| record.key.clone()));
                }
            });
        }
        Ok(())
    }

    pub async fn commit_to<T: StateStore + ?Sized>(&self, store: &mut T) -> Result<(), StoreError> {
        let states = self.0.get();
        for (state_id, staged) in states
            .iter()
            .filter(|(_, staged)| staged.revision != staged.base_revision)
        {
            let state_id = state_id.as_str();
            let acquired = store
                .acquire(state_id, staged.owner.owner_id().clone())
                .await?;
            if Some(acquired.owner.fence()) != staged.base_fence.checked_add(1) {
                return Err(StoreError::Fenced);
            }
            if acquired.state.revision != staged.base_revision {
                return Err(StoreError::Conflict {
                    expected: staged.base_revision,
                    actual: acquired.state.revision,
                });
            }
            let mut records: Vec<_> = staged
                .records
                .iter()
                .filter(|(key, _)| !staged.stored.contains(*key))
                .map(|(key, value)| StoreRecord {
                    key: key.clone(),
                    value: value.to_string(),
                })
                .collect();
            if let Some(payload) = staged.payloads.last()
                && store
                    .replace_to(
                        state_id,
                        &acquired.owner,
                        staged.base_revision,
                        staged.revision,
                        payload,
                        &records,
                    )
                    .await?
                    .is_some()
            {
                continue;
            }
            let mut revision = staged.base_revision;
            for payload in &staged.payloads {
                revision = store
                    .commit(
                        state_id,
                        &acquired.owner,
                        revision,
                        StateCommit {
                            payload,
                            records: &records,
                            events: &[],
                            summary: &SummaryUpdate::Unchanged,
                        },
                    )
                    .await?
                    .revision;
                records.clear();
            }
        }
        Ok(())
    }

    fn get(&self, state_id: &str) -> Option<Arc<Staged>> {
        self.0.read(|states| states.get(state_id).cloned())
    }

    fn write(
        &self,
        state_id: &str,
        owner: &OwnerToken,
        expected_revision: u64,
        payload: &str,
        records: &[StoreRecord],
    ) -> Result<u64, StoreError> {
        self.0.modify(|states| {
            let staged = Arc::make_mut(states.get_mut(state_id).ok_or(StoreError::Fenced)?);
            Self::staged_write(staged, owner, expected_revision, payload, records)
        })
    }

    fn staged_write(
        staged: &mut Staged,
        owner: &OwnerToken,
        expected_revision: u64,
        payload: &str,
        records: &[StoreRecord],
    ) -> Result<u64, StoreError> {
        if staged.owner != *owner {
            return Err(StoreError::Fenced);
        }
        if staged.revision != expected_revision {
            return Err(StoreError::Conflict {
                expected: expected_revision,
                actual: staged.revision,
            });
        }
        if records.iter().any(|record| {
            staged
                .records
                .get(&record.key)
                .is_some_and(|value| **value != *record.value)
        }) {
            return Err(StoreError::NotCommitted(
                "a staged record already has different content".to_owned(),
            ));
        }
        let revision = staged.revision.checked_add(1).ok_or_else(|| {
            StoreError::NotCommitted("staged durability revision overflow".to_owned())
        })?;
        for record in records {
            staged
                .records
                .entry(record.key.clone())
                .or_insert_with(|| Arc::from(record.value.as_str()));
        }
        let payload = Arc::<str>::from(payload);
        staged.revision = revision;
        staged.payload = Some(Arc::clone(&payload));
        staged.payloads.push(payload);
        Ok(revision)
    }
}

pub struct StagedStore<S> {
    base: S,
    writes: StagedWrites,
}

impl<S: StateStore> StagedStore<S> {
    pub fn new(base: S) -> Self {
        Self {
            base,
            writes: StagedWrites::default(),
        }
    }

    #[must_use]
    pub fn writes(&self) -> StagedWrites {
        self.writes.clone()
    }

    async fn staged_records(
        &mut self,
        state_id: &str,
        keys: &[String],
    ) -> Result<Vec<Option<String>>, StoreError> {
        let staged = self.writes.get(state_id);
        let staged_record = |key: &str| {
            staged
                .as_ref()
                .and_then(|staged| staged.records.get(key))
                .map(|value| value.to_string())
        };
        let missing: Vec<_> = keys
            .iter()
            .filter(|key| {
                !staged
                    .as_ref()
                    .is_some_and(|staged| staged.records.contains_key(*key))
            })
            .cloned()
            .collect();
        let mut read = if missing.is_empty() {
            Vec::new()
        } else {
            self.base.read_records(state_id, &missing).await?
        }
        .into_iter();
        Ok(keys
            .iter()
            .map(|key| staged_record(key).or_else(|| read.next().flatten()))
            .collect())
    }

    async fn acquire_staged(
        &mut self,
        state_id: &str,
        owner_id: OwnerId,
    ) -> Result<OwnedState, StoreError> {
        let staged = match self.writes.get(state_id) {
            Some(staged) => Staged::clone(&staged),
            None => {
                let fence = self
                    .base
                    .owner(state_id)
                    .await?
                    .map_or(0, |owner| owner.fence());
                let state = self.base.peek(state_id).await?;
                Staged {
                    base_fence: fence,
                    base_revision: state.revision,
                    owner: OwnerToken::new(owner_id.clone(), fence),
                    revision: state.revision,
                    payload: state.payload.map(Arc::from),
                    payloads: Vec::new(),
                    records: BTreeMap::new(),
                    stored: BTreeSet::new(),
                }
            }
        };
        let fence =
            staged.owner.fence().checked_add(1).ok_or_else(|| {
                StoreError::NotCommitted("staged owner fence overflow".to_owned())
            })?;
        let owner = OwnerToken::new(owner_id, fence);
        let entry = self.writes.0.update(|states| {
            let entry = Arc::make_mut(
                states
                    .entry(state_id.to_owned())
                    .or_insert_with(|| Arc::new(staged.clone())),
            );
            entry.owner = owner.clone();
            (entry.revision, entry.payload.clone())
        });
        Ok(OwnedState {
            owner,
            state: StoredState {
                revision: entry.0,
                payload: entry.1.as_deref().map(str::to_owned),
            },
        })
    }
}

impl<S: StateStore> StateStore for StagedStore<S> {
    fn read_record<'a>(
        &'a mut self,
        state_id: &'a str,
        key: &'a str,
    ) -> StoreFuture<'a, Result<Option<String>, StoreError>> {
        Box::pin(async move {
            Ok(self
                .staged_records(state_id, std::slice::from_ref(&key.to_owned()))
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
        Box::pin(self.staged_records(state_id, keys))
    }

    fn peek<'a>(
        &'a mut self,
        state_id: &'a str,
    ) -> StoreFuture<'a, Result<StoredState, StoreError>> {
        Box::pin(async move {
            match self.writes.get(state_id) {
                Some(staged) => Ok(StoredState {
                    revision: staged.revision,
                    payload: staged.payload.as_deref().map(str::to_owned),
                }),
                None => self.base.peek(state_id).await,
            }
        })
    }

    fn list_states<'a>(
        &'a mut self,
        limit: usize,
    ) -> StoreFuture<'a, Result<Vec<String>, StoreError>> {
        Box::pin(async move {
            let mut states = self.base.list_states(limit).await?;
            let staged: Vec<_> = self.writes.0.read(|staged| {
                staged
                    .iter()
                    .filter(|(state_id, staged)| {
                        staged.payload.is_some() && !states.contains(*state_id)
                    })
                    .map(|(state_id, _)| state_id.clone())
                    .collect()
            });
            states.splice(0..0, staged);
            states.truncate(limit);
            Ok(states)
        })
    }

    fn list_records<'a>(
        &'a mut self,
        state_id: &'a str,
    ) -> StoreFuture<'a, Result<Vec<String>, StoreError>> {
        Box::pin(async move {
            let mut keys = self.base.list_records(state_id).await?;
            if let Some(staged) = self.writes.get(state_id) {
                keys.extend(
                    staged
                        .records
                        .keys()
                        .filter(|key| !keys.contains(key))
                        .cloned()
                        .collect::<Vec<_>>(),
                );
            }
            Ok(keys)
        })
    }

    fn owner<'a>(
        &'a mut self,
        state_id: &'a str,
    ) -> StoreFuture<'a, Result<Option<OwnerToken>, StoreError>> {
        Box::pin(async move {
            match self.writes.get(state_id) {
                Some(staged) => Ok(Some(staged.owner.clone())),
                None => self.base.owner(state_id).await,
            }
        })
    }

    fn acquire<'a>(
        &'a mut self,
        state_id: &'a str,
        owner_id: OwnerId,
    ) -> StoreFuture<'a, Result<OwnedState, StoreError>> {
        Box::pin(self.acquire_staged(state_id, owner_id))
    }

    fn replace<'a>(
        &'a mut self,
        state_id: &'a str,
        owner: &'a OwnerToken,
        expected_revision: u64,
        payload: &'a str,
        records: &'a [StoreRecord],
    ) -> StoreFuture<'a, Result<u64, StoreError>> {
        Box::pin(async move {
            self.writes
                .write(state_id, owner, expected_revision, payload, records)
        })
    }

    fn commit<'a>(
        &'a mut self,
        state_id: &'a str,
        owner: &'a OwnerToken,
        expected_revision: u64,
        commit: StateCommit<'a>,
    ) -> StoreFuture<'a, Result<Committed, StoreError>> {
        Box::pin(async move {
            if !commit.events.is_empty() {
                return Err(StoreError::NotCommitted(
                    "this store does not record state events".to_owned(),
                ));
            }
            let revision = self.writes.write(
                state_id,
                owner,
                expected_revision,
                commit.payload,
                commit.records,
            )?;
            Ok(Committed {
                revision,
                first_event: None,
            })
        })
    }
}
