use std::sync::Arc;

use nanocode_durability_core::{
    Committed, KnownSummary, ListedState, OwnedCommit, OwnedState, OwnerId, OwnerToken,
    StagedWrites, StateCommit, StateStore, StoreError, StoreFuture, StoreHandles, StoreRecord,
    StoreUpdate, StoredState,
};
use nanocode_sync::Atom;

type Key = (Arc<str>, Arc<str>);

const STALE_ORDER_PER_VALUE: usize = 1;

#[derive(Clone, Default)]
struct Records {
    used: usize,
    inserted: u64,
    values: nanocode_sync::Map<Key, (Arc<str>, u64)>,
    order: nanocode_sync::Vector<(Key, u64)>,
}

fn size_of(key: &Key, value: &str) -> usize {
    key.0.len() + key.1.len() + value.len()
}

impl Records {
    fn insert(&mut self, budget: usize, key: Key, value: Arc<str>) {
        assert!(
            !value.is_empty(),
            "a record cache never holds a blank record"
        );
        let size = size_of(&key, &value);
        if size > budget || self.values.contains_key(&key) {
            return;
        }
        while self.used + size > budget {
            let Some((oldest, at)) = self.order.pop_front() else {
                break;
            };
            if self
                .values
                .get(&oldest)
                .is_some_and(|(_, inserted)| *inserted == at)
                && let Some((evicted, _)) = self.values.remove(&oldest)
            {
                self.used -= size_of(&oldest, &evicted);
            }
        }
        assert!(
            self.used + size <= budget,
            "a record cache evicts until a record fits its budget"
        );
        self.inserted = self
            .inserted
            .checked_add(1)
            .expect("a record cache counts its inserts in u64");
        self.used += size;
        self.order.push_back((key.clone(), self.inserted));
        self.values.insert(key, (value, self.inserted));
        if self.order.len() > self.values.len() * (1 + STALE_ORDER_PER_VALUE) {
            let values = &self.values;
            self.order
                .retain(|(key, at)| values.get(key).is_some_and(|(_, inserted)| inserted == at));
        }
        assert!(
            self.order.len() >= self.values.len(),
            "a record cache orders every record it holds"
        );
        assert!(
            self.order.len() <= self.values.len() * (1 + STALE_ORDER_PER_VALUE),
            "a record cache's eviction order holds at most one forgotten record per held one"
        );
    }

    fn forget(&mut self, key: &Key) {
        if let Some((removed, _)) = self.values.remove(key) {
            let size = size_of(key, &removed);
            assert!(
                self.used >= size,
                "a record cache counts the bytes of every record it holds"
            );
            self.used -= size;
        }
    }
}

#[derive(Clone)]
pub struct RecordCache {
    records: Arc<Atom<Records>>,
    budget: usize,
}

impl RecordCache {
    pub fn new(budget_bytes: usize) -> std::io::Result<Self> {
        Ok(Self {
            records: Arc::default(),
            budget: budget_bytes,
        })
    }

    fn get(&self, state_id: &str, keys: &[String]) -> Vec<Option<Arc<str>>> {
        let state: Arc<str> = Arc::from(state_id);
        self.records.read(|records| {
            keys.iter()
                .map(|key| {
                    records
                        .values
                        .get(&(Arc::clone(&state), Arc::from(key.as_str())))
                        .map(|(value, _)| Arc::clone(value))
                })
                .collect()
        })
    }

    fn put(&self, state_id: &str, records: Vec<(String, Arc<str>)>) {
        if records.is_empty() {
            return;
        }
        let state: Arc<str> = Arc::from(state_id);
        let records = records
            .into_iter()
            .map(|(key, value)| ((Arc::clone(&state), Arc::<str>::from(key)), value))
            .collect::<Vec<_>>();
        let budget = self.budget;
        self.records.update(|cached| {
            for (key, value) in &records {
                if value.is_empty() {
                    cached.forget(key);
                } else {
                    cached.insert(budget, key.clone(), Arc::clone(value));
                }
            }
        });
    }

    pub fn remember(&self, writes: &StagedWrites) {
        writes.written(|state_id, records| self.put(state_id, records));
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
            let cached = self.cache.get(state_id, keys);
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
            self.cache.put(state_id, found);
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

    fn list_summaries<'a>(
        &'a mut self,
        limit: usize,
    ) -> StoreFuture<'a, Result<Option<Vec<ListedState>>, StoreError>> {
        self.base.list_summaries(limit)
    }

    fn remember_summaries<'a>(
        &'a mut self,
        summaries: Vec<KnownSummary>,
    ) -> StoreFuture<'a, Result<(), StoreError>> {
        self.base.remember_summaries(summaries)
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

    fn acquire_many(
        &mut self,
        acquisitions: Vec<(String, OwnerId)>,
    ) -> StoreFuture<'_, Vec<Result<OwnedState, StoreError>>> {
        self.base.acquire_many(acquisitions)
    }

    fn acquire_with<'a>(
        &'a mut self,
        state_id: &'a str,
        owner_id: OwnerId,
        update: StoreUpdate,
    ) -> StoreFuture<'a, Result<OwnedState, StoreError>> {
        self.base.acquire_with(state_id, owner_id, update)
    }

    fn colocation(&self) -> Option<std::sync::Arc<dyn crate::Colocation>> {
        self.base.colocation()
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
            self.cache.put(state_id, written(records));
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
                self.cache.put(state_id, written(records));
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
                self.cache.put(state_id, written(records));
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
            self.cache.put(state_id, written(records));
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
                    self.cache.put(&state_id, records);
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

    fn group_writers(&self) -> usize {
        self.base.group_writers()
    }

    fn pipeline_handle(&self) -> Option<Box<dyn StateStore>> {
        self.base.pipeline_handle().map(|base| {
            Box::new(CachedStore {
                cache: self.cache.clone(),
                base,
            }) as Box<dyn StateStore>
        })
    }

    fn handles(&self) -> Option<Arc<dyn StoreHandles>> {
        self.base.handles().map(|base| {
            Arc::new(CachedHandles {
                cache: self.cache.clone(),
                base,
            }) as Arc<dyn StoreHandles>
        })
    }
}

struct CachedHandles {
    cache: RecordCache,
    base: Arc<dyn StoreHandles>,
}

impl StoreHandles for CachedHandles {
    fn handle(&self) -> Box<dyn StateStore> {
        Box::new(CachedStore {
            cache: self.cache.clone(),
            base: self.base.handle(),
        })
    }
}
