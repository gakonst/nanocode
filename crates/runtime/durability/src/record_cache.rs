use std::{
    collections::{HashMap, VecDeque},
    sync::Arc,
};

use nanocode_durability_core::{
    Committed, OwnedCommit, OwnedState, OwnerId, OwnerToken, StateCommit, StateStore, StoreError,
    StoreFuture, StoreRecord, StoreUpdate, StoredState,
};
use nanocode_sync::Actor;

type Key = (Arc<str>, Arc<str>);

struct Records {
    budget: usize,
    used: usize,
    values: HashMap<Key, Arc<str>>,
    order: VecDeque<Key>,
}

impl Records {
    fn insert(&mut self, key: Key, value: Arc<str>) {
        let size = key.0.len() + key.1.len() + value.len();
        if size > self.budget || self.values.contains_key(&key) {
            return;
        }
        while self.used + size > self.budget {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            if let Some(evicted) = self.values.remove(&oldest) {
                self.used -= oldest.0.len() + oldest.1.len() + evicted.len();
            }
        }
        self.used += size;
        self.order.push_back(key.clone());
        self.values.insert(key, value);
    }
}

#[derive(Clone)]
pub struct RecordCache {
    records: Actor<Records>,
}

impl RecordCache {
    pub fn new(budget_bytes: usize) -> std::io::Result<Self> {
        Ok(Self {
            records: Actor::spawn(Records {
                budget: budget_bytes,
                used: 0,
                values: HashMap::new(),
                order: VecDeque::new(),
            })?,
        })
    }

    async fn get(&self, state_id: &str, keys: &[String]) -> Vec<Option<Arc<str>>> {
        let state: Arc<str> = Arc::from(state_id);
        let keys = keys
            .iter()
            .map(|key| Arc::<str>::from(key.as_str()))
            .collect::<Vec<_>>();
        let missed = keys.len();
        self.records
            .run(move |records| {
                Box::pin(async move {
                    keys.into_iter()
                        .map(|key| records.values.get(&(Arc::clone(&state), key)).cloned())
                        .collect::<Vec<_>>()
                })
            })
            .await
            .unwrap_or_else(|_| vec![None; missed])
    }

    async fn put(&self, state_id: &str, records: Vec<(String, Arc<str>)>) {
        if records.is_empty() {
            return;
        }
        let state: Arc<str> = Arc::from(state_id);
        let _ = self
            .records
            .run(move |cached| {
                Box::pin(async move {
                    for (key, value) in records {
                        cached.insert((Arc::clone(&state), Arc::from(key)), value);
                    }
                })
            })
            .await;
    }

    pub fn wrap<S: StateStore + 'static>(&self, store: S) -> CachedStore<S> {
        CachedStore {
            cache: self.clone(),
            base: Box::new(store),
        }
    }
}

fn written(records: &[StoreRecord]) -> Vec<(String, Arc<str>)> {
    records
        .iter()
        .map(|record| (record.key.clone(), Arc::from(record.value.as_str())))
        .collect()
}

pub struct CachedStore<S: ?Sized> {
    cache: RecordCache,
    base: Box<S>,
}

impl<S: StateStore + ?Sized + 'static> StateStore for CachedStore<S> {
    fn read_record<'a>(
        &'a mut self,
        state_id: &'a str,
        key: &'a str,
    ) -> StoreFuture<'a, Result<Option<String>, StoreError>> {
        Box::pin(async move {
            Ok(self
                .read_records(state_id, std::slice::from_ref(&key.to_owned()))
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
        Box::pin(async move {
            let cached = self.cache.get(state_id, keys).await;
            let missing = keys
                .iter()
                .zip(&cached)
                .filter(|(_, value)| value.is_none())
                .map(|(key, _)| key.clone())
                .collect::<Vec<_>>();
            if missing.is_empty() {
                return Ok(cached
                    .into_iter()
                    .map(|value| value.map(|value| value.to_string()))
                    .collect());
            }
            let read = self.base.read_records(state_id, &missing).await?;
            if read.len() != missing.len() {
                return Err(StoreError::Backend(format!(
                    "store answered {} of {} record reads",
                    read.len(),
                    missing.len()
                )));
            }
            let found = missing
                .iter()
                .zip(&read)
                .filter_map(|(key, value)| {
                    value
                        .as_deref()
                        .map(|value| (key.clone(), Arc::<str>::from(value)))
                })
                .collect::<Vec<_>>();
            self.cache.put(state_id, found).await;
            let mut read = read.into_iter();
            Ok(cached
                .into_iter()
                .map(|value| match value {
                    Some(value) => Some(value.to_string()),
                    None => read.next().flatten(),
                })
                .collect())
        })
    }

    fn peek<'a>(
        &'a mut self,
        state_id: &'a str,
    ) -> StoreFuture<'a, Result<StoredState, StoreError>> {
        self.base.peek(state_id)
    }

    fn list_states<'a>(
        &'a mut self,
        limit: usize,
    ) -> StoreFuture<'a, Result<Vec<String>, StoreError>> {
        self.base.list_states(limit)
    }

    fn list_records<'a>(
        &'a mut self,
        state_id: &'a str,
    ) -> StoreFuture<'a, Result<Vec<String>, StoreError>> {
        self.base.list_records(state_id)
    }

    fn owner<'a>(
        &'a mut self,
        state_id: &'a str,
    ) -> StoreFuture<'a, Result<Option<OwnerToken>, StoreError>> {
        self.base.owner(state_id)
    }

    fn acquire<'a>(
        &'a mut self,
        state_id: &'a str,
        owner_id: OwnerId,
    ) -> StoreFuture<'a, Result<OwnedState, StoreError>> {
        self.base.acquire(state_id, owner_id)
    }

    fn acquire_with<'a>(
        &'a mut self,
        state_id: &'a str,
        owner_id: OwnerId,
        update: StoreUpdate,
    ) -> StoreFuture<'a, Result<OwnedState, StoreError>> {
        self.base.acquire_with(state_id, owner_id, update)
    }

    fn batches_companion_acquisition(&self) -> bool {
        self.base.batches_companion_acquisition()
    }

    fn supports_events(&self) -> bool {
        self.base.supports_events()
    }

    fn supports_summaries(&self) -> bool {
        self.base.supports_summaries()
    }

    fn acquire_with_companion<'a>(
        &'a mut self,
        state_id: &'a str,
        owner_id: OwnerId,
        update: StoreUpdate,
        companion_id: &'a str,
        companion_owner: OwnerId,
    ) -> StoreFuture<'a, Result<(OwnedState, OwnedState), StoreError>> {
        self.base
            .acquire_with_companion(state_id, owner_id, update, companion_id, companion_owner)
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
            let revision = self
                .base
                .replace(state_id, owner, expected_revision, payload, records)
                .await?;
            self.cache.put(state_id, written(records)).await;
            Ok(revision)
        })
    }

    fn replace_to<'a>(
        &'a mut self,
        state_id: &'a str,
        owner: &'a OwnerToken,
        expected_revision: u64,
        revision: u64,
        payload: &'a str,
        records: &'a [StoreRecord],
    ) -> StoreFuture<'a, Result<Option<u64>, StoreError>> {
        Box::pin(async move {
            let replaced = self
                .base
                .replace_to(
                    state_id,
                    owner,
                    expected_revision,
                    revision,
                    payload,
                    records,
                )
                .await?;
            if replaced.is_some() {
                self.cache.put(state_id, written(records)).await;
            }
            Ok(replaced)
        })
    }

    fn put_records<'a>(
        &'a mut self,
        state_id: &'a str,
        records: &'a [StoreRecord],
    ) -> StoreFuture<'a, Result<bool, StoreError>> {
        Box::pin(async move {
            let stored = self.base.put_records(state_id, records).await?;
            if stored {
                self.cache.put(state_id, written(records)).await;
            }
            Ok(stored)
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
            let records = commit.records;
            let committed = self
                .base
                .commit(state_id, owner, expected_revision, commit)
                .await?;
            self.cache.put(state_id, written(records)).await;
            Ok(committed)
        })
    }

    fn commit_many(
        &mut self,
        commits: Vec<OwnedCommit>,
    ) -> StoreFuture<'_, Vec<Result<Committed, StoreError>>> {
        Box::pin(async move {
            let records = commits
                .iter()
                .map(|commit| (commit.state_id.clone(), written(&commit.records)))
                .collect::<Vec<_>>();
            let results = self.base.commit_many(commits).await;
            for ((state_id, records), result) in records.into_iter().zip(&results) {
                if result.is_ok() {
                    self.cache.put(&state_id, records).await;
                }
            }
            results
        })
    }

    fn flush(&mut self) -> StoreFuture<'_, Result<(), StoreError>> {
        self.base.flush()
    }

    fn concurrent_handle(&self) -> Option<Box<dyn StateStore>> {
        self.base.concurrent_handle().map(|base| {
            Box::new(CachedStore {
                cache: self.cache.clone(),
                base,
            }) as Box<dyn StateStore>
        })
    }
}
