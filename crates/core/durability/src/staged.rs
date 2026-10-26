use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, MutexGuard},
};

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
    payload: Option<String>,
    payloads: Vec<String>,
    records: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Default)]
pub struct StagedWrites(Arc<Mutex<BTreeMap<String, Staged>>>);

impl StagedWrites {
    fn lock(&self) -> MutexGuard<'_, BTreeMap<String, Staged>> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.lock()
            .values()
            .all(|staged| staged.revision == staged.base_revision)
    }

    pub fn discard(&self) {
        self.lock().clear();
    }

    pub async fn commit_to<T: StateStore + ?Sized>(&self, store: &mut T) -> Result<(), StoreError> {
        let staged: Vec<_> = self
            .lock()
            .iter()
            .filter(|(_, staged)| staged.revision != staged.base_revision)
            .map(|(state_id, staged)| (state_id.clone(), staged.clone()))
            .collect();
        for (state_id, staged) in staged {
            let acquired = store
                .acquire(&state_id, staged.owner.owner_id().clone())
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
                .into_iter()
                .map(|(key, value)| StoreRecord { key, value })
                .collect();
            if let Some(payload) = staged.payloads.last()
                && store
                    .replace_to(
                        &state_id,
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
                        &state_id,
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

    fn get(&self, state_id: &str) -> Option<Staged> {
        self.lock().get(state_id).cloned()
    }

    fn write(
        &self,
        state_id: &str,
        owner: &OwnerToken,
        expected_revision: u64,
        payload: &str,
        records: &[StoreRecord],
    ) -> Result<u64, StoreError> {
        let mut states = self.lock();
        let staged = states.get_mut(state_id).ok_or(StoreError::Fenced)?;
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
                .is_some_and(|value| *value != record.value)
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
                .or_insert_with(|| record.value.clone());
        }
        staged.revision = revision;
        staged.payload = Some(payload.to_owned());
        staged.payloads.push(payload.to_owned());
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
        let staged = self
            .writes
            .get(state_id)
            .map(|staged| staged.records)
            .unwrap_or_default();
        let missing: Vec<_> = keys
            .iter()
            .filter(|key| !staged.contains_key(*key))
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
            .map(|key| match staged.get(key) {
                Some(value) => Some(value.clone()),
                None => read.next().flatten(),
            })
            .collect())
    }

    async fn acquire_staged(
        &mut self,
        state_id: &str,
        owner_id: OwnerId,
    ) -> Result<OwnedState, StoreError> {
        let staged = match self.writes.get(state_id) {
            Some(staged) => staged,
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
                    payload: state.payload,
                    payloads: Vec::new(),
                    records: BTreeMap::new(),
                }
            }
        };
        let fence =
            staged.owner.fence().checked_add(1).ok_or_else(|| {
                StoreError::NotCommitted("staged owner fence overflow".to_owned())
            })?;
        let owner = OwnerToken::new(owner_id, fence);
        let mut states = self.writes.lock();
        let entry = states.entry(state_id.to_owned()).or_insert(staged);
        entry.owner = owner.clone();
        Ok(OwnedState {
            owner,
            state: StoredState {
                revision: entry.revision,
                payload: entry.payload.clone(),
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
                    payload: staged.payload,
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
            let staged: Vec<_> = self
                .writes
                .lock()
                .iter()
                .filter(|(state_id, staged)| {
                    staged.payload.is_some() && !states.contains(*state_id)
                })
                .map(|(state_id, _)| state_id.clone())
                .collect();
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
                        .into_keys()
                        .filter(|key| !keys.contains(key))
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
                Some(staged) => Ok(Some(staged.owner)),
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
