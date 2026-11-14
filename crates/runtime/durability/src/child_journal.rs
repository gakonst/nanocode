use std::{
    collections::{HashMap, HashSet},
    future::Future,
    sync::{
        Arc, Weak,
        atomic::{AtomicBool, Ordering},
    },
};

use nanocode_agent_core::backend::{
    BackendFuture, ChildJournal, ChildJournalStore, DeferredJournal, JournalCommit, JournalFill,
};
use nanocode_sync::SwapOption;

use nanocode_sync::{mpsc, oneshot};

use crate::{
    EncodedPayload, OperationStatus, OwnedState, OwnerId, OwnerToken, StateStore, StoreError,
    session::PendingTurn,
    shared_store::{LatestWrite, SharedStore, latest_write},
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

pub struct JournalSeed(Option<Arc<SwapOption<Seed>>>);

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
pub struct JournalClaim(Weak<SwapOption<Seed>>);

impl JournalClaim {
    pub(crate) fn take(&self) -> Option<Seed> {
        Arc::try_unwrap(self.0.upgrade()?.swap(None)?).ok()
    }
}

struct Fenced {
    owner: Option<(OwnerToken, u64)>,

    committed: HashSet<String>,

    seed: Option<(oneshot::Receiver<OwnedState>, Arc<AtomicBool>)>,

    next: u64,

    inflight: std::collections::VecDeque<InFlight>,

    failure: Option<std::io::Error>,

    retry: Vec<String>,

    pending: Pending,
}

type Pending = Arc<nanocode_sync::Atom<nanocode_sync::Map<String, Arc<str>>>>;

type DeferredKeys = Arc<nanocode_sync::Slot<Vec<String>>>;

struct InFlight {
    expected: u64,
    write: Option<Arc<LatestWrite>>,
    deferred: Option<DeferredKeys>,
    done: oneshot::Receiver<Result<u64, StoreError>>,
    staged: Vec<String>,
    pending: Vec<String>,
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
        self.next = owned.state.revision;
        self.owner = Some((owned.owner, owned.state.revision));
        Ok(owned.state.payload)
    }

    async fn save(
        &mut self,
        store: &mut SharedStore,
        state_id: &str,
        payload: String,
        records: Vec<(Arc<str>, Arc<str>)>,
    ) -> std::io::Result<()> {
        if self.failure.is_some() {
            self.drain().await;
            if let Some((_, revision)) = &self.owner {
                self.next = *revision;
            }
            if let Some(error) = self.failure.take() {
                return Err(error);
            }
        }
        let retry = std::mem::take(&mut self.retry)
            .into_iter()
            .filter(|key| !self.committed.contains(key))
            .filter_map(|key| {
                let json = self.pending.read(|pending| pending.get(&key).cloned())?;
                Some((Arc::<str>::from(key), json))
            })
            .collect::<Vec<_>>();
        let mut staged = Vec::new();
        let mut pending = Vec::with_capacity(records.len() + retry.len());
        for (key, json) in records.into_iter().chain(retry) {
            let committed = &self.committed;
            pending.push((key.to_string(), Arc::clone(&json)));
            EncodedPayload::keyed(key, json)
                .stage_unless(&mut staged, &|key| committed.contains(key));
        }
        staged.sort_unstable_by(|a, b| a.key.cmp(&b.key));
        staged.dedup_by(|a, b| a.key == b.key);
        if self.owner.is_none() {
            self.acquire(store, state_id).await?;
        }
        let (owner, _) = self
            .owner
            .clone()
            .ok_or_else(|| std::io::Error::other("journal owner was not acquired"))?;
        if !pending.is_empty() {
            self.pending.update(|known| {
                for (key, json) in &pending {
                    known.insert(key.clone(), Arc::clone(json));
                }
            });
        }
        let pending = pending.into_iter().map(|(key, _)| key);
        let keys = staged.iter().map(|record| record.key.clone());
        if let Some(last) = self.inflight.back_mut()
            && last
                .write
                .as_ref()
                .is_some_and(|write| write.update(&payload, &staged))
        {
            last.staged.extend(keys);
            last.pending.extend(pending);
            return Ok(());
        }
        let staged_keys = keys.collect();
        let write = Arc::new(latest_write(payload, staged));
        let expected = self.next;
        let next = self.next_revision()?;
        let done = store
            .replace_latest(state_id, &owner, expected, Arc::clone(&write))
            .map_err(std::io::Error::other)?;
        self.queue(expected, next);
        self.inflight.push_back(InFlight {
            expected,
            write: Some(write),
            deferred: None,
            done,
            staged: staged_keys,
            pending: pending.collect(),
        });
        Ok(())
    }

    async fn save_deferred(
        &mut self,
        store: &mut SharedStore,
        state_id: &str,
    ) -> std::io::Result<DeferredJournal> {
        if self.failure.is_some() {
            self.drain().await;
            if let Some((_, revision)) = &self.owner {
                self.next = *revision;
            }
            if let Some(error) = self.failure.take() {
                return Err(error);
            }
        }
        if self.owner.is_none() {
            self.acquire(store, state_id).await?;
        }
        let (owner, _) = self
            .owner
            .clone()
            .ok_or_else(|| std::io::Error::other("journal owner was not acquired"))?;
        let retry = std::mem::take(&mut self.retry)
            .into_iter()
            .filter(|key| !self.committed.contains(key))
            .filter_map(|key| {
                let json = self.pending.read(|pending| pending.get(&key).cloned())?;
                Some((Arc::<str>::from(key), json))
            })
            .collect::<Vec<_>>();
        let (deliver, fill) = oneshot::channel::<(JournalFill, JournalCommit)>();
        let (committed, commit) = oneshot::channel::<std::io::Result<()>>();
        let keys = Arc::new(nanocode_sync::Slot::default());
        let filled = Arc::clone(&keys);
        let pending = Arc::clone(&self.pending);
        let content: crate::shared_store::DeferredContent = Box::pin(async move {
            let stopped =
                || StoreError::NotCommitted("the subagent journal writer stopped".to_owned());
            let (send, receive) = oneshot::channel();
            deliver.send((send, commit)).map_err(|_| stopped())?;
            let (payload, records) = receive.await.map_err(|_| stopped())?;
            let mut staged = Vec::new();
            let mut known = Vec::with_capacity(records.len() + retry.len());
            for (key, json) in records.into_iter().chain(retry) {
                known.push((key.to_string(), Arc::clone(&json)));
                EncodedPayload::keyed(key, json).stage(&mut staged);
            }
            staged.sort_unstable_by(|a, b| a.key.cmp(&b.key));
            staged.dedup_by(|a, b| a.key == b.key);
            if !known.is_empty() {
                pending.update(|overlay| {
                    for (key, json) in &known {
                        overlay.insert(key.clone(), Arc::clone(json));
                    }
                });
            }
            filled.put(known.into_iter().map(|(key, _)| key).collect());
            Ok((payload, staged))
        });
        let expected = self.next;
        let next = self.next_revision()?;
        let replaced = store
            .replace_deferred(state_id, &owner, expected, content)
            .map_err(std::io::Error::other)?;
        let (settle, done) = oneshot::channel();
        crate::shared_store::spawn_operation(async move {
            let result = replaced.await.unwrap_or_else(|_| {
                Err(StoreError::NotCommitted(
                    "the durability store stopped".to_owned(),
                ))
            });
            let _ = committed.send(match &result {
                Ok(_) => Ok(()),
                Err(error) => Err(std::io::Error::other(error.to_string())),
            });
            let _ = settle.send(result);
        });
        self.queue(expected, next);
        self.inflight.push_back(InFlight {
            expected,
            write: None,
            deferred: Some(keys),
            done,
            staged: Vec::new(),
            pending: Vec::new(),
        });
        Ok(fill)
    }

    fn next_revision(&self) -> std::io::Result<u64> {
        self.next
            .checked_add(1)
            .ok_or_else(|| std::io::Error::other("subagent journal revision overflow"))
    }

    fn queue(&mut self, expected: u64, next: u64) {
        if let Some(last) = self.inflight.back() {
            assert_eq!(
                last.expected.checked_add(1),
                Some(expected),
                "pipelined journal writes name consecutive revisions"
            );
        }
        self.next = next;
    }

    async fn drain(&mut self) {
        while !self.inflight.is_empty() {
            let result = settled(&mut self.inflight).await;
            self.settled(result);
        }
    }

    fn settled(&mut self, result: Result<u64, StoreError>) {
        let Some(mut entry) = self.inflight.pop_front() else {
            return;
        };
        if let Some(pending) = entry.deferred.take().and_then(|keys| keys.take()) {
            entry.pending.extend(pending);
        }
        let result = result.and_then(|revision| {
            if Some(revision) == entry.expected.checked_add(1) {
                return Ok(revision);
            }
            Err(StoreError::Backend(format!(
                "the store committed journal revision {revision} over revision {}",
                entry.expected
            )))
        });
        match result {
            Ok(revision) => {
                if let Some((owner, _)) = self.owner.take() {
                    self.owner = Some((owner, revision));
                }
                self.committed.extend(entry.staged);
                if !entry.pending.is_empty() {
                    self.pending.update(|known| {
                        for key in &entry.pending {
                            known.remove(key);
                        }
                    });
                }
            }
            Err(error) => {
                if !matches!(error, StoreError::NotCommitted(_) | StoreError::Fenced) {
                    self.owner = None;
                }
                self.retry.extend(entry.pending);
                self.failure.get_or_insert(std::io::Error::other(error));
            }
        }
    }
}

async fn settled(inflight: &mut std::collections::VecDeque<InFlight>) -> Result<u64, StoreError> {
    match inflight.front_mut() {
        Some(entry) => (&mut entry.done).await.unwrap_or_else(|_| {
            Err(StoreError::Backend(
                "the durability store stopped".to_owned(),
            ))
        }),
        None => std::future::pending().await,
    }
}

enum Write {
    Load(oneshot::Sender<std::io::Result<Option<String>>>),
    Save {
        payload: String,
        records: Vec<(Arc<str>, Arc<str>)>,
        result: oneshot::Sender<std::io::Result<()>>,
    },
    Defer(oneshot::Sender<std::io::Result<DeferredJournal>>),
}

async fn write_journal(
    state_id: Arc<str>,
    mut store: SharedStore,
    mut fenced: Fenced,
    mut writes: mpsc::UnboundedReceiver<Write>,
) {
    loop {
        nanocode_sync::select! {
            result = settled(&mut fenced.inflight), if !fenced.inflight.is_empty() => {
                fenced.settled(result);
            }
            write = writes.recv() => match write {
                Some(Write::Load(result)) => {
                    fenced.drain().await;
                    drop(result.send(fenced.acquire(&mut store, &state_id).await));
                }
                Some(Write::Save {
                    payload,
                    records,
                    result,
                }) => {
                    drop(result.send(fenced.save(&mut store, &state_id, payload, records).await));
                }
                Some(Write::Defer(result)) => {
                    drop(result.send(fenced.save_deferred(&mut store, &state_id).await));
                }
                None => break,
            },
        }
    }
    fenced.drain().await;
}

#[derive(Clone)]
struct SharedJournal {
    state_id: Arc<str>,
    store: SharedStore,
    writes: mpsc::UnboundedSender<Write>,
    manifests: Arc<SwapOption<HashMap<String, String>>>,
    pending: Pending,
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
            match crate::session::pending_turn(&store, &session_id)
                .await
                .map_err(std::io::Error::other)?
            {
                PendingTurn::Unacquired => {}
                PendingTurn::Idle => return Ok(None),
                PendingTurn::Turn { id, input } => {
                    return Ok(Some((id, input.decode().map_err(std::io::Error::other)?)));
                }
            }
            let stored = store
                .peek(&session_id)
                .await
                .map_err(std::io::Error::other)?;
            if stored
                .payload
                .as_deref()
                .is_some_and(crate::session::retains_no_operation)
            {
                return Ok(None);
            }
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

    fn save(
        &self,
        payload: String,
        records: Vec<(Arc<str>, Arc<str>)>,
    ) -> BackendFuture<std::io::Result<()>> {
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

    fn verifies_records(&self) -> bool {
        true
    }

    fn save_deferred(&self) -> BackendFuture<std::io::Result<Option<DeferredJournal>>> {
        let journal = self.clone();
        Box::pin(async move { journal.write(Write::Defer).await.map(Some) })
    }

    fn load_records(&self, keys: Vec<String>) -> BackendFuture<std::io::Result<Vec<String>>> {
        if self
            .pending
            .read(|pending| keys.iter().any(|key| pending.contains_key(key)))
        {
            let loads = keys
                .into_iter()
                .map(|key| self.load_record(key))
                .collect::<Vec<_>>();
            return Box::pin(async move {
                let mut records = Vec::with_capacity(loads.len());
                for load in loads {
                    records.push(load.await?);
                }
                Ok(records)
            });
        }
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
        if let Some(json) = self.pending.read(|pending| pending.get(&key).cloned()) {
            return Box::pin(async move { Ok(json.to_string()) });
        }
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
    manifests: &SwapOption<HashMap<String, String>>,
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
    let pending = Pending::default();
    spawn_writer(write_journal(
        Arc::clone(&state_id),
        store.clone(),
        Fenced {
            owner: None,
            committed: HashSet::new(),
            seed,
            next: 0,
            inflight: std::collections::VecDeque::new(),
            failure: None,
            retry: Vec::new(),
            pending: Arc::clone(&pending),
        },
        received,
    ));
    ChildJournal::new(Arc::new(SharedJournal {
        state_id,
        store,
        writes,
        manifests: Arc::default(),
        pending,
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
        JournalSeed(Some(Arc::new(SwapOption::from_pointee(Seed {
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
    if let Ok(runtime) = nanocode_platform::Handle::try_current() {
        runtime.spawn(writer);
    }
}

#[cfg(target_family = "wasm")]
fn spawn_writer(writer: impl Future<Output = ()> + 'static) {
    nanocode_platform::spawn_local(writer);
}
