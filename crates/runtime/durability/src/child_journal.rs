use std::{
    collections::{HashMap, HashSet},
    future::Future,
    sync::{
        Arc, Weak,
        atomic::{AtomicBool, Ordering},
    },
};

use arc_swap::ArcSwapOption;
use nanocode_agent_core::backend::{BackendFuture, ChildJournal, ChildJournalStore};

use tokio::sync::{mpsc, oneshot};

use crate::{
    EncodedPayload, OperationStatus, OwnedState, OwnerId, OwnerToken, StateStore, StoreError,
    shared_store::SharedStore,
};

pub(crate) struct Seed {
    pub(crate) store: SharedStore,
    pub(crate) state_id: String,
    pub(crate) root_state_id: String,
    awaited: Arc<AtomicBool>,
    sender: oneshot::Sender<OwnedState>,
}

impl Seed {
    pub(crate) fn awaited(&self) -> bool {
        self.awaited.load(Ordering::Acquire) && !self.sender.is_closed()
    }

    pub(crate) fn resolve(self, owned: OwnedState) {
        drop(self.sender.send(owned));
    }
}

pub struct JournalSeed(Option<Arc<ArcSwapOption<Seed>>>);

impl JournalSeed {
    #[must_use]
    pub fn claim(&self) -> JournalClaim {
        JournalClaim(self.0.as_ref().map_or_else(Weak::new, Arc::downgrade))
    }

    pub(crate) fn adopt(&self, root_state_id: &str, store: &SharedStore) -> bool {
        let Some(seed) = &self.0 else {
            return false;
        };
        let seed = seed.load();
        let Some(seed) = seed
            .as_ref()
            .filter(|seed| seed.root_state_id == root_state_id && seed.store.same_store(store))
        else {
            return false;
        };
        seed.awaited.store(true, Ordering::Release);
        true
    }
}

#[derive(Clone)]
pub struct JournalClaim(Weak<ArcSwapOption<Seed>>);

impl JournalClaim {
    pub(crate) fn take(&self) -> Option<Seed> {
        Arc::try_unwrap(self.0.upgrade()?.swap(None)?).ok()
    }
}

struct Fenced {
    owner: Option<(OwnerToken, u64)>,

    committed: HashSet<String>,

    seed: Option<(oneshot::Receiver<OwnedState>, Arc<AtomicBool>)>,
}

impl Fenced {
    async fn acquire(
        &mut self,
        store: &mut SharedStore,
        state_id: &str,
    ) -> std::io::Result<Option<String>> {
        let seeded = match self.seed.as_mut() {
            Some((seed, awaited)) => {
                awaited.store(true, Ordering::Release);
                seed.await.ok()
            }
            None => None,
        };
        self.seed = None;
        let owned = match seeded {
            Some(owned) => owned,
            None => store
                .acquire(state_id, OwnerId::new())
                .await
                .map_err(std::io::Error::other)?,
        };
        self.owner = Some((owned.owner, owned.state.revision));
        Ok(owned.state.payload)
    }

    async fn save(
        &mut self,
        store: &mut SharedStore,
        state_id: &str,
        payload: String,
        records: Vec<Arc<str>>,
    ) -> std::io::Result<()> {
        let mut staged = Vec::new();
        for json in records {
            let committed = &self.committed;
            EncodedPayload::from_json(json)
                .stage_unless(&mut staged, &|key| committed.contains(key));
        }
        staged.sort_unstable_by(|a, b| a.key.cmp(&b.key));
        staged.dedup_by(|a, b| a.key == b.key);
        if self.owner.is_none() {
            self.acquire(store, state_id).await?;
        }
        let (owner, revision) = self
            .owner
            .clone()
            .ok_or_else(|| std::io::Error::other("journal owner was not acquired"))?;
        let revision = match store
            .replace(state_id, &owner, revision, &payload, &staged)
            .await
        {
            Ok(revision) => revision,
            Err(error) => {
                if !matches!(error, StoreError::NotCommitted(_) | StoreError::Fenced) {
                    self.owner = None;
                }
                return Err(std::io::Error::other(error));
            }
        };
        self.owner = Some((owner, revision));
        self.committed
            .extend(staged.into_iter().map(|record| record.key));
        Ok(())
    }
}

enum Write {
    Load(oneshot::Sender<std::io::Result<Option<String>>>),
    Save {
        payload: String,
        records: Vec<Arc<str>>,
        result: oneshot::Sender<std::io::Result<()>>,
    },
}

async fn write_journal(
    state_id: Arc<str>,
    mut store: SharedStore,
    mut fenced: Fenced,
    mut writes: mpsc::UnboundedReceiver<Write>,
) {
    while let Some(write) = writes.recv().await {
        match write {
            Write::Load(result) => {
                drop(result.send(fenced.acquire(&mut store, &state_id).await));
            }
            Write::Save {
                payload,
                records,
                result,
            } => {
                drop(result.send(fenced.save(&mut store, &state_id, payload, records).await));
            }
        }
    }
}

#[derive(Clone)]
struct SharedJournal {
    state_id: Arc<str>,
    store: SharedStore,
    writes: mpsc::UnboundedSender<Write>,
    manifests: Arc<ArcSwapOption<HashMap<String, String>>>,
}

impl SharedJournal {
    async fn write<T>(
        &self,
        write: impl FnOnce(oneshot::Sender<std::io::Result<T>>) -> Write,
    ) -> std::io::Result<T> {
        let (result, receiver) = oneshot::channel();
        self.writes
            .send(write(result))
            .map_err(|_| writer_stopped())?;
        receiver.await.map_err(|_| writer_stopped())?
    }
}

fn writer_stopped() -> std::io::Error {
    std::io::Error::other("subagent journal writer stopped")
}

impl ChildJournalStore for SharedJournal {
    fn pending_turn(
        &self,
        session_id: String,
    ) -> BackendFuture<std::io::Result<Option<(String, serde_json::Value)>>> {
        let mut store = self.store.clone();
        Box::pin(async move {
            let stored = store
                .peek(&session_id)
                .await
                .map_err(std::io::Error::other)?;
            let state = crate::session::reduce_peeked(stored).map_err(std::io::Error::other)?;
            let Some((id, operation)) = state.pending_operations().into_iter().next() else {
                return Ok(None);
            };
            let input = operation
                .input
                .load(&mut store, &session_id)
                .await
                .map_err(std::io::Error::other)?;
            Ok(Some((
                id.to_owned(),
                input.decode().map_err(std::io::Error::other)?,
            )))
        })
    }

    fn cancelled_turns(
        &self,
        session_id: String,
        operation_ids: Vec<String>,
    ) -> BackendFuture<std::io::Result<Vec<String>>> {
        let mut store = self.store.clone();
        Box::pin(async move {
            let stored = store
                .peek(&session_id)
                .await
                .map_err(std::io::Error::other)?;
            let state = crate::session::reduce_peeked(stored).map_err(std::io::Error::other)?;
            Ok(operation_ids
                .into_iter()
                .filter(|id| {
                    state.operation(id).is_some_and(|operation| {
                        matches!(operation.status, OperationStatus::Cancelled { .. })
                    })
                })
                .collect())
        })
    }

    fn load(&self) -> BackendFuture<std::io::Result<Option<String>>> {
        let journal = self.clone();
        Box::pin(async move { journal.write(Write::Load).await })
    }

    fn save(&self, payload: String, records: Vec<Arc<str>>) -> BackendFuture<std::io::Result<()>> {
        let journal = self.clone();
        Box::pin(async move {
            journal
                .write(|result| Write::Save {
                    payload,
                    records,
                    result,
                })
                .await
        })
    }

    fn records_children(&self) -> bool {
        false
    }

    fn load_records(&self, keys: Vec<String>) -> BackendFuture<std::io::Result<Vec<String>>> {
        let mut store = self.store.clone();
        let state_id = Arc::clone(&self.state_id);
        let fallback = keys
            .iter()
            .map(|key| self.load_record(key.clone()))
            .collect::<Vec<_>>();
        Box::pin(async move {
            let wanted = keys
                .iter()
                .map(|key| EncodedPayload::by_key(key))
                .collect::<Vec<_>>();
            if let Ok(loaded) = EncodedPayload::load_many(&wanted, &mut store, &state_id).await
                && let Ok(records) = loaded
                    .iter()
                    .map(|payload| payload.json().map(str::to_owned))
                    .collect::<Result<Vec<_>, _>>()
            {
                return Ok(records);
            }
            let mut records = Vec::with_capacity(fallback.len());
            for load in fallback {
                records.push(load.await?);
            }
            Ok(records)
        })
    }

    fn load_record(&self, key: String) -> BackendFuture<std::io::Result<String>> {
        let mut store = self.store.clone();
        let state_id = Arc::clone(&self.state_id);
        let manifests = Arc::clone(&self.manifests);
        Box::pin(async move {
            let loaded = match EncodedPayload::by_key(&key)
                .load(&mut store, &state_id)
                .await
            {
                Ok(loaded) => loaded,
                Err(error) => {
                    let Some(filed) =
                        filed_by_content(&mut store, &state_id, &manifests, &key).await?
                    else {
                        return Err(std::io::Error::other(error));
                    };
                    EncodedPayload::by_key(&filed)
                        .load(&mut store, &state_id)
                        .await
                        .map_err(std::io::Error::other)?
                }
            };
            Ok(loaded.json().map_err(std::io::Error::other)?.to_owned())
        })
    }
}

async fn filed_by_content(
    store: &mut SharedStore,
    state_id: &str,
    manifests: &ArcSwapOption<HashMap<String, String>>,
    key: &str,
) -> std::io::Result<Option<String>> {
    if store
        .read_record(state_id, key)
        .await
        .map_err(std::io::Error::other)?
        .is_some()
    {
        return Ok(None);
    }
    let known = match manifests.load_full() {
        Some(known) => known,
        None => {
            let scanned = Arc::new(
                EncodedPayload::manifests_by_content(store, state_id)
                    .await
                    .map_err(std::io::Error::other)?,
            );
            manifests.store(Some(Arc::clone(&scanned)));
            scanned
        }
    };
    Ok(known.get(key).cloned())
}

fn journal(
    store: SharedStore,
    state_id: String,
    seed: Option<(oneshot::Receiver<OwnedState>, Arc<AtomicBool>)>,
) -> ChildJournal {
    let state_id = Arc::<str>::from(state_id);
    let (writes, received) = mpsc::unbounded_channel();
    spawn_writer(write_journal(
        Arc::clone(&state_id),
        store.clone(),
        Fenced {
            owner: None,
            committed: HashSet::new(),
            seed,
        },
        received,
    ));
    ChildJournal::new(Arc::new(SharedJournal {
        state_id,
        store,
        writes,
        manifests: Arc::default(),
    }))
}

pub(crate) fn child_journal(store: SharedStore, root_state_id: &str) -> ChildJournal {
    journal(store, format!("{root_state_id}:subagents"), None)
}

pub(crate) fn seeded_child_journal(
    store: SharedStore,
    root_state_id: &str,
) -> (ChildJournal, JournalSeed) {
    if !store.batches_companion_acquisition() {
        return (child_journal(store, root_state_id), JournalSeed(None));
    }
    let state_id = format!("{root_state_id}:subagents");
    let (sender, receiver) = oneshot::channel();
    let awaited = Arc::new(AtomicBool::new(false));
    (
        journal(
            store.clone(),
            state_id.clone(),
            Some((receiver, Arc::clone(&awaited))),
        ),
        JournalSeed(Some(Arc::new(ArcSwapOption::from_pointee(Seed {
            store,
            state_id,
            root_state_id: root_state_id.to_owned(),
            awaited,
            sender,
        })))),
    )
}

#[cfg(not(target_family = "wasm"))]
fn spawn_writer(writer: impl Future<Output = ()> + Send + 'static) {
    if let Ok(runtime) = tokio::runtime::Handle::try_current() {
        drop(runtime.spawn(writer));
    }
}

#[cfg(target_family = "wasm")]
fn spawn_writer(writer: impl Future<Output = ()> + 'static) {
    nanocode_platform::spawn_local(writer);
}
