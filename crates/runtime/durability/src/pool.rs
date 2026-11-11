use std::{collections::BTreeSet, sync::Arc};

use nanocode_durability_core::{
    OwnedState, OwnerId, StateStore, StoreError, StoreRecord, StoredState,
};
use nanocode_sync::{Atom, AtomMap, Owned};

use crate::SharedStore;

const POOL: &str = ":pool";

#[derive(Default)]
pub(crate) struct Pools {
    roots: AtomMap<String, Arc<str>>,
    pools: AtomMap<Arc<str>, Arc<Pool>>,
}

pub(crate) struct Pool {
    state: String,
    held: Atom<nanocode_sync::Map<String, Arc<str>>>,
    listed: Atom<Option<Arc<BTreeSet<String>>>>,
    writer: Owned<Writer>,
}

enum Writer {
    Unread,
    Owned {
        owned: OwnedState,
        keys: BTreeSet<String>,
    },
}

#[derive(serde::Deserialize)]
struct Head {
    pool: BTreeSet<String>,
}

#[derive(serde::Serialize)]
struct HeadRef<'a> {
    pool: &'a BTreeSet<String>,
}

#[derive(serde::Deserialize)]
struct Described<'a> {
    #[serde(borrow, default)]
    nanocode_session: Option<Session<'a>>,
}

#[derive(serde::Deserialize)]
struct Session<'a> {
    #[serde(borrow, default)]
    lineage: Option<Lineage<'a>>,
}

#[derive(serde::Deserialize)]
struct Lineage<'a> {
    #[serde(borrow)]
    root_session_id: std::borrow::Cow<'a, str>,
    #[serde(borrow)]
    origin: std::borrow::Cow<'a, str>,
}

pub fn pool_state(root: &str) -> String {
    format!("{root}{POOL}")
}

fn session_of(state_id: &str) -> &str {
    state_id
        .split_once(':')
        .map_or(state_id, |(session, _)| session)
}

fn listed(payload: Option<&str>) -> Result<BTreeSet<String>, StoreError> {
    match payload {
        None => Ok(BTreeSet::new()),
        Some(payload) => serde_json::from_str::<Head>(payload)
            .map(|head| head.pool)
            .map_err(|error| StoreError::Backend(format!("invalid record pool: {error}"))),
    }
}

impl Pools {
    pub(crate) fn adopt(&self, state_id: &str, root: &str) {
        if state_id != root && !self.roots.contains_key(state_id) {
            self.roots.insert(state_id.to_owned(), Arc::from(root));
        }
    }

    pub(crate) fn observe(&self, state_id: &str, payload: Option<&str>) {
        let Some(payload) = payload else {
            return;
        };
        if state_id.contains(':') || self.roots.contains_key(state_id) {
            return;
        }
        let Ok(Described {
            nanocode_session: Some(Session {
                lineage: Some(lineage),
            }),
        }) = serde_json::from_str::<Described<'_>>(payload)
        else {
            return;
        };
        if lineage.origin == "subagent" {
            self.adopt(state_id, &lineage.root_session_id);
        }
    }

    pub(crate) fn of(&self, state_id: &str) -> Option<Arc<Pool>> {
        if state_id.ends_with(POOL) {
            return None;
        }
        let session = session_of(state_id);
        let root = self
            .roots
            .get(session)
            .unwrap_or_else(|| Arc::from(session));
        if let Some(pool) = self.pools.get(&root) {
            return Some(pool);
        }
        Some(self.pools.update(|pools| {
            Arc::clone(pools.entry(Arc::clone(&root)).or_insert_with(|| {
                Arc::new(Pool {
                    state: pool_state(&root),
                    held: Atom::default(),
                    listed: Atom::default(),
                    writer: Owned::new(Writer::Unread),
                })
            }))
        }))
    }
}

impl Pool {
    pub(crate) fn held(&self, keys: &[String]) -> Vec<Option<Arc<str>>> {
        self.held
            .read(|held| keys.iter().map(|key| held.get(key).cloned()).collect())
    }

    fn hold(&self, records: impl IntoIterator<Item = (String, Arc<str>)>) {
        let records = records.into_iter().collect::<Vec<_>>();
        if records.is_empty() {
            return;
        }
        self.held.update(|held| {
            for (key, value) in &records {
                held.insert(key.clone(), Arc::clone(value));
            }
        });
    }

    async fn listing(
        &self,
        store: &SharedStore,
        wanted: &[&String],
    ) -> Result<Arc<BTreeSet<String>>, StoreError> {
        if let Some(listed) = Option::clone(&self.listed.get())
            && wanted.iter().all(|key| listed.contains(*key))
        {
            return Ok(listed);
        }
        let stored = store.clone().peek(&self.state).await?;
        let keys = Arc::new(listed(stored.payload.as_deref())?);
        self.listed.set(Some(Arc::clone(&keys)));
        Ok(keys)
    }

    pub(crate) async fn fill(
        &self,
        store: &SharedStore,
        keys: &[String],
        values: &mut [Option<String>],
    ) -> Result<(), StoreError> {
        let missing = keys
            .iter()
            .zip(values.iter())
            .filter(|(_, value)| value.is_none())
            .map(|(key, _)| key)
            .collect::<Vec<_>>();
        if missing.is_empty() {
            return Ok(());
        }
        let listed = self.listing(store, &missing).await?;
        let pooled = missing
            .into_iter()
            .filter(|key| listed.contains(*key))
            .cloned()
            .collect::<Vec<_>>();
        if pooled.is_empty() {
            return Ok(());
        }
        let read = store.clone().read_records(&self.state, &pooled).await?;
        let found = pooled
            .into_iter()
            .zip(read)
            .filter_map(|(key, value)| Some((key, Arc::<str>::from(value?))))
            .collect::<Vec<_>>();
        for (key, value) in keys.iter().zip(values.iter_mut()) {
            if value.is_none()
                && let Some((_, found)) = found.iter().find(|(pooled, _)| pooled == key)
            {
                *value = Some(found.to_string());
            }
        }
        self.hold(found);
        Ok(())
    }

    pub(crate) async fn ensure(
        &self,
        store: &SharedStore,
        records: Vec<StoreRecord>,
    ) -> Result<(), StoreError> {
        let keys = records
            .iter()
            .map(|record| record.key.clone())
            .collect::<Vec<_>>();
        let fresh = records
            .into_iter()
            .zip(self.held(&keys))
            .filter_map(|(record, held)| held.is_none().then_some(record))
            .collect::<Vec<_>>();
        if fresh.is_empty() {
            return Ok(());
        }
        let mut store = store.clone();
        let mut writer = self.writer.acquire().await;
        let mut fenced = false;
        loop {
            if matches!(*writer, Writer::Unread) {
                let owned = store.acquire(&self.state, OwnerId::new()).await?;
                let keys = listed(owned.state.payload.as_deref())?;
                *writer = Writer::Owned { owned, keys };
            }
            let Writer::Owned { owned, keys } = &mut *writer else {
                continue;
            };
            let added = fresh
                .iter()
                .filter(|record| !keys.contains(&record.key))
                .cloned()
                .collect::<Vec<_>>();
            if added.is_empty() {
                break;
            }
            let mut all = keys.clone();
            all.extend(added.iter().map(|record| record.key.clone()));
            let payload = serde_json::to_string(&HeadRef { pool: &all })
                .map_err(|error| StoreError::Backend(error.to_string()))?;
            match store
                .replace(
                    &self.state,
                    &owned.owner,
                    owned.state.revision,
                    &payload,
                    &added,
                )
                .await
            {
                Ok(revision) => {
                    owned.state = StoredState {
                        revision,
                        payload: Some(payload),
                    };
                    *keys = all;
                    break;
                }
                Err(StoreError::Fenced | StoreError::Conflict { .. }) if !fenced => {
                    fenced = true;
                    *writer = Writer::Unread;
                }
                Err(error) => return Err(error),
            }
        }
        if let Writer::Owned { keys, .. } = &*writer {
            self.listed.set(Some(Arc::new(keys.clone())));
        }
        drop(writer);
        self.hold(
            fresh
                .into_iter()
                .map(|record| (record.key, Arc::from(record.value))),
        );
        Ok(())
    }
}
